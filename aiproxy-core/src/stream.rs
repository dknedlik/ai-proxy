//! Streaming primitives exposed by ai-proxy.
//!
//! Contract:
//! - Providers may emit 0..n `DeltaText` events followed by an optional `Usage` update.
//! - The stream **must** terminate with exactly one terminal event: `Stop`, `Final`, or `Error`.
//! - After a terminal event, no further events are emitted.
//!
//! This module intentionally avoids deriving `Clone` / `PartialEq` because `Error` contains
//! `AiProxyError`, which is not (and should not be) `Clone` or `Eq`.

/// What the caller receives incrementally.
#[non_exhaustive]
#[derive(Debug)]
pub enum StreamEvent {
    /// Partial assistant text (delta). Empty string is allowed but should be rare.
    DeltaText(String),
    /// Optional token usage updates mid-stream.
    Usage {
        prompt: Option<u32>,
        completion: Option<u32>,
    },
    /// Provider has decided to stop (with reason).
    Stop {
        reason: Option<crate::model::StopReason>,
    },
    /// Final synthesized response (optional convenience, may repeat Stop).
    Final(crate::model::ChatResponse),
    /// Transport/parse error surfaced mid-stream; stream ends after this.
    Error(crate::error::AiProxyError),
}

impl StreamEvent {
    /// Returns true if this event terminates the stream (`Stop`, `Final`, or `Error`).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stop { .. } | Self::Final(_) | Self::Error(_))
    }

    /// Convenience accessor for `DeltaText` contents.
    pub fn as_text_delta(&self) -> Option<&str> {
        match self {
            Self::DeltaText(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Boxed stream of streaming events. Providers that support streaming return this.
pub type BoxStreamEv = futures::stream::BoxStream<'static, StreamEvent>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_work() {
        let d = StreamEvent::DeltaText("hi".into());
        assert!(!d.is_terminal());
        assert_eq!(d.as_text_delta(), Some("hi"));

        let s = StreamEvent::Stop { reason: None };
        assert!(s.is_terminal());
        assert_eq!(s.as_text_delta(), None);
    }
}
