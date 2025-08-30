use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreResult;
use crate::http_client::{HttpClient, RequestCtx};
use crate::model::{
    ChatMessage, ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, StopReason,
};
use crate::provider::{Capability, ChatProvider, EmbedProvider, ProviderCaps};

#[derive(Debug, Clone)]
pub struct OpenAI {
    http: HttpClient,
    base: String,
    org: Option<String>,
    name: String, // usually "openai"
    api_key: String,
}

impl OpenAI {
    pub fn new(http: HttpClient, api_key: String, base: String, org: Option<String>) -> Self {
        Self {
            http,
            api_key,
            base,
            org,
            name: "openai".into(),
        }
    }

    #[cfg(test)]
    pub fn new_for_tests(server_base: &str) -> Self {
        OpenAI::new(
            HttpClient::new_default().unwrap(),
            "test-key".into(),
            server_base.to_string(),
            None,
        )
    }

    fn headers(&self, _ctx: &RequestCtx<'_>) -> Vec<(String, String)> {
        let mut h = vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        if let Some(org) = &self.org {
            h.push(("OpenAI-Organization".into(), org.clone()));
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
        let (resp, provider_id, latency_ms) = self
            .http
            .post_json::<_, OAChatResp>(&url, &payload, &hdrs, &ctx)
            .await?;

        let text = resp
            .choices
            .get(0)
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        let stop_reason = resp
            .choices
            .get(0)
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
}

#[derive(Serialize)]
struct OAEmbedReq<'a> {
    model: &'a str,
    input: &'a [String],
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
        let payload = OAEmbedReq {
            model: &req.model,
            input: &req.inputs,
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
        &[Capability::Chat, Capability::Embed]
    }
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
}
