use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::github::{DashboardData, DashboardPr};
use crate::session::Session;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Success,
    Error,
    Info,
}

struct Status {
    text: String,
    kind: StatusKind,
    expires_at: Instant,
}

const STATUS_TTL: Duration = Duration::from_secs(3);

/// One row in the dashboard list — either a section header or a PR/session entry.
pub enum Row {
    Header(String),
    Pr {
        pr: DashboardPr,
    },
    Session {
        pr_number: u64,
        worktree_path: PathBuf,
        head_sha: String,
        viewed: usize,
        total: usize,
    },
    Empty(String),
}

pub enum DashboardOutcome {
    Quit,
    Open(u64),
    /// Open a PR that isn't in the GitHub-fetched lists (came from the local
    /// "Sessions" section).
    OpenLocal(u64),
    Refresh,
    Cleanup(u64),
}

pub struct DashboardState {
    pub owner: String,
    pub repo: String,
    pub repo_root: PathBuf,
    data: DashboardData,
    sessions: Vec<Session>,
    rows: Vec<Row>,
    list_state: ListState,
    status: Option<Status>,
    last_refreshed: Instant,
}

impl DashboardState {
    pub fn new(
        data: DashboardData,
        owner: String,
        repo: String,
        repo_root: &Path,
    ) -> anyhow::Result<Self> {
        let sessions = load_local_sessions(repo_root)?;
        let mut state = DashboardState {
            owner,
            repo,
            repo_root: repo_root.to_path_buf(),
            data,
            sessions,
            rows: Vec::new(),
            list_state: ListState::default(),
            status: None,
            last_refreshed: Instant::now(),
        };
        state.rebuild_rows();
        if state.first_pr_index().is_some() {
            state.list_state.select(state.first_pr_index());
        }
        Ok(state)
    }

    pub fn set_data(&mut self, data: DashboardData) {
        self.data = data;
        self.last_refreshed = Instant::now();
        self.rebuild_rows();
        self.clamp_selection();
    }

    pub fn reload_sessions(&mut self) -> anyhow::Result<()> {
        self.sessions = load_local_sessions(&self.repo_root)?;
        self.rebuild_rows();
        self.clamp_selection();
        Ok(())
    }

    pub fn set_status(&mut self, text: impl Into<String>, kind: StatusKind) {
        self.status = Some(Status {
            text: text.into(),
            kind,
            expires_at: Instant::now() + STATUS_TTL,
        });
    }

    pub fn set_error(&mut self, text: impl Into<String>) {
        self.set_status(text, StatusKind::Error);
    }

    pub fn set_success(&mut self, text: impl Into<String>) {
        self.set_status(text, StatusKind::Success);
    }

    pub fn set_info(&mut self, text: impl Into<String>) {
        self.set_status(text, StatusKind::Info);
    }

    fn clamp_selection(&mut self) {
        if self.rows.is_empty() {
            self.list_state.select(None);
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let cur = cur.min(self.rows.len() - 1);
        // If we landed on a header, scoot to the next selectable.
        let target = (cur..self.rows.len())
            .find(|&i| matches!(self.rows[i], Row::Pr { .. } | Row::Session { .. }))
            .or_else(|| {
                (0..self.rows.len())
                    .find(|&i| matches!(self.rows[i], Row::Pr { .. } | Row::Session { .. }))
            });
        self.list_state.select(target);
    }

    fn first_pr_index(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Pr { .. } | Row::Session { .. }))
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();

        let push_section = |rows: &mut Vec<Row>, header: &str, list: &[DashboardPr]| {
            rows.push(Row::Header(format!("{header} ({})", list.len())));
            if list.is_empty() {
                rows.push(Row::Empty("    (none)".into()));
            } else {
                for pr in list {
                    rows.push(Row::Pr { pr: pr.clone() });
                }
            }
        };

        push_section(&mut rows, "Awaiting your review", &self.data.review_requested);
        rows.push(Row::Empty(String::new()));
        push_section(&mut rows, "Opened by you", &self.data.authored);
        rows.push(Row::Empty(String::new()));
        push_section(&mut rows, "Assigned to you", &self.data.assigned);
        rows.push(Row::Empty(String::new()));

