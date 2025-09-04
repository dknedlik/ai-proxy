// SSE buffer growth guard: 2 MiB
const MAX_SSE_BUFFER: usize = 2 * 1024 * 1024;

// DRY helper to apply request-context headers.
fn apply_ctx_headers(mut req: reqwest::RequestBuilder, ctx: &RequestCtx<'_>) -> reqwest::RequestBuilder {
    if let Some(rid) = ctx.request_id { req = req.header("X-Request-Id", rid); }
    if let Some(tid) = ctx.turn_id { req = req.header("X-Turn-Id", tid); }
    if let Some(ik) = ctx.idempotency_key { req = req.header("Idempotency-Key", ik); }
    req
}
use std::time::Instant;

use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Serialize};

use tracing::Instrument;

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
        // Tracing span for HTTP request lifecycle
        let span = tracing::info_span!(
            "http.request",
            provider = "http",
            method = "POST",
            url = %url,
            turn_id = %ctx.turn_id.unwrap_or_default(),
            request_id = %ctx.request_id.unwrap_or_default(),
            idempotency_key = %ctx.idempotency_key.unwrap_or_default(),
            status = tracing::field::Empty,
            provider_request_id = tracing::field::Empty,
            latency_ms = tracing::field::Empty,
            error_kind = tracing::field::Empty,
            error_message = tracing::field::Empty,
        );
        async move {
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
            req = apply_ctx_headers(req, ctx);

            let resp = req
                .send()
                .await
                .map_err(|_e| AiProxyError::ProviderUnavailable {
                    provider: "http".into(),
                })?;

            let status = resp.status();
            tracing::Span::current().record("status", tracing::field::display(status.as_u16()));
            let headers = resp.headers().clone();
            let provider_request_id = extract_request_id(&headers);
            if let Some(ref rid) = provider_request_id {
                tracing::Span::current().record("provider_request_id", tracing::field::display(rid));
            }

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let ra = parse_retry_after(&headers);
                let latency = start.elapsed().as_millis() as u32;
                // Telemetry: HTTP error
                {
                    let trace = crate::telemetry::ProviderTrace::new()
                        .provider("http")
                        .latency_ms(latency as u64)
                        .provider_request_id_opt(provider_request_id.as_deref())
                        .error_kind("http_error")
                        .error_message(&truncate(&text, 200));
                    crate::telemetry::emit(trace);
                }
                tracing::Span::current().record("error_kind", tracing::field::display("http_error"));
                tracing::Span::current().record("error_message", tracing::field::display(truncate(&text, 200)));
                tracing::Span::current().record("latency_ms", latency);
                return Err(map_http_error("http", status, ra, &text));
            }

            let parsed = resp.json::<R>().await.map_err(|e| {
                let latency = start.elapsed().as_millis() as u32;
                // Telemetry: decode error
                let trace = crate::telemetry::ProviderTrace::new()
                    .provider("http")
                    .latency_ms(latency as u64)
                    .provider_request_id_opt(provider_request_id.as_deref())
                    .error_kind("decode_error")
                    .error_message(&format!("json decode error: {e}"));
                crate::telemetry::emit(trace);
                tracing::Span::current().record("error_kind", tracing::field::display("decode_error"));
                tracing::Span::current().record("error_message", tracing::field::display(format!("json decode error: {e}")));
                tracing::Span::current().record("latency_ms", latency);
                AiProxyError::ProviderError {
                    provider: "http".into(),
                    code: status.as_u16().to_string(),
                    message: format!("json decode error: {e}"),
                }
            })?;
            let latency = start.elapsed().as_millis() as u32;
            // Telemetry: success
            {
                let trace = crate::telemetry::ProviderTrace::new()
                    .provider("http")
                    .latency_ms(latency as u64)
                    .provider_request_id_opt(provider_request_id.as_deref());
                crate::telemetry::emit(trace);
            }
            tracing::Span::current().record("latency_ms", latency);
            Ok((parsed, provider_request_id, latency))
        }
        .instrument(span)
        .await
    }

    /// POST JSON and return an SSE (Server-Sent Events) line stream.
    /// Each yielded item is one raw line (trim not applied) from the SSE channel.
    pub async fn post_sse_lines<T: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<(SseStream, Option<String>)> {
        // Build request
        let start = Instant::now();
        let mut req = self
            .inner
            .post(url)
            .json(body)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "text/event-stream");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req = apply_ctx_headers(req, ctx);

        // HTTP request span for header roundtrip
        let span = tracing::info_span!(
            "http.request",
            provider = "http",
            method = "POST",
            url = %url,
            turn_id = %ctx.turn_id.unwrap_or_default(),
            request_id = %ctx.request_id.unwrap_or_default(),
            idempotency_key = %ctx.idempotency_key.unwrap_or_default(),
            status = tracing::field::Empty,
            provider_request_id = tracing::field::Empty,
            latency_ms = tracing::field::Empty,
            error_kind = tracing::field::Empty,
            error_message = tracing::field::Empty,
        );
        let resp = {
            let req = req;
            async move {
                let resp = req.send().await.map_err(|_| AiProxyError::ProviderUnavailable {
                    provider: "http".into(),
                })?;
                let status = resp.status();
                tracing::Span::current().record("status", tracing::field::display(status.as_u16()));
                let headers = resp.headers().clone();
                let provider_request_id = extract_request_id(&headers);
                if let Some(ref rid) = provider_request_id {
                    tracing::Span::current().record("provider_request_id", tracing::field::display(rid));
                }
                if !status.is_success() {
                    let ra = parse_retry_after(&headers);
                    let body = resp.text().await.unwrap_or_default();
                    let latency = start.elapsed().as_millis() as u64;
                    // Telemetry: HTTP error
                    {
                        let trace = crate::telemetry::ProviderTrace::new()
                            .provider("http")
                            .latency_ms(latency)
                            .provider_request_id_opt(provider_request_id.as_deref())
                            .error_kind("http_error")
                            .error_message(&truncate(&body, 200));
                        crate::telemetry::emit(trace);
                    }
                    tracing::Span::current().record("error_kind", tracing::field::display("http_error"));
                    tracing::Span::current().record("error_message", tracing::field::display(truncate(&body, 200)));
                    tracing::Span::current().record("latency_ms", latency);
                    return Err(map_http_error("http", status, ra, &body));
                }
                let latency = start.elapsed().as_millis() as u64;
                tracing::Span::current().record("latency_ms", latency);
                Ok::<_, AiProxyError>(resp)
            }
            .instrument(span)
            .await?
        };

        // Stream body as bytes and split on '\n'
        let provider_request_id = extract_request_id(resp.headers());
        let byte_stream = resp.bytes_stream();
        let line_stream = LineStream::new(Box::pin(byte_stream));
        let sse_span = tracing::info_span!(
            "sse.stream",
            provider = "http",
            provider_request_id = %provider_request_id.as_deref().unwrap_or(""),
            latency_ms = tracing::field::Empty,
            error_kind = tracing::field::Empty,
        );
        let wrapped = TelemetryOnDrop {
            inner: Box::pin(line_stream),
            start,
            provider_request_id: provider_request_id.clone(),
            emitted: false,
            span: sse_span,
        };
        Ok((Box::pin(wrapped), provider_request_id))
    }

    pub async fn get_json<R: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        ctx: &RequestCtx<'_>,
    ) -> CoreResult<(R, Option<String>, u32)> {
        // Tracing span for HTTP request lifecycle (GET)
        let span = tracing::info_span!(
            "http.request",
            provider = "http",
            method = "GET",
            url = %url,
            turn_id = %ctx.turn_id.unwrap_or_default(),
            request_id = %ctx.request_id.unwrap_or_default(),
            idempotency_key = %ctx.idempotency_key.unwrap_or_default(),
            status = tracing::field::Empty,
            provider_request_id = tracing::field::Empty,
            latency_ms = tracing::field::Empty,
            error_kind = tracing::field::Empty,
            error_message = tracing::field::Empty,
        );
        async move {
            let start = Instant::now();
            let mut req = self.inner.get(url).header("User-Agent", &self.user_agent);
            for (k, v) in headers { req = req.header(*k, *v); }
            req = apply_ctx_headers(req, ctx);

            let resp = req
                .send()
                .await
                .map_err(|_e| AiProxyError::ProviderUnavailable { provider: "http".into() })?;

            let status = resp.status();
            tracing::Span::current().record("status", tracing::field::display(status.as_u16()));
            let headers = resp.headers().clone();
            let provider_request_id = extract_request_id(&headers);
            if let Some(ref rid) = provider_request_id {
                tracing::Span::current().record("provider_request_id", tracing::field::display(rid));
            }

            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let ra = parse_retry_after(&headers);
                let latency = start.elapsed().as_millis() as u32;
                // Telemetry: HTTP error
                {
                    let trace = crate::telemetry::ProviderTrace::new()
                        .provider("http")
                        .latency_ms(latency as u64)
                        .provider_request_id_opt(provider_request_id.as_deref())
                        .error_kind("http_error")
                        .error_message(&truncate(&text, 200));
                    crate::telemetry::emit(trace);
                }
                tracing::Span::current().record("error_kind", tracing::field::display("http_error"));
                tracing::Span::current().record("error_message", tracing::field::display(truncate(&text, 200)));
                tracing::Span::current().record("latency_ms", latency);
                return Err(map_http_error("http", status, ra, &text));
            }

            let parsed = resp.json::<R>().await.map_err(|e| {
                let latency = start.elapsed().as_millis() as u32;
                // Telemetry: decode error
                let trace = crate::telemetry::ProviderTrace::new()
                    .provider("http")
                    .latency_ms(latency as u64)
                    .provider_request_id_opt(provider_request_id.as_deref())
                    .error_kind("decode_error")
                    .error_message(&format!("json decode error: {e}"));
                crate::telemetry::emit(trace);
                tracing::Span::current().record("error_kind", tracing::field::display("decode_error"));
                tracing::Span::current().record("error_message", tracing::field::display(format!("json decode error: {e}")));
                tracing::Span::current().record("latency_ms", latency);
                AiProxyError::ProviderError {
                    provider: "http".into(),
                    code: status.as_u16().to_string(),
                    message: format!("json decode error: {e}"),
                }
            })?;
            let latency = start.elapsed().as_millis() as u32;
            // Telemetry: success
            {
                let trace = crate::telemetry::ProviderTrace::new()
                    .provider("http")
                    .latency_ms(latency as u64)
                    .provider_request_id_opt(provider_request_id.as_deref());
                crate::telemetry::emit(trace);
            }
            tracing::Span::current().record("latency_ms", latency);
            Ok((parsed, provider_request_id, latency))
        }
        .instrument(span)
        .await
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
                    if self.buf.len() > MAX_SSE_BUFFER {
                        return Poll::Ready(Some(Err(AiProxyError::ProviderError {
                            provider: "http".into(),
                            code: "sse_buffer_overflow".into(),
                            message: "SSE buffer exceeded 2MiB without a newline".into(),
                        })));
                    }
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

