pub mod model_picker;
mod transcript;
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Task,
    ToolCall,
    ToolResult,
    CloudEscalation,
    Stagnation,
    Budget,
    Answer,
    Error,
    Detail,
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
    streamed_answer: String,
    pub show_details: bool,
    log_revision: u64,
    transcript_cache: std::cell::RefCell<transcript::Cache>,
    submissions: Vec<String>,
    history_position: Option<usize>,
    draft: String,
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
            streamed_answer: String::new(),
            show_details: false,
            log_revision: 0,
            transcript_cache: std::cell::RefCell::new(transcript::Cache::default()),
            submissions: Vec::new(),
            history_position: None,
            draft: String::new(),
            picker: None,
        }
    }

    fn push_log(&mut self, kind: LogKind, text: impl Into<String>) {
        self.log_revision = self.log_revision.wrapping_add(1);
        let text = crate::sanitize(&text.into());
        for line in text.split('\n').take(1000) {
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
        let gateway = config.gateway.clone();
        match tokio::task::spawn_blocking(move || crate::config::load_selected_model(&gateway))
            .await?
        {
            Ok(Some(model)) => config.model = model,
            Ok(None) => {}
            Err(error) => app.push_log(
                LogKind::Error,
                format!("Could not restore model preference: {error}. Choose with /model."),
            ),
        }
    }
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
            remember_model(app, config).await;
            app.picker = None;
            app.push_log(
                LogKind::ToolResult,
                format!(
                    "Selected {} · {} token context",
                    ui::model_label(&config.model),
                    ready.context_limit
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
                    Ok(result) => {
                        if result.is_ok() {
                            remember_model(app, config).await;
                        }
                        process_completion(app, result);
                    }
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

async fn remember_model(app: &mut App, config: &TuiConfig) {
    let gateway = config.gateway.clone();
    let model = config.model.clone();
    let result =
        tokio::task::spawn_blocking(move || crate::config::save_selected_model(&gateway, &model))
            .await;
    let error = match result {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error.to_string(),
        Err(error) => error.to_string(),
    };
    app.push_log(LogKind::Error, format!("Could not remember model: {error}"));
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers, config: &TuiConfig) {
    if code == KeyCode::Char('t') && mods == KeyModifiers::CONTROL {
        app.show_details = !app.show_details;
        app.scroll = 0;
        return;
    }
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
        (KeyCode::Esc, _) if is_running => {
            app.cancel.cancel();
            app.push_log(
                LogKind::ToolResult,
                "Cancelling; waiting for command cleanup…",
            );
        }
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
        (KeyCode::Up, KeyModifiers::NONE) if !is_running => {
            if !app.submissions.is_empty() {
                let position = match app.history_position {
                    Some(position) => position.saturating_sub(1),
                    None => {
                        app.draft = app.input.clone();
                        app.submissions.len() - 1
                    }
                };
                app.input = app.submissions[position].clone();
                app.cursor_pos = app.input.len();
                app.history_position = Some(position);
            }
        }
        (KeyCode::Down, KeyModifiers::NONE) if !is_running => {
            if let Some(position) = app.history_position {
                if position + 1 < app.submissions.len() {
                    app.history_position = Some(position + 1);
                    app.input = app.submissions[position + 1].clone();
                } else {
                    app.history_position = None;
                    app.input = app.draft.clone();
                }
                app.cursor_pos = app.input.len();
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
    app.submissions.push(task.clone());
    let mut bytes: usize = app.submissions.iter().map(String::len).sum();
    while app.submissions.len() > 100 || bytes > 1024 * 1024 {
        bytes -= app.submissions.remove(0).len();
    }
    app.history_position = None;
    app.draft.clear();
    app.push_log(LogKind::Task, format!("› {task}"));
    app.status = AgentStatus::Running;
    app.calls_made = 0;
    app.escalations_made = 0;
    app.streamed_answer.clear();
    app.streaming = false;

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
            if app.streamed_answer.len() + text.len() <= 1024 * 1024 {
                app.streamed_answer.push_str(&text);
            }
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
                        app.log_revision = app.log_revision.wrapping_add(1);
                    }
                } else {
                    app.push_log(LogKind::Answer, line);
                }
                app.streaming = !part.ends_with('\n');
            }
        }
        AgentEvent::ToolOutput { name, text } => {
            app.push_log(LogKind::Detail, format!("  {name}: {text}"))
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
            app.streamed_answer.clear();
            app.push_log(LogKind::ToolCall, format!("• {name}"));
            app.push_log(LogKind::Detail, format!("  {args_preview}"));
        }
        AgentEvent::ToolCallCompleted {
            name,
            success,
            result_preview,
            ..
        } => {
            let command_failed = name == "run_command"
                && serde_json::from_str::<serde_json::Value>(&result_preview)
                    .ok()
                    .is_some_and(|value| {
                        value["exit_code"].as_i64().is_some_and(|code| code != 0)
                            || !value["stopped"].is_null()
                    });
            if success && !command_failed {
                if let Some(entry) = app.log.iter_mut().rev().find(|entry| {
                    entry.kind == LogKind::ToolCall && entry.text == format!("• {name}")
                }) {
                    entry.text = format!("✓ {name}");
                }
                app.push_log(LogKind::Detail, format!("  ✓ {result_preview}"));
            } else {
                app.push_log(LogKind::Error, format!("✗ {name}: {result_preview}"));
            }
        }
        AgentEvent::CloudEscalation { question_preview } => {
            app.escalations_made += 1;
            app.streamed_answer.clear();
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
            let answer = crate::sanitize(&output.answer);
            if app.streamed_answer.trim() != answer.trim() {
                app.push_log(LogKind::Answer, answer);
            }
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
            if e.chain().nth(1).is_some() {
                app.push_log(LogKind::Detail, format!("  {e:#}"));
            }
        }
    }
    app.status = AgentStatus::Done;
    app.streaming = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{controller::Controller, tools::access::FileAccess};
    use ratatui::{backend::TestBackend, style::Color};

    fn config(root: &std::path::Path) -> Result<Controller> {
        Ok(Controller::new(
            "hf.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf".into(),
            "http://localhost:11434".into(),
            FileAccess::new(root.into(), vec![root.into()], vec![root.into()])?,
        ))
    }

    fn screen(app: &App, config: &Controller, width: u16, height: u16) -> Result<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| ui::render(frame, app, config))?;
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        Ok(text)
    }

    #[test]
    fn history_restores_draft_and_error_details_are_expandable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path())?;
        let mut app = App::new();
        app.submissions = vec!["first".into(), "second 界".into()];
        app.input = "unfinished draft".into();
        handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE, &config);
        assert_eq!(app.input, "second 界");
        assert_eq!(app.cursor_pos, app.input.len());
        handle_key(&mut app, KeyCode::Up, KeyModifiers::NONE, &config);
        assert_eq!(app.input, "first");
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, &config);
        handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, &config);
        assert_eq!(app.input, "unfinished draft");
        process_completion(
            &mut app,
            Err(anyhow::anyhow!("underlying context error")
                .context("model stream reported an error")),
        );
        assert!(!screen(&app, &config, 100, 24)?.contains("underlying context error"));
        handle_key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL, &config);
        assert!(screen(&app, &config, 100, 24)?.contains("underlying context error"));
        Ok(())
    }

    #[test]
    fn footer_and_detail_toggle_work_while_running_and_after_completion() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut config = config(dir.path())?;
        config.dangerous = true;
        assert_eq!(
            ui::model_label(&config.model),
            "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M"
        );
        assert_eq!(ui::model_label("qwen3:30b"), "qwen3:30b");
        let mut app = App::new();
        app.status = AgentStatus::Running;
        process_agent_event(
            &mut app,
            AgentEvent::ToolCallStarted {
                invocation_id: "1".into(),
                name: "run_command".into(),
                args_preview: "git diff --stat".into(),
            },
        );
        process_agent_event(
            &mut app,
            AgentEvent::ToolOutput {
                name: "run_command".into(),
                text: "secret-detail-marker".into(),
            },
        );
        let collapsed = screen(&app, &config, 100, 24)?;
        assert!(collapsed.contains("Qwen3-Coder-30B-A3B-Instruct-Q4_K_M"));
        assert!(collapsed.contains(&dir.path().display().to_string()));
        assert!(collapsed.contains("approvals off"));
        assert!(collapsed.contains("run_command"));
        assert!(!collapsed.contains("secret-detail-marker"));
        handle_key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL, &config);
        assert!(screen(&app, &config, 100, 24)?.contains("secret-detail-marker"));
        app.status = AgentStatus::Done;
        handle_key(&mut app, KeyCode::Char('t'), KeyModifiers::CONTROL, &config);
        assert!(!screen(&app, &config, 100, 24)?.contains("secret-detail-marker"));
        config.model = "replacement-model".into();
        assert!(screen(&app, &config, 80, 24)?.contains("replacement-model"));
        Ok(())
    }

    #[test]
    fn completion_deduplicates_only_the_final_streamed_response() {
        let mut app = App::new();
        for text in ["First line\n", "\n", "second ", "line"] {
            process_agent_event(&mut app, AgentEvent::TextDelta { text: text.into() });
        }
        process_completion(
            &mut app,
            Ok(AgentOutput {
                answer: "First line\n\nsecond line".into(),
                tool_calls_made: 0,
                escalations: 0,
                interventions: 0,
            }),
        );
        assert_eq!(
            app.log.iter().filter(|e| e.text == "second line").count(),
            1
        );
        assert!(app.log.iter().any(|e| e.text.is_empty()));
        process_agent_event(
            &mut app,
            AgentEvent::ToolCallStarted {
                invocation_id: "1".into(),
                name: "read_file".into(),
                args_preview: "file".into(),
            },
        );
        process_completion(
            &mut app,
            Ok(AgentOutput {
                answer: "First line\n\nsecond line".into(),
                tool_calls_made: 1,
                escalations: 0,
                interventions: 0,
            }),
        );
        assert_eq!(
            app.log.iter().filter(|e| e.text == "second line").count(),
            2
        );
    }

    #[test]
    fn failed_tools_are_visible_with_details_collapsed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path())?;
        let mut app = App::new();
        process_agent_event(
            &mut app,
            AgentEvent::ToolCallCompleted {
                invocation_id: "1".into(),
                name: "run_command".into(),
                success: false,
                truncated_bytes: 0,
                result_preview: "permission denied".into(),
            },
        );
        assert!(screen(&app, &config, 80, 24)?.contains("permission denied"));
        process_agent_event(
            &mut app,
            AgentEvent::ToolCallCompleted {
                invocation_id: "2".into(),
                name: "run_command".into(),
                success: true,
                truncated_bytes: 0,
                result_preview: r#"{"exit_code":1,"output":"test failed","stopped":null}"#.into(),
            },
        );
        assert!(screen(&app, &config, 80, 24)?.contains("test failed"));
        Ok(())
    }

    #[test]
    fn narrow_unicode_layout_and_terminal_default_colors() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let config = config(dir.path())?;
        let mut app = App::new();
        app.input = "界🦀long input ".repeat(40);
        app.cursor_pos = app.input.len();
        app.push_log(
            LogKind::Answer,
            "hello world with Unicode 界🦀 and a_long_unbroken_token",
        );
        for (width, height) in [(1, 1), (20, 10), (80, 24), (160, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height))?;
            terminal.draw(|frame| ui::render(frame, &app, &config))?;
            let buffer = terminal.backend().buffer();
            if width >= 80 {
                assert!((0..width).any(|x| buffer[(x, height - 1)].fg == Color::Yellow));
                assert!((0..width).any(|x| buffer[(x, height - 1)].fg == Color::Green));
                assert_eq!(buffer[(5, height - 3)].bg, Color::Indexed(236));
                assert_eq!(buffer[(5, 3)].bg, Color::Reset);
            }
        }
        Ok(())
    }
}
