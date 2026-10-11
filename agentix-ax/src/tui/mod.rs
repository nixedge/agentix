pub mod model_picker;
mod ui;

use crate::mcp::McpServer;
use agentix_api::agent::{ApprovalDecision, ApprovalRequest, ApprovalResponse};
use agentix_harness::{approval::Cancellation, AgentEvent, AgentOutput};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, Mutex};

pub struct McpToolHandle {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub server: Arc<Mutex<McpServer>>,
}

pub type TuiConfig = crate::controller::Controller;

pub struct LogEntry {
    pub kind: LogKind,
    pub text: String,
}

#[derive(Clone, Copy)]
pub enum LogKind {
    Task,
    ToolCall,
    ToolResult,
    CloudEscalation,
    Stagnation,
    Budget,
    Answer,
    Error,
}

pub enum AgentStatus {
    Idle,
    Running,
    Done,
}

pub struct App {
    pub log: Vec<LogEntry>,
    /// Lines from the bottom; 0 = follow latest.
    pub scroll: usize,
    pub input: String,
    pub cursor_pos: usize,
    pub status: AgentStatus,
    pub spinner_frame: usize,
    pub calls_made: usize,
    pub escalations_made: usize,
    pub should_quit: bool,
    agent_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    done_rx: Option<oneshot::Receiver<Result<AgentOutput>>>,
    cancel: Cancellation,
    approval: Option<ApprovalRequest>,
    streaming: bool,
    pub picker: Option<model_picker::ModelPicker>,
}

impl App {
    fn new() -> Self {
        Self {
            log: vec![],
            scroll: 0,
            input: String::new(),
            cursor_pos: 0,
            status: AgentStatus::Idle,
            spinner_frame: 0,
            calls_made: 0,
            escalations_made: 0,
            should_quit: false,
            agent_rx: None,
            done_rx: None,
            cancel: Cancellation::default(),
            approval: None,
            streaming: false,
            picker: None,
        }
    }

    fn push_log(&mut self, kind: LogKind, text: impl Into<String>) {
        let text = crate::sanitize(&text.into());
        for line in text.lines().take(1000) {
            self.log.push(LogEntry {
                kind,
                text: line.chars().take(16384).collect(),
            });
        }
        if self.log.len() > 2000 {
            self.log.drain(..self.log.len() - 2000);
        }
        let mut bytes: usize = self.log.iter().map(|entry| entry.text.len()).sum();
        let mut discard = 0;
        while bytes > 1024 * 1024 && discard < self.log.len() {
            bytes -= self.log[discard].text.len();
            discard += 1;
        }
        self.log.drain(..discard);
        // When following (scroll == 0), auto-scroll keeps working because
        // render always shows the last N entries at scroll=0. No state change needed.
    }
}

