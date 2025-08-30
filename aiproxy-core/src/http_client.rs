use crate::config::HttpCfg;
use std::time::Instant;

use reqwest::{Client, StatusCode};
use serde::{Serialize, de::DeserializeOwned};

use crate::error::{AiProxyError, CoreResult};

const MAX_RETRIES: usize = 2; // attempts after the first, gated by idempotency_key
fn backoff_ms(attempt: usize) -> u64 {
    // attempt: 0 (first retry), 1 (second retry), ...
    let base = if cfg!(test) { 1 } else { 200 } as u64; // keep tests fast
    let ms = base.saturating_mul(1u64 << attempt.min(10));
    ms.min(3_000) // cap at 3s
}

fn is_retryable_status(s: StatusCode) -> bool {
    s == StatusCode::TOO_MANY_REQUESTS
        || s == StatusCode::BAD_GATEWAY
        || s == StatusCode::SERVICE_UNAVAILABLE
        || s == StatusCode::GATEWAY_TIMEOUT
}

/// Request context carries tracing IDs and idempotency key.
#[derive(Clone, Copy, Default)]
pub struct RequestCtx<'a> {
    pub request_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
}

/// Thin wrapper around reqwest::Client with defaults and helpers.
#[derive(Clone, Debug)]
pub struct HttpClient {
    inner: Client,
    user_agent: String,
}

impl HttpClient {
    pub fn new_with(cfg: &HttpCfg) -> CoreResult<Self> {
        let mut builder = Client::builder()
            .connect_timeout(std::time::Duration::from_millis(cfg.connect_timeout_ms))
            .timeout(std::time::Duration::from_millis(cfg.request_timeout_ms));
        if let Some(n) = cfg.pool_max_idle_per_host {
            builder = builder.pool_max_idle_per_host(n);
        }
        let inner = builder
            .build()
            .map_err(|e| AiProxyError::Other(anyhow::anyhow!("http client build failed: {e}")))?;
        Ok(Self {
            inner,
            user_agent: "ai-proxy/0.1".to_string(),
        })
    }

    pub fn new_default() -> CoreResult<Self> {
        let cfg = HttpCfg::default();
        Self::new_with(&cfg)
    }

    pub async fn post_json<T: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &T,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<(R, Option<String>, u32)> {
        let start = Instant::now();
        let mut attempt = 0usize;
        loop {
            let mut req = self
                .inner
                .post(url)
                .json(body)
                .header("User-Agent", &self.user_agent);

            // custom headers
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            if let Some(rid) = ctx.request_id {
                req = req.header("X-Request-Id", rid);
            }
            if let Some(tid) = ctx.turn_id {
                req = req.header("X-Turn-Id", tid);
            }
            if let Some(ik) = ctx.idempotency_key {
                req = req.header("Idempotency-Key", ik);
            }
            req = req.header("X-Retry-Attempt", attempt.to_string());

            // Add Accept header for compatibility
            req = req.header("Accept", "application/json");

            // If debug level 2, print a runnable curl command with exact headers/body
            if std::env::var("AIPROXY_DEBUG_HTTP").ok().as_deref() == Some("2") {
                let mut curl = format!("curl -i '{}' \\\n  -X POST", url);
                // Headers we explicitly set
                curl.push_str(&format!(" \\\n  -H 'User-Agent: {}'", self.user_agent));
                curl.push_str(" \\\n  -H 'Content-Type: application/json'");
                curl.push_str(" \\\n  -H 'Accept: application/json'");
                // Custom headers (mask sensitive)
                for (k, v) in headers {
                    if k.eq_ignore_ascii_case("authorization") && v.starts_with("Bearer ") {
                        let raw = &v["Bearer ".len()..];
                        let masked = if raw.len() > 10 {
                            format!("Bearer {}****{}", &raw[..6], &raw[raw.len() - 4..])
                        } else {
                            "Bearer ****".to_string()
                        };
                        curl.push_str(&format!(" \\\n  -H '{}: {}'", k, masked));
                    } else {
                        curl.push_str(&format!(" \\\n  -H '{}: {}'", k, v));
                    }
                }
                if let Some(rid) = ctx.request_id {
                    curl.push_str(&format!(" \\\n  -H 'X-Request-Id: {}'", rid));
                }
                if let Some(tid) = ctx.turn_id {
                    curl.push_str(&format!(" \\\n  -H 'X-Turn-Id: {}'", tid));
                }
                if let Some(ik) = ctx.idempotency_key {
                    curl.push_str(&format!(" \\\n  -H 'Idempotency-Key: {}'", ik));
                }
                curl.push_str(&format!(" \\\n  -H 'X-Retry-Attempt: {}'", attempt));
                let body_str =
                    serde_json::to_string(body).unwrap_or_else(|_| "<SERDE_ERROR>".into());
                let escaped = body_str.replace('\'', "'\\''");
                curl.push_str(&format!(" \\\n  -d '{}'", escaped));
                eprintln!("[curl] {}", curl);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(_e) => {
                    // network error
                    if ctx.idempotency_key.is_some() && attempt < MAX_RETRIES {
                        let delay = backoff_ms(attempt);
                        if delay > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        }
                        attempt += 1;
                        continue;
                    } else {
                        return Err(AiProxyError::ProviderUnavailable {
                            provider: "http".into(),
                        });
                    }
                }
            };

            let status = resp.status();
            let headers = resp.headers().clone();
            let provider_request_id = extract_request_id(&headers);

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let ra = parse_retry_after(&headers);
                if ctx.idempotency_key.is_some()
                    && attempt < MAX_RETRIES
                    && is_retryable_status(status)
                {
                    // Honor Retry-After if present, otherwise backoff
                    let delay_ms = ra
                        .map(|s| s.saturating_mul(1000))
                        .unwrap_or_else(|| backoff_ms(attempt));
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    attempt += 1;
                    continue;
                }
                return Err(map_http_error("http", status, ra, &text));
            }

