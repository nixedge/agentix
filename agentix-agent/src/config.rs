use anyhow::Context;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub gateway_url: String,
    pub local_model: String,
    pub db_path: String,
    pub bind_addr: String,
    pub ntfy: NtfyConfig,
    pub google: Option<GoogleConfig>,
    pub scheduler: SchedulerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NtfyConfig {
    pub url: String,
    pub topic: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleConfig {
    pub credentials_path: String,
    pub token_cache_path: String,
    pub calendar_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default = "default_notification_interval")]
    pub notification_interval_secs: u64,
    #[serde(default = "default_gmail_scan_interval")]
    pub gmail_scan_interval_secs: u64,
    #[serde(default = "default_event_reminder_lead")]
    pub event_reminder_lead_secs: i64,
    #[serde(default = "default_overdue_grace")]
    pub overdue_grace_secs: i64,
}

fn default_notification_interval() -> u64 {
    300
}
fn default_gmail_scan_interval() -> u64 {
    900
}
fn default_event_reminder_lead() -> i64 {
    1800
}
fn default_overdue_grace() -> i64 {
    300
}

impl AgentConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config from {}", path.display()))?;
        let mut config: AgentConfig =
            toml::from_str(&raw).with_context(|| "parsing config TOML")?;
        config.db_path = expand_home(&config.db_path);
        if let Some(ref mut g) = config.google {
            g.credentials_path = expand_home(&g.credentials_path);
            g.token_cache_path = expand_home(&g.token_cache_path);
        }
        Ok(config)
    }
}

fn expand_home(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest).to_string_lossy().into_owned();
        }
    }
    s.to_owned()
}
