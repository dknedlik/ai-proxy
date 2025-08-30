use std::{collections::HashMap, sync::Arc};

use crate::config::{Config, Providers};
use crate::error::CoreResult;
use crate::provider::{Capability, ChatProvider, EmbedProvider, NullProvider, ProviderCaps};
use crate::providers::openai::OpenAI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    Anthropic,
    OpenRouter,
    Null,
}

/// Registry of concrete provider instances by name.
/// Names correspond to config keys (e.g., "openai", "anthropic", "openrouter", "null").
pub struct ProviderRegistry {
    chat: HashMap<String, Arc<dyn ChatProvider>>, // name -> chat provider
    embed: HashMap<String, Arc<dyn EmbedProvider>>, // name -> embed provider
    caps: HashMap<String, &'static [Capability]>, // name -> capabilities
}

impl ProviderRegistry {
    /// Build a registry from configuration. For now, we always register a `null` provider.
    /// Future: register real providers when adapters land and API keys are present.
    pub fn from_config(cfg: &Config) -> CoreResult<Self> {
        let mut chat: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();
        let mut embed: HashMap<String, Arc<dyn EmbedProvider>> = HashMap::new();
        let mut caps: HashMap<String, &'static [Capability]> = HashMap::new();

        // Always provide a fallback null provider
        let null = Arc::new(NullProvider);
        chat.insert("null".into(), null.clone());
        embed.insert("null".into(), null.clone());
        caps.insert("null".into(), null.capabilities());

        // --- OpenAI registration (enabled if OPENAI_API_KEY is present) ---
        if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            let base = std::env::var("OPENAI_BASE")
                .unwrap_or_else(|_| "https://api.openai.com".to_string());
            let org = std::env::var("OPENAI_ORG").ok();

            let http = crate::http_client::HttpClient::new_default()?;
            let openai = Arc::new(OpenAI::new(http, api_key, base, org));

            chat.insert("openai".to_string(), openai.clone());
            embed.insert("openai".to_string(), openai.clone());
            caps.insert("openai".to_string(), openai.capabilities());
        }

        // Stubs for future wiring: once adapters exist, we'll construct them here and insert under their key names.
        // Validate presence of API keys if providers are configured, but return a clear not-implemented error for now.
        if has_any_provider(&cfg.providers) {
            // We allow the config to reference providers we haven't implemented yet, but we surface a Validation error
            // so callers understand why they can't route to them yet.
            // (Comment out if you prefer silent ignore.)
            // return Err(AiProxyError::Validation("configured providers not implemented yet".to_string()));
        }

        Ok(Self { chat, embed, caps })
    }

    /// Test-only helper to build a registry with a single OpenAI provider wired in.
    /// This avoids touching environment variables in integration tests.
    #[cfg(test)]
    pub fn with_openai_for_tests(openai: Arc<OpenAI>) -> Self {
        let mut chat: HashMap<String, Arc<dyn ChatProvider>> = HashMap::new();
        let mut embed: HashMap<String, Arc<dyn EmbedProvider>> = HashMap::new();
        let mut caps: HashMap<String, &'static [Capability]> = HashMap::new();

        // Always include null for fallback behavior
        let null = Arc::new(NullProvider);
        chat.insert("null".into(), null.clone());
        embed.insert("null".into(), null.clone());
        caps.insert("null".into(), null.capabilities());

        // Register the provided OpenAI instance for both chat and embed
        chat.insert("openai".to_string(), openai.clone());
        embed.insert("openai".to_string(), openai.clone());
        const OAI_CAPS: &[Capability] = &[Capability::Chat, Capability::Embed];
        caps.insert("openai".to_string(), OAI_CAPS);

        Self { chat, embed, caps }
    }

    /// Get a chat provider by name (e.g., "openai", "anthropic", "null").
    pub fn chat(&self, name: &str) -> Option<Arc<dyn ChatProvider>> {
        self.chat.get(name).cloned()
    }

    /// Get an embed provider by name.
    pub fn embed(&self, name: &str) -> Option<Arc<dyn EmbedProvider>> {
        self.embed.get(name).cloned()
    }

    /// Capabilities advertised for a given provider name.
    pub fn caps(&self, name: &str) -> Option<&'static [Capability]> {
        self.caps.get(name).copied()
    }
}

fn has_any_provider(p: &Providers) -> bool {
    p.openai.is_some() || p.anthropic.is_some() || p.openrouter.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CacheCfg, FsyncPolicy, RoutingCfg, TranscriptCfg};

    fn minimal_cfg() -> Config {
        Config {
            providers: Providers {
                openai: None,
                anthropic: None,
                openrouter: None,
            },
            cache: CacheCfg {
                path: ":memory:".into(),
                ttl_seconds: 60,
            },
            transcript: TranscriptCfg {
                dir: ".tx".into(),
                segment_mb: 64,
                fsync: FsyncPolicy::Commit,
                redact_builtin: true,
            },
            routing: RoutingCfg {
                default: "null".into(),
                rules: vec![],
            },
        }
    }

    #[test]
    fn builds_registry_with_null() {
        let reg = ProviderRegistry::from_config(&minimal_cfg()).unwrap();
        assert!(reg.chat("null").is_some());
        assert!(reg.embed("null").is_some());
        let caps = reg.caps("null").unwrap();
        assert!(caps.contains(&Capability::Chat));
        assert!(caps.contains(&Capability::Embed));
    }

    #[test]
    fn missing_provider_returns_none() {
        let reg = ProviderRegistry::from_config(&minimal_cfg()).unwrap();
        assert!(reg.chat("missing").is_none());
        assert!(reg.embed("missing").is_none());
        assert!(reg.caps("missing").is_none());
    }
}