/// Adapter that emits a single telemetry record when the inner stream completes or is dropped.
struct TelemetryOnDrop<S> {
    inner: std::pin::Pin<Box<S>>, // keep pinned
    start: Instant,
    provider_request_id: Option<String>,
    emitted: bool,
    span: tracing::Span,
}

impl<S> futures_util::stream::Stream for TelemetryOnDrop<S>
where
    S: futures_util::stream::Stream<Item = CoreResult<SseLine>> + Unpin,
{
    type Item = CoreResult<SseLine>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            std::task::Poll::Ready(None) => {
                if !self.emitted {
                    self.emitted = true;
                    let latency = (self.start.elapsed().as_millis() as u64).max(1);
                    let _enter = self.span.enter();
                    tracing::Span::current().record("latency_ms", latency);
                    let trace = crate::telemetry::ProviderTrace::new()
                        .provider("http")
                        .latency_ms(latency)
                        .provider_request_id_opt(self.provider_request_id.as_deref());
                    crate::telemetry::emit(trace);
                }
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Ready(Some(item)) => {
                if let Err(ref e) = item {
                    let kind = match e {
                        AiProxyError::ProviderError { code, .. } => code.as_str(),
                        AiProxyError::ProviderUnavailable { .. } => "provider_unavailable",
                        AiProxyError::RateLimited { .. } => "rate_limited",
                        AiProxyError::Validation(_) => "validation",
                        AiProxyError::Io(_) => "io",
                        AiProxyError::Other(_) => "other",
                        AiProxyError::BudgetExceeded { .. } => "budget_exceeded",
                    };
                    let _enter = self.span.enter();
                    tracing::Span::current().record("error_kind", tracing::field::display(kind));
                }
                std::task::Poll::Ready(Some(item))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl<S> Drop for TelemetryOnDrop<S> {
    fn drop(&mut self) {
        if !self.emitted {
            self.emitted = true;
            let latency = (self.start.elapsed().as_millis() as u64).max(1);
            let _enter = self.span.enter();
            tracing::Span::current().record("latency_ms", latency);
            let trace = crate::telemetry::ProviderTrace::new()
                .provider("http")
                .latency_ms(latency)
                .provider_request_id_opt(self.provider_request_id.as_deref());
            crate::telemetry::emit(trace);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;
    use serde_json::json;
    use crate::test_util::{install_trace_sink, TRACE_LOGS};

    #[tokio::test(flavor = "current_thread")]
    async fn sse_early_drop_records_latency() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
        let server = MockServer::start();
        // Single delta, no [DONE]; client will drop early
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";
        let _m = server.mock(|when, then| {
            when.method(POST).path("/sse-one");
            then.status(200)
                .header("content-type", "text/event-stream")
                .header("x-request-id", "sse-early")
                .body(sse_body);
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let (mut stream, _pid) = client.post_sse_lines(
            &format!("{}/sse-one", server.base_url()),
            &serde_json::json!({"stream": true}),
            &[],
            &ctx,
        ).await.expect("sse ok");

        use futures_util::StreamExt;
        let _first = stream.next().await.expect("one item");
        drop(stream); // early drop should emit latency via Drop impl

        // Telemetry emitted
        let traces = TRACE_LOGS.lock().unwrap();
        let hit = traces.iter().rev().find(|t| t.provider.as_deref() == Some("http") && t.provider_request_id.as_deref() == Some("sse-early"));
        assert!(hit.is_some(), "telemetry record for sse-early not found; have: {:?}", *traces);
        let hit = hit.unwrap();
        assert!(hit.latency_ms.unwrap_or(0) > 0);

        // sse.stream span recorded latency
        let spans = span_store.spans.lock().unwrap();
        let mut saw = false;
        for (_id, data) in spans.iter() {
            if data.name == "sse.stream" {
                let fields = data.fields.lock().unwrap();
                let prid = fields.get("provider_request_id").cloned().unwrap_or_default();
                if prid.trim_matches('"') == "sse-early" {
                    assert!(fields.get("latency_ms").is_some());
                    saw = true;
                    break;
                }
            }
        }
        assert!(saw, "sse.stream span for sse-early not found; have: {spans:?}");
    }

    #[tokio::test]
    async fn get_json_success_span_fields() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/info");
            then.status(200)
                .header("x-request-id", "get123")
                .json_body(json!({"ok": true}));
        });
        #[derive(serde::Deserialize)]
        struct Resp { ok: bool }
        let client = HttpClient::new_default().unwrap();
        let ctx = RequestCtx::default();
        let (resp, provider_id, latency) = client
            .get_json::<Resp>(&format!("{}/info", server.base_url()), &[], &ctx)
            .await
            .unwrap();
        assert!(resp.ok);
        assert_eq!(provider_id, Some("get123".into()));
        assert!(latency > 0);
        m.assert();

        // Span assertion
        let spans = span_store.spans.lock().unwrap();
        let mut found = false;
        for (_id, data) in spans.iter() {
            if data.name == "http.request" {
                let fields = data.fields.lock().unwrap();
                let url = fields.get("url").cloned().unwrap_or_default();
                if url.contains("/info") {
                    assert_eq!(fields.get("provider").map(String::as_str).unwrap_or(""), "\"http\"");
                    assert_eq!(fields.get("method").map(String::as_str).unwrap_or(""), "\"GET\"");
                    assert_eq!(fields.get("status").map(String::as_str).unwrap_or(""), "200");
                    let prid = fields.get("provider_request_id").cloned().unwrap_or_default();
                    assert_eq!(prid.trim_matches('"'), "get123");
                    assert!(fields.get("latency_ms").is_some());
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "http.request span for GET /info not found; have: {spans:?}");
    }

    #[tokio::test]
    async fn get_json_404_span_fields() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/missing");
            then.status(404).body("nope");
        });
        let client = HttpClient::new_default().unwrap();
        let ctx = RequestCtx::default();
        let err = client
            .get_json::<serde_json::Value>(&format!("{}/missing", server.base_url()), &[], &ctx)
            .await
            .unwrap_err();
        match err {
            AiProxyError::ProviderError { code, .. } => assert_eq!(code, "404"),
            other => panic!("expected ProviderError, got: {:?}", other),
        }

        // Span assertion: method/status/error_kind present
        let spans = span_store.spans.lock().unwrap();
        let mut found = false;
        for (_id, data) in spans.iter() {
            if data.name == "http.request" {
                let fields = data.fields.lock().unwrap();
                let url = fields.get("url").cloned().unwrap_or_default();
                if url.contains("/missing") {
                    assert_eq!(fields.get("method").map(String::as_str).unwrap_or(""), "\"GET\"");
                    assert_eq!(fields.get("status").map(String::as_str).unwrap_or(""), "404");
                    assert!(fields.get("error_kind").is_some());
                    assert!(fields.get("latency_ms").is_some());
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "http.request span for GET /missing not found; have: {spans:?}");
    }
    

    #[tokio::test(flavor = "current_thread")]
    async fn post_json_success() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
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

        // Telemetry assertion
        let traces = TRACE_LOGS.lock().unwrap();
        assert!(!traces.is_empty());
        let hit = traces.iter().rev().find(|t| t.provider.as_deref() == Some("http") && t.provider_request_id.as_deref() == Some("abc123"));
        assert!(hit.is_some(), "telemetry record with provider_request_id=abc123 not found; have: {:?}", *traces);
        let hit = hit.unwrap();
        assert!(hit.latency_ms.unwrap_or(0) > 0);

        // Span assertion: http.request
        let spans = span_store.spans.lock().unwrap();
        let mut found = false;
        for (_id, data) in spans.iter() {
            if data.name == "http.request" {
                let fields = data.fields.lock().unwrap();
                let url = fields.get("url").cloned().unwrap_or_default();
                if url.contains("/chat") {
                    assert_eq!(fields.get("provider").map(String::as_str).unwrap_or(""), "\"http\"");
                    assert_eq!(fields.get("method").map(String::as_str).unwrap_or(""), "\"POST\"");
                    assert_eq!(fields.get("status").map(String::as_str).unwrap_or(""), "200");
                    let prid = fields.get("provider_request_id").cloned().unwrap_or_default();
                    assert_eq!(prid.trim_matches('"'), "abc123");
                    assert!(fields.get("latency_ms").is_some());
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "http.request span with /chat not found; have: {spans:?}");
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

    #[tokio::test(flavor = "current_thread")]
    async fn post_json_503_maps_to_unavailable() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
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

        // Telemetry assertion
        let traces = TRACE_LOGS.lock().unwrap();
        assert!(!traces.is_empty());
        let hit = traces.iter().rev().find(|t| t.provider.as_deref() == Some("http") && t.error_kind.as_deref() == Some("http_error"));
        assert!(hit.is_some(), "telemetry record with http_error not found; have: {:?}", *traces);
        let hit = hit.unwrap();
        assert!(hit.latency_ms.unwrap_or(0) > 0);

        // Span assertion: http.request 503
        let spans = span_store.spans.lock().unwrap();
        let mut found = false;
        for (_id, data) in spans.iter() {
            if data.name == "http.request" {
                let fields = data.fields.lock().unwrap();
                let url = fields.get("url").cloned().unwrap_or_default();
                if url.contains("/chat") {
                    assert_eq!(fields.get("status").map(String::as_str).unwrap_or(""), "503");
                    let ek = fields.get("error_kind").cloned().unwrap_or_default();
                    assert!(ek.contains("http_error"));
                    assert!(fields.get("latency_ms").is_some());
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "http.request 503 span not found; have: {spans:?}");
    }

    #[tokio::test]
    async fn post_json_200_bad_json_maps_to_provider_error() {
        let span_store = crate::telemetry::test_span::install_capture();
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

        // Spans: http.request records decode error
        let spans = span_store.spans.lock().unwrap();
        let mut found = false;
        for (_id, data) in spans.iter() {
            if data.name == "http.request" {
                let fields = data.fields.lock().unwrap();
                let url = fields.get("url").cloned().unwrap_or_default();
                if url.contains("/chat") {
                    assert_eq!(fields.get("status").map(String::as_str).unwrap_or(""), "200");
                    let ek = fields.get("error_kind").cloned().unwrap_or_default();
                    assert!(ek.contains("decode_error"));
                    assert!(fields.get("latency_ms").is_some());
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "http.request decode_error span not found; have: {spans:?}");
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
        install_trace_sink();
        // Use an unreachable loopback port to force a connect error deterministically
        let url = "http://127.0.0.1:9/chat";
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let err = client.post_json::<_, serde_json::Value>(
            url,
            &serde_json::json!({"msg":"hi"}),
            &[],
            &ctx,
        ).await.unwrap_err();
        match err {
            AiProxyError::ProviderUnavailable { .. } => {}
            other => panic!("expected ProviderUnavailable, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn post_sse_lines_emits_telemetry_on_completion() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
        let server = MockServer::start();
        // Simulate SSE with two chunks then DONE
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n\
data: [DONE]\n\n";
        let _m = server.mock(|when, then| {
            when.method(POST).path("/sse");
            then.status(200)
                .header("content-type", "text/event-stream")
                .header("x-request-id", "sse123")
                .body(sse_body);
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let (mut stream, _pid) = client.post_sse_lines(
            &format!("{}/sse", server.base_url()),
            &serde_json::json!({"stream": true}),
            &[],
            &ctx,
        ).await.expect("sse ok");

        use futures_util::StreamExt;
        while let Some(_line) = stream.next().await { /* drain */ }

        let traces = TRACE_LOGS.lock().unwrap();
        assert!(!traces.is_empty());
        let hit = traces.iter().rev().find(|t| t.provider.as_deref() == Some("http")
            && t.provider_request_id.as_deref() == Some("sse123"));
        assert!(hit.is_some(), "telemetry record with provider_request_id=sse123 not found; have: {:?}", *traces);
        let hit = hit.unwrap();
        assert!(hit.latency_ms.unwrap_or(0) > 0);

        // Spans: http.request and sse.stream
        let spans = span_store.spans.lock().unwrap();
        let mut saw_http = false;
        let mut saw_sse = false;
        for (_id, data) in spans.iter() {
            let fields = data.fields.lock().unwrap();
            match data.name.as_str() {
                "http.request" => {
                    let url = fields.get("url").cloned().unwrap_or_default();
                    if url.contains("/sse") {
                        assert_eq!(fields.get("status").map(String::as_str).unwrap_or(""), "200");
                        let prid = fields.get("provider_request_id").cloned().unwrap_or_default();
                        assert_eq!(prid.trim_matches('"'), "sse123");
                        saw_http = true;
                    }
                }
                "sse.stream" => {
                    let prid = fields.get("provider_request_id").cloned().unwrap_or_default();
                    assert_eq!(prid.trim_matches('"'), "sse123");
                    assert!(fields.get("latency_ms").is_some());
                    saw_sse = true;
                }
                _ => {}
            }
        }
        assert!(saw_http, "http.request span for /sse not found");
        assert!(saw_sse, "sse.stream span not found");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_sse_lines_buffer_overflow_sets_error_kind() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
        let server = MockServer::start();
        // Construct a large chunk > MAX_SSE_BUFFER with no newline
        let big = "x".repeat(super::MAX_SSE_BUFFER + 1024);
        let _m = server.mock(|when, then| {
            when.method(POST).path("/sse-big");
            then.status(200)
                .header("content-type", "text/event-stream")
                .header("x-request-id", "big-1")
                .body(big.clone());
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let (mut stream, _pid) = client.post_sse_lines(
            &format!("{}/sse-big", server.base_url()),
            &serde_json::json!({"stream": true}),
            &[],
            &ctx,
        ).await.expect("sse ok");

        use futures_util::StreamExt;
        let first = stream.next().await.expect("one item");
        assert!(matches!(first, Err(AiProxyError::ProviderError { code, .. }) if code == "sse_buffer_overflow"));
        drop(stream); // trigger Drop and span close

        let spans = span_store.spans.lock().unwrap();
        let mut saw_err = false;
        for (_id, data) in spans.iter() {
            if data.name == "sse.stream" {
                let fields = data.fields.lock().unwrap();
                if let Some(kind) = fields.get("error_kind") {
                    assert!(kind.contains("sse_buffer_overflow"));
                    saw_err = true;
                    break;
                }
            }
        }
        assert!(saw_err, "sse.stream error_kind not recorded; have: {spans:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sse_server_closes_without_done_records_latency_once() {
        install_trace_sink();
        let span_store = crate::telemetry::test_span::install_capture();
        let server = MockServer::start();
        // Two deltas, then connection closes without [DONE]
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"A\"}}]}\n\n\
                        data: {\"choices\":[{\"delta\":{\"content\":\"B\"}}]}\n\n";
        let _m = server.mock(|when, then| {
            when.method(POST).path("/sse-close");
            then.status(200)
                .header("content-type", "text/event-stream")
                .header("x-request-id", "sse-close-1")
                .body(sse_body);
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let (mut stream, _pid) = client.post_sse_lines(
            &format!("{}/sse-close", server.base_url()),
            &serde_json::json!({"stream": true}),
            &[],
            &ctx,
        ).await.expect("sse ok");

        use futures_util::StreamExt;
        let mut count = 0usize;
        while let Some(_line) = stream.next().await { count += 1; }
        assert!(count >= 2);

        // Telemetry emitted once with latency
        let traces = TRACE_LOGS.lock().unwrap();
        let hits: Vec<_> = traces.iter().filter(|t| t.provider_request_id.as_deref() == Some("sse-close-1")).collect();
        assert_eq!(hits.len(), 1, "expected exactly one telemetry emit, got {}: {:?}", hits.len(), *traces);
        assert!(hits[0].latency_ms.unwrap_or(0) > 0);

        // sse.stream span latency present
        let spans = span_store.spans.lock().unwrap();
        let mut saw = false;
        for (_id, data) in spans.iter() {
            if data.name == "sse.stream" {
                let fields = data.fields.lock().unwrap();
                let prid = fields.get("provider_request_id").cloned().unwrap_or_default();
                if prid.trim_matches('"') == "sse-close-1" {
                    assert!(fields.get("latency_ms").is_some());
                    saw = true;
                    break;
                }
            }
        }
        assert!(saw, "sse.stream span for sse-close-1 not found; have: {spans:?}");
    }

    #[tokio::test]
    async fn post_json_429_parses_retry_after_numeric() {
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(POST).path("/limit");
            then.status(429)
                .header("Retry-After", "3")
                .body("slow down");
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx::default();
        let err = client
            .post_json::<_, serde_json::Value>(
                &format!("{}/limit", server.base_url()),
                &serde_json::json!({"msg":"hi"}),
                &[],
                &ctx,
            )
            .await
            .unwrap_err();
        match err {
            AiProxyError::RateLimited { retry_after, .. } => assert_eq!(retry_after, Some(3)),
            other => panic!("expected RateLimited with retry_after, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn sse_headers_include_accept_and_ctx_ids() {
        let server = MockServer::start();
        // We will assert on headers by capturing the request in httpmock
        let _m = server.mock(|when, then| {
            when.method(POST)
                .path("/sse-headers")
                .header("Accept", "text/event-stream")
                .header("X-Request-Id", "rid-1")
                .header("X-Turn-Id", "tid-1");
            then.status(200)
                .header("content-type", "text/event-stream")
                .header("x-request-id", "hdr-123")
                .body("data: {\"ok\":true}\n\n");
        });
        let client = HttpClient::new_default().expect("client");
        let ctx = RequestCtx { request_id: Some("rid-1"), turn_id: Some("tid-1"), idempotency_key: None };
        let (mut stream, _pid) = client.post_sse_lines(
            &format!("{}/sse-headers", server.base_url()),
            &serde_json::json!({"stream": true}),
            &[],
            &ctx,
        ).await.expect("sse ok");
        use futures_util::StreamExt; let _ = stream.next().await; // poke once
    }

    #[tokio::test]
    async fn request_id_candidates_are_extracted() {
        let ids = [
            ("x-request-id", "rid-A"),
            ("request-id", "rid-B"),
            ("x-amzn-requestid", "rid-C"),
            ("x-amz-request-id", "rid-D"),
            ("x-cdn-request-id", "rid-E"),
        ];
        for (hdr, val) in ids.iter() {
            let server = MockServer::start();
            let _m = server.mock(|when, then| {
                when.method(POST).path("/rid");
                then.status(200)
                    .header(*hdr, *val)
                    .json_body(json!({"ok": true}));
            });
            #[derive(serde::Deserialize)] struct Resp { ok: bool }
            let client = HttpClient::new_default().unwrap();
            let ctx = RequestCtx::default();
            let (resp, provider_id, _latency) = client
                .post_json::<_, Resp>(&format!("{}/rid", server.base_url()), &json!({}), &[], &ctx)
                .await
                .unwrap();
            assert!(resp.ok);
            assert_eq!(provider_id.as_deref(), Some(*val));
        }
    }
}
