pub mod gateway;
mod routes;
#[cfg(test)]
mod tests;

use agentix_assistant::Assistant;
use agentix_assistant_core::project_pending;
use agentix_assistant_lance::LanceProjection;
use agentix_assistant_sled::SledRepository;
use anyhow::ensure;
pub use routes::routes;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct HostedAssistant {
    pub service: Arc<Assistant>,
    pub owner_token: Option<String>,
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
        let token = resolve_owner_token(settings, |variable| std::env::var(variable))?;
        let gateway = Arc::new(gateway::LocalGateway::new(
            &config.gateway_url,
            &config.local_model,
        )?);
        let repository = SledRepository::open(&settings.db_path).await?;
        let projection = LanceProjection::open(&settings.index_path).await?;
        let shared = Arc::new(repository.clone());
        let mut service = Assistant::new(shared.clone(), shared.clone(), gateway.clone())
            .with_reminders(shared, gateway)
            .with_projection(Arc::new(projection.clone()))
            .with_inference_timeout(Duration::from_secs(settings.inference_timeout_secs))?;
        if let Some(groceries) = &settings.ourgroceries {
            let (email, password) =
                resolve_grocery_credentials(groceries, |name| std::env::var(name))?;
            service = service.with_groceries(Arc::new(agentix_ourgroceries::Client::new(
                email,
                password,
                Some(groceries.list_id.clone()),
            )?));
            tracing::info!("OurGroceries integration configured");
        }
        let service = Arc::new(service);
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

fn resolve_grocery_credentials(
    settings: &crate::config::OurGroceriesConfig,
    read: impl Fn(&str) -> std::result::Result<String, std::env::VarError>,
) -> anyhow::Result<(String, String)> {
    for name in [&settings.email_env, &settings.password_env] {
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "OurGroceries runtime environment variable name is invalid"
        );
    }
    let email = read(&settings.email_env).map_err(|_| {
        anyhow::anyhow!("OurGroceries email environment variable is missing or invalid")
    })?;
    let password = read(&settings.password_env).map_err(|_| {
        anyhow::anyhow!("OurGroceries password environment variable is missing or invalid")
    })?;
    Ok((email, password))
}

fn resolve_owner_token(
    settings: &crate::config::AssistantConfig,
    read: impl FnOnce(&str) -> std::result::Result<String, std::env::VarError>,
) -> anyhow::Result<Option<String>> {
    let variable = settings
        .token_env
        .as_deref()
        .unwrap_or("AGENTIX_ASSISTANT_TOKEN");
    ensure!(
        !variable.is_empty(),
        "assistant token environment variable name must not be empty"
    );
    match read(variable) {
        Ok(token) => {
            routes::validate_token(&token)?;
            Ok(Some(token))
        }
        Err(std::env::VarError::NotPresent) if settings.token_env.is_none() => Ok(None),
        Err(_) => anyhow::bail!("configured assistant owner token is missing or not Unicode"),
    }
}
