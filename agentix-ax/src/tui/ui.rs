use ratatui::{
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::{AgentStatus, App, LogKind, TuiConfig};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(f: &mut Frame, app: &App, config: &TuiConfig) {
    let area = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(area);

    // Title bar
    let status_suffix = match &app.status {
        AgentStatus::Idle => "  idle".to_string(),
        AgentStatus::Running => format!("  {} calls", app.calls_made),
        AgentStatus::Done => "  done".to_string(),
    };
    let no_cloud_tag = if config.no_cloud { "" } else { " → " };
    let cloud_part = if config.no_cloud {
        ""
    } else {
        config.cloud.as_str()
    };
    let title = format!(
        " ax{}  {}{}{}  |{}",
        if config.dangerous {
            " [approvals off]"
        } else {
            ""
        },
        crate::sanitize(if config.model.is_empty() {
            "no model selected"
        } else {
            &config.model
        }),
        no_cloud_tag,
        cloud_part,
        status_suffix
    );
    f.render_widget(
        Paragraph::new(title).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        chunks[0],
    );

    // Log area — scroll from bottom (scroll=0 means show latest)
    let log_height = chunks[1].height as usize;
    let width = usize::from(chunks[1].width).max(1);
    let mut lines = Vec::new();
    for entry in &app.log {
        let mut line = String::new();
        let mut used = 0;
        for c in entry.text.chars() {
            let size = c.width().unwrap_or(0);
            if used + size > width && !line.is_empty() {
                lines.push((entry.kind, std::mem::take(&mut line)));
                used = 0;
            }
            line.push(c);
            used += size;
        }
        lines.push((entry.kind, line));
    }
    let total = lines.len();
    let visible_start = if total > log_height {
        let max_offset = total - log_height;
        max_offset.saturating_sub(app.scroll)
    } else {
        0
    };

    let items: Vec<ListItem> = lines
        .iter()
        .skip(visible_start)
        .take(log_height)
        .map(|(kind, text)| {
            let color = match kind {
                LogKind::Task => Color::Cyan,
                LogKind::ToolCall => Color::Yellow,
                LogKind::ToolResult => Color::Green,
                LogKind::CloudEscalation => Color::LightBlue,
                LogKind::Stagnation | LogKind::Budget => Color::Magenta,
                LogKind::Answer => Color::LightGreen,
                LogKind::Error => Color::Red,
            };
            ListItem::new(Line::from(Span::styled(
                text.as_str(),
                Style::default().fg(color),
            )))
        })
        .collect();

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::NONE)),
        chunks[1],
    );

    // Activity / hint line
    let hint = match &app.status {
        AgentStatus::Idle => {
            Paragraph::new(" Idle — type a task · /model selects model · Ctrl+C quits")
                .style(Style::default().fg(Color::DarkGray))
        }
        AgentStatus::Running => {
            let sp = SPINNER[app.spinner_frame];
            Paragraph::new(if app.approval.is_some() {
                " Review action above: y approve / n deny • ↑↓ scroll • Ctrl+C cancel".into()
            } else {
                format!(
                    " {} Working…  ({} calls, {} cloud escalations)",
                    sp, app.calls_made, app.escalations_made
                )
            })
            .style(Style::default().fg(Color::Yellow))
        }
        AgentStatus::Done => {
            Paragraph::new(" Done — type another task · /model selects model · Ctrl+C quits")
                .style(Style::default().fg(Color::Green))
        }
    };
    f.render_widget(hint, chunks[2]);

    // Input box
    let is_running = matches!(app.status, AgentStatus::Running);
    let border_style = if is_running {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let input_display = format!("> {}", app.input);
    f.render_widget(
        Paragraph::new(input_display.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Task ")
                    .border_style(border_style),
            )
            .wrap(Wrap { trim: false }),
        chunks[3],
    );

    // Cursor (only when input is active)
    if !is_running {
        let inner_w = chunks[3].width.saturating_sub(4) as usize; // 2 borders + "> "
        let cursor_width = app.input[..app.cursor_pos].width();
        if cursor_width <= inner_w {
            f.set_cursor_position(Position {
                x: chunks[3].x + 1 + 2 + cursor_width as u16,
                y: chunks[3].y + 1,
            });
        }
    }
    if let Some(picker) = &app.picker {
        render_picker(f, picker);
    }
}

fn render_picker(f: &mut Frame, picker: &super::model_picker::ModelPicker) {
    use agentix_api::models::CodingSupport;
    use ratatui::widgets::{Clear, ListState};
    let area = f.area();
    f.render_widget(Clear, area);
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(4),
    ])
    .split(area);
    f.render_widget(
        Paragraph::new(format!("Filter: {}", crate::sanitize(&picker.filter)))
            .block(Block::bordered().title(" Local models ")),
        chunks[0],
    );
    let visible = picker.visible();
    let rows: Vec<ListItem> = visible
        .iter()
        .map(|m| {
            let coding = match &m.coding_support {
                CodingSupport::Supported { .. } => "coding supported",
                CodingSupport::Unsupported { .. } => "not a coding model",
                CodingSupport::Unknown => "coding support checked on load",
            };
            ListItem::new(vec![
                Line::from(crate::sanitize(m.id.as_str())),
                Line::from(format!(
                    "  {:?} · {coding} · {} context · {:.1} GiB",
                    m.resident,
                    m.context_limit,
                    m.byte_size.get() as f64 / 1073741824.0
                )),
            ])
        })
        .collect();
    let mut state =
        ListState::default().with_selected((!visible.is_empty()).then_some(picker.selected));
    f.render_stateful_widget(
        List::new(rows).highlight_symbol("› ").highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        chunks[1],
        &mut state,
    );
    f.render_widget(
        Paragraph::new(crate::sanitize(&picker.message))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Status ")),
        chunks[2],
    );
}
