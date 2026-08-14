//! Pre-processor for Literate Haskell Source (`.lhs`) files.
//!
//! Strips Bird-style (`> ` prefix) and LaTeX-style (`\begin{code}...\end{code}`)
//! markup, splitting the file into alternating [`BlockKind::Code`] and
//! [`BlockKind::Prose`] blocks with original file line numbers preserved.

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LhsStyle {
    Bird,
    LaTeX,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Code,
    Prose,
}

#[derive(Debug, Clone)]
pub struct LhsBlock {
    pub kind: BlockKind,
    /// Stripped content: Bird `> ` prefix removed; LaTeX delimiters excluded.
    pub content: String,
    /// 1-indexed line number of the first content line in the original file.
    pub start_line: usize,
    /// 1-indexed line number of the last content line in the original file.
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedLhs {
    pub style: LhsStyle,
    pub blocks: Vec<LhsBlock>,
}

impl ParsedLhs {
    pub fn code_blocks(&self) -> impl Iterator<Item = &LhsBlock> {
        self.blocks.iter().filter(|b| b.kind == BlockKind::Code)
    }

    pub fn prose_blocks(&self) -> impl Iterator<Item = &LhsBlock> {
        self.blocks.iter().filter(|b| b.kind == BlockKind::Prose)
    }

    pub fn has_code(&self) -> bool {
        self.blocks.iter().any(|b| b.kind == BlockKind::Code)
    }
}

// ── Style detection ───────────────────────────────────────────────────────────

/// Returns [`LhsStyle::LaTeX`] if any line is exactly `\begin{code}`;
/// otherwise [`LhsStyle::Bird`].
pub fn detect_style(source: &str) -> LhsStyle {
    if source.lines().any(|l| l == r"\begin{code}") {
        LhsStyle::LaTeX
    } else {
        LhsStyle::Bird
    }
}

// ── Parsers ───────────────────────────────────────────────────────────────────

fn parse_bird_style(source: &str) -> ParsedLhs {
    let mut blocks: Vec<LhsBlock> = Vec::new();
    let mut current_kind: Option<BlockKind> = None;
    let mut current_lines: Vec<String> = Vec::new();
    let mut current_start = 1usize;
    let total_lines = source.lines().count();

    for (i, line) in source.lines().enumerate() {
        let file_line = i + 1;
        // GHC rule: `> ` (bird tick followed by exactly one space) is code.
        // A bare `>` with no trailing space is prose.
        let kind = if line.starts_with("> ") {
            BlockKind::Code
        } else {
            BlockKind::Prose
        };

        match &current_kind {
            Some(ck) if *ck != kind => {
                // Kind changed — flush the current block.
                blocks.push(LhsBlock {
                    kind: ck.clone(),
                    content: current_lines.join("\n"),
                    start_line: current_start,
                    end_line: file_line - 1,
                });
                current_lines.clear();
                current_kind = Some(kind.clone());
                current_start = file_line;
            }
            None => {
                current_kind = Some(kind.clone());
                current_start = file_line;
            }
            _ => {}
        }

        let content = if kind == BlockKind::Code {
            line[2..].to_string() // strip the `> ` prefix
        } else {
            line.to_string()
        };
        current_lines.push(content);
    }

    // Flush the final block.
    if let Some(kind) = current_kind {
        if !current_lines.is_empty() {
            blocks.push(LhsBlock {
                kind,
                content: current_lines.join("\n"),
                start_line: current_start,
                end_line: total_lines.max(current_start),
            });
        }
    }

    ParsedLhs {
        style: LhsStyle::Bird,
        blocks,
    }
}

fn parse_latex_style(source: &str) -> ParsedLhs {
    let mut blocks: Vec<LhsBlock> = Vec::new();
    let mut in_code = false;
    let mut current_lines: Vec<String> = Vec::new();
    // Set to 0 until the first content line of the current block is encountered.
    let mut current_start = 0usize;
    let all_lines: Vec<&str> = source.lines().collect();
    let n = all_lines.len();

    for (i, &line) in all_lines.iter().enumerate() {
        let file_line = i + 1;

        if !in_code && line == r"\begin{code}" {
            // Flush any accumulated prose.
            if !current_lines.is_empty() {
                blocks.push(LhsBlock {
                    kind: BlockKind::Prose,
                    content: current_lines.join("\n"),
                    start_line: current_start,
                    end_line: file_line - 1,
                });
                current_lines.clear();
                current_start = 0;
            }
            in_code = true;
        } else if in_code && line == r"\end{code}" {
            // Flush any accumulated code (may be empty if block was empty).
            if !current_lines.is_empty() {
                blocks.push(LhsBlock {
                    kind: BlockKind::Code,
                    content: current_lines.join("\n"),
                    start_line: current_start,
                    end_line: file_line - 1,
                });
                current_lines.clear();
                current_start = 0;
            }
            in_code = false;
        } else {
            // Regular content line.
            if current_lines.is_empty() {
                current_start = file_line;
            }
            current_lines.push(line.to_string());
        }
    }

    // Flush the final block; warn if \begin{code} was never closed.
    if !current_lines.is_empty() {
        let kind = if in_code {
            eprintln!("warning: unclosed \\begin{{code}} in .lhs file; treating remainder as code");
            BlockKind::Code
        } else {
            BlockKind::Prose
        };
        blocks.push(LhsBlock {
            kind,
            content: current_lines.join("\n"),
            start_line: current_start,
            end_line: n,
        });
    }

    ParsedLhs {
        style: LhsStyle::LaTeX,
        blocks,
    }
}

/// Parse a `.lhs` source file into alternating code and prose blocks.
///
/// Style is auto-detected: LaTeX-style if any line is exactly `\begin{code}`,
/// Bird-style otherwise.
pub fn parse_lhs(source: &str) -> ParsedLhs {
    match detect_style(source) {
        LhsStyle::Bird => parse_bird_style(source),
        LhsStyle::LaTeX => parse_latex_style(source),
    }
}

// ── LineMap ───────────────────────────────────────────────────────────────────

/// Maps 1-indexed line numbers in the concatenated stripped-code buffer back
/// to 1-indexed line numbers in the original `.lhs` file.
///
/// When tree-sitter extracts symbols from the concatenated code buffer, the
/// reported `start_line`/`end_line` are relative to that buffer. `LineMap`
/// translates them back to the original file coordinates for accurate search
/// result metadata.
pub struct LineMap {
    // Each entry: (code_buf_start, file_start, line_count).
    // Sorted by code_buf_start; built from the code blocks in declaration order.
    ranges: Vec<(usize, usize, usize)>,
}

impl LineMap {
    /// Build a `LineMap` from a slice of code blocks (in order of appearance).
    /// Blocks must have `kind == BlockKind::Code`; non-code blocks are skipped.
    pub fn from_code_blocks(blocks: &[&LhsBlock]) -> Self {
        let mut ranges = Vec::new();
        let mut code_buf_line = 1usize;

        for block in blocks {
            if block.kind != BlockKind::Code {
                continue;
            }
            let line_count = block.content.lines().count().max(1);
            ranges.push((code_buf_line, block.start_line, line_count));
            code_buf_line += line_count;
        }

        LineMap { ranges }
    }

