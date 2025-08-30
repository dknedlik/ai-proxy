use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreResult;
use crate::http_client::{HttpClient, RequestCtx};
use crate::model::{
    ChatMessage, ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, StopReason,
};
use crate::provider::{Capability, ChatProvider, EmbedProvider, ProviderCaps};
use crate::stream::{BoxStreamEv, StreamEvent};
use secrecy::{ExposeSecret, SecretString};

#[derive(Debug, Clone)]
pub struct OpenAI {
    http: HttpClient,
    base: String,
    org: Option<String>,
    project: Option<String>,
    name: String, // usually "openai"
    api_key: SecretString,
}

impl OpenAI {
    pub fn new(
        http: HttpClient,
        api_key: SecretString,
        base: String,
        org: Option<String>,
        project: Option<String>,
    ) -> Self {
        Self {
            http,
            api_key,
            base,
            org,
            project,
            name: "openai".into(),
        }
    }

    #[cfg(test)]
    pub fn new_for_tests(server_base: &str) -> Self {
        OpenAI::new(
            HttpClient::new_default().unwrap(),
            SecretString::new("test-key".into()),
            server_base.to_string(),
            None,
            None,
        )
    }

    fn headers(&self, _ctx: &RequestCtx<'_>) -> Vec<(String, String)> {
        let mut h = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", self.api_key.expose_secret()),
        )];
        if let Some(org) = &self.org {
            h.push(("OpenAI-Organization".into(), org.clone()));
        }
        if let Some(project) = &self.project {
            h.push(("OpenAI-Project".into(), project.clone()));
        }
        h
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}

// ---- Wire structs (minimal) ----
#[derive(Serialize)]
struct OAChatReq<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Deserialize)]
struct OAChatResp {
    id: String,
    choices: Vec<OAChoice>,
    usage: Option<OAUsage>,
}

#[derive(Deserialize)]
struct OAChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OAUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

// ---- Streaming wire structs (SSE "chunk" shape) ----
// Temporary: unused until SSE transport is wired
#[allow(dead_code)]
#[derive(Deserialize)]
struct OAChatStreamChunk {
    id: Option<String>,
    choices: Vec<OAStreamChoice>,
}

// Temporary: unused until SSE transport is wired
#[allow(dead_code)]
#[derive(Deserialize)]
struct OAStreamChoice {
    #[serde(default)]
    delta: OAStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

// Temporary: unused until SSE transport is wired
#[allow(dead_code)]
#[derive(Default, Deserialize)]
struct OAStreamDelta {
    #[serde(default)]
    content: Option<String>,
    // NOTE: extend here if/when we support tool calls, role changes, etc.
}

fn map_finish(s: Option<&str>) -> Option<StopReason> {
    match s {
        Some("stop") => Some(StopReason::Stop),
        Some("length") => Some(StopReason::Length),
        Some("content_filter") => Some(StopReason::ContentFilter),
        Some("tool_calls") => Some(StopReason::ToolUse),
        Some(_) => Some(StopReason::Other),
        None => None,
    }
}

#[async_trait]
impl ChatProvider for OpenAI {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat(&self, req: ChatRequest) -> CoreResult<ChatResponse> {
        let payload = OAChatReq {
            model: &req.model,
            messages: &req.messages,
            temperature: req.temperature,
            top_p: req.top_p,
            max_tokens: req.max_output_tokens,
            stop: req.stop_sequences.clone(),
            stream: None,
        };
        let ctx = RequestCtx {
            request_id: req.request_id.as_deref(),
            turn_id: req.trace_id.as_deref(), // we’ll thread a real turn_id at the HTTP layer later
            idempotency_key: req.idempotency_key.as_deref(),
        };
        let owned_headers = self.headers(&ctx);
        let hdrs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{}/v1/chat/completions", self.base);
        if std::env::var("AIPROXY_DEBUG_HTTP").ok().as_deref() == Some("1") {
            eprintln!("CHAT url: {}", url);
            for (k, v) in &hdrs {
                if k.eq_ignore_ascii_case("authorization") && v.starts_with("Bearer ") {
                    let raw = &v["Bearer ".len()..];
                    let masked = if raw.len() > 10 {
                        format!("Bearer {}****{}", &raw[..6], &raw[raw.len() - 4..])
                    } else {
                        "Bearer ****".to_string()
                    };
                    eprintln!("CHAT header: {}: {}", k, masked);
                } else {
                    eprintln!("CHAT header: {}: {}", k, v);
                }
            }
            eprintln!(
                "CHAT payload: {}",
                serde_json::to_string(&payload).unwrap_or_default()
            );
        }
        let (resp, provider_id, latency_ms) = self
            .http
            .post_json::<_, OAChatResp>(&url, &payload, &hdrs, &ctx)
            .await?;

