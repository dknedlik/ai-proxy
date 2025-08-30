use aiproxy_core::{
    config::{Config, HttpCfg},
    model::{ChatMessage, ChatRequest, EmbedRequest, Role},
    provider_factory::ProviderRegistry,
    router::RoutingResolver,
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about = "ai-proxy CLI smoke tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Send a chat completion request
    Chat {
        #[arg(long)]
        model: String,
        #[arg(short, long, help = "Message from the user")]
        message: String,
    },
    /// Send an embedding request
    Embed {
        #[arg(long)]
        model: String,
        #[arg(short, long, help = "Input text")]
        input: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // For now, just load default config (ignores file loading).
    let cfg = Config {
        providers: aiproxy_core::config::Providers {
            openai: None,
            anthropic: None,
            openrouter: None,
        },
        cache: aiproxy_core::config::CacheCfg {
            path: ":memory:".into(),
            ttl_seconds: 60,
        },
        transcript: aiproxy_core::config::TranscriptCfg {
            dir: ".tx".into(),
            segment_mb: 64,
            fsync: aiproxy_core::config::FsyncPolicy::Commit,
            redact_builtin: true,
        },
        routing: aiproxy_core::config::RoutingCfg {
            default: "openai".into(),
            rules: vec![],
        },
        http: HttpCfg::default(),
    };

    let reg = ProviderRegistry::from_config(&cfg)?;
    let router = RoutingResolver::new(&cfg)?;

    match cli.command {
        Commands::Chat { model, message } => {
            let provider = router.select_chat(&reg, &model)?;
            let req = ChatRequest {
                model,
                messages: vec![ChatMessage {
                    role: Role::User,
                    content: message,
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
            let resp = provider.chat(req).await?;
            println!("{} -> {}", resp.provider, resp.text);
        }
        Commands::Embed { model, input } => {
            let provider = router.select_embed(&reg, &model)?;
            let req = EmbedRequest {
                model,
                inputs: vec![input],
                client_key: None,
            };
            let resp = provider.embed(req).await?;
            for (i, v) in resp.vectors.iter().enumerate() {
                println!("{} -> dim={}", i, v.len());
            }
        }
    }

    Ok(())
}
