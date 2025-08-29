use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    EndTurn,
    ContentFilter,
    Other,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub metadata: Option<serde_json::Value>,
    pub client_key: Option<String>,
    pub request_id: Option<String>,
    pub trace_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChatResponse {
    pub model: String,
    pub text: String,
    pub usage_prompt: u32,
    pub usage_completion: u32,
    pub cached: bool,
    pub provider: String,
    pub transcript_id: Option<String>,
    pub turn_id: String,
    pub stop_reason: Option<StopReason>,
    pub provider_request_id: Option<String>,
    pub created_at_ms: i64,
    pub latency_ms: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct EmbedRequest {
    pub model: String,
    pub inputs: Vec<String>,
    pub client_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct EmbedResponse {
    pub model: String,
    pub vectors: Vec<Vec<f32>>,
    pub usage: u32,
    pub cached: bool,
    pub provider: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_roundtrip() {
        let req = ChatRequest {
            model: "gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hello".to_string(),
            }],
            temperature: Some(0.7),
            top_p: Some(0.9),
            metadata: None,
            client_key: Some("test-client".to_string()),
            request_id: Some("req-123".to_string()),
            trace_id: Some("trace-abc".to_string()),
            idempotency_key: Some("idem-xyz".to_string()),
            max_output_tokens: Some(256),
            stop_sequences: Some(vec!["\n\n".to_string()]),
        };

        let json = serde_json::to_string(&req).unwrap();
        let de: ChatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, de);
    }

    #[test]
    fn role_json_roundtrip_lowercase() {
        let json = r#"{"role":"assistant","content":"ok"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, Role::Assistant);
        let back = serde_json::to_string(&msg).unwrap();
        assert!(back.contains("\"assistant\""));
    }

    #[test]
    fn chat_response_roundtrip() {
        let resp = ChatResponse {
            model: "gpt-4o".to_string(),
            text: "Hello back".to_string(),
            usage_prompt: 10,
            usage_completion: 20,
            cached: false,
            provider: "openai".to_string(),
            transcript_id: Some("transcript-1".to_string()),
            turn_id: "turn-1".to_string(),
            stop_reason: Some(StopReason::Stop),
            provider_request_id: Some("prov-123".to_string()),
            created_at_ms: 1234567890,
            latency_ms: 42,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let de: ChatResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, de);
    }

    #[test]
    fn embed_request_roundtrip() {
        let req = EmbedRequest {
            model: "text-embedding-ada-002".to_string(),
            inputs: vec!["hello".to_string(), "world".to_string()],
            client_key: Some("client-1".to_string()),
        };

        let json = serde_json::to_string(&req).unwrap();
        let de: EmbedRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, de);
    }

    #[test]
    fn embed_response_roundtrip() {
        let resp = EmbedResponse {
            model: "text-embedding-ada-002".to_string(),
            vectors: vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5]],
            usage: 123,
            cached: true,
            provider: "openai".to_string(),
        };

        let json = serde_json::to_string(&resp).unwrap();
        let de: EmbedResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, de);
    }
}
