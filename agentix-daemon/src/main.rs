mod config;
mod gateway;

use anyhow::Result;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agentix_daemon=info".into()),
        )
        .init();

    let cfg = config::Config::from_env();
    info!(
        port = cfg.gateway_port,
        llama_socket = %cfg.llama_socket.display(),
        qwen_socket = %cfg.qwen_socket.display(),
        qwen_model = %cfg.qwen_model,
        whisper_socket = %cfg.whisper_socket.display(),
        "agentix-daemon starting",
    );

    let model_router = Arc::new(agentix_router::Router::new());
    let router = gateway::router(model_router, cfg.clone())?;

    let addr = format!("{}:{}", cfg.gateway_host, cfg.gateway_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "gateway listening");

    axum::serve(listener, router).await?;
    Ok(())
}
