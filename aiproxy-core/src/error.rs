use thiserror::Error;

/// Core error type for ai-proxy.
/// Internally, modules can use `anyhow::Result<T>` for convenience,
/// but public boundaries should expose `CoreResult<T>` with this error.
#[derive(Debug, Error)]
pub enum AiProxyError {
    #[error("validation failed: {0}")]
    Validation(String),

    #[error("rate limited by provider {provider}")]
    RateLimited {
        provider: String,
        retry_after: Option<u64>,
    },

    #[error("budget exceeded: remaining {remaining}")]
    BudgetExceeded { remaining: u32 },

    #[error("provider unavailable: {provider}")]
    ProviderUnavailable { provider: String },

    #[error("upstream error from {provider}: {code} {message}")]
    ProviderError {
        provider: String,
        code: String,
        message: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type CoreResult<T> = std::result::Result<T, AiProxyError>;
