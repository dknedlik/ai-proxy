


# Streaming Interface: Purpose, Design, and Contract

## 1. Overview

The streaming interface in ai-proxy enables providers to deliver responses token-by-token (or chunk-by-chunk) as soon as possible, rather than waiting for the entire completion. This approach reduces latency and improves user experience, allowing clients to display partial results in real time, such as with chatbots or code assistants.

## 2. Contract

The streaming interface is defined by the `StreamEvent` type, which represents the events emitted by a provider during a streaming response. The contract is as follows:

- Providers may emit zero or more `DeltaText` events (for partial text).
- Providers may emit an optional `Usage` event (for token/usage statistics).
- The stream **must** terminate with exactly one terminal event: `Stop`, `Final`, or `Error`.
  - Only one terminal event is allowed, and it must be the last event in the stream.

## 3. Event Types

- **DeltaText**:  
  Emitted when new text (or tokens) are available. Can occur multiple times as the response is constructed.

- **Usage**:  
  Optionally emitted to report statistics such as token counts or billing information. At most one per stream.

- **Stop**:  
  Terminal event indicating the stream ended normally (e.g., the model finished generating output).

- **Final**:  
  Terminal event indicating the stream is complete, and may include additional final data (such as a full message or summary).

- **Error**:  
  Terminal event indicating the stream terminated due to an error (e.g., provider failure or invalid input).

## 4. Example Lifecycle

Below is a sample sequence of events for a typical streaming completion:

```json
{ "type": "DeltaText", "text": "Hello" }
{ "type": "DeltaText", "text": ", world" }
{ "type": "Usage", "prompt_tokens": 3, "completion_tokens": 2 }
{ "type": "Stop" }
```

Or, in case of an error:

```json
{ "type": "Error", "message": "Provider timeout" }
```

## 5. Testing Notes

Property-based and round-trip tests are used to verify that every stream emits **exactly one terminal event** at the end (`Stop`, `Final`, or `Error`). These tests ensure the contract is upheld, preventing ambiguous or incomplete stream lifecycles.

## 6. Provider Usage (SSE)

- OpenAI streaming helper: `providers/openai/mod.rs::OpenAI::chat_streaming_sse`.
- Behavior: emits text deltas as they arrive and calls `on_stop` at most once when a finish reason is seen or when the stream ends without one.

Quick example:

```
let provider = OpenAI::new_for_tests(&mock_url);
let req = ChatRequest { /* model, messages, … */ .. };
provider
    .chat_streaming_sse(
        req,
        |delta| eprintln!("delta: {}", delta),
        |stop| eprintln!("stop: {:?}", stop),
    )
    .await?;
```

Robustness:
- Ignores blank lines, comments (e.g., lines starting with ':'), and non-`data:` SSE lines.
- Accepts both `data: {...}` and `data:{...}` forms.
