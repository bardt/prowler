use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use std::collections::HashSet;

use crate::diff::{DiffLine, FileDiff};
use crate::github::{CommentSide, CommentThread};
use crate::tui::syntax;

// Diff rows distinguish themselves via the leading marker color (green `+`,
// red `-`, blue `~`) instead of a row-wide background tint, so the user's
// terminal palette stays in charge. Suggestion blocks use REVERSED on the
// marker for emphasis (terminal-theme-friendly).

#[derive(Clone)]
pub enum Cell {
    Empty,
    HunkHeader { text: String, is_synthetic: bool },
    Context(String),
    Added(String),
    Removed(String),
    Moved(String),
    /// Header line of a comment within a thread: `┌ @author 2026-04-20 14:30`
    /// (`├` for replies, with optional dim `(pending)` / `(outdated)` /
    /// `(resolved)` suffixes on the root row only).
    CommentHeader {
        text: String,
        is_root: bool,
        is_pending: bool,
        is_outdated: bool,
        is_resolved: bool,
    },
    /// Body line of a comment: `│ {text}`. `in_suggestion` flags lines that
    /// are inside a ` ```suggestion ` … ` ``` ` fence (so the renderer can
    /// highlight them as a green code block, and `a` knows what to apply).
    CommentBody {
        text: String,
        in_suggestion: bool,
    },
    /// Thread terminator: `└`.
    CommentEnd,
    /// Single-row summary of a collapsed thread: `▸ 3 comments • @alice: preview`.
    CollapsedThread {
        text: String,
        has_pending: bool,
        is_outdated: bool,
        is_resolved: bool,
    },
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
    /// Set when this row belongs to a specific comment (header or body lines).
    /// Used by `M`/`X` (edit/delete) to identify the comment under the cursor.
    pub comment_id: Option<String>,
}

pub struct LaidOutDiff {
    pub rows: Vec<Row>,
    pub hunk_starts: Vec<usize>,
}

impl LaidOutDiff {
    pub fn from_file(
        file: &FileDiff,
        threads: &[CommentThread],
        wrap_width: usize,
        expanded: &HashSet<String>,
        hide_resolved: bool,
    ) -> Self {
        let mut rows = Vec::new();
        let mut hunk_starts = Vec::new();

        for hunk in &file.hunks {
            hunk_starts.push(rows.len());
            rows.push(Row {
                base: Cell::HunkHeader {
                    text: hunk.header.clone(),
                    is_synthetic: hunk.is_synthetic,
                },
                head: Cell::HunkHeader {
                    text: hunk.header.clone(),
                    is_synthetic: hunk.is_synthetic,
                },
                base_line: None,
                head_line: None,
                thread_id: None,
                comment_id: None,
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
                            comment_id: None,
                        });
                        attach_threads(&mut rows, threads, Some(old_line), Some(new_line), wrap_width, expanded, hide_resolved);
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
                            comment_id: None,
                        });
                        attach_threads(&mut rows, threads, Some(old_line), Some(new_line), wrap_width, expanded, hide_resolved);
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
                                comment_id: None,
                            });
                            attach_threads(&mut rows, threads, base_line, head_line, wrap_width, expanded, hide_resolved);
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
    expanded: &HashSet<String>,
    hide_resolved: bool,
) {
    for thread in threads {
        if hide_resolved && thread.is_resolved {
            continue;
        }
        let matches = match thread.side {
            CommentSide::Base => base_line == Some(thread.line),
            CommentSide::Head => head_line == Some(thread.line),
        };
        if !matches {
            continue;
        }
        if !expanded.contains(&thread.id) {
            push_comment_row(
                rows,
                thread.side,
                Cell::CollapsedThread {
                    text: collapsed_summary(thread, wrap_width),
                    has_pending: thread.comments.iter().any(|c| c.is_pending),
                    is_outdated: thread.is_outdated,
                    is_resolved: thread.is_resolved,
                },
                &thread.id,
                None,
            );
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
                    // Only show (outdated) / (resolved) on the root header —
                    // replies belong to the same thread so it'd be redundant.
                    is_outdated: thread.is_outdated && idx == 0,
                    is_resolved: thread.is_resolved && idx == 0,
                },
                &thread.id,
                Some(&comment.id),
            );
            if comment.body.is_empty() {
                // Empty body still gets a `│` line so the thread structure is visible.
                push_comment_row(
                    rows,
                    thread.side,
                    Cell::CommentBody { text: String::new(), in_suggestion: false },
                    &thread.id,
                    Some(&comment.id),
                );
            } else {
                let mut in_suggestion = false;
                for body_line in comment.body.lines() {
                    let trimmed = body_line.trim_start();
                    let is_fence = trimmed.starts_with("```");
                    let is_suggest_open = is_fence
                        && trimmed.trim_end_matches(|c: char| c.is_whitespace())
                            .strip_prefix("```")
                            .map(|s| s.trim() == "suggestion")
                            .unwrap_or(false);
                    let mark_this = in_suggestion || is_suggest_open;
                    for chunk in wrap_line(body_line, wrap_width) {
                        push_comment_row(
                            rows,
                            thread.side,
                            Cell::CommentBody {
                                text: chunk,
                                in_suggestion: mark_this,
                            },
                            &thread.id,
                            Some(&comment.id),
                        );
                    }
                    if is_suggest_open {
                        in_suggestion = true;
                    } else if is_fence && in_suggestion {
                        in_suggestion = false;
                    }
                }
            }
        }
        push_comment_row(rows, thread.side, Cell::CommentEnd, &thread.id, None);
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