        let text = resp
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        let stop_reason = resp
            .choices
            .first()
            .and_then(|c| map_finish(c.finish_reason.as_deref()));
        let (usage_p, usage_c) = resp
            .usage
            .map(|u| (u.prompt_tokens, u.completion_tokens))
            .unwrap_or((0, 0));

        Ok(ChatResponse {
            model: req.model,
            text,
            usage_prompt: usage_p,
            usage_completion: usage_c,
            cached: false,
            provider: self.name.clone(),
            transcript_id: None,
            turn_id: req.request_id.unwrap_or_else(|| "turn".into()),
            stop_reason,
            provider_request_id: provider_id.or(Some(resp.id)),
            created_at_ms: Self::now_ms(),
            latency_ms,
        })
    }

    async fn chat_stream_events(&self, req: ChatRequest) -> CoreResult<BoxStreamEv> {
        // Build payload with stream=true, initiate SSE
        let payload = OAChatReq {
            model: &req.model,
            messages: &req.messages,
            temperature: req.temperature,
            top_p: req.top_p,
            max_tokens: req.max_output_tokens,
            stop: req.stop_sequences.clone(),
            stream: Some(true),
        };
        let ctx = RequestCtx {
            request_id: req.request_id.as_deref(),
            turn_id: req.trace_id.as_deref(),
            idempotency_key: req.idempotency_key.as_deref(),
        };
        let owned_headers = self.headers(&ctx);
        let hdrs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{}/v1/chat/completions", self.base);

        let mut sse = self.http.post_sse_lines(&url, &payload, &hdrs, &ctx).await?;

        // Bridge SSE → StreamEvent via mpsc channel
        use futures::channel::mpsc;
        use futures_util::StreamExt;
        let (tx, rx) = mpsc::unbounded::<StreamEvent>();

        tokio::spawn(async move {
            let mut sent_stop = false;
            while let Some(line_res) = sse.next().await {
                match line_res {
                    Ok(line) => {
                        let raw = line.line.trim();
                        if raw == "data: [DONE]" { break; }
                        if let Some(rest) = raw.strip_prefix("data:") {
                            let json = rest.trim_start();
                            if json.is_empty() { continue; }
                            if let Ok(chunk) = serde_json::from_str::<OAChatStreamChunk>(json)
                                && let Some(choice) = chunk.choices.first()
                            {
                                if let Some(ref txt) = choice.delta.content {
                                    let _ = tx.unbounded_send(StreamEvent::DeltaText(txt.clone()));
                                }
                                if !sent_stop && choice.finish_reason.is_some() {
                                    let _ = tx.unbounded_send(StreamEvent::Stop { reason: map_finish(choice.finish_reason.as_deref()) });
                                    sent_stop = true;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.unbounded_send(StreamEvent::Error(e));
                        return; // terminal
                    }
                }
            }
            if !sent_stop {
                let _ = tx.unbounded_send(StreamEvent::Stop { reason: None });
            }
        });

        Ok(Box::pin(rx))
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum OAInput<'a> {
    Many(&'a [String]),
}

#[derive(Serialize)]
struct OAEmbedReq<'a> {
    model: &'a str,
    input: OAInput<'a>,
}

#[derive(Deserialize)]
struct OAEmbedResp {
    data: Vec<OAVector>,
}

#[derive(Deserialize)]
struct OAVector {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbedProvider for OpenAI {
    fn name(&self) -> &str {
        &self.name
    }

    async fn embed(&self, req: EmbedRequest) -> CoreResult<EmbedResponse> {
        // Always send array form for maximum compatibility
        let payload = OAEmbedReq {
            model: &req.model,
            input: OAInput::Many(&req.inputs),
        };
        let ctx = RequestCtx {
            request_id: None,
            turn_id: None,
            idempotency_key: req.client_key.as_deref(),
        };
        let owned_headers = self.headers(&ctx);
        let hdrs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{}/v1/embeddings", self.base);
        if std::env::var("AIPROXY_DEBUG_HTTP").ok().as_deref() == Some("1") {
            eprintln!("EMBED url: {}", url);
            for (k, v) in &hdrs {
                if k.eq_ignore_ascii_case("authorization") && v.starts_with("Bearer ") {
                    let raw = &v["Bearer ".len()..];
                    let masked = if raw.len() > 10 {
                        format!("Bearer {}****{}", &raw[..6], &raw[raw.len() - 4..])
                    } else {
                        "Bearer ****".to_string()
                    };
                    eprintln!("EMBED header: {}: {}", k, masked);
                } else {
                    eprintln!("EMBED header: {}: {}", k, v);
                }
            }
            eprintln!(
                "EMBED payload: {}",
                serde_json::to_string(&payload).unwrap_or_default()
            );
        }
        let (resp, _provider_id, _lat) = self
            .http
            .post_json::<_, OAEmbedResp>(&url, &payload, &hdrs, &ctx)
            .await?;
        let vectors = resp.data.into_iter().map(|d| d.embedding).collect();
        Ok(EmbedResponse {
            model: req.model,
            vectors,
            usage: 0,
            cached: false,
            provider: self.name.clone(),
        })
    }
}

impl ProviderCaps for OpenAI {
    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Chat, Capability::ChatStream, Capability::Embed]
    }
}

#[tokio::test]
async fn embed_posts_model_and_input_shape() {
    use httpmock::prelude::*;

    let server = MockServer::start();
    let provider = OpenAI::new_for_tests(&server.base_url());

    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/embeddings")
            .body_contains("\"model\":\"text-embedding-3-small\"")
            .body_contains("\"input\"");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{ "data": [ { "embedding": [0.1, 0.2] } ] }"#);
    });

    let req = EmbedRequest {
        model: "text-embedding-3-small".into(),
        inputs: vec!["hello".into()],
        client_key: None,
    };
    let _ = provider.embed(req).await.expect("embed ok");

    m.assert();
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;
    use serde_json::json;

    use crate::model::{ChatMessage, Role};

    #[tokio::test]
    async fn chat_200_maps_fields() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());

        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(json!({
                "id": "cmpl_123",
                "choices": [{
                    "message": {"role":"assistant", "content":"Hello!"},
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            }));
        });

        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: Some(1.0),
            top_p: Some(1.0),
            metadata: None,
            client_key: None,
            request_id: Some("turn-1".into()),
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: Some(128),
            stop_sequences: None,
        };

