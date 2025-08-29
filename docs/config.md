


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

- **path:** Filesystem path where cache files are stored.
- **ttl_seconds:** Time-to-live for cache entries, in seconds. Entries older than this are invalidated.

---

## 4. Transcript

The `transcript` section configures logging of requests and responses for auditing or debugging.

```json
"transcript": {
  "path": "./transcripts",
  "fsync": "write"
}
```

| fsync Mode | Description                                      |
|------------|--------------------------------------------------|
| `none`     | No explicit fsync; may be faster, less durable   |
| `write`    | Fsync after each write; safer, slightly slower   |
| `batch`    | Fsync at intervals; compromise between speed and safety |

- **path:** Directory where transcript logs are stored.
- **fsync:** Controls how often data is flushed to disk for durability.

---

## 5. Routing

The `routing` section determines which provider handles a request based on the model name, using regular expressions.

```json
"routing": [
  {
    "pattern": "^gpt-",
    "provider": "openai"
  },
  {
    "pattern": "^claude-",
    "provider": "anthropic"
  }
],
"default_provider": "openai"
```

- **pattern:** Regular expression matched against the `model` field in requests.
- **provider:** The provider to use if the pattern matches.
- **default_provider:** Provider to use if no pattern matches.

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
    "ttl_seconds": 3600
  },
  "transcript": {
    "path": "./transcripts",
    "fsync": "write"
  },
  "routing": [
    { "pattern": "^gpt-", "provider": "openai" }
  ],
  "default_provider": "openai"
}
```

**Best Practices:**
- Use environment variables for API keys.
- Set a reasonable cache TTL to balance freshness and performance.
- Use `fsync: write` for transcript durability unless performance is critical.
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
    "ttl_seconds": 3600
  },
  "transcript": {
    "path": "./transcripts",
    "fsync": "write"
  },
  "routing": [
    { "pattern": "^gpt-", "provider": "openai" },
    { "pattern": "^claude-", "provider": "anthropic" }
  ],
  "default_provider": "openai"
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
ttl_seconds = 3600

[transcript]
path = "./transcripts"
fsync = "write"

[[routing]]
pattern = "^gpt-"
provider = "openai"

[[routing]]
pattern = "^claude-"
provider = "anthropic"

default_provider = "openai"
```