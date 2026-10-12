use ratatui::{
    layout::{Constraint, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{AgentStatus, App, TuiConfig};

pub const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn muted() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub(super) fn model_label(model: &str) -> &str {
    let model = if model.starts_with("hf.co/") {
        model
            .rsplit_once(':')
            .map_or(model, |(_, artifact)| artifact)
    } else {
        model
    };
    let name = model.rsplit('/').next().unwrap_or(model);
    name.strip_suffix(".gguf").unwrap_or(name)
}

fn ellipsize(text: &str, width: usize, keep_end: bool) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut chars = Vec::new();
    let mut used = 1;
    let iter: Box<dyn Iterator<Item = char>> = if keep_end {
        Box::new(text.chars().rev())
    } else {
        Box::new(text.chars())
    };
    for c in iter {
        let size = c.width().unwrap_or(0);
        if used + size > width {
            break;
        }
        chars.push(c);
        used += size;
    }
    if keep_end {
        chars.reverse();
        format!("…{}", chars.into_iter().collect::<String>())
    } else {
        format!("{}…", chars.into_iter().collect::<String>())
    }
}

pub fn render(f: &mut Frame, app: &App, config: &TuiConfig) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(f.area());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ax", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("  local coding agent", muted()),
        ])),
        chunks[0],
    );

    let width = usize::from(chunks[1].width.saturating_sub(2)).max(1);
    let mut cache = app.transcript_cache.borrow_mut();
    cache.update(app, width);
    let lines = &cache.lines;
    let height = usize::from(chunks[1].height);
    let start = lines
        .len()
        .saturating_sub(height)
        .saturating_sub(app.scroll);
    f.render_widget(
        Paragraph::new(
            lines
                .iter()
                .skip(start)
                .take(height)
                .cloned()
                .collect::<Vec<_>>(),
        ),
        chunks[1],
    );

    let running = matches!(app.status, AgentStatus::Running);
    let hint = if app.approval.is_some() {
        " Review action above · y approve / n deny · Ctrl+C cancel".to_owned()
    } else if running {
        format!(
            " {} Working · {} tool calls · Esc cancel · Ctrl+T details",
            SPINNER[app.spinner_frame], app.calls_made
        )
    } else {
        let state = if matches!(app.status, AgentStatus::Idle) {
            "Idle"
        } else {
            "Done"
        };
        format!(
            " {state} · /model · Ctrl+T {} details · ↑↓ history · PgUp/Dn scroll",
            if app.show_details { "hide" } else { "show" }
        )
    };
    f.render_widget(Paragraph::new(hint).style(muted()), chunks[2]);

    let available = usize::from(chunks[3].width.saturating_sub(5));
    let before = app.input[..app.cursor_pos].replace('\n', " ");
    let after = app.input[app.cursor_pos..].replace('\n', " ");
    let before = ellipsize(&before, available.saturating_sub(1), true);
    let input = if app.input.is_empty() && !running {
        "› Ask for a change, review, or explanation".to_owned()
    } else {
        format!("› {before}{after}")
    };
    f.render_widget(
        Paragraph::new(format!(" {input}"))
            .style(if app.input.is_empty() {
                Style::default().fg(Color::Gray).bg(Color::Indexed(236))
            } else {
                Style::default().fg(Color::White).bg(Color::Indexed(236))
            })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Message ")
                    .border_style(muted()),
            ),
        chunks[3],
    );
    if !running && chunks[3].height >= 3 && chunks[3].width > 5 {
        f.set_cursor_position(Position {
            x: chunks[3].x + 4 + before.width() as u16,
            y: chunks[3].y + 1,
        });
    }

    let model = if config.model.is_empty() {
        "no model selected"
    } else {
        model_label(&config.model)
    };
    let approvals = if config.dangerous {
        "approvals off"
    } else {
        "approvals on"
    };
    let cloud = if config.no_cloud {
        ""
    } else {
        " · cloud enabled"
    };
    let path = crate::sanitize(&config.access.cwd.display().to_string());
    let path = match std::env::var_os("HOME") {
        Some(home) => config
            .access
            .cwd
            .strip_prefix(&home)
            .ok()
            .map(|relative| format!("~/{}", crate::sanitize(&relative.display().to_string())))
            .unwrap_or(path),
        None => path,
    };
    let footer_width = usize::from(chunks[4].width.saturating_sub(1));
    let suffix = if footer_width >= 70 {
        format!(" · {approvals}{cloud}")
    } else {
        String::new()
    };
    let content_width = footer_width.saturating_sub(suffix.width() + 3);
    let model_width = (content_width / 2).max(content_width.saturating_sub(path.width()));
    let label = ellipsize(&crate::sanitize(model), model_width, false);
    let path = ellipsize(&path, content_width.saturating_sub(label.width()), true);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(label, Style::default().fg(Color::Yellow)),
            Span::styled(" · ", muted()),
            Span::styled(path, Style::default().fg(Color::Green)),
            Span::styled(suffix, muted()),
        ])),
        chunks[4],
    );
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
        Paragraph::new(format!("Filter: {}", crate::sanitize(&picker.filter))).block(
            Block::bordered()
                .title(" Local models ")
                .border_style(muted()),
        ),
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
                Line::styled(
                    format!(
                        "  {:?} · {coding} · {} context · {:.1} GiB",
                        m.resident,
                        m.context_limit,
                        m.byte_size.get() as f64 / 1073741824.0
                    ),
                    muted(),
                ),
            ])
        })
        .collect();
    let mut state =
        ListState::default().with_selected((!visible.is_empty()).then_some(picker.selected));
    f.render_stateful_widget(
        List::new(rows)
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        chunks[1],
        &mut state,
    );
    f.render_widget(
        Paragraph::new(format!(
            "{}\n↑↓ select · Enter load · Esc back · Ctrl+R refresh",
            crate::sanitize(&picker.message)
        ))
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(" Status ").border_style(muted())),
        chunks[2],
    );
}