        let resp = provider.chat(req).await.expect("chat ok");
        assert_eq!(resp.text, "Hello!");
        assert_eq!(resp.stop_reason, Some(StopReason::Stop));
        assert_eq!(resp.usage_prompt, 10);
        assert_eq!(resp.usage_completion, 5);
        assert_eq!(resp.provider, "openai");
        assert_eq!(resp.provider_request_id, Some("cmpl_123".into()));
    }

    #[tokio::test]
    async fn embed_200_maps_vectors() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());

        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/embeddings");
            then.status(200).json_body(json!({
                "data": [
                    {"embedding": [0.1, 0.2]},
                    {"embedding": [0.3, 0.4]}
                ]
            }));
        });

        let req = EmbedRequest {
            model: "text-embedding-3-small".into(),
            inputs: vec!["hello".into(), "world".into()],
            client_key: None,
        };
        let resp = provider.embed(req).await.expect("embed ok");
        assert_eq!(resp.vectors.len(), 2);
        assert_eq!(resp.vectors[0].len(), 2);
        assert_eq!(resp.provider, "openai");
    }

    #[tokio::test]
    async fn chat_429_is_rate_limited() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());

        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(429).body("limit");
        });

        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };

        let err = provider.chat(req).await.unwrap_err();
        matches!(err, crate::error::AiProxyError::RateLimited { .. });
    }

    #[tokio::test]
    async fn chat_finish_reason_matrix() {
        // Helper to run a single isolated case on its own server to avoid mock overlap
        async fn run_case(finish: &str, expected: StopReason) {
            let server = MockServer::start();
            let provider = OpenAI::new_for_tests(&server.base_url());
            let _m = server.mock(|when, then| {
                when.method(POST).path("/v1/chat/completions");
                then.status(200).json_body(json!({
                    "id": "cmpl_x",
                    "choices": [{
                        "message": {"role":"assistant", "content":"hi"},
                        "finish_reason": finish
                    }]
                }));
            });
            let req = ChatRequest {
                model: "gpt-4o".into(),
                messages: vec![ChatMessage {
                    role: Role::User,
                    content: "Hi".into(),
                }],
                temperature: None,
                top_p: None,
                metadata: None,
                client_key: None,
                request_id: None,
                trace_id: None,
                idempotency_key: None,
                max_output_tokens: None,
                stop_sequences: None,
            };
            let resp = provider.chat(req).await.expect("chat ok");
            assert_eq!(resp.stop_reason, Some(expected));
        }

        run_case("stop", StopReason::Stop).await;
        run_case("length", StopReason::Length).await;
        run_case("content_filter", StopReason::ContentFilter).await;
        run_case("tool_calls", StopReason::ToolUse).await;

        // Unknown reason maps to Other
        run_case("weird_reason", StopReason::Other).await;
    }

    #[tokio::test]
    async fn chat_empty_choices_yields_defaults() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());

        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(json!({
                "id": "cmpl_empty",
                "choices": []
            }));
        });

        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let resp = provider.chat(req).await.expect("chat ok");
        assert_eq!(resp.text, "");
        assert_eq!(resp.stop_reason, None);
    }

    #[tokio::test]
    async fn chat_missing_usage_defaults_to_zero() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());

        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(json!({
                "id": "cmpl_no_usage",
                "choices": [{
                    "message": {"role":"assistant", "content":"ok"},
                    "finish_reason": "stop"
                }]
                // no usage field
            }));
        });

        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };

        let resp = provider.chat(req).await.expect("chat ok");
        assert_eq!(resp.usage_prompt, 0);
        assert_eq!(resp.usage_completion, 0);
    }

    use crate::error::AiProxyError;

    #[tokio::test]
    async fn chat_429_maps_to_rate_limited_none() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(429).body("limit");
        });
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let err = provider.chat(req).await.unwrap_err();
        match err {
            AiProxyError::RateLimited {
                provider,
                retry_after,
            } => {
                assert_eq!(provider, "http");
                assert_eq!(retry_after, None);
            }
            other => panic!("expected RateLimited, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn chat_429_with_retry_after_maps_to_rate_limited_some() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(429).header("Retry-After", "2").body("limit");
        });
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let err = provider.chat(req).await.unwrap_err();
        match err {
            AiProxyError::RateLimited { retry_after, .. } => assert_eq!(retry_after, Some(2)),
            other => panic!("expected RateLimited, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn chat_503_maps_to_provider_unavailable() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(503).body("down");
        });
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let err = provider.chat(req).await.unwrap_err();
        assert!(matches!(err, AiProxyError::ProviderUnavailable { .. }));
    }

    #[tokio::test]
    async fn chat_400_maps_to_provider_error_truncated() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());
        let big = "x".repeat(1000);
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(400).body(big);
        });
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let err = provider.chat(req).await.unwrap_err();
        match err {
            AiProxyError::ProviderError { code, message, .. } => {
                assert_eq!(code, "400");
                assert!(message.ends_with("..."));
                assert!(message.len() <= 303); // "..." after 300 chars
            }
            other => panic!("expected ProviderError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn chat_200_bad_json_maps_to_provider_error() {
        let server = MockServer::start();
        let provider = OpenAI::new_for_tests(&server.base_url());
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).body("not-json");
        });
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let err = provider.chat(req).await.unwrap_err();
        match err {
            AiProxyError::ProviderError { code, message, .. } => {
                assert_eq!(code, "200");
                assert!(message.starts_with("json decode error"));
            }
            other => panic!("expected ProviderError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn chat_network_error_maps_to_unavailable() {
        let provider = OpenAI::new_for_tests("http://127.0.0.1:9");
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let err = provider.chat(req).await.unwrap_err();
        assert!(matches!(err, AiProxyError::ProviderUnavailable { .. }));
    }

    #[tokio::test]
    async fn chat_streaming_sse_happy_path() {
        use std::sync::{Arc, Mutex};

        let server = MockServer::start();
        // Simulate an SSE body with two deltas, a stop, then [DONE]
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}] }\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}] }\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let _m = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(sse_body);
        });

        let provider = OpenAI::new_for_tests(&server.base_url());
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage { role: Role::User, content: "Hi".into() }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };

        let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_stop: Arc<Mutex<Option<StopReason>>> = Arc::new(Mutex::new(None));

        let d_clone = deltas.clone();
        let s_clone = seen_stop.clone();
        provider
            .chat_streaming_sse(
                req,
                move |txt| {
                    d_clone.lock().unwrap().push(txt.to_string());
                },
                move |reason| {
                    *s_clone.lock().unwrap() = reason;
                },
            )
            .await
            .expect("stream ok");

        let d = deltas.lock().unwrap().clone();
        assert_eq!(d, vec!["Hel".to_string(), "lo".to_string()]);
        let stop = *seen_stop.lock().unwrap();
        assert_eq!(stop, Some(StopReason::Stop));
    }

    #[tokio::test]
    async fn chat_streaming_sse_ignores_odd_lines_and_single_stop() {
        use futures_util::stream;
        use std::sync::{Arc, Mutex};

        // Build a synthetic SSE line stream with blanks, comments, and proper data lines
        let lines = vec![
            Ok(crate::http_client::SseLine { line: "".into() }),
            Ok(crate::http_client::SseLine { line: ":heartbeat".into() }),
            Ok(crate::http_client::SseLine { line: "event: ping".into() }),
            Ok(crate::http_client::SseLine { line: "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}".into() }),
            Ok(crate::http_client::SseLine { line: "data:{\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}".into() }), // no space after colon
            Ok(crate::http_client::SseLine { line: "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}".into() }),
            Ok(crate::http_client::SseLine { line: "data: [DONE]".into() }),
        ];
        let sse = stream::iter(lines);

        let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_stop: Arc<Mutex<Option<StopReason>>> = Arc::new(Mutex::new(None));

        let d_clone = deltas.clone();
        let s_clone = seen_stop.clone();

        OpenAI::drive_openai_sse(
            sse,
            move |txt| d_clone.lock().unwrap().push(txt.to_string()),
            move |reason| *s_clone.lock().unwrap() = reason,
        )
        .await
        .expect("stream ok");

        let d = deltas.lock().unwrap().clone();
        assert_eq!(d, vec!["Hel".to_string(), "lo".to_string()]);
        let stop = *seen_stop.lock().unwrap();
        assert_eq!(stop, Some(StopReason::Stop));
    }

    #[tokio::test]
    async fn chat_streaming_sse_no_explicit_stop_yields_none() {
        use futures_util::stream;
        use std::sync::{Arc, Mutex};

        // Deltas followed by [DONE], no explicit finish_reason
        let lines = vec![
            Ok(crate::http_client::SseLine { line: "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}".into() }),
            Ok(crate::http_client::SseLine { line: "data: [DONE]".into() }),
        ];
        let sse = stream::iter(lines);

        let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_stop: Arc<Mutex<Option<StopReason>>> = Arc::new(Mutex::new(None));
        let d_clone = deltas.clone();
        let s_clone = seen_stop.clone();
        OpenAI::drive_openai_sse(
            sse,
            move |txt| d_clone.lock().unwrap().push(txt.to_string()),
            move |reason| *s_clone.lock().unwrap() = reason,
        )
        .await
        .expect("stream ok");

        let d = deltas.lock().unwrap().clone();
        assert_eq!(d, vec!["Hi".to_string()]);
        let stop = *seen_stop.lock().unwrap();
        assert_eq!(stop, None);
    }

    #[tokio::test]
    async fn chat_streaming_sse_multiple_finish_reasons_only_one_stop() {
        use futures_util::stream;
        use std::sync::{Arc, Mutex};

        let lines = vec![
            Ok(crate::http_client::SseLine { line: "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}".into() }),
            Ok(crate::http_client::SseLine { line: "data: {\"choices\":[{\"finish_reason\":\"length\"}]}".into() }),
            Ok(crate::http_client::SseLine { line: "data: [DONE]".into() }),
        ];
        let sse = stream::iter(lines);

        let stop_calls: Arc<Mutex<Vec<Option<StopReason>>>> = Arc::new(Mutex::new(Vec::new()));
        let s_clone = stop_calls.clone();
        OpenAI::drive_openai_sse(sse, |_txt| {}, move |reason| s_clone.lock().unwrap().push(reason))
            .await
            .expect("stream ok");
        let calls = stop_calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], Some(StopReason::Stop));
    }

    #[tokio::test]
    async fn chat_streaming_sse_stream_error_bubbles_and_no_stop() {
        use futures_util::stream::{self, StreamExt};
        use crate::error::AiProxyError;

        // Build a stream that yields a line, then an error
        let s1 = stream::iter(vec![Ok(crate::http_client::SseLine { line: "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}".into() })]);
        let s2 = stream::iter(vec![Err(AiProxyError::ProviderUnavailable { provider: "http".into() })]);
        let sse = s1.chain(s2);

        let mut saw_stop = false;
        let res = OpenAI::drive_openai_sse(
            sse,
            |_txt| {},
            |_reason| { saw_stop = true; },
        )
        .await;

        assert!(res.is_err());
        assert!(!saw_stop, "on_stop should not be called on error path");
    }
}

