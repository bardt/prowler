use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::diff::FileDiff;
use crate::github::PrMetadata;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Files,
    Diff,
}

pub struct ReviewState {
    pub meta: PrMetadata,
    pub diffs: Vec<FileDiff>,
    pub list_state: ListState,
    pub focus: Focus,
}

impl ReviewState {
    pub fn new(meta: PrMetadata, diffs: Vec<FileDiff>) -> Self {
        let mut list_state = ListState::default();
        if !diffs.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            meta,
            diffs,
            list_state,
            focus: Focus::Files,
        }
    }

    pub fn next_file(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.diffs.len() - 1);
        self.list_state.select(Some(next));
    }

    pub fn prev_file(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let prev = i.saturating_sub(1);
        self.list_state.select(Some(prev));
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Files => Focus::Diff,
            Focus::Diff => Focus::Files,
        };
    }

    fn selected(&self) -> Option<&FileDiff> {
        self.list_state.selected().and_then(|i| self.diffs.get(i))
    }

    fn totals(&self) -> (usize, usize) {
        self.diffs
            .iter()
            .fold((0, 0), |(a, r), d| (a + d.added, r + d.removed))
    }
}

pub fn render(frame: &mut Frame, state: &mut ReviewState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_header(frame, outer[0], state);
    render_body(frame, outer[1], state);
    render_hotkeys(frame, outer[2], state);
}

fn render_header(frame: &mut Frame, area: Rect, state: &ReviewState) {
    let (added, removed) = state.totals();
    let title = Line::from(vec![
        Span::styled(
            format!("#{}: ", state.meta.pr_number()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(state.meta.title.clone()),
        Span::raw("   "),
        Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(Color::Red)),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn render_body(frame: &mut Frame, area: Rect, state: &mut ReviewState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(40), Constraint::Min(20)])
        .split(area);

    render_files(frame, cols[0], state);
    render_diff_placeholder(frame, cols[1], state);
}

fn render_files(frame: &mut Frame, area: Rect, state: &mut ReviewState) {
    let items: Vec<ListItem> = state
        .diffs
        .iter()
        .map(|d| {
            ListItem::new(Line::from(vec![
                Span::raw(d.path.clone()),
                Span::raw("  "),
                Span::styled(format!("+{}", d.added), Style::default().fg(Color::Green)),
                Span::raw(" "),
                Span::styled(format!("-{}", d.removed), Style::default().fg(Color::Red)),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("FILES [1]  {} files", state.diffs.len()))
        .border_style(border_style(state.focus == Focus::Files));

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    frame.render_stateful_widget(list, area, &mut state.list_state);
}

fn render_diff_placeholder(frame: &mut Frame, area: Rect, state: &ReviewState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("DIFF [2]")
        .border_style(border_style(state.focus == Focus::Diff));

    let body = match state.selected() {
        None => Paragraph::new("No files in this PR.").block(block),
        Some(d) => {
            let text = vec![
                Line::from(vec![Span::styled(
                    d.path.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )]),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(format!("+{}", d.added), Style::default().fg(Color::Green)),
                    Span::raw(" / "),
                    Span::styled(format!("-{}", d.removed), Style::default().fg(Color::Red)),
                    Span::raw(format!("   {} hunks", d.hunks.len())),
                ]),
                Line::raw(""),
                Line::styled(
                    "(diff rendering arrives in M5)",
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            Paragraph::new(text).block(block).wrap(Wrap { trim: false })
        }
    };

    frame.render_widget(body, area);
}

fn render_hotkeys(frame: &mut Frame, area: Rect, _state: &ReviewState) {
    let hotkeys = Line::from(vec![
        key("j/k"),
        Span::raw(" navigate  "),
        key("Tab"),
        Span::raw(" switch panel  "),
        key("q"),
        Span::raw(" quit"),
    ]);
    frame.render_widget(
        Paragraph::new(hotkeys).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn key(label: &str) -> Span<'_> {
    Span::styled(
        format!("[{label}]"),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}
