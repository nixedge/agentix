pub mod gateway;
mod routes;
#[cfg(test)]
mod tests;

use agentix_assistant::Assistant;
use agentix_assistant_core::project_pending;
use agentix_assistant_lance::LanceProjection;
use agentix_assistant_sled::SledRepository;
use anyhow::{ensure, Context};
pub use routes::routes;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct HostedAssistant {
    pub service: Arc<Assistant>,
    pub owner_token: String,
    repository: SledRepository,
    projection: LanceProjection,
}

impl HostedAssistant {
    pub async fn open(config: &crate::config::AgentConfig) -> anyhow::Result<Option<Self>> {
        let Some(settings) = &config.assistant else {
            return Ok(None);
        };
        ensure!(
            (1..=120).contains(&settings.inference_timeout_secs),
            "assistant inference timeout must be 1..120 seconds"
        );
        ensure!(
            settings.db_path != config.db_path && settings.index_path != settings.db_path,
            "assistant requires separate personal and legacy/index paths"
        );
        let token = std::env::var(&settings.token_env)
            .context("assistant owner token environment variable is missing")?;
        routes::validate_token(&token)?;
        let gateway = gateway::LocalGateway::new(&config.gateway_url, &config.local_model)?;
        let repository = SledRepository::open(&settings.db_path).await?;
        let projection = LanceProjection::open(&settings.index_path).await?;
        let shared = Arc::new(repository.clone());
        let service = Arc::new(
            Assistant::new(shared.clone(), shared, Arc::new(gateway))
                .with_projection(Arc::new(projection.clone()))
                .with_inference_timeout(Duration::from_secs(settings.inference_timeout_secs))?,
        );
        Ok(Some(Self {
            service,
            owner_token: token,
            repository,
            projection,
        }))
    }

    pub fn spawn_projection_worker(&self, cancellation: CancellationToken) {
        let repository = self.repository.clone();
        let projection = self.projection.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => break,
                    _ = tick.tick() => {
                        if project_pending(&repository, &projection, 100).await.is_err() {
                            tracing::warn!("personal projection pass failed; durable work will be retried");
                        }
                    }
                }
            }
        });
    }
}