        rows.push(Row::Header(format!("Local sessions ({})", self.sessions.len())));
        if self.sessions.is_empty() {
            rows.push(Row::Empty("    (none)".into()));
        } else {
            for s in &self.sessions {
                let total = s.files.len();
                let viewed = s
                    .files
                    .values()
                    .filter(|st| {
                        matches!(
                            st,
                            crate::session::FileStatus::Viewed
                                | crate::session::FileStatus::Skipped
                        )
                    })
                    .count();
                rows.push(Row::Session {
                    pr_number: s.pr_number,
                    worktree_path: s.worktree_path.clone(),
                    head_sha: s.head_sha.clone(),
                    viewed,
                    total,
                });
            }
        }

        self.rows = rows;
    }

    fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as i32;
        let mut idx = self.list_state.selected().unwrap_or(0) as i32;
        for _ in 0..len {
            idx = (idx + delta).rem_euclid(len);
            if matches!(
                self.rows[idx as usize],
                Row::Pr { .. } | Row::Session { .. }
            ) {
                self.list_state.select(Some(idx as usize));
                return;
            }
        }
    }

    fn current_outcome(&self) -> Option<DashboardOutcome> {
        match self.rows.get(self.list_state.selected()?)? {
            Row::Pr { pr } => Some(DashboardOutcome::Open(pr.number)),
            Row::Session { pr_number, .. } => Some(DashboardOutcome::OpenLocal(*pr_number)),
            _ => None,
        }
    }

    fn current_session_pr(&self) -> Option<u64> {
        match self.rows.get(self.list_state.selected()?)? {
            Row::Session { pr_number, .. } => Some(*pr_number),
            _ => None,
        }
    }

    /// Pure key dispatch. Returns Some(outcome) when the loop should react.
    pub fn apply_key(&mut self, key: KeyCode) -> Option<DashboardOutcome> {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => Some(DashboardOutcome::Quit),
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                None
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.list_state.select(self.first_pr_index());
                None
            }
            KeyCode::Char('G') | KeyCode::End => {
                let idx = self
                    .rows
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, r)| matches!(r, Row::Pr { .. } | Row::Session { .. }))
                    .map(|(i, _)| i);
                if idx.is_some() {
                    self.list_state.select(idx);
                }
                None
            }
            KeyCode::Enter => self.current_outcome(),
            KeyCode::Char('r') | KeyCode::F(5) => Some(DashboardOutcome::Refresh),
            KeyCode::Char('d') => self.current_session_pr().map(DashboardOutcome::Cleanup),
            _ => None,
        }
    }
}

fn load_local_sessions(repo_root: &Path) -> anyhow::Result<Vec<Session>> {
    let sessions_dir = repo_root.join(".review").join("sessions");
    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pr_number) = name.parse::<u64>() else { continue };
        if let Some(s) = Session::load(repo_root, pr_number)? {
            out.push(s);
        }
    }
    out.sort_by_key(|s| s.pr_number);
    Ok(out)
}

pub fn render(frame: &mut Frame, state: &mut DashboardState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, state, chunks[0]);
    render_list(frame, state, chunks[1]);
    render_footer(frame, state, chunks[2]);
}

fn render_header(frame: &mut Frame, state: &DashboardState, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            " prowler ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{}/{}", state.owner, state.repo),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            "Dashboard",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let p = Paragraph::new(vec![title, Line::from("")]);
    frame.render_widget(p, area);
}

fn render_list(frame: &mut Frame, state: &mut DashboardState, area: Rect) {
    let inner = Block::default()
        .borders(Borders::ALL)
        .title(" PRs ");
    let inner_area = inner.inner(area);
    frame.render_widget(inner, area);

    let width = inner_area.width as usize;
    let items: Vec<ListItem> = state
        .rows
        .iter()
        .map(|row| row_to_item(row, width))
        .collect();
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_stateful_widget(list, inner_area, &mut state.list_state);
}

