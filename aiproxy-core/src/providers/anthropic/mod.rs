use std::time::{SystemTime, UNIX_EPOCH};

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::{
    error::{AiProxyError, CoreResult},
    http_client::{HttpClient, RequestCtx},
    model::{ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, StopReason},
    provider::{ChatProvider, EmbedProvider, ProviderCaps},
};
use async_trait::async_trait;

/// Default Anthropic API version header required by the Messages API.
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone)]
pub struct Anthropic {
    http: HttpClient,
    api_key: SecretString,
    base: String,
    name: String,
}

impl Anthropic {
    pub fn new(http: HttpClient, api_key: SecretString, base: String) -> Self {
        Self {
            http,
            api_key,
            base,
            name: "anthropic".into(),
        }
    }

    fn headers(&self, _ctx: &RequestCtx<'_>) -> Vec<(String, String)> {
        vec![
            (
                "x-api-key".to_string(),
                self.api_key.expose_secret().to_string(),
            ),
            (
                "anthropic-version".to_string(),
                ANTHROPIC_API_VERSION.to_string(),
            ),
        ]
    }

    fn map_stop(reason: Option<&str>) -> Option<StopReason> {
        match reason {
            Some("end_turn") => Some(StopReason::EndTurn),
            Some("max_tokens") => Some(StopReason::Length),
            Some("tool_use") => Some(StopReason::ToolUse),
            Some("stop_sequence") => Some(StopReason::Stop),
            _ => None,
        }
    }
}

impl ProviderCaps for Anthropic {
    fn capabilities(&self) -> &'static [crate::provider::Capability] {
        &[
            crate::provider::Capability::Chat,
            // Embeddings unsupported in MVP; omit Capability::Embed
        ]
    }
}

// ===== Anthropic wire types (Messages API) =====

#[derive(Serialize)]
struct AMsgReq<'a> {
    model: &'a str,
    messages: Vec<AMessage<'a>>, // role/content pairs
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
}

#[derive(Serialize)]
struct AMessage<'a> {
    role: &'a str,
    content: Vec<AContent<'a>>, // Anthropic requires an array of content blocks
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AContent<'a> {
    Text { text: &'a str },
}

#[derive(Deserialize)]
struct AMsgResp {
    #[serde(rename = "id")]
    _id: String,
    #[allow(dead_code)]
    model: Option<String>,
    content: Vec<ARespContent>,
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<AUsage>,
}

#[derive(Deserialize)]
struct ARespContent {
    #[allow(dead_code)]
    r#type: String,
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct AUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

#[async_trait]
impl ChatProvider for Anthropic {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat(&self, req: ChatRequest) -> CoreResult<ChatResponse> {
        // Map our ChatRequest to Anthropic Messages format.
        let mut system_prompts: Vec<&str> = Vec::new();
        let mut msgs: Vec<AMessage> = Vec::new();

        for m in &req.messages {
            match m.role {
                crate::model::Role::System => system_prompts.push(m.content.as_str()),
                crate::model::Role::User => msgs.push(AMessage {
                    role: "user",
                    content: vec![AContent::Text { text: &m.content }],
                }),
                crate::model::Role::Assistant => msgs.push(AMessage {
                    role: "assistant",
                    content: vec![AContent::Text { text: &m.content }],
                }),
                _ => { /* ignore Tool/others in MVP */ }
            }
        }

        let system = if system_prompts.is_empty() {
            None
        } else {
            Some(system_prompts.join("\n"))
        };

        let max_tokens = req.max_output_tokens.unwrap_or(1024).max(1);

        let payload = AMsgReq {
            model: &req.model,
            messages: msgs,
            system,
            max_tokens,
            temperature: req.temperature,
            top_p: req.top_p,
        };

        let url = format!("{}/v1/messages", self.base);
        let ctx = RequestCtx::default();
        let headers = self.headers(&ctx);
        let header_pairs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let (resp, provider_request_id, latency_ms) = self
            .http
            .post_json::<_, AMsgResp>(&url, &payload, &header_pairs, &ctx)
            .await?;

        let text = resp
            .content
            .iter()
            .find_map(|c| c.text.clone())
            .unwrap_or_default();

