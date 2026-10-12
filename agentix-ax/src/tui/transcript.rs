use super::{App, LogKind};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::sync::OnceLock;
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
};
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub(super) struct Cache {
    key: Option<(u64, usize, bool)>,
    pub lines: Vec<Line<'static>>,
}
impl Cache {
    pub fn update(&mut self, app: &App, width: usize) {
        let key = (app.log_revision, width, app.show_details);
        if self.key != Some(key) {
            self.lines = render(app, width);
            self.key = Some(key);
        }
    }
}

fn resources() -> &'static (SyntaxSet, Theme) {
    static RESOURCES: OnceLock<(SyntaxSet, Theme)> = OnceLock::new();
    RESOURCES.get_or_init(|| {
        let syntax = SyntaxSet::load_defaults_nonewlines();
        let theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        (syntax, theme)
    })
}
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let marker = if rest.starts_with('`') {
            "`"
        } else if rest.starts_with("**") {
            "**"
        } else if rest.starts_with('*') {
            "*"
        } else {
            ""
        };
        if !marker.is_empty() {
            if let Some(end) = rest[marker.len()..].find(marker).filter(|end| *end > 0) {
                let body = &rest[marker.len()..marker.len() + end];
                let style = match marker {
                    "`" => base.fg(Color::Cyan),
                    "**" => base.add_modifier(Modifier::BOLD),
                    _ => base.add_modifier(Modifier::ITALIC),
                };
                spans.push(Span::styled(body.to_owned(), style));
                rest = &rest[marker.len() * 2 + end..];
                continue;
            }
        }
        let end = rest
            .char_indices()
            .skip(1)
            .find(|(_, c)| matches!(c, '`' | '*'))
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        spans.push(Span::styled(rest[..end].to_owned(), base));
        rest = &rest[end..];
    }
    spans
}

fn wrap(spans: Vec<Span<'static>>, width: usize, code: bool, prefix: &str) -> Vec<Line<'static>> {
    let chars: Vec<_> = spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |c| (c, span.style)))
        .collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut used = 0;
        let mut space = None;
        while end < chars.len() {
            let (c, _) = chars[end];
            let size = c.width().unwrap_or(0);
            if used + size > width.max(1) && end > start {
                break;
            }
            used += size;
            end += 1;
            if c.is_whitespace() {
                space = Some(end);
            }
        }
        if !code && end < chars.len() {
            if let Some(boundary) = space {
                end = boundary;
            }
        }
        let mut row = vec![Span::styled(
            if start == 0 {
                prefix.to_owned()
            } else {
                "  ".into()
            },
            dim(),
        )];
        for &(c, style) in &chars[start..end] {
            if row.last().is_some_and(|span| span.style == style) {
                if let Some(last) = row.last_mut() {
                    last.content.to_mut().push(c);
                }
            } else {
                row.push(Span::styled(c.to_string(), style));
            }
        }
        lines.push(Line::from(row));
        start = end;
    }
    if lines.is_empty() {
        lines.push(Line::raw(prefix.to_owned()));
    }
    lines
}