fn row_to_item(row: &Row, width: usize) -> ListItem<'static> {
    match row {
        Row::Header(text) => ListItem::new(Line::from(Span::styled(
            text.clone(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))),
        Row::Empty(text) => ListItem::new(Line::from(Span::styled(
            text.clone(),
            Style::default().fg(Color::DarkGray),
        ))),
        Row::Pr { pr } => ListItem::new(format_pr_row(pr, width)),
        Row::Session {
            pr_number,
            worktree_path,
            head_sha,
            viewed,
            total,
        } => ListItem::new(format_session_row(
            *pr_number,
            worktree_path,
            head_sha,
            *viewed,
            *total,
            width,
        )),
    }
}

fn format_pr_row(pr: &DashboardPr, width: usize) -> Line<'static> {
    let badge = if pr.is_draft {
        Span::styled(
            " DRAFT ",
            Style::default().fg(Color::Black).bg(Color::Gray),
        )
    } else {
        match pr.review_decision.as_str() {
            "APPROVED" => Span::styled(
                " APPROVED ",
                Style::default().fg(Color::Black).bg(Color::Green),
            ),
            "CHANGES_REQUESTED" => Span::styled(
                " CHANGES ",
                Style::default().fg(Color::Black).bg(Color::Red),
            ),
            "REVIEW_REQUIRED" => Span::styled(
                " REVIEW ",
                Style::default().fg(Color::Black).bg(Color::Cyan),
            ),
            _ => Span::styled(" OPEN ", Style::default().fg(Color::Black).bg(Color::Green)),
        }
    };

    let number = Span::styled(
        format!("  #{:<5}", pr.number),
        Style::default().fg(Color::Cyan),
    );
    let lines = Span::styled(
        format!("+{} -{}", pr.additions, pr.deletions),
        Style::default().fg(Color::DarkGray),
    );
    let comments = if pr.comment_count > 0 {
        Span::styled(
            format!(" \u{1F4AC} {}", pr.comment_count),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::raw("")
    };
    let author = Span::styled(
        format!(" @{}", pr.author),
        Style::default().fg(Color::Magenta),
    );
    let updated = Span::styled(
        format!(" {}", pr.updated_at),
        Style::default().fg(Color::DarkGray),
    );

    // Title is truncated to fit; the badge / numbers / metadata take ~50 cols on a
    // typical row.
    let prefix_cols: usize = 1 + 7 + 1 + 7 + 1; // marker + badge + space + #NNNN + space
    let suffix_cols: usize = author.content.len() + updated.content.len() + lines.content.len() + comments.content.len() + 4;
    let avail = width.saturating_sub(prefix_cols + suffix_cols);
    let title = truncate_to(&pr.title, avail.max(10));

    Line::from(vec![
        Span::raw(" "),
        badge,
        number,
        Span::raw(title),
        Span::raw("  "),
        lines,
        comments,
        author,
        updated,
    ])
}