pub async fn run_tui(mut config: TuiConfig) -> Result<()> {
    enable_raw_mode()?;
    let _restore = RestoreTerminal;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, event::EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut app = App::new();
    if config.model.is_empty() {
        app.picker = Some(model_picker::ModelPicker::open(config.gateway.clone()));
    }
    let result = event_loop(&mut terminal, &mut app, &mut config).await;
    if let Some(done) = app.done_rx.take() {
        app.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(6), done)
            .await
            .map_err(|_| anyhow::anyhow!("agent cleanup timed out"))??;
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        event::DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    result
}
struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            event::DisableBracketedPaste,
            crossterm::cursor::Show
        );
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    config: &mut TuiConfig,
) -> Result<()> {
    let mut tick: u64 = 0;

    loop {
        // The upstream TTY backend does not inspect input with a zero timeout.
        // Keep its bounded blocking poll off the Tokio runtime.
        let input_events = tokio::task::spawn_blocking(|| -> io::Result<Vec<Event>> {
            let mut events = Vec::new();
            for _ in 0..256 {
                if !event::poll(Duration::from_millis(1))? {
                    break;
                }
                events.push(event::read()?);
            }
            Ok(events)
        })
        .await??;
        for input_event in input_events {
            if let Event::Paste(text) = &input_event {
                if app.picker.is_none() && !matches!(app.status, AgentStatus::Running) {
                    let text = crate::sanitize(text);
                    let text: String = text
                        .chars()
                        .take(65536usize.saturating_sub(app.input.len()) / 4)
                        .collect();
                    app.input.insert_str(app.cursor_pos, &text);
                    app.cursor_pos += text.len();
                }
            }
            if let Event::Key(key) = input_event {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code, key.modifiers, config);
                }
            }
        }

        if let Some(ready) = app
            .picker
            .as_mut()
            .and_then(model_picker::ModelPicker::poll)
        {
            config.model = ready.model_id.as_str().to_owned();
            app.picker = None;
            app.push_log(
                LogKind::ToolResult,
                format!(
                    "Selected {} · {} token context",
                    config.model, ready.context_limit
                ),
            );
        }

        // Drain agent events — collect first so the borrow on app.agent_rx ends
        // before we call process_agent_event (which needs &mut App).
        let mut agent_disconnected = false;
        let mut pending_events: Vec<AgentEvent> = vec![];
        if let Some(rx) = &mut app.agent_rx {
            for _ in 0..256 {
                match rx.try_recv() {
                    Ok(ev) => pending_events.push(ev),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        agent_disconnected = true;
                        break;
                    }
                }
            }
        }
        if agent_disconnected {
            app.agent_rx = None;
        }
        for ev in pending_events {
            process_agent_event(app, ev);
        }

        // Check for agent completion.
        if app.agent_rx.as_ref().is_none_or(|rx| rx.is_empty()) {
            if let Some(mut rx) = app.done_rx.take() {
                match rx.try_recv() {
                    Ok(result) => process_completion(app, result),
                    Err(oneshot::error::TryRecvError::Empty) => {
                        app.done_rx = Some(rx);
                    }
                    Err(oneshot::error::TryRecvError::Closed) => {
                        app.push_log(LogKind::Error, "✗ Agent task dropped unexpectedly");
                        app.status = AgentStatus::Idle;
                    }
                }
            }
        }

        // Advance spinner every other tick (every ~100 ms).
        tick = tick.wrapping_add(1);
        if tick.is_multiple_of(2) {
            app.spinner_frame = (app.spinner_frame + 1) % ui::SPINNER.len();
        }

        terminal.draw(|f| ui::render(f, app, config))?;

        if app.should_quit {
            break;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers, config: &TuiConfig) {
    if let Some(picker) = &mut app.picker {
        match (code, mods) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => app.picker = None,
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => app.should_quit = true,
            (KeyCode::Char('r'), KeyModifiers::CONTROL) => {
                app.picker = Some(model_picker::ModelPicker::open(config.gateway.clone()))
            }
            (KeyCode::Up, _) => picker.selected = picker.selected.saturating_sub(1),
            (KeyCode::Down, _) => {
                picker.selected =
                    (picker.selected + 1).min(picker.visible().len().saturating_sub(1))
            }
            (KeyCode::Enter, _) => match config.selection_context() {
                Ok(context) => picker.choose(config.gateway.clone(), context),
                Err(error) => picker.message = error.to_string(),
            },
            (KeyCode::Backspace, _) => {
                picker.filter.pop();
                picker.selected = 0;
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT)
                if picker.filter.len() < 256 =>
            {
                picker.filter.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        return;
    }
    let is_running = matches!(app.status, AgentStatus::Running);

    match (code, mods) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
            if is_running {
                app.cancel.cancel();
                app.push_log(LogKind::Error, "Cancelling; waiting for command cleanup…");
            } else {
                app.should_quit = true;
            }
        }
        (KeyCode::Char(choice @ ('y' | 'n')), _) if app.approval.is_some() => {
            if let Some(request) = app.approval.take() {
                let decision = if choice == 'y' {
                    ApprovalDecision::Allow
                } else {
                    ApprovalDecision::Deny
                };
                if let Err(error) = config.approvals.respond(ApprovalResponse {
                    request_id: request.request_id,
                    context: request.context,
                    arguments_hash: request.arguments_hash,
                    decision,
                }) {
                    app.push_log(LogKind::Error, error.to_string());
                }
            }
        }
        (KeyCode::Enter, _) if !is_running => {
            let task = app.input.trim().to_string();
            if !task.is_empty() {
                app.input.clear();
                app.cursor_pos = 0;
                if task == "/model" || task == "/models" {
                    app.picker = Some(model_picker::ModelPicker::open(config.gateway.clone()));
                } else if task.starts_with('/') {
                    app.push_log(
                        LogKind::Error,
                        "Unknown command. Use /model to choose an installed model.",
                    );
                } else if config.model.is_empty() {
                    app.push_log(LogKind::Error, "Choose a model with /model first");
                    app.input = task;
                    app.cursor_pos = app.input.len();
                } else {
                    submit_task(app, task, config);
                }
            }
        }
        (KeyCode::Up, _) => {
            // Scroll back (increase offset from bottom), capped at log length.
            app.scroll = (app.scroll + 1).min(65536);
        }
        (KeyCode::Down, _) => {
            app.scroll = app.scroll.saturating_sub(1);
        }
        (KeyCode::PageUp, _) => {
            app.scroll = (app.scroll + 10).min(65536);
        }
        (KeyCode::PageDown, _) => {
            app.scroll = app.scroll.saturating_sub(10);
        }
        (KeyCode::Left, _) if !is_running => {
            app.cursor_pos = previous_boundary(&app.input, app.cursor_pos);
        }
        (KeyCode::Right, _) if !is_running => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos += app.input[app.cursor_pos..]
                    .chars()
                    .next()
                    .map(char::len_utf8)
                    .unwrap_or(0);
            }
        }
        (KeyCode::Home, _) if !is_running => {
            app.cursor_pos = 0;
        }
        (KeyCode::End, _) if !is_running => {
            app.cursor_pos = app.input.len();
        }
        (KeyCode::Backspace, _) if !is_running => {
            if app.cursor_pos > 0 {
                app.cursor_pos = previous_boundary(&app.input, app.cursor_pos);
                app.input.remove(app.cursor_pos);
            }
        }
        (KeyCode::Delete, _) if !is_running => {
            if app.cursor_pos < app.input.len() {
                app.input.remove(app.cursor_pos);
            }
        }
        (KeyCode::Char(c), KeyModifiers::NONE) | (KeyCode::Char(c), KeyModifiers::SHIFT)
            if !is_running =>
        {
            if app.input.len() + c.len_utf8() <= 65536 {
                app.input.insert(app.cursor_pos, c);
                app.cursor_pos += c.len_utf8();
            }
        }
        _ => {}
    }
}
fn previous_boundary(text: &str, position: usize) -> usize {
    text[..position]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn submit_task(app: &mut App, task: String, config: &TuiConfig) {
    app.push_log(LogKind::Task, format!("◉ {task}"));
    app.status = AgentStatus::Running;
    app.calls_made = 0;
    app.escalations_made = 0;

    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (done_tx, done_rx) = oneshot::channel::<Result<AgentOutput>>();
    app.agent_rx = Some(event_rx);
    app.done_rx = Some(done_rx);

    app.cancel = Cancellation::default();
    let agent = match config.build(app.cancel.clone(), event_tx.clone(), true) {
        Ok(agent) => agent,
        Err(error) => {
            process_completion(app, Err(error));
            return;
        }
    };

    tokio::spawn(async move {
        let result = agent.run_with_events(&task, event_tx).await;
        let _ = done_tx.send(result);
    });
}

fn process_agent_event(app: &mut App, event: AgentEvent) {
    if !matches!(&event, AgentEvent::TextDelta { .. }) {
        app.streaming = false;
    }
    match event {
        AgentEvent::TurnStarted { .. } => {}
        AgentEvent::TextDelta { text } => {
            let text = crate::sanitize(&text);
            for part in text.split_inclusive('\n') {
                let line = part.trim_end_matches('\n');
                if app.streaming
                    && app
                        .log
                        .last()
                        .is_some_and(|entry| entry.text.len() + line.len() <= 16384)
                {
                    if let Some(entry) = app.log.last_mut() {
                        entry.text.push_str(line);
                    }
                } else {
                    app.push_log(LogKind::Answer, line);
                }
                app.streaming = !part.ends_with('\n');
            }
        }
        AgentEvent::ToolOutput { name, text } => {
            app.push_log(LogKind::ToolResult, format!("{name}: {text}"))
        }
        AgentEvent::ApprovalRequired { request } => {
            app.push_log(
                LogKind::ToolCall,
                format!(
                    "Approve {} in {}? [y/n]\n{}",
                    request.tool.as_str(),
                    request.cwd.as_str(),
                    request.effects.as_str()
                ),
            );
            app.approval = Some(request);
        }
        AgentEvent::ToolCallStarted {
            name, args_preview, ..
        } => {
            app.calls_made += 1;
            app.push_log(LogKind::ToolCall, format!("⟳ {name}  {args_preview}"));
        }
        AgentEvent::ToolCallCompleted {
            name: _,
            result_preview,
            ..
        } => {
            app.push_log(LogKind::ToolResult, format!("✓ {result_preview}"));
        }
        AgentEvent::CloudEscalation { question_preview } => {
            app.escalations_made += 1;
            app.push_log(LogKind::CloudEscalation, format!("☁ {question_preview}"));
        }
        AgentEvent::StagnationDetected => {
            app.push_log(
                LogKind::Stagnation,
                "⚠ Stagnation detected — intervention injected",
            );
        }
        AgentEvent::BudgetExhausted => {
            app.push_log(
                LogKind::Budget,
                "◎ Tool budget exhausted — forcing final answer",
            );
        }
    }
}

fn process_completion(app: &mut App, result: Result<AgentOutput>) {
    app.agent_rx = None;
    app.approval = None;
    match result {
        Ok(output) => {
            app.push_log(LogKind::Answer, format!("◎ {}", output.answer));
            app.push_log(
                LogKind::ToolResult,
                format!(
                    "  [{} calls · {} cloud · {} interventions]",
                    output.tool_calls_made, output.escalations, output.interventions
                ),
            );
        }
        Err(e) => {
            app.push_log(LogKind::Error, format!("✗ {e}"));
        }
    }
    app.status = AgentStatus::Done;
}
