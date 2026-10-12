use agentix_ax::{config, controller, mcp, output::Output, tui};
use agentix_harness::approval::{ApprovalRequired, Cancellation, Cancelled};
use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
#[derive(Parser)]
#[command(name = "ax", about = "Local coding agent; launch inside ax-jail")]
struct Cli {
    /// Task, or '-' for stdin; omit for interactive mode.
    task: Option<String>,
    #[arg(short, long, env = "AGENTIX_MODEL")]
    model: Option<String>,
    #[arg(
        short,
        long,
        default_value = "http://localhost:11430",
        env = "AGENTIX_GATEWAY_URL"
    )]
    gateway: String,
    #[arg(
        short,
        long,
        default_value = "moonshotai/kimi-k2",
        env = "AGENTIX_CLOUD_MODEL"
    )]
    cloud: String,
    /// Explicitly enable approved cloud escalation (daemon must also enable it).
    #[arg(long, conflicts_with = "no_cloud")]
    allow_cloud: bool,
    #[arg(long)]
    no_cloud: bool,
    #[arg(long)]
    no_shell: bool,
    /// Skip tool approval prompts inside the jail. Mount and tool restrictions still apply.
    #[arg(long)]
    dangerous: bool,
    #[arg(long = "mcp")]
    mcp_servers: Vec<String>,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long, default_value_t = 20)]
    max_calls: usize,
    /// Emit newline-delimited JSON events in headless mode.
    #[arg(long)]
    json: bool,
    /// Warm the selected installed model and report coding compatibility, then exit.
    #[arg(long)]
    check: bool,
    /// List installed models and coding readiness, then exit (no jail required).
    #[arg(long)]
    list_models: bool,
}
#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ax: {error:#}");
            std::process::ExitCode::from(if error.is::<ApprovalRequired>() {
                3
            } else if error.is::<Cancelled>() {
                130
            } else {
                1
            })
        }
    }
}
async fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.list_models {
        let models = agentix_harness::client::GatewayClient::new(&cli.gateway)?
            .models()
            .await?;
        println!("{}", serde_json::to_string_pretty(&models)?);
        return Ok(());
    }
    if cli.check {
        let readiness = agentix_harness::client::GatewayClient::new(&cli.gateway)?
            .readiness(
                cli.model
                    .as_deref()
                    .context("--check requires --model or AGENTIX_MODEL")?,
            )
            .await?;
        println!("{}", serde_json::to_string_pretty(&readiness)?);
        anyhow::ensure!(readiness.coding_supported,"this GGUF chat template is not supported for coding; this checkpoint supports validated Qwen-style tool templates");
        return Ok(());
    }
    let access = config::file_access(cli.cwd.unwrap_or(std::env::current_dir()?))?;
    if cli.task.is_some() {
        anyhow::ensure!(
            cli.model.is_some(),
            "headless mode requires --model or AGENTIX_MODEL; launch interactively to use /model"
        );
    }
    let mut controller =
        controller::Controller::new(cli.model.unwrap_or_default(), cli.gateway, access);
    controller.cloud = cli.cloud;
    controller.no_cloud = !cli.allow_cloud || cli.no_cloud;
    controller.no_shell = cli.no_shell;
    controller.dangerous = cli.dangerous;
    controller.max_calls = cli.max_calls;
    for command in cli.mcp_servers {
        let (server, defs) = mcp::spawn_and_init(&command, cli.task.is_some()).await?;
        for def in defs {
            controller.mcp_handles.push(tui::McpToolHandle {
                name: def.name,
                description: def.description,
                input_schema: def.input_schema,
                server: server.clone(),
            });
        }
    }
    let Some(task) = cli.task else {
        return tui::run_tui(controller).await;
    };
    let task = if task == "-" {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        tokio::io::stdin()
            .take(65537)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(bytes.len() <= 65536, "task exceeds 64 KiB");
        String::from_utf8(bytes)?
    } else {
        task
    };
    anyhow::ensure!(task.len() <= 65536, "task exceeds 64 KiB");
    let cancel = Cancellation::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let agent = controller.build(cancel.clone(), tx.clone(), false)?;
    let mut task_handle = tokio::spawn(async move { agent.run_with_events(&task, tx).await });
    let mut output = Output::new(cli.json);
    let result = loop {
        tokio::select! {
         result=&mut task_handle=>break result.context("agent task failed")?,
         event=rx.recv()=>if let Some(event)=event {output.event(event)?;},
         signal=tokio::signal::ctrl_c()=>{signal?;cancel.cancel();},
        }
    };
    while let Ok(event) = rx.try_recv() {
        output.event(event)?;
    }
    output.finish(&result)?;
    result.map(|_| ())
}