fn collapsed_summary(thread: &CommentThread, wrap_width: usize) -> String {
    let count = thread.comments.len();
    let preview_source = thread
        .comments
        .first()
        .map(|c| c.body.lines().next().unwrap_or(""))
        .unwrap_or("");
    let preview = take_chars(preview_source, wrap_width.saturating_sub(40));
    let author = thread
        .comments
        .first()
        .map(|c| c.author.as_str())
        .unwrap_or("?");
    if count == 1 {
        if preview.is_empty() {
            format!("@{author}")
        } else {
            format!("@{author}: {preview}")
        }
    } else if preview.is_empty() {
        format!("{count} comments by @{author}")
    } else {
        format!("{count} comments \u{2022} @{author}: {preview}")
    }
}

fn take_chars(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('\u{2026}');
    }
    out
}

fn push_comment_row(
    rows: &mut Vec<Row>,
    side: CommentSide,
    cell: Cell,
    thread_id: &str,
    comment_id: Option<&str>,
) {
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
        comment_id: comment_id.map(|s| s.to_owned()),
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
    selection: Option<(usize, usize)>,
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
                    let in_selection = selection
                        .map(|(lo, hi)| idx >= lo && idx <= hi)
                        .unwrap_or(false);
                    let line = with_gutter(render_cell(cell, syntax_ref), active);
                    if in_selection {
                        // Use REVERSED so the selection respects the user's
                        // terminal palette (light or dark) instead of a fixed
                        // RGB tint.
                        let mut spans = line.spans;
                        for span in &mut spans {
                            span.style = span.style.add_modifier(Modifier::REVERSED);
                        }
                        Line::from(spans)
                    } else {
                        line
                    }
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
        Cell::HunkHeader { text: h, is_synthetic } => {
            let color = if *is_synthetic { Color::DarkGray } else { Color::Magenta };
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    strip_newline(h).to_owned(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ])
        }
        Cell::Context(t) => {
            let segs = syn.highlight_line(syntax, strip_newline(t));
            let mut spans = vec![Span::styled("  ", Style::default())];
            spans.extend(syntax::to_spans(&segs, None));
            Line::from(spans)
        }
        Cell::Added(t) => row_with_marker("+ ", t, Color::Green, syntax),
        Cell::Removed(t) => row_with_marker("- ", t, Color::Red, syntax),
        Cell::Moved(t) => row_with_marker("~ ", t, Color::Blue, syntax),
        Cell::CommentHeader {
            text,
            is_root,
            is_pending,
            is_outdated,
            is_resolved,
        } => {
            let lead = if *is_root { "\u{250C} " } else { "\u{251C} " };
            let header_color = if *is_resolved { Color::DarkGray } else { Color::Yellow };
            let mut spans = vec![
                Span::styled(lead, Style::default().fg(header_color)),
                Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(header_color)
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
            if *is_resolved {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "(resolved)",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            if *is_outdated {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "(outdated)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            Line::from(spans)
        }
        Cell::CommentBody { text, in_suggestion } => {
            if *in_suggestion {
                // Use REVERSED so the highlight follows the user's terminal
                // theme (light or dark) instead of a fixed RGB tint.
                Line::from(vec![
                    Span::styled("\u{2502} ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        text.clone(),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled("\u{2502} ", Style::default().fg(Color::Yellow)),
                    Span::styled(text.clone(), Style::default()),
                ])
            }
        }
        Cell::CommentEnd => Line::from(Span::styled(
            "\u{2514}",
            Style::default().fg(Color::Yellow),
        )),
        Cell::CollapsedThread {
            text,
            has_pending,
            is_outdated,
            is_resolved,
        } => {
            let header_color = if *is_resolved { Color::DarkGray } else { Color::Yellow };
            let mut spans = vec![
                Span::styled("\u{25B8} ", Style::default().fg(header_color)),
                Span::styled(
                    text.clone(),
                    Style::default()
                        .fg(header_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if *has_pending {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "(pending)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            if *is_resolved {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "(resolved)",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            if *is_outdated {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "(outdated)",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            Line::from(spans)
        }
    }
}

fn row_with_marker<'a>(
    marker: &'static str,
    text: &'a str,
    marker_fg: Color,
    syntax: &syntect::parsing::SyntaxReference,
) -> Line<'a> {
    let syn = syntax::highlighter();
    let segs = syn.highlight_line(syntax, strip_newline(text));
    let mut spans = vec![Span::styled(
        marker,
        Style::default()
            .fg(marker_fg)
            .add_modifier(Modifier::BOLD),
    )];
    spans.extend(syntax::to_spans(&segs, None));
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
