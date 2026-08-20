mod config;
mod db;
mod google;
mod http;
mod scheduler;
mod store;
mod tools;

use anyhow::Context;
use clap::{Parser, Subcommand};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use config::AgentConfig;
use db::AgentDb;
use store::{TodoStore, todos::TodoSource};
use tools::ntfy::NtfyClient;

#[derive(Parser)]
#[command(name = "agentix-agent", about = "Home assistant agent")]
struct Cli {
    #[arg(short, long, default_value = "~/.config/agentix-agent/agent.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Daemon,
    Todos {
        #[command(subcommand)]
        action: TodosAction,
    },
    Notify {
        #[command(subcommand)]
        action: NotifyAction,
    },
}

#[derive(Subcommand)]
enum TodosAction {
    Add {
        title: String,
        #[arg(long)]
        due: Option<chrono::DateTime<chrono::Utc>>,
        #[arg(long)]
        notes: Option<String>,
    },
    List {
        #[arg(long, default_value = "false")]
        completed: bool,
        #[arg(short, long)]
        query: Option<String>,
    },
    Done {
        id: uuid::Uuid,
    },
}

#[derive(Subcommand)]
enum NotifyAction {
    Test {
        #[arg(short, long)]
        title: String,
        #[arg(short, long)]
        body: String,
    },
}

fn expand_config_path(p: PathBuf) -> PathBuf {
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
    let config_path = expand_config_path(cli.config);

    match cli.command {
        Commands::Daemon => {
            let config = AgentConfig::load(&config_path)?;
            run_daemon(config).await
        }
        Commands::Todos { action } => {
            let config = AgentConfig::load(&config_path)?;
            let db = AgentDb::open(&config.db_path)?;
            let store = TodoStore(db);
            run_todos(store, action).await
        }
        Commands::Notify { action } => {
            let config = AgentConfig::load(&config_path)?;
            run_notify(config, action).await
        }
    }
}

async fn run_daemon(config: AgentConfig) -> anyhow::Result<()> {
    let db = AgentDb::open(&config.db_path)?;
    let ntfy = NtfyClient::new(config.ntfy.url.clone(), config.ntfy.topic.clone());

    let state = Arc::new(scheduler::AppState {
        db,
        ntfy,
        config: config.clone(),
        google_auth: None,
    });

    let token = CancellationToken::new();
    scheduler::spawn(Arc::clone(&state), token.clone());

    let app = http::router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("binding to {}", config.bind_addr))?;

    tracing::info!(addr = %config.bind_addr, "agentix-agent listening");

    let server = axum::serve(listener, app);

    tokio::select! {
        result = server => {
            result.context("HTTP server error")?;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("received Ctrl-C, shutting down");
        }
    }

    token.cancel();
    Ok(())
}

async fn run_todos(store: TodoStore, action: TodosAction) -> anyhow::Result<()> {
    match action {
        TodosAction::Add { title, due, notes } => {
            let todo = store
                .insert(title, due, TodoSource::Manual, None, notes)
                .await?;
            println!("Created todo: {} ({})", todo.title, todo.id);
        }
        TodosAction::List { completed, query } => {
            let todos = store.list(completed, query, 100).await?;
            if todos.is_empty() {
                println!("No todos found.");
            } else {
                for t in todos {
                    let status = if t.completed { "[x]" } else { "[ ]" };
                    let due = t
                        .due_at
                        .map(|d| format!(" (due {})", d.format("%Y-%m-%d %H:%M")))
                        .unwrap_or_default();
                    println!("{} {} {}{}", status, t.id, t.title, due);
                }
            }
        }
        TodosAction::Done { id } => {
            store.complete(id).await?;
            println!("Marked {} as done.", id);
        }
    }
    Ok(())
}

async fn run_notify(config: AgentConfig, action: NotifyAction) -> anyhow::Result<()> {
    match action {
        NotifyAction::Test { title, body } => {
            let ntfy = NtfyClient::new(config.ntfy.url, config.ntfy.topic);
            ntfy.send(&title, &body, "default").await?;
            println!("Notification sent.");
        }
    }
    Ok(())
}