    /// Translate a 1-indexed code-buffer line number to a 1-indexed original
    /// file line number. Falls back to returning `code_line` unchanged if no
    /// mapping is found (e.g. empty `LineMap`).
    pub fn file_line(&self, code_line: usize) -> usize {
        // Binary search: find the rightmost range whose start <= code_line.
        let idx = self
            .ranges
            .partition_point(|&(start, _, _)| start <= code_line);
        if idx == 0 {
            return code_line; // no mapping — return as-is
        }
        let (code_start, file_start, line_count) = self.ranges[idx - 1];
        let offset = code_line.saturating_sub(code_start);
        if offset < line_count {
            file_start + offset
        } else {
            // code_line is past the end of this range — clamp to last line.
            file_start + line_count.saturating_sub(1)
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Bird-style ─────────────────────────────────────────────────────────────

    #[test]
    fn bird_empty_file() {
        let parsed = parse_lhs("");
        assert!(parsed.blocks.is_empty());
        assert_eq!(parsed.style, LhsStyle::Bird);
    }

    #[test]
    fn bird_prose_only() {
        let src = "This is prose.\nNo code here.\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].kind, BlockKind::Prose);
        assert_eq!(parsed.blocks[0].start_line, 1);
        assert_eq!(parsed.blocks[0].end_line, 2);
    }

    #[test]
    fn bird_code_only() {
        let src = "> foo = 1\n> bar = 2\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].kind, BlockKind::Code);
        assert!(!parsed.blocks[0].content.contains("> "));
        assert_eq!(parsed.blocks[0].start_line, 1);
        assert_eq!(parsed.blocks[0].end_line, 2);
    }

    #[test]
    fn bird_alternating_blocks() {
        let src = "prose line\n> code line\nmore prose\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.blocks.len(), 3);
        assert_eq!(parsed.blocks[0].kind, BlockKind::Prose);
        assert_eq!(parsed.blocks[1].kind, BlockKind::Code);
        assert_eq!(parsed.blocks[2].kind, BlockKind::Prose);
    }

    #[test]
    fn bird_bare_gt_is_prose() {
        // A bare `>` with no trailing space is prose per GHC semantics.
        let src = "> code\n>\nnot code\n";
        let parsed = parse_lhs(src);
        // First block: one Code line
        assert_eq!(parsed.blocks[0].kind, BlockKind::Code);
        // Second block: Prose (bare `>` + "not code")
        assert_eq!(parsed.blocks[1].kind, BlockKind::Prose);
        assert!(parsed.blocks[1].content.contains('>'));
    }

    #[test]
    fn bird_strips_prefix_from_code() {
        let src = "> myFunc = 42\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.blocks[0].content, "myFunc = 42");
    }

    #[test]
    fn bird_line_numbers_correct() {
        // Lines 1-2 prose, line 3 code, lines 4-5 prose.
        let src = "prose1\nprosa2\n> code\nfinal prose\nstill prose\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.blocks[0].start_line, 1);
        assert_eq!(parsed.blocks[0].end_line, 2);
        assert_eq!(parsed.blocks[1].start_line, 3);
        assert_eq!(parsed.blocks[1].end_line, 3);
        assert_eq!(parsed.blocks[2].start_line, 4);
        assert_eq!(parsed.blocks[2].end_line, 5);
    }

    // ── LaTeX-style ────────────────────────────────────────────────────────────

    #[test]
    fn latex_detected_when_begin_code_present() {
        let src = "prose\n\\begin{code}\nfoo = 1\n\\end{code}\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.style, LhsStyle::LaTeX);
    }

    #[test]
    fn latex_basic_structure() {
        let src = "prose\n\\begin{code}\nfoo = 1\n\\end{code}\nmore prose\n";
        let parsed = parse_lhs(src);
        assert_eq!(parsed.blocks.len(), 3);
        assert_eq!(parsed.blocks[0].kind, BlockKind::Prose);
        assert_eq!(parsed.blocks[1].kind, BlockKind::Code);
        assert_eq!(parsed.blocks[2].kind, BlockKind::Prose);
        // Delimiter lines are NOT part of any block's content.
        assert!(!parsed.blocks[1].content.contains("begin{code}"));
        assert!(!parsed.blocks[1].content.contains("end{code}"));
    }

    #[test]
    fn latex_multiple_code_blocks() {
        let src = "intro\n\\begin{code}\nfoo = 1\n\\end{code}\nmiddle\n\\begin{code}\nbar = 2\n\\end{code}\noutro\n";
        let parsed = parse_lhs(src);
        let code_count = parsed
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Code)
            .count();
        let prose_count = parsed
            .blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Prose)
            .count();
        assert_eq!(code_count, 2);
        assert_eq!(prose_count, 3);
    }

