# Telemetry in ai-proxy

## Overview

The telemetry system in ai-proxy is designed to provide detailed, provider-agnostic insights into the operation of the proxy without imposing any API changes or significant overhead. It is lightweight and flexible, enabling integrators to capture and analyze telemetry data relevant to their needs while maintaining compatibility across different AI providers.

## Components

### `TelemetrySink` trait

The core abstraction for telemetry data consumers is the `TelemetrySink` trait. Implementors of this trait receive telemetry events emitted by the ai-proxy internals.

### `set_telemetry_sink` API

To enable telemetry collection, integrators call `set_telemetry_sink` with their implementation of `TelemetrySink`. This registers the sink globally so that emitted events are forwarded accordingly.

### `emit` (crate-internal)

The `emit` function is a crate-internal API used by ai-proxy components to send telemetry events to the registered sink. It is not exposed publicly, ensuring that telemetry emission remains controlled and consistent.

### `ProviderTrace` struct + builder-style API

`ProviderTrace` is a structured representation of telemetry events related to provider interactions. It supports a builder-style API for incrementally setting fields such as provider name, model, request IDs, latency, and errors. This struct forms the basis for detailed telemetry capture of provider calls.

### Attribute keys (`KEY_*`)

Telemetry attributes use predefined keys (`KEY_PROVIDER`, `KEY_MODEL`, `KEY_REQUEST_ID`, etc.) to ensure consistent naming and interpretation across different telemetry sinks and consumers.

## What we capture

The telemetry system captures a variety of attributes to provide a comprehensive view of AI provider interactions:

- **provider**: The name of the AI provider.
- **model**: The model used for the request.
- **request_id / turn_id**: Unique identifiers for requests and conversation turns.
- **provider_request_id**: The request ID assigned by the provider.
- **latency_ms**: The latency of the provider call in milliseconds.
- **finish_reason**: The reason the provider call finished (e.g., completed, cancelled).
- **error_kind**: The classification of any error encountered.
- **error_message**: A human-readable error message.
- **tokens** (planned future addition): Token usage and accounting data.

## Spans (1.15.3)

Starting with version 1.15.3, ai-proxy emits tracing spans to provide a taxonomy of internal operations. These spans include:

- `http.request`: Represents HTTP request lifecycles.
- `sse.stream`: Represents Server-Sent Events streaming.
- `provider.call`: Represents calls to AI providers.

Each span can have attached fields corresponding to telemetry attributes, enabling correlation between spans and `ProviderTrace` events for comprehensive tracing.

## Usage by integrators

By default, telemetry is a no-op to avoid overhead and unnecessary complexity.

To enable telemetry:

1. Implement the `TelemetrySink` trait in your application.
2. Register your implementation by calling `set_telemetry_sink`.

### Example sink that logs JSON lines

```rust
use aiproxy_core::telemetry::{TelemetrySink, ProviderTrace};
use serde_json::json;

struct JsonLoggingSink;

impl TelemetrySink for JsonLoggingSink {
    fn record(&self, trace: &ProviderTrace) {
        let json_line = json!({
            "provider": trace.provider(),
            "model": trace.model(),
            "request_id": trace.request_id(),
            "latency_ms": trace.latency_ms(),
            "error_kind": trace.error_kind(),
            "error_message": trace.error_message(),
            // Additional fields...
        });
        println!("{}", json_line.to_string());
    }
}

fn setup() {
    aiproxy_core::telemetry::set_telemetry_sink(Box::new(JsonLoggingSink));
}
```

## Testing

For unit tests, ai-proxy provides test-only helpers such as `test_set_capture_enabled` to enable capturing telemetry events during tests. This allows assertions on emitted telemetry and ensures correctness of telemetry-related logic.

## Future extensions

Planned enhancements to the telemetry system include:

- Deduplication of HTTP and provider traces to reduce noise.
- Token accounting to capture usage and cost metrics.
- Support for OTLP and metrics export for integration with observability platforms.
