use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Providers {
    pub openai: Option<ProviderCfg>,
    pub anthropic: Option<ProviderCfg>,
    pub openrouter: Option<ProviderCfg>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ProviderCfg {
    /// Name of the environment variable that contains the API key.
    pub api_key_env: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct CacheCfg {
    pub path: String,
    pub ttl_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FsyncPolicy {
    Off,
    Commit,
    Always,
}

fn default_segment_mb() -> u32 {
    64
}
fn default_fsync() -> FsyncPolicy {
    FsyncPolicy::Commit
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct TranscriptCfg {
    pub dir: String,
    #[serde(default = "default_segment_mb")]
    pub segment_mb: u32,
    #[serde(default = "default_fsync")]
    pub fsync: FsyncPolicy,
    #[serde(default)]
    pub redact_builtin: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct HttpCfg {
    /// TCP connect timeout in milliseconds (default 5000ms)
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// Total request timeout in milliseconds (default 60000ms)
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// Optional per-host idle connection pool cap (None = reqwest default)
    #[serde(default)]
    pub pool_max_idle_per_host: Option<usize>,
}

impl Default for HttpCfg {
    fn default() -> Self {
        Self {
            connect_timeout_ms: default_connect_timeout_ms(),
            request_timeout_ms: default_request_timeout_ms(),
            pool_max_idle_per_host: None,
        }
    }
}

fn default_connect_timeout_ms() -> u64 {
    5_000
}
fn default_request_timeout_ms() -> u64 {
    60_000
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RoutingRule {
    /// Regex applied to the model name, e.g. ^gpt-.*
    pub model: String,
    /// Provider to route to when this rule matches
    pub provider: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RoutingCfg {
    pub default: String,
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Config {
    pub providers: Providers,
    pub cache: CacheCfg,
    pub transcript: TranscriptCfg,
    pub routing: RoutingCfg,
    /// HTTP client configuration (timeouts, pooling). Missing in older configs → defaults.
    #[serde(default)]
    pub http: HttpCfg,
}

impl Config {
    /// Load a Config from a file path (JSON or TOML by extension). If the
    /// extension is missing or unrecognized, try JSON first, then TOML.
    pub fn from_path<P: AsRef<Path>>(path: P) -> crate::error::CoreResult<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(crate::error::AiProxyError::from)?;
        let s =
            std::str::from_utf8(&bytes).map_err(|e| crate::error::AiProxyError::Other(e.into()))?;
        let cfg: Self = match path.extension().and_then(|e| e.to_str()) {
            Some("json") => serde_json::from_str::<Self>(s)
                .map_err(|e| crate::error::AiProxyError::Other(e.into()))?,
            Some("toml") => toml::from_str::<Self>(s)
                .map_err(|e| crate::error::AiProxyError::Other(e.into()))?,
            _ => serde_json::from_str::<Self>(s)
                .map_err(|e| crate::error::AiProxyError::Other(e.into()))
                .or_else(|_| {
                    toml::from_str::<Self>(s)
                        .map_err(|e| crate::error::AiProxyError::Other(e.into()))
                })?,
        };
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn load_from_json() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("poc.json");
        let json = r#"{
          "providers": {
            "openai": {"api_key_env":"OPENAI_API_KEY"},
            "anthropic": {"api_key_env":"ANTHROPIC_API_KEY"},
            "openrouter": {"api_key_env":"OPENROUTER_API_KEY"}
          },
          "cache": {"path":".aiproxy/cache.db","ttl_seconds":604800},
          "transcript": {"dir":".aiproxy/tx","segment_mb":64,"fsync":"commit","redact_builtin":true},
          "routing": {
            "default": "openai",
            "rules": [
              {"model":"^gpt-.*","provider":"openai"},
              {"model":"^claude-.*","provider":"anthropic"}
            ]
          }
        }"#;
        fs::write(&file, json).unwrap();
        let cfg = Config::from_path(&file).unwrap();
        assert_eq!(cfg.routing.default, "openai");
        assert_eq!(cfg.transcript.segment_mb, 64);
        assert!(cfg.providers.openai.is_some());
        assert_eq!(cfg.http.connect_timeout_ms, 5_000);
        assert_eq!(cfg.http.request_timeout_ms, 60_000);
        assert_eq!(cfg.http.pool_max_idle_per_host, None);
    }

    #[test]
    fn missing_file_returns_io_error() {
        let missing = std::path::PathBuf::from("/definitely/not/here/aiproxy-missing.json");
        let err = Config::from_path(&missing).unwrap_err();
        // Should map to our typed Io error
        match err {
            crate::error::AiProxyError::Io(_) => {}
            other => panic!("expected Io error, got: {:?}", other),
        }
    }

    #[test]
    fn bad_utf8_returns_other_error() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("bad.bin");
        // Write invalid UTF-8 bytes
        let bytes = vec![0xff, 0xfe, 0xfd, 0x00, 0x80];
        fs::write(&file, bytes).unwrap();
        let err = Config::from_path(&file).unwrap_err();
        match err {
            crate::error::AiProxyError::Other(_) => {}
            other => panic!("expected Other(utf8) error, got: {:?}", other),
        }
    }

    #[test]
    fn bad_json_returns_other_error() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("bad.json");
        // Intentionally malformed JSON
        let json = r#"{ "providers": { "openai": { "api_key_env": 123 } }"#; // missing closing }
        fs::write(&file, json).unwrap();
        let err = Config::from_path(&file).unwrap_err();
        match err {
            crate::error::AiProxyError::Other(_) => {}
            other => panic!("expected Other(json parse) error, got: {:?}", other),
        }
    }

    #[test]
    fn load_from_toml() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("poc.toml");
        let toml = r#"
[providers.openai]
api_key_env = "OPENAI_API_KEY"

[providers.anthropic]
api_key_env = "ANTHROPIC_API_KEY"

[providers.openrouter]
api_key_env = "OPENROUTER_API_KEY"

[cache]
path = ".aiproxy/cache.db"
ttl_seconds = 604800

[transcript]
dir = ".aiproxy/tx"
segment_mb = 64
fsync = "commit"
redact_builtin = true

[routing]
default = "openai"
[[routing.rules]]
model = "^gpt-.*"
provider = "openai"
[[routing.rules]]
model = "^claude-.*"
provider = "anthropic"
"#;
        fs::write(&file, toml).unwrap();
        let cfg = Config::from_path(&file).unwrap();
        assert_eq!(cfg.routing.default, "openai");
        assert!(cfg.providers.openai.is_some());
        assert_eq!(cfg.http.connect_timeout_ms, 5_000);
        assert_eq!(cfg.http.request_timeout_ms, 60_000);
        assert_eq!(cfg.http.pool_max_idle_per_host, None);
    }

    #[test]
    fn unknown_extension_falls_back_to_json_then_toml() {
        let dir = tempdir().unwrap();
        // First try with a .conf that is valid JSON
        let json_path = dir.path().join("poc.conf");
        let json = r#"{"providers":{},"cache":{"path":"p","ttl_seconds":1},"transcript":{"dir":"t","segment_mb":1,"fsync":"commit","redact_builtin":false},"routing":{"default":"openai","rules":[]}}"#;
        fs::write(&json_path, json).unwrap();
        let cfg_json_first = Config::from_path(&json_path).unwrap();
        assert_eq!(cfg_json_first.routing.default, "openai");
        assert_eq!(cfg_json_first.http.connect_timeout_ms, 5_000);
        assert_eq!(cfg_json_first.http.request_timeout_ms, 60_000);
        assert_eq!(cfg_json_first.http.pool_max_idle_per_host, None);

        // Now write TOML to a different .conf and ensure TOML fallback works when JSON fails
        let toml_path = dir.path().join("poc2.conf");
        let toml = r#"
[providers]

[cache]
path = "p"
ttl_seconds = 1

[transcript]
dir = "t"
segment_mb = 1
fsync = "commit"
redact_builtin = false

[routing]
default = "openai"
rules = []
"#;
        fs::write(&toml_path, toml).unwrap();
        let cfg_toml_fallback = Config::from_path(&toml_path).unwrap();
        assert_eq!(cfg_toml_fallback.cache.ttl_seconds, 1);
        assert_eq!(cfg_toml_fallback.http.connect_timeout_ms, 5_000);
        assert_eq!(cfg_toml_fallback.http.request_timeout_ms, 60_000);
        assert_eq!(cfg_toml_fallback.http.pool_max_idle_per_host, None);
    }
}
