Streaming SSE: Status + Next Steps

- Where we are: OpenAI chat supports `stream: true` via `chat_streaming_sse(...)` and emits text deltas followed by a single stop callback. HTTP client exposes `post_sse_lines(...)` built on `reqwest` streaming and a tiny newline splitter. Unit tests cover JSON chat, finish-reason mapping, and an end-to-end SSE happy path.

- Cargo setup: `aiproxy-core/Cargo.toml` enables `reqwest = { version = "0.12", features = ["json", "gzip", "brotli", "deflate", "rustls-tls", "stream"] }` and includes `futures-util` and `bytes`. `resp.bytes_stream()` is available and used.

- Still to do:
  - Tests: Add a case with blank lines/heartbeats and non-`data:` lines to ensure they’re ignored and `on_stop` fires once.
  - Docs: Expand `aiproxy-core/docs/stream.md` with provider-level streaming contract, error mapping, and a quick usage snippet with callbacks.
  - Robustness: Consider buffer bounding in the line splitter and a cancellation/timeout path.

- Quick re-entry checklist:
  - Edit/verify `aiproxy-core/docs/stream.md` with SSE notes and example.
  - Add test for odd SSE lines (blank lines, unrelated lines) and assert single `on_stop`.
  - `cargo test -p aiproxy-core` and smoke the CLI if desired.

- Handy refs:
  - HTTP client: `aiproxy-core/src/http_client.rs`
  - OpenAI provider: `aiproxy-core/src/providers/openai/mod.rs`
  - Current docs: `aiproxy-core/docs/stream.md`

