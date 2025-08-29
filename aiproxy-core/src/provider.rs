use async_trait::async_trait;

use crate::error::CoreResult;
use crate::model::{ChatRequest, ChatResponse, EmbedRequest, EmbedResponse};

/// Capability marker for providers.
/// Used to advertise what verbs a provider supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    Chat,
    ChatStream,
    Embed,
    Transcribe,
    Moderate,
    Rerank,
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn chat(&self, req: ChatRequest) -> CoreResult<ChatResponse>;
    // streaming variant is optional
    async fn chat_stream(&self, req: ChatRequest) -> CoreResult<Vec<ChatResponse>> {
        // default: call chat once and wrap it
        let single = self.chat(req).await?;
        Ok(vec![single])
    }
}

#[async_trait]
pub trait EmbedProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn embed(&self, req: EmbedRequest) -> CoreResult<EmbedResponse>;
}

/// Providers can expose their supported capabilities
pub trait ProviderCaps {
    fn capabilities(&self) -> &'static [Capability];
}

/// A dummy provider implementation that always returns canned responses.
/// Useful for tests or as a placeholder.
pub struct NullProvider;

#[async_trait]
impl ChatProvider for NullProvider {
    fn name(&self) -> &str { "null" }

    async fn chat(&self, req: ChatRequest) -> CoreResult<ChatResponse> {
        Ok(ChatResponse {
            model: req.model,
            text: "[null provider response]".into(),
            usage_prompt: req.messages.iter().map(|m| m.content.len() as u32).sum(),
            usage_completion: 0,
            cached: false,
            provider: "null".into(),
            transcript_id: None,
            turn_id: "null-turn".into(),
            stop_reason: None,
            provider_request_id: None,
            created_at_ms: 0,
            latency_ms: 0,
        })
    }
}

#[async_trait]
impl EmbedProvider for NullProvider {
    fn name(&self) -> &str { "null" }

    async fn embed(&self, req: EmbedRequest) -> CoreResult<EmbedResponse> {
        Ok(EmbedResponse {
            model: req.model,
            vectors: req.inputs.iter().map(|_| vec![0.0_f32; 3]).collect(),
            usage: req.inputs.len() as u32,
            cached: false,
            provider: "null".into(),
        })
    }
}

impl ProviderCaps for NullProvider {
    fn capabilities(&self) -> &'static [Capability] {
        &[Capability::Chat, Capability::Embed]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChatMessage, Role};

    #[tokio::test]
    async fn null_provider_chat() {
        let prov = NullProvider;
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage{ role: Role::User, content: "hi".into() }],
            temperature: Some(1.0),
            top_p: Some(1.0),
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };
        let resp = prov.chat(req).await.expect("chat ok");
        assert_eq!(resp.provider, "null");
        assert_eq!(resp.text, "[null provider response]");
        assert_eq!(resp.usage_prompt, 2); // "hi" length
    }

    #[tokio::test]
    async fn null_provider_embed() {
        let prov = NullProvider;
        let req = EmbedRequest { model: "text-embedding-3-small".into(), inputs: vec!["a".into(), "b".into()], client_key: None };
        let resp = prov.embed(req).await.expect("embed ok");
        assert_eq!(resp.provider, "null");
        assert_eq!(resp.vectors.len(), 2);
        assert_eq!(resp.vectors[0].len(), 3);
    }
}