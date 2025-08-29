# Request Normalization

## 1. Overview

Request normalization is a process that standardizes input requests before processing or caching. This ensures that semantically identical requests with minor differences (e.g., whitespace, encoding) are treated as the same, improving cache hit rates and reducing redundant processing.

Normalization affects caching by removing non-semantic differences, so that requests differing only in formatting or ignored fields map to the same cache key.

## 2. ChatRequest Normalization

ChatRequest normalization includes the following behaviors:

- **Trim whitespace**: Leading and trailing spaces are removed from all text fields.
- **Unicode NFC normalization**: Text is normalized to Unicode Normalization Form C.
- **Line ending normalization**: All CRLF (`\r\n`) sequences are converted to LF (`\n`).
- **BOM strip**: Byte Order Marks (BOM) are removed from text.
- **Default values**: Missing optional fields are set to default values.
- **Deduplicate stop sequences**: Duplicate stop sequences are removed while preserving order.
- **Cap `max_output_tokens`**: The `max_output_tokens` field is capped at a maximum allowed value.

Example:

```json
{
  "prompt": "Hello world!\r\n",
  "stop_sequences": ["\n", "\n"],
  "max_output_tokens": 5000
}
```

is normalized to:

```json
{
  "prompt": "Hello world!\n",
  "stop_sequences": ["\n"],
  "max_output_tokens": 2048
}
```

## 3. EmbedRequest Normalization

EmbedRequest normalization includes:

- **Trim whitespace**: Leading and trailing spaces removed from each input string.
- **Drop empty inputs**: Empty strings are removed from the input list.
- **Deduplicate inputs**: Duplicate inputs are removed while preserving order.
- **Unicode NFC normalization**: Inputs normalized to Unicode NFC.
- **Line ending normalization**: CRLF converted to LF.
- **BOM strip**: BOM removed from inputs.

Example:

```json
{
  "inputs": ["  text\r\n", "", "text", "example"]
}
```

is normalized to:

```json
{
  "inputs": ["text\n", "example"]
}
```

## 4. Cache Key Implications

Normalization removes non-semantic differences and ignores certain fields when generating cache keys:

- Fields like `request_id`, `trace_id`, and `idempotency_key` are ignored.
- Formatting differences such as whitespace, line endings, and Unicode normalization do not affect cache keys.
- Deduplication and defaulting ensure semantically equivalent requests share the same cache key.

## 5. Best Practices

- **Do not rely on leading or trailing spaces** in request fields; they will be trimmed.
- **Avoid including secrets or sensitive data** in request fields as normalization and caching may expose them.
- **Expect sanitized values** in request processing, as inputs will be normalized and cleaned before use.
