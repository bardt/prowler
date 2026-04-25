use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::diff::{DiffLine, FileDiff};
use crate::github::{CommentSide, CommentThread};
use crate::tui::syntax;

const BG_ADDED: Color = Color::Rgb(20, 50, 25);
const BG_REMOVED: Color = Color::Rgb(60, 25, 25);
const BG_MOVED: Color = Color::Rgb(20, 35, 55);

#[derive(Clone)]
pub enum Cell {
    Empty,
    HunkHeader(String),
    Context(String),
    Added(String),
    Removed(String),
    Moved(String),
    /// Header line of a comment within a thread: `┌ @author 2026-04-20 14:30`
    /// (`├` for replies, with an optional dim `(pending)` suffix).
    CommentHeader {
        text: String,
        is_root: bool,
        is_pending: bool,
    },
    /// Body line of a comment: `│ {text}`.
    CommentBody(String),
    /// Thread terminator: `└`.
    CommentEnd,
}

#[derive(Clone)]
pub struct Row {
    pub base: Cell,
    pub head: Cell,
    pub base_line: Option<u32>,
    pub head_line: Option<u32>,
    /// Set when this row is part of a comment thread (header / body / end).
    /// Used by `r` (reply) to identify which thread the cursor is on.
    pub thread_id: Option<String>,
}

pub struct LaidOutDiff {
    pub rows: Vec<Row>,
    pub hunk_starts: Vec<usize>,
}

impl LaidOutDiff {
    pub fn from_file(file: &FileDiff, threads: &[CommentThread], wrap_width: usize) -> Self {
        let mut rows = Vec::new();
        let mut hunk_starts = Vec::new();

        for hunk in &file.hunks {
            hunk_starts.push(rows.len());
            rows.push(Row {
                base: Cell::HunkHeader(hunk.header.clone()),
                head: Cell::HunkHeader(hunk.header.clone()),
                base_line: None,
                head_line: None,
                thread_id: None,
            });

            let (mut old_line, mut new_line) =
                parse_hunk_header(&hunk.header).unwrap_or((1, 1));

            let lines = &hunk.lines;
            let mut i = 0;
            while i < lines.len() {
                match &lines[i] {
                    DiffLine::Context(t) => {
                        rows.push(Row {
                            base: Cell::Context(t.clone()),
                            head: Cell::Context(t.clone()),
                            base_line: Some(old_line),
                            head_line: Some(new_line),
                            thread_id: None,
                        });
                        attach_threads(&mut rows, threads, Some(old_line), Some(new_line), wrap_width);
                        old_line += 1;
                        new_line += 1;
                        i += 1;
                    }
                    DiffLine::Moved(t) => {
                        rows.push(Row {
                            base: Cell::Moved(t.clone()),
                            head: Cell::Moved(t.clone()),
                            base_line: Some(old_line),
                            head_line: Some(new_line),
                            thread_id: None,
                        });
                        attach_threads(&mut rows, threads, Some(old_line), Some(new_line), wrap_width);
                        old_line += 1;
                        new_line += 1;
                        i += 1;
                    }
                    DiffLine::Removed(_) | DiffLine::Added(_) => {
                        let mut removed = Vec::new();
                        while let Some(DiffLine::Removed(t)) = lines.get(i) {
                            removed.push((t.clone(), old_line));
                            old_line += 1;
                            i += 1;
                        }
                        let mut added = Vec::new();
                        while let Some(DiffLine::Added(t)) = lines.get(i) {
                            added.push((t.clone(), new_line));
                            new_line += 1;
                            i += 1;
                        }
                        let n = removed.len().max(added.len());
                        for k in 0..n {
                            let (base, base_line) = removed
                                .get(k)
                                .map(|(t, l)| (Cell::Removed(t.clone()), Some(*l)))
                                .unwrap_or((Cell::Empty, None));
                            let (head, head_line) = added
                                .get(k)
                                .map(|(t, l)| (Cell::Added(t.clone()), Some(*l)))
                                .unwrap_or((Cell::Empty, None));
                            rows.push(Row {
                                base,
                                head,
                                base_line,
                                head_line,
                                thread_id: None,
                            });
                            attach_threads(&mut rows, threads, base_line, head_line, wrap_width);
                        }
                    }
                }
            }
        }

        Self { rows, hunk_starts }
    }
}

/// After pushing a content row, append any comment threads whose anchor matches
/// either side's line number on this row. Body text is word-wrapped to
/// `wrap_width` columns.
fn attach_threads(
    rows: &mut Vec<Row>,
    threads: &[CommentThread],
    base_line: Option<u32>,
    head_line: Option<u32>,
    wrap_width: usize,
) {
    for thread in threads {
        let matches = match thread.side {
            CommentSide::Base => base_line == Some(thread.line),
            CommentSide::Head => head_line == Some(thread.line),
        };
        if !matches {
            continue;
        }
        for (idx, comment) in thread.comments.iter().enumerate() {
            let header_text = format!("@{} {}", comment.author, comment.created_at);
            push_comment_row(
                rows,
                thread.side,
                Cell::CommentHeader {
                    text: header_text,
                    is_root: idx == 0,
                    is_pending: comment.is_pending,
                },
                &thread.id,
            );
            if comment.body.is_empty() {
                // Empty body still gets a `│` line so the thread structure is visible.
                push_comment_row(
                    rows,
                    thread.side,
                    Cell::CommentBody(String::new()),
                    &thread.id,
                );
            } else {
                for body_line in comment.body.lines() {
                    for chunk in wrap_line(body_line, wrap_width) {
                        push_comment_row(
                            rows,
                            thread.side,
                            Cell::CommentBody(chunk),
                            &thread.id,
                        );
                    }
                }
            }
        }
        push_comment_row(rows, thread.side, Cell::CommentEnd, &thread.id);
    }
}

