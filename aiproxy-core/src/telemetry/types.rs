use serde::{Deserialize, Serialize};

/// Canonical, provider-agnostic tracing payload.
/// Attach this to all provider responses (streaming and non-streaming) later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderTrace {
    /// Caller-supplied correlation id for a single chat turn or operation.
    pub turn_id: Option<String>,

    /// Provider identifier, e.g. "openai", "anthropic".
    pub provider: Option<String>,

    /// Model identifier, e.g. "gpt-4o", "claude-3-opus".
    pub model: Option<String>,

    /// Your internal request id, if you generate one.
    pub request_id: Option<String>,

    /// Provider's returned request id/correlation id.
    pub provider_request_id: Option<String>,

    /// Elapsed time in milliseconds (to be filled by later milestones).
    pub latency_ms: Option<u128>,

    /// Optional token usage (if provider supplies).
    pub tokens_prompt: Option<u32>,
    pub tokens_completion: Option<u32>,
    pub tokens_total: Option<u32>,

    /// Finish reason as a normalized string (e.g., "Stop", "Length", "Error").
    pub finish_reason: Option<String>,

    /// Optional error metadata, if applicable.
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
}

impl ProviderTrace {
    pub fn with_provider_model(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: Some(provider.into()),
            model: Some(model.into()),
            ..Default::default()
        }
    }

    pub fn set_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    pub fn set_request_id(mut self, rid: impl Into<String>) -> Self {
        self.request_id = Some(rid.into());
        self
    }

    pub fn set_provider_request_id(mut self, prid: impl Into<String>) -> Self {
        self.provider_request_id = Some(prid.into());
        self
    }

    pub fn set_tokens(mut self, prompt: Option<u32>, completion: Option<u32>, total: Option<u32>) -> Self {
        self.tokens_prompt = prompt;
        self.tokens_completion = completion;
        self.tokens_total = total;
        self
    }

    pub fn set_finish_reason(mut self, reason: impl Into<String>) -> Self {
        self.finish_reason = Some(reason.into());
        self
    }

    pub fn set_latency_ms(mut self, ms: u128) -> Self {
        self.latency_ms = Some(ms);
        self
    }

    // Shorthand fluent setters used by instrumentation
    pub fn new() -> Self {
        Self::default()
    }
    pub fn provider(mut self, provider: &str) -> Self {
        self.provider = Some(provider.to_string());
        self
    }
    pub fn model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }
    pub fn request_id_opt(mut self, rid: Option<&str>) -> Self {
        self.request_id = rid.map(|s| s.to_string());
        self
    }
    pub fn turn_id_opt(mut self, tid: Option<&str>) -> Self {
        self.turn_id = tid.map(|s| s.to_string());
        self
    }
    pub fn provider_request_id_opt<S: AsRef<str>>(mut self, prid: Option<S>) -> Self {
        self.provider_request_id = prid.map(|s| s.as_ref().to_string());
        self
    }
    pub fn latency_ms(mut self, ms: u64) -> Self {
        self.latency_ms = Some(ms as u128);
        self
    }
    pub fn finish_reason_opt(mut self, reason: Option<&str>) -> Self {
        self.finish_reason = reason.map(|s| s.to_string());
        self
    }
    pub fn error_kind(mut self, kind: &str) -> Self {
        self.error_kind = Some(kind.to_string());
        self
    }
    pub fn error_message(mut self, msg: &str) -> Self {
        self.error_message = Some(msg.to_string());
        self
    }
}

/// Structured, provider-agnostic completion log event.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CompletionLog {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub turn_id: Option<String>,
    pub provider_request_id: Option<String>,
    pub created_at_ms: Option<u64>,
    pub latency_ms: Option<u64>,

    pub stop_reason: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,

    pub text: Option<String>,
    pub tokens_prompt: Option<u32>,
    pub tokens_completion: Option<u32>,
    pub tokens_total: Option<u32>,

    pub span_name: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
}

impl CompletionLog {
    pub fn new() -> Self { Self::default() }
    pub fn provider(mut self, v: &str) -> Self { self.provider = Some(v.to_string()); self }
    pub fn model(mut self, v: &str) -> Self { self.model = Some(v.to_string()); self }
    pub fn request_id_opt(mut self, v: Option<&str>) -> Self { self.request_id = v.map(|s| s.to_string()); self }
    pub fn turn_id_opt(mut self, v: Option<&str>) -> Self { self.turn_id = v.map(|s| s.to_string()); self }
    pub fn provider_request_id_opt(mut self, v: Option<&str>) -> Self { self.provider_request_id = v.map(|s| s.to_string()); self }
    pub fn created_at_ms(mut self, v: u64) -> Self { self.created_at_ms = Some(v); self }
    pub fn latency_ms(mut self, v: u64) -> Self { self.latency_ms = Some(v); self }
    pub fn stop_reason_opt(mut self, v: Option<&str>) -> Self { self.stop_reason = v.map(|s| s.to_string()); self }
    pub fn error_kind_opt(mut self, v: Option<&str>) -> Self { self.error_kind = v.map(|s| s.to_string()); self }
    pub fn error_message(mut self, v: &str) -> Self { self.error_message = Some(v.to_string()); self }
    pub fn text_opt(mut self, v: Option<&str>) -> Self { self.text = v.map(|s| s.to_string()); self }
    pub fn tokens(mut self, p: Option<u32>, c: Option<u32>, t: Option<u32>) -> Self {
        self.tokens_prompt = p; self.tokens_completion = c; self.tokens_total = t; self
    }
    pub fn span(mut self, name: Option<&str>, id: Option<&str>, parent: Option<&str>) -> Self {
        self.span_name = name.map(|s| s.to_string());
        self.span_id = id.map(|s| s.to_string());
        self.parent_span_id = parent.map(|s| s.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_trace_serializes() {
        let trace = ProviderTrace::with_provider_model("openai", "gpt-4o")
            .set_turn_id("turn-123")
            .set_request_id("req-abc")
            .set_provider_request_id("prov-xyz")
            .set_latency_ms(42)
            .set_tokens(Some(10), Some(20), Some(30))
            .set_finish_reason("Stop");

        let as_json = serde_json::to_value(&trace).unwrap();
        assert_eq!(as_json["provider"], json!("openai"));
        assert_eq!(as_json["model"], json!("gpt-4o"));
        assert_eq!(as_json["turn_id"], json!("turn-123"));
        assert_eq!(as_json["latency_ms"], json!(42));
        assert_eq!(as_json["tokens_total"], json!(30));
        assert_eq!(as_json["finish_reason"], json!("Stop"));
    }
}
