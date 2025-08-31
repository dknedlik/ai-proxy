Telemetry Spans and Trace Fields (1.15.3–1.15.4)

Overview
- Spans: http.request, sse.stream, provider.call
- Fields: provider/model/ids, status, provider_request_id, latency_ms, error_kind/message
- Telemetry: ProviderTrace emitted side-band; spans complement it.

Testing Helpers
- See test-only capture layer at `aiproxy-core/src/telemetry/test_span.rs` (compiled under #[cfg(test)]).
  Use `install_capture()` in tests to collect spans and their fields.

