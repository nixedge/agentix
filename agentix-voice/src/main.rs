mod audio;
mod config;
mod state;
mod vad;
mod voice_loop;
mod wake;

use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use config::VoiceConfig;
use voice_loop::VoiceLoop;

#[derive(Parser)]
#[command(name = "agentix-voice", about = "Desktop voice assistant loop")]
struct Cli {
    #[arg(short, long, default_value = "~/.config/agentix-voice/voice.toml")]
    config: PathBuf,
}

fn expand_path(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    p
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let config_path = expand_path(cli.config);
    let config = VoiceConfig::load(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    let voice = VoiceLoop::new(config).await?;
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            tracing::info!("received Ctrl-C, shutting down");
            cancel_clone.cancel();
        }
    });

    voice.run(cancel).await
}