impl OpenAI {
    /// Experimental: Streaming chat over SSE.
    /// Calls `on_text_delta` for each content delta chunk and `on_stop` once when finish_reason arrives.
    /// This is a thin wrapper intended to map OpenAI's SSE format into simple text deltas.
    pub async fn chat_streaming_sse<F, G>(&self, req: ChatRequest, on_text_delta: F, on_stop: G) -> CoreResult<()>
    where
        F: FnMut(&str) + Send,
        G: FnMut(Option<StopReason>) + Send,
    {
        // Build payload with stream=true
        let payload = OAChatReq {
            model: &req.model,
            messages: &req.messages,
            temperature: req.temperature,
            top_p: req.top_p,
            max_tokens: req.max_output_tokens,
            stop: req.stop_sequences.clone(),
            stream: Some(true),
        };
        let ctx = RequestCtx {
            request_id: req.request_id.as_deref(),
            turn_id: req.trace_id.as_deref(),
            idempotency_key: req.idempotency_key.as_deref(),
        };
        let owned_headers = self.headers(&ctx);
        let hdrs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{}/v1/chat/completions", self.base);
        // Stream SSE lines and forward text deltas
        let sse = self
            .http
            .post_sse_lines(&url, &payload, &hdrs, &ctx)
            .await?;

        Self::drive_openai_sse(sse, on_text_delta, on_stop).await
    }

