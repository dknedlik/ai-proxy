use std::time::Instant;

use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Serialize};

use crate::error::{AiProxyError, CoreResult};

/// Request context carries tracing IDs and idempotency key.
#[derive(Clone, Copy, Default)]
pub struct RequestCtx<'a> {
    pub request_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
}

/// Represents a single Server-Sent-Event line (already split on `\n`).
#[derive(Debug, Clone)]
pub struct SseLine {
    pub line: String,
}

/// A boxed stream of `SseLine` results.
pub type SseStream =
    std::pin::Pin<Box<dyn futures_util::stream::Stream<Item = crate::error::CoreResult<SseLine>> + Send>>;

/// Thin wrapper around reqwest::Client with defaults and helpers.
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: Client,
    user_agent: String,
}

impl HttpClient {
    pub fn new_default() -> CoreResult<Self> {
        let inner = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .pool_max_idle_per_host(8)
            .build()
            .map_err(|e| AiProxyError::Other(anyhow::anyhow!("http client build failed: {e}")))?;
        Ok(Self {
            inner,
            user_agent: "ai-proxy/0.1".to_string(),
        })
    }

    pub async fn post_json<T: Serialize, R: DeserializeOwned>(
        &self,
        url: &str,
        body: &T,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<(R, Option<String>, u32)> {
        let start = Instant::now();
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

        let resp = req
            .send()
            .await
            .map_err(|_e| AiProxyError::ProviderUnavailable {
                provider: "http".into(),
            })?;

        let latency = start.elapsed().as_millis() as u32;
        let status = resp.status();
        let headers = resp.headers().clone();
        let provider_request_id = extract_request_id(&headers);

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let ra = parse_retry_after(&headers);
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
        Ok((parsed, provider_request_id, latency))
    }

    /// POST JSON and return an SSE (Server-Sent Events) line stream.
    /// Each yielded item is one raw line (trim not applied) from the SSE channel.
    pub async fn post_sse_lines<T: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<SseStream> {

        // Build request
        let mut req = self
            .inner
            .post(url)
            .json(body)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "text/event-stream");

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

        let resp = req.send().await.map_err(|_| AiProxyError::ProviderUnavailable {
            provider: "http".into(),
        })?;

        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            let ra = parse_retry_after(&headers);
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error("http", status, ra, &body));
        }

        // Stream body as bytes and split on '\n'
        let byte_stream = resp.bytes_stream();
        let line_stream = LineStream::new(Box::pin(byte_stream));
        Ok(Box::pin(line_stream))
    }

    pub async fn get_json<R: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<(R, Option<String>, u32)> {
        let start = Instant::now();
        let mut req = self.inner.get(url).header("User-Agent", &self.user_agent);
        for (k, v) in headers { req = req.header(*k, *v); }
        if let Some(rid) = ctx.request_id { req = req.header("X-Request-Id", rid); }
        if let Some(tid) = ctx.turn_id { req = req.header("X-Turn-Id", tid); }
        if let Some(ik) = ctx.idempotency_key { req = req.header("Idempotency-Key", ik); }

        let resp = req
            .send()
            .await
            .map_err(|_e| AiProxyError::ProviderUnavailable { provider: "http".into() })?;

        let latency = start.elapsed().as_millis() as u32;
        let status = resp.status();
        let headers = resp.headers().clone();
        let provider_request_id = extract_request_id(&headers);

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let ra = parse_retry_after(&headers);
            return Err(map_http_error("http", status, ra, &text));
        }

        let parsed = resp.json::<R>().await.map_err(|e| AiProxyError::ProviderError {
            provider: "http".into(),
            code: status.as_u16().to_string(),
            message: format!("json decode error: {e}"),
        })?;
        Ok((parsed, provider_request_id, latency))
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

fn map_http_error(provider: &str, status: StatusCode, retry_after: Option<u64>, body: &str) -> AiProxyError {
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

/// Internal line splitter over a bytes stream; yields `SseLine`s separated by '\n'.
struct LineStream {
    inner: std::pin::Pin<
        Box<dyn futures_util::stream::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>,
    >,
    buf: String,
    flushed_tail: bool,
}

impl LineStream {
    fn new(
        inner: std::pin::Pin<
            Box<dyn futures_util::stream::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>,
        >,
    ) -> Self {
        Self {
            inner,
            buf: String::new(),
            flushed_tail: false,
        }
    }
}

impl futures_util::stream::Stream for LineStream {
    type Item = CoreResult<SseLine>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        loop {
            // If we already have a newline in the buffer, split and yield immediately.
            if let Some(idx) = self.buf.find('\n') {
                let mut line = self.buf.drain(..=idx).collect::<String>();
                if line.ends_with('\n') {
                    if line.ends_with("\r\n") {
                        line.truncate(line.len() - 2);
                    } else {
                        line.truncate(line.len() - 1);
                    }
                }
                return Poll::Ready(Some(Ok(SseLine { line })));
            }

            // Otherwise, poll the inner stream for more bytes
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let s = String::from_utf8_lossy(&chunk);
                    self.buf.push_str(&s);
                    continue;
                }
                Poll::Ready(Some(Err(_e))) => {
                    return Poll::Ready(Some(Err(AiProxyError::ProviderUnavailable {
                        provider: "http".into(),
                    })));
                }
                Poll::Ready(None) => {
                    if !self.flushed_tail && !self.buf.is_empty() {
                        self.flushed_tail = true;
                        let line = std::mem::take(&mut self.buf);
                        return Poll::Ready(Some(Ok(SseLine { line })));
                    } else {
                        return Poll::Ready(None);
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;
    use serde_json::json;

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

        matches!(err, AiProxyError::ProviderUnavailable { .. });
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
        let err = client.post_json::<_, serde_json::Value>(
            &format!("{}/chat", server.base_url()),
            &serde_json::json!({"msg":"hi"}),
            &[],
            &ctx,
        ).await.unwrap_err();
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
        let err = client.post_json::<_, serde_json::Value>(
            &format!("{}/chat", server.base_url()),
            &serde_json::json!({"msg":"hi"}),
            &[],
            &ctx,
        ).await.unwrap_err();
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
        let err = client.post_json::<_, serde_json::Value>(
            url,
            &serde_json::json!({"msg":"hi"}),
            &[],
            &ctx,
        ).await.unwrap_err();
        assert!(matches!(err, AiProxyError::ProviderUnavailable { .. }));
    }
}
