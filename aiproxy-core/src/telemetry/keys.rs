/// Span/Log attribute keys for provider calls.
/// Keep these stable; changing them is a breaking change for dashboards.
pub const KEY_PROVIDER: &str = "llm.provider";
pub const KEY_MODEL: &str = "llm.model";
pub const KEY_TURN_ID: &str = "turn.id";
pub const KEY_REQUEST_ID: &str = "req.id"; // your internal request id (if you have one)
pub const KEY_PROVIDER_REQUEST_ID: &str = "llm.req_id";

pub const KEY_LATENCY_MS: &str = "latency.ms";
pub const KEY_FINISH_REASON: &str = "finish.reason";
pub const KEY_TOKENS_PROMPT: &str = "tokens.prompt";
pub const KEY_TOKENS_COMPLETION: &str = "tokens.completion";
pub const KEY_TOKENS_TOTAL: &str = "tokens.total";

/// Error-related (if applicable)
pub const KEY_ERROR_KIND: &str = "error.kind";
pub const KEY_ERROR_MESSAGE: &str = "error.message";