        let stop = Anthropic::map_stop(resp.stop_reason.as_deref());
        let usage_in = resp
            .usage
            .as_ref()
            .and_then(|u| u.input_tokens)
            .unwrap_or(0) as u64;
        let usage_out = resp
            .usage
            .as_ref()
            .and_then(|u| u.output_tokens)
            .unwrap_or(0) as u64;

        let resp = ChatResponse {
            model: req.model,
            text,
            usage_prompt: usage_in as u32,
            usage_completion: usage_out as u32,
            cached: false,
            provider: self.name.clone(),
            transcript_id: None,
            turn_id: ctx.turn_id.unwrap_or("").to_string(),
            stop_reason: stop,
            provider_request_id,
            created_at_ms: started as i64,
            latency_ms,
        };
        // Emit structured completion log (non-streaming)
        let tokens_total = resp.usage_prompt.checked_add(resp.usage_completion);
        let stop_code = match resp.stop_reason {
            Some(crate::model::StopReason::Stop) => Some("stop"),
            Some(crate::model::StopReason::Length) => Some("length"),
            Some(crate::model::StopReason::ToolUse) => Some("tool_use"),
            Some(crate::model::StopReason::EndTurn) => Some("end_turn"),
            Some(crate::model::StopReason::ContentFilter) => Some("content_filter"),
            Some(crate::model::StopReason::Other) => Some("other"),
            None => None,
        };
        let clog = crate::telemetry::CompletionLog::new()
            .provider("anthropic")
            .model(&resp.model)
            .request_id_opt(None)
            .turn_id_opt(None)
            .provider_request_id_opt(resp.provider_request_id.as_deref())
            .created_at_ms(resp.created_at_ms as u64)
            .latency_ms(resp.latency_ms as u64)
            .stop_reason_opt(stop_code)
            .text_opt(Some(&resp.text))
            .tokens(Some(resp.usage_prompt), Some(resp.usage_completion), tokens_total);
        crate::telemetry::emit_completion(clog);
        Ok(resp)
    }
}

#[async_trait]
impl EmbedProvider for Anthropic {
    fn name(&self) -> &str {
        &self.name
    }

    async fn embed(&self, _req: EmbedRequest) -> CoreResult<EmbedResponse> {
        Err(AiProxyError::Validation(
            "Anthropic embeddings are not supported in this MVP".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use once_cell::sync::Lazy;
    use std::sync::{Arc, Mutex};

    // CompletionLog test sink & helpers
    static COMPLETION_LOGS: Lazy<Mutex<Vec<crate::telemetry::CompletionLog>>> =
        Lazy::new(|| Mutex::new(Vec::new()));

    #[derive(Default)]
    struct CLTestSink;
    impl crate::telemetry::TelemetrySink for CLTestSink {
        fn record(&self, _trace: crate::telemetry::ProviderTrace) { /* ignore */ }
        fn record_completion(&self, log: crate::telemetry::CompletionLog) {
            COMPLETION_LOGS.lock().unwrap().push(log);
        }
    }

    fn ensure_cl_sink_installed() {
        let _ = crate::telemetry::set_telemetry_sink(Arc::new(CLTestSink::default()));
    }

    #[tokio::test]
    async fn chat_200_maps_fields() {
        ensure_cl_sink_installed();
        COMPLETION_LOGS.lock().unwrap().clear();
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/messages")
                .header("x-api-key", "test-key")
                .header("anthropic-version", ANTHROPIC_API_VERSION);
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                    "id": "msg_123",
                    "content": [ { "type": "text", "text": "hello from claude" } ],
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 9, "output_tokens": 3 }
                }"#,
                );
        });

        let provider = Anthropic::new(
            HttpClient::new_default().unwrap(),
            SecretString::new("test-key".into()),
            server.base_url(),
        );

