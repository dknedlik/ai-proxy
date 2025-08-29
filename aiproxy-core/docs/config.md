# ai-proxy Configuration File

## 1. Overview

The `ai-proxy` configuration file defines how the proxy interacts with upstream AI providers, manages caching, logging, and routing of requests. This file allows you to customize the proxy's behavior for your environment and use case.

- **Purpose:** Centralize all proxy settings including provider credentials, cache settings, transcript logging, and request routing.
- **Location:** By default, the configuration file is expected at `./ai-proxy.json` (or `ai-proxy.toml`). You can specify a different location using the `--config` flag when starting `ai-proxy`.

---

## 2. Providers

The `providers` section lists all upstream AI providers you want to connect to. Each provider requires an `api_key_env` field which should match the name of an environment variable containing the API key.

```json
"providers": {
  "openai": {
    "api_key_env": "OPENAI_API_KEY"
  },
  "anthropic": {
    "api_key_env": "ANTHROPIC_API_KEY"
  }
}
```

- **api_key_env:** Name of the environment variable containing the API key for the provider. This keeps secrets out of the config file.

---

## 3. Cache

The `cache` section configures local caching to improve performance and reduce duplicate requests.

```json
"cache": {
  "path": "./cache",
  "ttl_seconds": 3600
}
```

- **path:** Filesystem path to the cache database, usually a SQLite file (e.g., `.aiproxy/cache.db`).
- **ttl_seconds:** Time-to-live for cache entries, in seconds. Entries older than this are invalidated.

---

## 4. Transcript

The `transcript` section configures logging of requests and responses for auditing or debugging.

```json
"transcript": {
  "dir": "./transcripts",
  "segment_mb": 64,
  "redact_builtin": true,
  "fsync": "commit"
}
```

| fsync Mode | Description                                      |
|------------|--------------------------------------------------|
| `off`      | No explicit fsync; may be faster, less durable   |
| `commit`   | Fsync at commit points; balance between speed and safety |
| `always`   | Fsync after each write; safest, slightly slower  |

- **dir:** Directory where transcript logs are stored.
- **segment_mb:** Maximum size in megabytes of each transcript segment file before rolling over.
- **redact_builtin:** Whether to automatically redact sensitive information using built-in rules.
- **fsync:** Controls how often data is flushed to disk for durability.

---

## 5. Routing

The `routing` section determines which provider handles a request based on the model name, using regular expressions.

```json
"routing": [
  {
    "model": "^gpt-",
    "provider": "openai"
  },
  {
    "model": "^claude-",
    "provider": "anthropic"
  }
],
"default": "openai"
```

- **model:** Regular expression matched against the `model` field in requests.
- **provider:** The provider to use if the model regex matches.
- **default:** Provider to use if no model regex matches.

---

## 6. Defaults & Best Practices

Below is a recommended minimal configuration:

```json
{
  "providers": {
    "openai": { "api_key_env": "OPENAI_API_KEY" }
  },
  "cache": {
    "path": "./cache",
    "ttl_seconds": 604800
  },
  "transcript": {
    "dir": "./transcripts",
    "segment_mb": 64,
    "redact_builtin": true,
    "fsync": "commit"
  },
  "routing": [
    { "model": "^gpt-", "provider": "openai" }
  ],
  "default": "openai"
}
```

**Best Practices:**
- Use environment variables for API keys.
- Set a reasonable cache TTL of several days to a week to balance freshness and performance.
- Use `fsync: commit` for transcript durability unless performance is critical.
- Set `segment_mb` to 64 and enable `redact_builtin` to protect sensitive data.
- Route requests explicitly by model name using regex patterns.

---

## 7. Example Config

### JSON Example

```json
{
  "providers": {
    "openai": { "api_key_env": "OPENAI_API_KEY" },
    "anthropic": { "api_key_env": "ANTHROPIC_API_KEY" }
  },
  "cache": {
    "path": "./cache",
    "ttl_seconds": 604800
  },
  "transcript": {
    "dir": "./transcripts",
    "segment_mb": 64,
    "redact_builtin": true,
    "fsync": "commit"
  },
  "routing": [
    { "model": "^gpt-", "provider": "openai" },
    { "model": "^claude-", "provider": "anthropic" }
  ],
  "default": "openai"
}
```

### TOML Example

```toml
[providers.openai]
api_key_env = "OPENAI_API_KEY"

[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"

[cache]
path = "./cache"
ttl_seconds = 604800

[transcript]
dir = "./transcripts"
segment_mb = 64
redact_builtin = true
fsync = "commit"

[[routing]]
model = "^gpt-"
provider = "openai"

[[routing]]
model = "^claude-"
provider = "anthropic"

default = "openai"
```