    #[test]
    fn latex_unclosed_begin_code_treated_as_code() {
        let src = "prose\n\\begin{code}\nfoo = 1\nbar = 2\n";
        let parsed = parse_lhs(src);
        let last = parsed.blocks.last().expect("should have blocks");
        assert_eq!(last.kind, BlockKind::Code);
        assert!(last.content.contains("foo = 1"));
        assert!(last.content.contains("bar = 2"));
    }

    #[test]
    fn latex_empty_code_block_not_emitted() {
        // \begin{code} immediately followed by \end{code} — nothing to emit.
        let src = "prose\n\\begin{code}\n\\end{code}\nmore\n";
        let parsed = parse_lhs(src);
        assert!(parsed.blocks.iter().all(|b| !b.content.is_empty()));
    }

    #[test]
    fn latex_line_numbers_correct() {
        // line 1: prose, line 2: \begin{code}, line 3: code, line 4: \end{code}, line 5: prose
        let src = "intro\n\\begin{code}\nfoo = 1\n\\end{code}\noutro\n";
        let parsed = parse_lhs(src);
        let prose1 = &parsed.blocks[0];
        let code1 = &parsed.blocks[1];
        let prose2 = &parsed.blocks[2];
        assert_eq!(prose1.start_line, 1);
        assert_eq!(prose1.end_line, 1);
        assert_eq!(code1.start_line, 3); // delimiter line 2 is excluded
        assert_eq!(code1.end_line, 3);
        assert_eq!(prose2.start_line, 5); // delimiter line 4 is excluded
        assert_eq!(prose2.end_line, 5);
    }

    // ── LineMap ────────────────────────────────────────────────────────────────

    #[test]
    fn linemap_empty() {
        let map = LineMap::from_code_blocks(&[]);
        // Falls back to identity.
        assert_eq!(map.file_line(1), 1);
        assert_eq!(map.file_line(5), 5);
    }

    #[test]
    fn linemap_single_block() {
        // Code block starts at file line 5, has 3 lines.
        let block = LhsBlock {
            kind: BlockKind::Code,
            content: "a\nb\nc".to_string(),
            start_line: 5,
            end_line: 7,
        };
        let map = LineMap::from_code_blocks(&[&block]);
        assert_eq!(map.file_line(1), 5);
        assert_eq!(map.file_line(2), 6);
        assert_eq!(map.file_line(3), 7);
    }

    #[test]
    fn linemap_multiple_blocks() {
        // Block A: file lines 3-5 (3 lines), Block B: file lines 10-11 (2 lines).
        let block_a = LhsBlock {
            kind: BlockKind::Code,
            content: "x\ny\nz".to_string(),
            start_line: 3,
            end_line: 5,
        };
        let block_b = LhsBlock {
            kind: BlockKind::Code,
            content: "p\nq".to_string(),
            start_line: 10,
            end_line: 11,
        };
        let map = LineMap::from_code_blocks(&[&block_a, &block_b]);
        // Block A: code buf lines 1-3 → file lines 3-5
        assert_eq!(map.file_line(1), 3);
        assert_eq!(map.file_line(2), 4);
        assert_eq!(map.file_line(3), 5);
        // Block B: code buf lines 4-5 → file lines 10-11
        assert_eq!(map.file_line(4), 10);
        assert_eq!(map.file_line(5), 11);
    }
}
