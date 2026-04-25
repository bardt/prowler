use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use std::path::{Path, PathBuf};

use crate::diff::FileDiff;
use crate::github::{CommentThread, PrMetadata};
use crate::session::{FileStatus, Session};
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
    /// Viewport scroll offset (in rows) per file.
    scroll: Vec<u16>,
    /// Cursor row index per file. The marker shown in the diff panes.
    cursor: Vec<u16>,
    last_pane_height: u16,
    session: Session,
    repo_root: PathBuf,
    token: String,
}

pub struct EditorTarget {
    pub file: PathBuf,
    pub line: u32,
}

impl ReviewState {
    pub fn new(
        meta: PrMetadata,
        diffs: Vec<FileDiff>,
        threads: Vec<CommentThread>,
        session: Session,
        repo_root: PathBuf,
        token: String,
    ) -> Self {
        let mut threads_by_file: Vec<Vec<CommentThread>> = vec![Vec::new(); diffs.len()];
        for thread in threads {
            if let Some(idx) = diffs.iter().position(|d| d.path == thread.path) {
                threads_by_file[idx].push(thread);
            }
        }
        let laid: Vec<LaidOutDiff> = diffs
            .iter()
            .zip(&threads_by_file)
            .map(|(d, t)| LaidOutDiff::from_file(d, t))
            .collect();
        let scroll = vec![0; diffs.len()];
        let cursor = vec![0; diffs.len()];
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
            cursor,
            last_pane_height: 20,
            session,
            repo_root,
            token,
        }
    }

    fn file_status(&self, path: &str) -> FileStatus {
        self.session
            .files
            .get(path)
            .copied()
            .unwrap_or(FileStatus::Unviewed)
    }

    fn set_status(&mut self, path: String, status: FileStatus) {
        let was_viewed = self.file_status(&path) == FileStatus::Viewed;
        let now_viewed = status == FileStatus::Viewed;

        if status == FileStatus::Unviewed {
            self.session.files.remove(&path);
        } else {
            self.session.files.insert(path.clone(), status);
        }
        if let Err(e) = self.session.save(&self.repo_root) {
            eprintln!("warning: failed to save session: {e:#}");
        }

        if was_viewed != now_viewed {
            self.spawn_set_viewed(path, now_viewed);
        }
    }

    fn spawn_set_viewed(&self, path: String, viewed: bool) {
        let token = self.token.clone();
        let node_id = self.meta.node_id.clone();
        let pr = self.session.pr_number;
        tokio::spawn(async move {
            let result = crate::github::set_viewed(&token, &node_id, &path, viewed).await;
            let line = match result {
                Ok(()) => format!("[ok]   PR #{pr} {path} viewed={viewed}\n"),
                Err(e) => format!("[FAIL] PR #{pr} {path} viewed={viewed}: {e:#}\n"),
            };
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/prowler-sync.log")
            {
                use std::io::Write;
                let _ = f.write_all(line.as_bytes());
            }
        });
    }

    pub fn toggle_viewed(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let path = self.diffs[i].path.clone();
        let next = match self.file_status(&path) {
            FileStatus::Viewed => FileStatus::Unviewed,
            _ => FileStatus::Viewed,
        };
        self.set_status(path, next);
    }

    pub fn toggle_skipped(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let path = self.diffs[i].path.clone();
        let next = match self.file_status(&path) {
            FileStatus::Skipped => FileStatus::Unviewed,
            _ => FileStatus::Skipped,
        };
        self.set_status(path, next);
    }

    pub fn editor_target(&self, side: Side) -> Option<EditorTarget> {
        let i = self.selected_idx()?;
        let file = &self.diffs[i];
        let laid = &self.laid[i];
        let cur = self.cursor[i] as usize;

        let pick = |row: &crate::tui::diff_view::Row| match side {
            Side::Base => row.base_line,
            Side::Head => row.head_line,
        };

        let line = laid.rows[cur..]
            .iter()
            .find_map(pick)
            .or_else(|| laid.rows[..cur].iter().rev().find_map(pick))
            .unwrap_or(1);

        let rel_path: &Path = match side {
            Side::Base => Path::new(file.previous_path.as_deref().unwrap_or(&file.path)),
            Side::Head => Path::new(&file.path),
        };
        let root = match side {
            Side::Base => &self.session.base_worktree_path,
            Side::Head => &self.session.worktree_path,
        };

        Some(EditorTarget {
            file: root.join(rel_path),
            line,
        })
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
            Focus::Base | Focus::Head => self.move_cursor(1),
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            Focus::Files => self.prev_file(),
            Focus::Base | Focus::Head => self.move_cursor(-1),
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

    fn move_cursor(&mut self, delta: i32) {
        let Some(i) = self.selected_idx() else { return };
        let last = self.laid[i].rows.len().saturating_sub(1) as i32;
        let cur = self.cursor[i] as i32;
        let next = (cur + delta).clamp(0, last.max(0)) as u16;
        self.cursor[i] = next;
        self.ensure_cursor_visible(i);
    }

    fn ensure_cursor_visible(&mut self, i: usize) {
        let visible = self.last_pane_height.saturating_sub(2); // borders
        if visible == 0 {
            return;
        }
        let cursor = self.cursor[i];
        let mut scroll = self.scroll[i];
        if cursor < scroll {
            scroll = cursor;
        } else if cursor >= scroll + visible {
            scroll = cursor + 1 - visible;
        }
        let max = self.max_scroll(i);
        self.scroll[i] = scroll.min(max);
    }

    fn max_scroll(&self, i: usize) -> u16 {
        let rows = self.laid[i].rows.len() as u16;
        let visible = self.last_pane_height.saturating_sub(2); // borders
        rows.saturating_sub(visible)
    }

    pub fn next_hunk(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let cur = self.cursor[i];
        if let Some(&next) = self.laid[i]
            .hunk_starts
            .iter()
            .find(|&&s| (s as u16) > cur)
        {
            self.cursor[i] = next as u16;
            self.ensure_cursor_visible(i);
        }
    }

    pub fn prev_hunk(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let cur = self.cursor[i];
        if let Some(&prev) = self.laid[i]
            .hunk_starts
            .iter()
            .rev()
            .find(|&&s| (s as u16) < cur)
        {
            self.cursor[i] = prev as u16;
            self.ensure_cursor_visible(i);
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
    let cursor = i.map(|i| state.cursor[i] as usize);

    render_pane(
        frame,
        cols[1],
        "BASE [2]",
        state.focus == Focus::Base,
        pair,
        Side::Base,
        scroll,
        cursor,
    );
    render_pane(
        frame,
        cols[2],
        "HEAD [3]",
        state.focus == Focus::Head,
        pair,
        Side::Head,
        scroll,
        cursor,
    );
}

fn render_files(frame: &mut Frame, area: Rect, state: &mut ReviewState) {
    let items: Vec<ListItem> = state
        .diffs
        .iter()
        .map(|d| {
            let status = state.file_status(&d.path);
            let (marker, marker_style) = match status {
                FileStatus::Unviewed => (" ", Style::default()),
                FileStatus::InProgress => ("*", Style::default().fg(Color::Cyan)),
                FileStatus::Viewed => ("\u{2713}", Style::default().fg(Color::Green)),
                FileStatus::Skipped => ("~", Style::default().fg(Color::Yellow)),
            };
            let path_style = if status == FileStatus::Viewed {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::raw(" "),
                Span::styled(d.path.clone(), path_style),
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
        Span::raw(" move  "),
        key("]/["),
        Span::raw(" hunk  "),
        key("Tab"),
        Span::raw(" panel  "),
        key("e/E"),
        Span::raw(" edit  "),
        key("v/s"),
        Span::raw(" view/skip  "),
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
