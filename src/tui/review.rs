use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::diff::FileDiff;
use crate::github::PrMetadata;
use crate::tui::diff_view::{LaidOutDiff, Side, render_pane};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Files,
    Base,
    Head,
}

pub struct ReviewState {
    pub meta: PrMetadata,
    pub diffs: Vec<FileDiff>,
    laid: Vec<LaidOutDiff>,
    pub list_state: ListState,
    pub focus: Focus,
    /// Scroll offset (in rows) for the diff panes, per file.
    scroll: Vec<u16>,
    last_pane_height: u16,
}

impl ReviewState {
    pub fn new(meta: PrMetadata, diffs: Vec<FileDiff>) -> Self {
        let laid = diffs.iter().map(LaidOutDiff::from_file).collect();
        let scroll = vec![0; diffs.len()];
        let mut list_state = ListState::default();
        if !diffs.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            meta,
            diffs,
            laid,
            list_state,
            focus: Focus::Files,
            scroll,
            last_pane_height: 20,
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Files => Focus::Base,
            Focus::Base => Focus::Head,
            Focus::Head => Focus::Files,
        };
    }

    pub fn move_down(&mut self) {
        match self.focus {
            Focus::Files => self.next_file(),
            Focus::Base | Focus::Head => self.scroll_by(1),
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            Focus::Files => self.prev_file(),
            Focus::Base | Focus::Head => self.scroll_by(-1),
        }
    }

    fn next_file(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.diffs.len() - 1);
        self.list_state.select(Some(next));
    }

    fn prev_file(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
    }

    fn selected_idx(&self) -> Option<usize> {
        self.list_state.selected().filter(|i| *i < self.diffs.len())
    }

    fn scroll_by(&mut self, delta: i32) {
        let Some(i) = self.selected_idx() else { return };
        let max = self.max_scroll(i);
        let cur = self.scroll[i] as i32;
        let next = (cur + delta).clamp(0, max as i32);
        self.scroll[i] = next as u16;
    }

    fn max_scroll(&self, i: usize) -> u16 {
        let rows = self.laid[i].rows.len() as u16;
        let visible = self.last_pane_height.saturating_sub(2); // borders
        rows.saturating_sub(visible)
    }

    pub fn next_hunk(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let cur = self.scroll[i];
        if let Some(&next) = self.laid[i]
            .hunk_starts
            .iter()
            .find(|&&s| (s as u16) > cur)
        {
            self.scroll[i] = (next as u16).min(self.max_scroll(i));
        }
    }

    pub fn prev_hunk(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let cur = self.scroll[i];
        if let Some(&prev) = self.laid[i]
            .hunk_starts
            .iter()
            .rev()
            .find(|&&s| (s as u16) < cur)
        {
            self.scroll[i] = prev as u16;
        }
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
    render_hotkeys(frame, outer[2]);
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
        .constraints([
            Constraint::Length(36),
            Constraint::Percentage(50),
            Constraint::Min(20),
        ])
        .split(area);

    state.last_pane_height = cols[1].height;

    render_files(frame, cols[0], state);

    let i = state.selected_idx();
    let pair = i.map(|i| (&state.diffs[i], &state.laid[i]));
    let scroll = i.map(|i| state.scroll[i]).unwrap_or(0);

    render_pane(
        frame,
        cols[1],
        "BASE [2]",
        state.focus == Focus::Base,
        pair,
        Side::Base,
        scroll,
    );
    render_pane(
        frame,
        cols[2],
        "HEAD [3]",
        state.focus == Focus::Head,
        pair,
        Side::Head,
        scroll,
    );
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
        .border_style(if state.focus == Focus::Files {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        });

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

fn render_hotkeys(frame: &mut Frame, area: Rect) {
    let hotkeys = Line::from(vec![
        key("j/k"),
        Span::raw(" scroll  "),
        key("]/["),
        Span::raw(" hunk  "),
        key("Tab"),
        Span::raw(" panel  "),
        key("1/2/3"),
        Span::raw(" jump  "),
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