fn format_session_row(
    pr_number: u64,
    worktree_path: &Path,
    head_sha: &str,
    viewed: usize,
    total: usize,
    width: usize,
) -> Line<'static> {
    let head_short: String = head_sha.chars().take(7).collect();
    let progress = format!("{viewed}/{total} viewed");
    let path_str = worktree_path.display().to_string();
    let prefix = format!("  #{:<5}", pr_number);
    let used = prefix.len() + progress.len() + head_short.len() + 8;
    let path_room = width.saturating_sub(used);
    let path_show = truncate_to(&path_str, path_room.max(10));

    Line::from(vec![
        Span::raw(" "),
        Span::styled("  SESSION", Style::default().fg(Color::Black).bg(Color::Yellow)),
        Span::styled(prefix, Style::default().fg(Color::Cyan)),
        Span::styled(path_show, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(head_short, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        Span::styled(progress, Style::default().fg(Color::Green)),
    ])
}

fn truncate_to(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    if s.chars().count() <= max_cols {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_cols.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn render_footer(frame: &mut Frame, state: &mut DashboardState, area: Rect) {
    if let Some(s) = &state.status {
        if s.expires_at > Instant::now() {
            let (fg, bg) = match s.kind {
                StatusKind::Success => (Color::Black, Color::Green),
                StatusKind::Error => (Color::Black, Color::Red),
                StatusKind::Info => (Color::Black, Color::Cyan),
            };
            let line = Line::from(vec![
                Span::styled(format!(" {} ", s.text), Style::default().fg(fg).bg(bg)),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
    }
    let cur_is_session = matches!(
        state.rows.get(state.list_state.selected().unwrap_or(usize::MAX)),
        Some(Row::Session { .. })
    );
    let mut spans: Vec<Span> = Vec::new();
    spans.push(hot("Enter", "open"));
    spans.push(Span::raw("  "));
    spans.push(hot("j/k", "move"));
    spans.push(Span::raw("  "));
    spans.push(hot("r", "refresh"));
    if cur_is_session {
        spans.push(Span::raw("  "));
        spans.push(hot("d", "delete session"));
    }
    spans.push(Span::raw("  "));
    spans.push(hot("q", "quit"));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn hot(key: &str, label: &str) -> Span<'static> {
    Span::raw(format!("[{key}] {label}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{DashboardData, DashboardPr};

    fn pr(n: u64, title: &str) -> DashboardPr {
        DashboardPr {
            number: n,
            title: title.to_owned(),
            author: "alice".into(),
            is_draft: false,
            updated_at: "2026-04-26 10:00".into(),
            additions: 10,
            deletions: 2,
            review_decision: String::new(),
            repo_name_with_owner: "owner/repo".into(),
            url: "https://example".into(),
            comment_count: 0,
        }
    }

    fn make_state(req: Vec<DashboardPr>, auth: Vec<DashboardPr>, asgn: Vec<DashboardPr>) -> DashboardState {
        let data = DashboardData {
            review_requested: req,
            authored: auth,
            assigned: asgn,
        };
        let dir = tempdir();
        DashboardState::new(data, "owner".into(), "repo".into(), &dir).unwrap()
    }

    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("prowler-dash-test-{}", std::process::id()));
        std::fs::create_dir_all(&p).ok();
        p
    }

    #[test]
    fn cursor_lands_on_first_pr() {
        let s = make_state(vec![pr(1, "first"), pr(2, "second")], vec![], vec![]);
        assert!(matches!(
            s.rows[s.list_state.selected().unwrap()],
            Row::Pr { .. }
        ));
    }

    #[test]
    fn j_skips_over_section_headers() {
        let mut s = make_state(vec![pr(1, "first")], vec![pr(2, "second")], vec![]);
        let start = s.list_state.selected().unwrap();
        s.apply_key(KeyCode::Char('j'));
        let next = s.list_state.selected().unwrap();
        assert_ne!(start, next);
        assert!(matches!(s.rows[next], Row::Pr { .. }));
    }

    #[test]
    fn enter_returns_open_outcome() {
        let mut s = make_state(vec![pr(42, "answer")], vec![], vec![]);
        let outcome = s.apply_key(KeyCode::Enter);
        assert!(matches!(outcome, Some(DashboardOutcome::Open(42))));
    }

    #[test]
    fn q_returns_quit_outcome() {
        let mut s = make_state(vec![], vec![], vec![]);
        let outcome = s.apply_key(KeyCode::Char('q'));
        assert!(matches!(outcome, Some(DashboardOutcome::Quit)));
    }

    #[test]
    fn r_returns_refresh_outcome() {
        let mut s = make_state(vec![pr(1, "x")], vec![], vec![]);
        let outcome = s.apply_key(KeyCode::Char('r'));
        assert!(matches!(outcome, Some(DashboardOutcome::Refresh)));
    }

    #[test]
    fn empty_dashboard_renders_without_panic() {
        let mut s = make_state(vec![], vec![], vec![]);
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| render(f, &mut s)).unwrap();
    }

    #[test]
    fn render_shows_section_headers_and_pr_titles() {
        let mut s = make_state(
            vec![pr(101, "Add feature X")],
            vec![pr(202, "Refactor module Y")],
            vec![],
        );
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| render(f, &mut s)).unwrap();
        let buf = term.backend().buffer().clone();
        let dump = render_to_string(&buf);
        assert!(dump.contains("Awaiting your review"));
        assert!(dump.contains("Opened by you"));
        assert!(dump.contains("Add feature X"));
        assert!(dump.contains("Refactor module Y"));
    }

    fn render_to_string(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area().height {
            for x in 0..buf.area().width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}
