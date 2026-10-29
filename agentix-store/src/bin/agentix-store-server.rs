use agentix_store::{server, transport, Store};
use anyhow::{ensure, Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        std::env::var_os("AGENTIX_STORE_URL").is_none(),
        "storage server must own a local index; unset AGENTIX_STORE_URL"
    );
    let token = transport::credential()?;
    let store = Store::from_env().await?;
    let host = std::env::var("AGENTIX_STORE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("AGENTIX_STORE_PORT")
        .unwrap_or_else(|_| "8091".into())
        .parse()
        .context("invalid AGENTIX_STORE_PORT")?;
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .context("cannot bind storage listener")?;
    eprintln!("Agentix storage listening on {}", listener.local_addr()?);
    axum::serve(listener, server::router(store, token)?)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
