# Error Handling in AI Proxy

## Overview

In AI Proxy, errors are carefully typed to provide clear, structured information about failure modes. This approach allows clients and developers to understand the cause of an error precisely and respond accordingly. Errors are exposed through the API as well-defined variants of the `AiProxyError` enum. Each variant corresponds to a specific category of failure.

When errors cross the HTTP or FFI boundary, they are mapped to appropriate HTTP status codes and serialized into a consistent JSON error envelope. This ensures that clients receive standardized, machine-readable error information regardless of the transport layer.

## Error Types

The core error type is `AiProxyError`, which includes the following variants:

- **Validation**  
  Indicates that the input request failed validation checks. This can include malformed parameters, missing required fields, or invalid values.

- **RateLimited**  
  Signals that the client has exceeded the allowed request rate and must retry after some delay.

- **BudgetExceeded**  
  Occurs when the client has exhausted their usage budget, such as API call quota or spending limits.

- **ProviderUnavailable**  
  Denotes that the configured AI provider is temporarily unavailable or unreachable.

- **ProviderError**  
  Represents errors returned by the AI provider itself, such as internal server errors or unexpected responses.

- **Io**  
  Covers input/output errors, such as network failures or file system errors encountered during processing.

- **Other**  
  A catch-all variant for errors that do not fit into the above categories, including unexpected or unknown failures.

## Mapping to HTTP/FFI

Each `AiProxyError` variant is mapped to an HTTP status code and serialized into a JSON error envelope with the following structure:

```json
{
  "error": {
    "type": "<error_type>",
    "message": "<human_readable_message>",
    "details": { /* optional additional info */ }
  }
}
```

| AiProxyError Variant   | HTTP Status Code | Description                                       |
|-----------------------|------------------|-------------------------------------------------|
| Validation            | 400 Bad Request  | Client sent invalid input                        |
| RateLimited           | 429 Too Many Requests | Client exceeded rate limits                    |
| BudgetExceeded        | 402 Payment Required | Client exceeded usage budget                     |
| ProviderUnavailable   | 503 Service Unavailable | AI provider is temporarily down                |
| ProviderError         | 502 Bad Gateway  | AI provider returned an error                    |
| Io                    | 500 Internal Server Error | Internal I/O failure                            |
| Other                 | 500 Internal Server Error | Unexpected or unknown error                     |

## Logging

Errors are logged using structured tracing, which includes contextual information such as request IDs and cache status. This enables efficient debugging and monitoring by correlating error events with specific requests and system states.

## Best Practices

- **For Clients:**  
  - Handle each error type explicitly when possible.  
  - Respect `RateLimited` errors by backing off and retrying after the indicated delay.  
  - For `BudgetExceeded`, review usage and billing settings.  
  - Retry `ProviderUnavailable` errors with exponential backoff.  
  - Log and report unexpected `Other` errors for further investigation.

- **For Developers:**  
  - Use the typed error variants to guide error handling logic.  
  - Ensure error messages are clear and actionable.  
  - Maintain accurate HTTP status code mappings.  
  - Include rich context in logs to facilitate troubleshooting.