fn render(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut previous = None;
    let mut fence: Option<(char, usize)> = None;
    let mut highlighter: Option<HighlightLines<'static>> = None;
    for entry in &app.log {
        // Non-assistant events end a Markdown block, even if details are hidden.
        if entry.kind != LogKind::Answer {
            fence = None;
            highlighter = None;
        }
        if entry.kind == LogKind::Detail && !app.show_details {
            continue;
        }
        let new_group = previous != Some(entry.kind);
        if previous.is_some()
            && new_group
            && matches!(
                entry.kind,
                LogKind::Task | LogKind::Answer | LogKind::ToolCall
            )
        {
            lines.push(Line::default());
        }
        let mut prefix = if entry.kind == LogKind::Answer && new_group {
            "• "
        } else {
            "  "
        };
        let mut code = false;
        let spans = if entry.kind == LogKind::Answer {
            let text = entry.text.trim_start();
            let marker = text.chars().next().filter(|c| matches!(c, '`' | '~'));
            let count = marker
                .map(|m| text.chars().take_while(|c| *c == m).count())
                .unwrap_or(0);
            if let Some((open, length)) = fence {
                code = true;
                if marker == Some(open) && count >= length && text[count..].trim().is_empty() {
                    fence = None;
                    highlighter = None;
                    vec![Span::styled("└", dim())]
                } else if let Some(h) = highlighter.as_mut().filter(|_| entry.text.len() <= 4096) {
                    let (syntaxes, theme) = resources();
                    match h.highlight_line(&entry.text, syntaxes) {
                        Ok(tokens) => tokens
                            .into_iter()
                            .map(|(style, text)| {
                                let mut result = Style::default();
                                if Some(style.foreground) != theme.settings.foreground {
                                    result = result.fg(Color::Rgb(
                                        style.foreground.r,
                                        style.foreground.g,
                                        style.foreground.b,
                                    ));
                                }
                                if style.font_style.contains(FontStyle::BOLD) {
                                    result = result.add_modifier(Modifier::BOLD);
                                }
                                if style.font_style.contains(FontStyle::ITALIC) {
                                    result = result.add_modifier(Modifier::ITALIC);
                                }
                                Span::styled(text.to_owned(), result)
                            })
                            .collect(),
                        Err(_) => {
                            highlighter = None;
                            vec![Span::raw(entry.text.clone())]
                        }
                    }
                } else {
                    // Skipping a pathological line invalidates multiline syntax state.
                    highlighter = None;
                    vec![Span::raw(entry.text.clone())]
                }
            } else if count >= 3 {
                if let Some(marker) = marker {
                    fence = Some((marker, count));
                }
                let language = text[count..].split_whitespace().next().unwrap_or("");
                let (syntaxes, theme) = resources();
                highlighter = syntaxes
                    .find_syntax_by_token(language)
                    .map(|syntax| HighlightLines::new(syntax, theme));
                vec![Span::styled(format!("┌ {language}"), dim())]
            } else {
                let heading = text.chars().take_while(|c| *c == '#').count();
                if (1..=6).contains(&heading) && text[heading..].starts_with(' ') {
                    inline(
                        text[heading..].trim_start(),
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Cyan),
                    )
                } else {
                    inline(&entry.text, Style::default())
                }
            }
        } else {
            let style = match entry.kind {
                LogKind::Task => {
                    prefix = "";
                    Style::default().add_modifier(Modifier::BOLD)
                }
                LogKind::Error => Style::default().fg(Color::Red),
                LogKind::Stagnation | LogKind::Budget => Style::default().fg(Color::Yellow),
                _ => dim(),
            };
            vec![Span::styled(entry.text.clone(), style)]
        };
        lines.extend(wrap(spans, width, code, prefix));
        previous = Some(entry.kind);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn fenced_code_is_colored_and_indentation_survives_wrapping() {
        let mut app = App::new();
        app.push_log(
            LogKind::Answer,
            "```rust\n    let message = \"界hello\";\n```\nplain text",
        );
        let rows = render(&app, 80);
        assert!(text(&rows).contains("      let message"));
        assert!(rows
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| matches!(span.style.fg, Some(Color::Rgb(..)))));
        assert!(rows
            .last()
            .is_some_and(|line| line.spans.iter().all(|span| span.style.fg.is_none())));
        let wrapped = wrap(vec![Span::raw("    界hello")], 5, true, "  ");
        let reconstructed: String = wrapped
            .iter()
            .map(|row| {
                row.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()[2..]
                    .to_owned()
            })
            .collect();
        assert_eq!(reconstructed, "    界hello");
    }
    #[test]
    fn streaming_fence_unknown_language_and_literal_tools() {
        let mut app = App::new();
        app.push_log(LogKind::Answer, "```not-a-language\n  **literal**");
        let mut cache = Cache::default();
        cache.update(&app, 80);
        assert!(text(&cache.lines).contains("**literal**"));
        let pointer = cache.lines.as_ptr();
        cache.update(&app, 80);
        assert_eq!(pointer, cache.lines.as_ptr());
        app.push_log(LogKind::Answer, "```\n**bold** and `inline` and *emphasis*");
        cache.update(&app, 80);
        let rendered = text(&cache.lines);
        assert!(rendered.contains("bold and inline and emphasis"));
        app.push_log(LogKind::Detail, "**literal tool**");
        app.show_details = true;
        cache.update(&app, 80);
        assert!(text(&cache.lines).contains("**literal tool**"));
        assert!(cache
            .lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.fg == Some(Color::Cyan)));
    }
}