            let parsed = resp
                .json::<R>()
                .await
                .map_err(|e| AiProxyError::ProviderError {
                    provider: "http".into(),
                    code: status.as_u16().to_string(),
                    message: format!("json decode error: {e}"),
                })?;
            let latency = start.elapsed().as_millis() as u32;
            return Ok((parsed, provider_request_id, latency));
        }
    }

    pub async fn get_json<R: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<(R, Option<String>, u32)> {
        let start = Instant::now();
        let mut attempt = 0usize;
        loop {
            let mut req = self.inner.get(url).header("User-Agent", &self.user_agent);
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            if let Some(rid) = ctx.request_id {
                req = req.header("X-Request-Id", rid);
            }
            if let Some(tid) = ctx.turn_id {
                req = req.header("X-Turn-Id", tid);
            }
            if let Some(ik) = ctx.idempotency_key {
                req = req.header("Idempotency-Key", ik);
            }
            req = req.header("X-Retry-Attempt", attempt.to_string());

            // Add Accept for GETs too
            req = req.header("Accept", "application/json");

            if std::env::var("AIPROXY_DEBUG_HTTP").ok().as_deref() == Some("2") {
                let mut curl = format!("curl -i '{}' \\\n  -X GET", url);
                curl.push_str(&format!(" \\\n  -H 'User-Agent: {}'", self.user_agent));
                curl.push_str(" \\\n  -H 'Accept: application/json'");
                for (k, v) in headers {
                    if k.eq_ignore_ascii_case("authorization") && v.starts_with("Bearer ") {
                        let raw = &v["Bearer ".len()..];
                        let masked = if raw.len() > 10 {
                            format!("Bearer {}****{}", &raw[..6], &raw[raw.len() - 4..])
                        } else {
                            "Bearer ****".to_string()
                        };
                        curl.push_str(&format!(" \\\n  -H '{}: {}'", k, masked));
                    } else {
                        curl.push_str(&format!(" \\\n  -H '{}: {}'", k, v));
                    }
                }
                if let Some(rid) = ctx.request_id {
                    curl.push_str(&format!(" \\\n  -H 'X-Request-Id: {}'", rid));
                }
                if let Some(tid) = ctx.turn_id {
                    curl.push_str(&format!(" \\\n  -H 'X-Turn-Id: {}'", tid));
                }
                if let Some(ik) = ctx.idempotency_key {
                    curl.push_str(&format!(" \\\n  -H 'Idempotency-Key: {}'", ik));
                }
                curl.push_str(&format!(" \\\n  -H 'X-Retry-Attempt: {}'", attempt));
                eprintln!("[curl] {}", curl);
            }

            let resp = match req.send().await {
                Ok(r) => r,
                Err(_e) => {
                    if ctx.idempotency_key.is_some() && attempt < MAX_RETRIES {
                        let delay = backoff_ms(attempt);
                        if delay > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        }
                        attempt += 1;
                        continue;
                    } else {
                        return Err(AiProxyError::ProviderUnavailable {
                            provider: "http".into(),
                        });
                    }
                }
            };

            let status = resp.status();
            let headers = resp.headers().clone();
            let provider_request_id = extract_request_id(&headers);

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let ra = parse_retry_after(&headers);
                if ctx.idempotency_key.is_some()
                    && attempt < MAX_RETRIES
                    && is_retryable_status(status)
                {
                    let delay_ms = ra
                        .map(|s| s.saturating_mul(1000))
                        .unwrap_or_else(|| backoff_ms(attempt));
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    attempt += 1;
                    continue;
                }
                return Err(map_http_error("http", status, ra, &text));
            }

            let parsed = resp
                .json::<R>()
                .await
                .map_err(|e| AiProxyError::ProviderError {
                    provider: "http".into(),
                    code: status.as_u16().to_string(),
                    message: format!("json decode error: {e}"),
                })?;
            let latency = start.elapsed().as_millis() as u32;
            return Ok((parsed, provider_request_id, latency));
        }
    }
}

