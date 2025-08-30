use std::sync::Arc;

use regex::Regex;

use crate::config::{Config, RoutingRule};
use crate::error::{AiProxyError, CoreResult};
use crate::provider::{ChatProvider, EmbedProvider};
use crate::provider_factory::ProviderRegistry;

/// Compiled routing rule
#[derive(Debug)]
struct CompiledRule {
    regex: Regex,
    provider: String,
}

/// Resolves a model string to a provider name, then fetches the provider
/// from the registry, validating the capability.
#[derive(Debug)]
pub struct RoutingResolver {
    rules: Vec<CompiledRule>,
    default_provider: String,
}

impl RoutingResolver {
    /// Build a resolver by compiling regexes from config.
    pub fn new(cfg: &Config) -> CoreResult<Self> {
        let mut rules = Vec::new();
        for RoutingRule { model, provider } in &cfg.routing.rules {
            let regex = Regex::new(model).map_err(|e| {
                AiProxyError::Validation(format!("invalid routing regex '{model}': {e}"))
            })?;
            rules.push(CompiledRule {
                regex,
                provider: provider.clone(),
            });
        }
        Ok(Self {
            rules,
            default_provider: cfg.routing.default.clone(),
        })
    }

    fn pick_provider_name<'a>(&'a self, model: &str) -> &'a str {
        for r in &self.rules {
            if r.regex.is_match(model) {
                return &r.provider;
            }
        }
        &self.default_provider
    }

    /// Select a chat provider for the given model.
    pub fn select_chat(
        &self,
        reg: &ProviderRegistry,
        model: &str,
    ) -> CoreResult<Arc<dyn ChatProvider>> {
        let name = self.pick_provider_name(model);
        reg.chat(name).ok_or_else(|| {
            AiProxyError::Validation(format!(
                "provider '{name}' not found or lacks chat capability"
            ))
        })
    }

    /// Select an embed provider for the given model.
    pub fn select_embed(
        &self,
        reg: &ProviderRegistry,
        model: &str,
    ) -> CoreResult<Arc<dyn EmbedProvider>> {
        let name = self.pick_provider_name(model);
        reg.embed(name).ok_or_else(|| {
            AiProxyError::Validation(format!(
                "provider '{name}' not found or lacks embed capability"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CacheCfg, FsyncPolicy, Providers, RoutingCfg, TranscriptCfg};

    fn cfg_with_rules(default: &str, rules: Vec<(&str, &str)>) -> Config {
        let compiled_rules = rules
            .into_iter()
            .map(|(model, provider)| RoutingRule {
                model: model.into(),
                provider: provider.into(),
            })
            .collect::<Vec<_>>();
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
                default: default.into(),
                rules: compiled_rules,
            },
        }
    }

    #[test]
    fn picks_rule_then_fallback_default() {
        // Rule sends gpt-* to "null", default also "null"
        let cfg = cfg_with_rules("null", vec![("^gpt-.*", "null")]);
        let reg = ProviderRegistry::from_config(&cfg).expect("should build provider registry");
        let router = RoutingResolver::new(&cfg).expect("should build routing resolver");

        // Matches the rule
        let chat = router
            .select_chat(&reg, "gpt-4o")
            .expect("chat provider should be found");
        assert_eq!(chat.name(), "null");

        // No rule match => default
        let emb = router
            .select_embed(&reg, "text-embedding-3-small")
            .expect("embed provider should be found");
        assert_eq!(emb.name(), "null");
    }

    #[test]
    fn missing_provider_yields_validation_error() {
        // Default points to a provider name that isn't registered
        let cfg = cfg_with_rules("missing", vec![]);
        let reg = ProviderRegistry::from_config(&cfg).expect("should build provider registry");
        let router = RoutingResolver::new(&cfg).expect("should build routing resolver");
        let err = router.select_chat(&reg, "gpt-4o").unwrap_err();
        match err {
            AiProxyError::Validation(msg) => assert!(msg.contains("missing")),
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_regex_yields_validation_error() {
        // An invalid regex in config should produce a Validation error on construction
        let cfg = cfg_with_rules("null", vec![("(", "null")]); // invalid pattern
        let err = RoutingResolver::new(&cfg).unwrap_err();
        match err {
            AiProxyError::Validation(msg) => assert!(msg.contains("invalid routing regex")),
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn rule_points_to_missing_provider() {
        // Rule matches, but points to a provider name not in the registry
        let cfg = cfg_with_rules("null", vec![("^gpt-.*", "missing")]);
        let reg = ProviderRegistry::from_config(&cfg).expect("should build provider registry");
        let router = RoutingResolver::new(&cfg).expect("should build routing resolver");
        let err = router.select_chat(&reg, "gpt-4o").unwrap_err();
        match err {
            AiProxyError::Validation(msg) => assert!(msg.contains("missing")),
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn first_match_wins_rule_order() {
        // Two rules could match; ensure first in list wins
        let cfg = cfg_with_rules("null", vec![("^gpt-.*", "null"), ("^gpt-4o$", "missing")]);
        let reg = ProviderRegistry::from_config(&cfg).expect("should build provider registry");
        let router = RoutingResolver::new(&cfg).expect("should build routing resolver");
        let chat = router
            .select_chat(&reg, "gpt-4o")
            .expect("chat provider should be found");
        assert_eq!(chat.name(), "null"); // proves first rule took precedence over later more-specific rule
    }

    #[tokio::test]
    async fn router_selects_openai_and_calls_chat() {
        use crate::providers::openai::OpenAI;
        use httpmock::{Method::POST, MockServer};
        use serde_json::json;

        // Mock OpenAI server
        let server = MockServer::start();
        let _m = server.mock(|when, then| {
            when.method(POST).path("/v1/chat/completions");
            then.status(200).json_body(json!({
                "id": "cmpl_rtr",
                "choices": [{
                    "message": {"role":"assistant", "content":"pong"},
                    "finish_reason": "stop"
                }]
            }));
        });

        // Build a config routing gpt-* to openai
        let cfg = cfg_with_rules("openai", vec![("^gpt-.*", "openai")]);
        let router = RoutingResolver::new(&cfg).expect("router");

        // Build registry directly with a test OpenAI pointing to mock server
        let http = crate::http_client::HttpClient::new_default().expect("http");
        let oi = std::sync::Arc::new(OpenAI::new(
            http,
            "test-key".into(),
            server.base_url(),
            None,
        ));
        let reg = ProviderRegistry::with_openai_for_tests(oi);

        let chat = router.select_chat(&reg, "gpt-4o").expect("chat provider");
        let req = crate::model::ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![crate::model::ChatMessage {
                role: crate::model::Role::User,
                content: "ping".into(),
            }],
            temperature: None,
            top_p: None,
            metadata: None,
            client_key: None,
            request_id: None,
            trace_id: None,
            idempotency_key: None,
            max_output_tokens: None,
            stop_sequences: None,
        };

        let resp = chat.chat(req).await.expect("chat resp");
        assert_eq!(resp.text, "pong");
        assert_eq!(resp.provider, "openai");
    }
}
