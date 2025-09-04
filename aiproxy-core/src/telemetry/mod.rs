//! Telemetry primitives for provider-agnostic tracing.
//! By default, no telemetry is emitted unless a sink is installed via `set_telemetry_sink`.

pub mod keys;
pub mod types;
#[cfg(test)]
pub mod test_span;

pub use keys::*;
pub use types::*;

use std::sync::Arc;

use once_cell::sync::OnceCell;

/// Implement this to receive telemetry events.
///
/// Requirements:
/// - Implementations must be thread-safe (`Send + Sync`) and `'static`.
/// - `record` **may** be called from any thread; implementations should avoid panicking.
/// - Keep overhead minimal; this may be on hot paths.
pub trait TelemetrySink: Send + Sync + 'static {
    fn record(&self, trace: crate::telemetry::ProviderTrace);

    // 1.15.5: optional completion event; default no-op to avoid breaking existing sinks
    fn record_completion(&self, _log: crate::telemetry::CompletionLog) {}
}

static TELEMETRY_SINK: OnceCell<Arc<dyn TelemetrySink>> = OnceCell::new();

// In tests, gate emission to only the calling test thread to avoid cross-test interference.
#[cfg(test)]
thread_local! {
    static TEST_CAPTURE: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Install a global telemetry sink. Returns `false` if a sink is already installed.
///
/// Notes:
/// - This is a write-once global for the process lifetime (backed by `OnceCell`).
/// - If you need to clear captured data in tests, clear it in your sink implementation.
pub fn set_telemetry_sink(sink: Arc<dyn TelemetrySink>) -> bool {
    TELEMETRY_SINK.set(sink).is_ok()
}

/// Emit a telemetry record if a sink is installed. Crate-visible by design.
///
/// In tests, emission is suppressed unless explicitly enabled via `test_set_capture_enabled`.
#[inline]
pub(crate) fn emit(trace: crate::telemetry::ProviderTrace) {
    #[cfg(test)]
    {
        if !TEST_CAPTURE.with(|c| c.get()) {
            return;
        }
    }
    if let Some(sink) = TELEMETRY_SINK.get() {
        sink.record(trace);
    }
}

/// Emit a structured completion event if a sink is installed. Crate-visible by design.
#[inline]
pub(crate) fn emit_completion(log: crate::telemetry::CompletionLog) {
    #[cfg(test)]
    {
        if !TEST_CAPTURE.with(|c| c.get()) {
            return;
        }
    }
    if let Some(sink) = TELEMETRY_SINK.get() {
        sink.record_completion(log);
    }
}

#[cfg(test)]
/// Test-only helper: enable or disable capture for the current test thread.
///
/// Spawned threads in a test must call this as well if they should emit.
/// Integrations can also choose to enable capture only around specific sections.
pub fn test_set_capture_enabled(enabled: bool) {
    TEST_CAPTURE.with(|c| c.set(enabled));
}