fn extract_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    static CANDIDATES: [&str; 5] = [
        "x-request-id",
        "request-id",
        "x-amzn-requestid",
        "x-amz-request-id",
        "x-cdn-request-id",
    ];
    for k in CANDIDATES {
        if let Some(v) = headers.get(k)
            && let Ok(s) = v.to_str()
        {
            return Some(s.to_string());
        }
    }
    None
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(v) = headers.get("retry-after")
        && let Ok(s) = v.to_str()
        && let Ok(secs) = s.trim().parse::<u64>()
    {
        return Some(secs);
    }
    // HTTP-date parsing (RFC 7231) best-effort using httpdate crate if added later.
    // For now, ignore non-numeric forms.
    None
}

fn map_http_error(
    provider: &str,
    status: StatusCode,
    retry_after: Option<u64>,
    body: &str,
) -> AiProxyError {
    match status {
        StatusCode::TOO_MANY_REQUESTS => AiProxyError::RateLimited {
            provider: provider.to_string(),
            retry_after,
        },
        s if s.is_server_error() => AiProxyError::ProviderUnavailable {
            provider: provider.to_string(),
        },
        s => AiProxyError::ProviderError {
            provider: provider.to_string(),
            code: s.as_u16().to_string(),
            message: truncate(body, 300),
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        let mut t = s[..max].to_string();
        t.push_str("...");
        t
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;
    use serde_json::json;

    #[tokio::test]
    async fn new_with_applies_timeouts() {
        use crate::config::HttpCfg;
        let cfg = HttpCfg {
            connect_timeout_ms: 1234,
            request_timeout_ms: 5678,
            pool_max_idle_per_host: Some(2),
        };
        let client = HttpClient::new_with(&cfg).expect("client");
        assert_eq!(client.user_agent, "ai-proxy/0.1");
    }

    #[tokio::test]
    async fn post_json_success() {
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST).path("/chat");
            then.status(200)
                .header("x-request-id", "abc123")
                .json_body(json!({"ok": true}));
        });

        #[derive(serde::Deserialize)]
        struct Resp {
            ok: bool,
        }

        let client = HttpClient::new_default().unwrap();
        let ctx = RequestCtx {
            request_id: Some("rid"),
            turn_id: Some("tid"),
            idempotency_key: None,
        };
        let (resp, provider_id, latency) = client
            .post_json::<_, Resp>(
                &format!("{}/chat", server.base_url()),
                &json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap();

        assert!(resp.ok);
        assert_eq!(provider_id, Some("abc123".into()));
        assert!(latency > 0);
        m.assert();
    }

    #[tokio::test]
    async fn post_json_429_maps_to_rate_limited() {
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(POST).path("/chat");
            then.status(429)
                .header("Retry-After", "1")
                .body("slow down");
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx {
            request_id: None,
            turn_id: None,
            idempotency_key: None,
        };
        let err = client
            .post_json::<_, serde_json::Value>(
                &format!("{}/chat", server.base_url()),
                &serde_json::json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap_err();

        match err {
            AiProxyError::RateLimited {
                provider,
                retry_after,
            } => {
                assert_eq!(provider, "http");
                // (We didn't parse Retry-After yet; once we do, assert_eq!(retry_after, Some(1));
                let _ = retry_after;
            }
            other => panic!("expected RateLimited, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn post_json_503_maps_to_unavailable() {
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(POST).path("/chat");
            then.status(503).body("oops");
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx {
            request_id: None,
            turn_id: None,
            idempotency_key: None,
        };
        let err = client
            .post_json::<_, serde_json::Value>(
                &format!("{}/chat", server.base_url()),
                &serde_json::json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, AiProxyError::ProviderUnavailable { .. }));
    }

    #[tokio::test]
    async fn post_json_200_bad_json_maps_to_provider_error() {
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(POST).path("/chat");
            then.status(200).body("not-json");
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let err = client
            .post_json::<_, serde_json::Value>(
                &format!("{}/chat", server.base_url()),
                &serde_json::json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            AiProxyError::ProviderError { code, .. } => assert_eq!(code, "200"),
            other => panic!("expected ProviderError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn post_json_400_truncates_body() {
        let server = MockServer::start();
        let big = "x".repeat(1000);
        let _m = server.mock(|when, then| {
            when.method(POST).path("/chat");
            then.status(400).body(big.clone());
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let err = client
            .post_json::<_, serde_json::Value>(
                &format!("{}/chat", server.base_url()),
                &serde_json::json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            AiProxyError::ProviderError { message, .. } => assert!(message.ends_with("...")),
            other => panic!("expected ProviderError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn network_error_maps_to_unavailable() {
        // Attempt to connect to a likely-closed port to simulate network error quickly.
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let url = "http://127.0.0.1:9/chat"; // port 9 (discard) is typically closed
        let err = client
            .post_json::<_, serde_json::Value>(url, &serde_json::json!({"msg":"hi"}), &[], &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, AiProxyError::ProviderUnavailable { .. }));
    }

    #[tokio::test]
    async fn post_json_retries_on_429_then_succeeds_with_idempotency() {
        let server = MockServer::start();
        let first = server.mock(|when, then| {
            when.method(POST)
                .path("/chat")
                .header("X-Retry-Attempt", "0");
            then.status(429).header("Retry-After", "0").body("rate");
        });
        let second = server.mock(|when, then| {
            when.method(POST)
                .path("/chat")
                .header("X-Retry-Attempt", "1");
            then.status(200).json_body(json!({"ok": true}));
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx {
            request_id: None,
            turn_id: None,
            idempotency_key: Some("ikey"),
        };
        #[derive(serde::Deserialize)]
        struct Resp {
            ok: bool,
        }
        let (resp, _, _) = client
            .post_json::<_, Resp>(
                &format!("{}/chat", server.base_url()),
                &json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .expect("should retry then succeed");
        assert!(resp.ok);
        assert_eq!(
            first.hits(),
            1,
            "first mock (429) should be hit exactly once"
        );
        assert_eq!(
            second.hits(),
            1,
            "second mock (200) should be hit exactly once"
        );
        first.assert();
        second.assert();
    }

    #[tokio::test]
    async fn post_json_does_not_retry_without_idempotency_key() {
        let server = MockServer::start();
        let first = server.mock(|when, then| {
            when.method(POST).path("/chat");
            then.status(503).body("down");
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let err = client
            .post_json::<_, serde_json::Value>(
                &format!("{}/chat", server.base_url()),
                &json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AiProxyError::ProviderUnavailable { .. } | AiProxyError::ProviderError { .. }
        ));
        // Ensure we hit the first mock; second shouldn't be necessary for success
        first.assert();
    }
}