/// Word-wrap a single line of text to `width` columns. Whitespace-delimited
/// words are kept together when possible; words longer than `width` are
/// hard-split at UTF-8 boundaries. Empty input returns a single empty line.
///
/// Width is measured in bytes/codepoints, not visual columns — wide characters
/// (CJK / emoji) will still over-flow. Acceptable for v1; revisit with
/// `unicode-width` if comments contain a lot of wide chars.
fn wrap_line(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if word.len() > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut consumed = 0;
            while consumed < word.len() {
                let remaining = &word[consumed..];
                let mut end = remaining.len().min(width);
                while end > 0 && !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                if end == 0 {
                    break;
                }
                out.push(remaining[..end].to_owned());
                consumed += end;
            }
            continue;
        }
        if current.is_empty() {
            current = word.to_owned();
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(std::mem::take(&mut current));
            current = word.to_owned();
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn push_comment_row(rows: &mut Vec<Row>, side: CommentSide, cell: Cell, thread_id: &str) {
    let (base, head) = match side {
        CommentSide::Base => (cell, Cell::Empty),
        CommentSide::Head => (Cell::Empty, cell),
    };
    rows.push(Row {
        base,
        head,
        base_line: None,
        head_line: None,
        thread_id: Some(thread_id.to_owned()),
    });
}

/// Parse `@@ -10,5 +12,7 @@` → `(10, 12)`.
fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let rest = header.strip_prefix("@@ -")?;
    let (old_part, after) = rest.split_once(' ')?;
    let new_part = after.strip_prefix('+')?.split(' ').next()?;
    let old_start: u32 = old_part.split(',').next()?.parse().ok()?;
    let new_start: u32 = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

pub fn render_pane(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    focused: bool,
    diff: Option<(&FileDiff, &LaidOutDiff)>,
    side: Side,
    scroll: u16,
    cursor: Option<usize>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.to_owned())
        .border_style(border_style(focused));

    let lines: Vec<Line> = match diff {
        None => vec![Line::styled(
            "(select a file to view its diff)",
            Style::default().fg(Color::DarkGray),
        )],
        Some((file, laid)) => {
            let syn = syntax::highlighter();
            let syntax_ref = syn.syntax_for(&file.path);
            laid.rows
                .iter()
                .enumerate()
                .map(|(idx, row)| {
                    let cell = match side {
                        Side::Base => &row.base,
                        Side::Head => &row.head,
                    };
                    let active = Some(idx) == cursor;
                    with_gutter(render_cell(cell, syntax_ref), active)
                })
                .collect()
        }
    };

    let para = Paragraph::new(lines).block(block).scroll((scroll, 0));
    frame.render_widget(para, area);
}

fn with_gutter(line: Line<'_>, active: bool) -> Line<'_> {
    let marker = if active {
        Span::styled(
            "\u{25B6}",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(" ")
    };
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(marker);
    spans.extend(line.spans);
    Line::from(spans)
}

#[derive(Clone, Copy)]
pub enum Side {
    Base,
    Head,
}

fn render_cell<'a>(cell: &'a Cell, syntax: &syntect::parsing::SyntaxReference) -> Line<'a> {
    let syn = syntax::highlighter();
    match cell {
        Cell::Empty => Line::raw(""),
        Cell::HunkHeader(h) => Line::from(vec![
            Span::raw(" "),
            Span::styled(
                strip_newline(h).to_owned(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Cell::Context(t) => {
            let segs = syn.highlight_line(syntax, strip_newline(t));
            let mut spans = vec![Span::styled("  ", Style::default())];
            spans.extend(syntax::to_spans(&segs, None));
            Line::from(spans)
        }
        Cell::Added(t) => row_with_marker("+ ", t, BG_ADDED, syntax),
        Cell::Removed(t) => row_with_marker("- ", t, BG_REMOVED, syntax),
        Cell::Moved(t) => row_with_marker("~ ", t, BG_MOVED, syntax),
        Cell::CommentHeader {
            text,
            is_root,
            is_pending,
        } => {
            let lead = if *is_root { "\u{250C} " } else { "\u{251C} " };
            let mut spans = vec![
                Span::styled(lead, Style::default().fg(Color::Yellow)),
                Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if *is_pending {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "(pending)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            Line::from(spans)
        }
        Cell::CommentBody(text) => Line::from(vec![
            Span::styled("\u{2502} ", Style::default().fg(Color::Yellow)),
            Span::styled(text.clone(), Style::default()),
        ]),
        Cell::CommentEnd => Line::from(Span::styled(
            "\u{2514}",
            Style::default().fg(Color::Yellow),
        )),
    }
}

fn row_with_marker<'a>(
    marker: &'static str,
    text: &'a str,
    bg: Color,
    syntax: &syntect::parsing::SyntaxReference,
) -> Line<'a> {
    let syn = syntax::highlighter();
    let segs = syn.highlight_line(syntax, strip_newline(text));
    let mut spans = vec![Span::styled(marker, Style::default().bg(bg))];
    spans.extend(syntax::to_spans(&segs, Some(bg)));
    Line::from(spans)
}

fn strip_newline(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}
