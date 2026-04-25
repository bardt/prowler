use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::diff::{DiffLine, FileDiff};
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
}

#[derive(Clone)]
pub struct Row {
    pub base: Cell,
    pub head: Cell,
}

pub struct LaidOutDiff {
    pub rows: Vec<Row>,
    pub hunk_starts: Vec<usize>,
}

impl LaidOutDiff {
    pub fn from_file(file: &FileDiff) -> Self {
        let mut rows = Vec::new();
        let mut hunk_starts = Vec::new();

        for hunk in &file.hunks {
            hunk_starts.push(rows.len());
            rows.push(Row {
                base: Cell::HunkHeader(hunk.header.clone()),
                head: Cell::HunkHeader(hunk.header.clone()),
            });

            let lines = &hunk.lines;
            let mut i = 0;
            while i < lines.len() {
                match &lines[i] {
                    DiffLine::Context(t) => {
                        rows.push(Row {
                            base: Cell::Context(t.clone()),
                            head: Cell::Context(t.clone()),
                        });
                        i += 1;
                    }
                    DiffLine::Moved(t) => {
                        rows.push(Row {
                            base: Cell::Moved(t.clone()),
                            head: Cell::Moved(t.clone()),
                        });
                        i += 1;
                    }
                    DiffLine::Removed(_) | DiffLine::Added(_) => {
                        let mut removed = Vec::new();
                        while let Some(DiffLine::Removed(t)) = lines.get(i) {
                            removed.push(t.clone());
                            i += 1;
                        }
                        let mut added = Vec::new();
                        while let Some(DiffLine::Added(t)) = lines.get(i) {
                            added.push(t.clone());
                            i += 1;
                        }
                        let n = removed.len().max(added.len());
                        for k in 0..n {
                            let base = removed
                                .get(k)
                                .map(|t| Cell::Removed(t.clone()))
                                .unwrap_or(Cell::Empty);
                            let head = added
                                .get(k)
                                .map(|t| Cell::Added(t.clone()))
                                .unwrap_or(Cell::Empty);
                            rows.push(Row { base, head });
                        }
                    }
                }
            }
        }

        Self { rows, hunk_starts }
    }
}

pub fn render_pane(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    focused: bool,
    diff: Option<(&FileDiff, &LaidOutDiff)>,
    side: Side,
    scroll: u16,
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
                .map(|row| {
                    let cell = match side {
                        Side::Base => &row.base,
                        Side::Head => &row.head,
                    };
                    render_cell(cell, syntax_ref)
                })
                .collect()
        }
    };

    let para = Paragraph::new(lines).block(block).scroll((scroll, 0));
    frame.render_widget(para, area);
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