    // Internal helper to drive an SSE line stream and invoke callbacks.
    // Split out for easier unit testing without a real HTTP server.
    pub(crate) async fn drive_openai_sse<St, F, G>(
        mut sse: St,
        mut on_text_delta: F,
        mut on_stop: G,
    ) -> CoreResult<()>
    where
        St: futures_util::stream::Stream<Item = CoreResult<crate::http_client::SseLine>> + Unpin,
        F: FnMut(&str) + Send,
        G: FnMut(Option<StopReason>) + Send,
    {
        use futures_util::StreamExt;

        let mut sent_stop = false;
        while let Some(line) = sse.next().await {
            let line = line?;
            let raw = line.line.trim();

            // OpenAI terminator
            if raw == "data: [DONE]" {
                break;
            }

            // Accept both "data:..." and "data: ..." variants
            if let Some(rest) = raw.strip_prefix("data:") {
                let json = rest.trim_start();
                if json.is_empty() { continue; }
                if let Ok(chunk) = serde_json::from_str::<OAChatStreamChunk>(json)
                    && let Some(choice) = chunk.choices.first()
                {
                    if let Some(ref txt) = choice.delta.content {
                        on_text_delta(txt);
                    }
                    if !sent_stop && choice.finish_reason.is_some() {
                        on_stop(map_finish(choice.finish_reason.as_deref()));
                        sent_stop = true;
                    }
                }
            }
            // Ignore other SSE lines (e.g., comments/heartbeats)
        }

        if !sent_stop {
            on_stop(None);
        }
        Ok(())
    }
}