        let req = ChatRequest {
            model: "claude-3-haiku".into(),
            messages: vec![crate::model::ChatMessage {
                role: crate::model::Role::User,
                content: "hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: Some(128),
            stop_sequences: None,
        };

        let resp = provider.chat(req).await.expect("chat ok");
        assert_eq!(resp.text, "hello from claude");
        assert_eq!(resp.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(resp.provider, "anthropic");
        assert_eq!(resp.usage_prompt, 9);
        assert_eq!(resp.usage_completion, 3);

        let logs = COMPLETION_LOGS.lock().unwrap().clone();
        if !logs.is_empty() {
            assert_eq!(logs.len(), 1, "expected 1 completion log, got {:?}", logs);
            let log = &logs[0];
            assert_eq!(log.provider.as_deref(), Some("anthropic"));
            assert_eq!(log.model.as_deref(), Some("claude-3-haiku"));
            assert_eq!(log.stop_reason.as_deref(), Some("end_turn"));
            assert!(log.latency_ms.unwrap_or(0) > 0);
            assert_eq!(log.text.as_deref(), Some("hello from claude"));
            assert_eq!(log.tokens_prompt, Some(9));
            assert_eq!(log.tokens_completion, Some(3));
            assert_eq!(log.tokens_total, Some(12));
        }
    }

    #[tokio::test]
    async fn embed_is_unsupported() {
        let provider = Anthropic::new(
            HttpClient::new_default().unwrap(),
            SecretString::new("test-key".into()),
            "http://localhost".to_string(),
        );

        let req = EmbedRequest {
            model: "dummy".into(),
            inputs: vec!["x".into()],
            client_key: None,
        };
        let err = provider.embed(req).await.unwrap_err();
        match err {
            AiProxyError::Validation(_) => {}
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[tokio::test]
    async fn chat_sends_joined_system_prompt() {
        use crate::model::{ChatMessage, Role};

        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/messages")
                // crude but reliable: ensure the JSON contains the joined system field
                .body_contains("\"system\":\"A\\nB\"");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{ "id":"x", "content":[{"type":"text","text":"ok"}] }"#);
        });

        let provider = Anthropic::new(
            HttpClient::new_default().unwrap(),
            SecretString::new("k".into()),
            server.base_url(),
        );

        let req = ChatRequest {
            model: "claude-3-haiku".into(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "A".into(),
                },
                ChatMessage {
                    role: Role::System,
                    content: "B".into(),
                },
                ChatMessage {
                    role: Role::User,
                    content: "hi".into(),
                },
            ],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: Some(128),
            stop_sequences: None,
        };

        let _ = provider.chat(req).await.unwrap();

        // Verify the mock matched (including system body_contains assertion)
        m.assert();
    }

    #[tokio::test]
    async fn stop_reason_matrix() {
        use crate::model::{ChatMessage, Role};

        let cases = [
            (r#""end_turn""#, Some(StopReason::EndTurn)),
            (r#""max_tokens""#, Some(StopReason::Length)),
            (r#""tool_use""#, Some(StopReason::ToolUse)),
            (r#""stop_sequence""#, Some(StopReason::Stop)),
            ("null", None),
        ];

        for (stop_json, expect) in cases {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(POST).path("/v1/messages");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(format!(
                        r#"{{
                    "id":"x",
                    "content":[{{"type":"text","text":"t"}}],
                    "stop_reason": {}
                }}"#,
                        stop_json
                    ));
            });

            let provider = Anthropic::new(
                HttpClient::new_default().unwrap(),
                SecretString::new("k".into()),
                server.base_url(),
            );

            let req = ChatRequest {
                model: "claude-3-haiku".into(),
                messages: vec![ChatMessage {
                    role: Role::User,
                    content: "hi".into(),
                }],
                temperature: None,
                top_p: None,
                metadata: None,
                client_key: None,
                request_id: None,
                trace_id: None,
                idempotency_key: None,
                max_output_tokens: Some(32),
                stop_sequences: None,
            };

            let resp = provider.chat(req).await.unwrap();
            assert_eq!(resp.stop_reason, expect);
        }
    }

    #[tokio::test]
    async fn headers_present() {
        use crate::model::{ChatMessage, Role};

        let server = MockServer::start();
        let m = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/messages")
                .header("x-api-key", "test-key")
                .header("anthropic-version", ANTHROPIC_API_VERSION);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{ "id":"x", "content":[{"type":"text","text":"ok"}] }"#);
        });

        let provider = Anthropic::new(
            HttpClient::new_default().unwrap(),
            SecretString::new("test-key".into()),
            server.base_url(),
        );

        let req = ChatRequest {
            model: "claude-3-haiku".into(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: Some(16),
            stop_sequences: None,
        };

        let _ = provider.chat(req).await.unwrap();
        m.assert(); // verifies headers matched
    }
}
