pub mod config;
pub mod controller;
pub mod mcp;
pub mod output;
pub mod tools;
pub mod tui;
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
