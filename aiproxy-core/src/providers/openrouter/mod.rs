use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CoreResult;
use crate::http_client::{HttpClient, RequestCtx};
use crate::model::{
    ChatMessage, ChatRequest, ChatResponse, EmbedRequest, EmbedResponse, StopReason,
};
use crate::provider::{Capability, ChatProvider, EmbedProvider, ProviderCaps};

#[derive(Debug, Clone)]
pub struct OpenRouter {
    http: HttpClient,
    base: String,
    name: String, // "openrouter"
    api_key: SecretString,
}

impl OpenRouter {
    pub fn new(http: HttpClient, api_key: SecretString, base: String) -> Self {
        Self {
            http,
            api_key,
            base,
            name: "openrouter".into(),
        }
    }

    #[cfg(test)]
    pub fn new_for_tests(server_base: &str) -> Self {
        OpenRouter::new(
            HttpClient::new_default().unwrap(),
            SecretString::new("test-key".into()),
            server_base.to_string(),
        )
    }

    fn headers(&self, _ctx: &RequestCtx<'_>) -> Vec<(String, String)> {
        vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", self.api_key.expose_secret()),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}

// ----- Wire structs (OpenRouter is OpenAI-compatible for these endpoints) -----
#[derive(Serialize)]
struct ORChatReq<'a> {
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
struct ORChatResp {
    id: String,
    choices: Vec<ORChoice>,
    usage: Option<ORUsage>,
}
#[derive(Deserialize)]
struct ORChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct ORUsage {
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
impl ChatProvider for OpenRouter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat(&self, req: ChatRequest) -> CoreResult<ChatResponse> {
        let payload = ORChatReq {
            model: &req.model,
            messages: &req.messages,
            temperature: req.temperature,
            top_p: req.top_p,
            max_tokens: req.max_output_tokens,
            stop: req.stop_sequences.clone(),
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
        let (resp, provider_id, latency_ms) = self
            .http
            .post_json::<_, ORChatResp>(&url, &payload, &hdrs, &ctx)
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
}

#[derive(Serialize)]
struct OREmbedReq<'a> {
    model: &'a str,
    input: &'a [String],
}
#[derive(Deserialize)]
struct OREmbedResp {
    data: Vec<ORVector>,
}
#[derive(Deserialize)]
struct ORVector {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbedProvider for OpenRouter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn embed(&self, req: EmbedRequest) -> CoreResult<EmbedResponse> {
        let payload = OREmbedReq {
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
            .post_json::<_, OREmbedResp>(&url, &payload, &hdrs, &ctx)
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

impl ProviderCaps for OpenRouter {
    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Chat, Capability::Embed]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatMessage, Role};
    use httpmock::{Method::POST, MockServer};
    use serde_json::json;

    #[tokio::test]
    async fn chat_200_maps_fields() {
        let server = MockServer::start();
        let provider = OpenRouter::new_for_tests(&server.base_url());
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(json!({
                "id": "req_123",
                "choices": [{ "message": {"role":"assistant", "content":"Hello via OR!"}, "finish_reason": "stop" }],
                "usage": {"prompt_tokens": 7, "completion_tokens": 3}
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
        assert_eq!(resp.text, "Hello via OR!");
        assert_eq!(resp.stop_reason, Some(StopReason::Stop));
        assert_eq!(resp.provider, "openrouter");
        assert_eq!(resp.usage_prompt, 7);
        assert_eq!(resp.usage_completion, 3);
    }

    #[tokio::test]
    async fn embed_200_maps_vectors() {
        let server = MockServer::start();
        let provider = OpenRouter::new_for_tests(&server.base_url());
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/embeddings");
            then.status(200).json_body(json!({
                "data": [ {"embedding": [0.11, 0.22]} ]
            }));
        });
        let req = EmbedRequest {
            model: "text-embedding-3-small".into(),
            inputs: vec!["hello".into()],
            client_key: None,
        };
        let resp = provider.embed(req).await.expect("embed ok");
        assert_eq!(resp.vectors.len(), 1);
        assert_eq!(resp.vectors[0].len(), 2);
        assert_eq!(resp.provider, "openrouter");
    }
}
