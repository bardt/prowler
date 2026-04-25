use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::diff::FileDiff;
use crate::github::{CommentSide, CommentThread, PrMetadata};
use crate::session::{FileStatus, Session};
use crate::tui::diff_view::{LaidOutDiff, Side, render_pane};
use crate::tui::file_tree::{FileTree, VisibleItem, VisibleRow};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Files,
    Base,
    Head,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Success,
    Error,
    #[allow(dead_code)] // Reserved for future "in-progress" / informational notices.
    Info,
}

pub struct Status {
    pub text: String,
    pub kind: StatusKind,
    pub expires_at: Instant,
}

const STATUS_TTL: Duration = Duration::from_secs(3);

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
    pub token: String,
    pub owner: String,
    pub repo: String,
    file_tree: FileTree,
    visible_rows: Vec<VisibleRow>,
    status: Option<Status>,
    /// Threads grouped by file index, kept so we can re-layout on terminal resize.
    threads_by_file: Vec<Vec<CommentThread>>,
    /// Last pane content width used to lay out diffs (for comment wrapping).
    last_layout_width: u16,
    /// Thread IDs currently expanded. Anything not in here renders as a one-row
    /// `CollapsedThread`. Newly-posted threads are auto-added on `apply_refresh`.
    expanded_threads: HashSet<String>,
}

/// Default wrap width used before we've measured the actual pane (e.g. on first paint).
const DEFAULT_WRAP_WIDTH: u16 = 80;

/// Columns consumed by per-row chrome inside a pane: 1 gutter + `│ ` (2) + 2 borders.
const PANE_CHROME_COLS: u16 = 5;
const MIN_WRAP_WIDTH: u16 = 20;

fn build_layout(
    diffs: &[FileDiff],
    threads_by_file: &[Vec<CommentThread>],
    pane_width: u16,
    expanded: &HashSet<String>,
) -> Vec<LaidOutDiff> {
    let wrap_width = pane_width
        .saturating_sub(PANE_CHROME_COLS)
        .max(MIN_WRAP_WIDTH) as usize;
    diffs
        .iter()
        .zip(threads_by_file)
        .map(|(d, t)| LaidOutDiff::from_file(d, t, wrap_width, expanded))
        .collect()
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
        owner: String,
        repo: String,
    ) -> Self {
        let mut threads_by_file: Vec<Vec<CommentThread>> = vec![Vec::new(); diffs.len()];
        for thread in threads {
            if let Some(idx) = diffs.iter().position(|d| d.path == thread.path) {
                threads_by_file[idx].push(thread);
            }
        }
        let expanded_threads: HashSet<String> = HashSet::new();
        let laid = build_layout(&diffs, &threads_by_file, DEFAULT_WRAP_WIDTH, &expanded_threads);
        let scroll = vec![0; diffs.len()];
        let cursor = vec![0; diffs.len()];
        let file_tree = FileTree::build(&diffs);
        let visible_rows = file_tree.visible_rows();
        let mut list_state = ListState::default();
        // Land cursor on the first file row so a diff is shown immediately.
        let first_file = visible_rows
            .iter()
            .position(|r| matches!(r.item, VisibleItem::File { .. }));
        list_state.select(first_file.or_else(|| (!visible_rows.is_empty()).then_some(0)));
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
            owner,
            repo,
            file_tree,
            visible_rows,
            status: None,
            threads_by_file,
            last_layout_width: DEFAULT_WRAP_WIDTH,
            expanded_threads,
        }
    }

    /// Re-lay-out diffs with a new wrap width. Called by `render_body` when the
    /// pane width changes.
    pub fn relayout_for_width(&mut self, content_width: u16) {
        if content_width == self.last_layout_width || content_width == 0 {
            return;
        }
        self.last_layout_width = content_width;
        self.relayout();
    }

    fn relayout(&mut self) {
        self.laid = build_layout(
            &self.diffs,
            &self.threads_by_file,
            self.last_layout_width,
            &self.expanded_threads,
        );
    }

    /// Toggle the expansion state of the comment thread under the cursor.
    /// No-op when the cursor is not on a thread row.
    pub fn toggle_thread(&mut self) {
        let Some(i) = self.selected_idx() else {
            return;
        };
        let cur = self.cursor[i] as usize;
        let Some(thread_id) = self.laid[i]
            .rows
            .get(cur)
            .and_then(|r| r.thread_id.clone())
        else {
            return;
        };
        if !self.expanded_threads.remove(&thread_id) {
            self.expanded_threads.insert(thread_id);
        }
        self.relayout();
    }

    pub fn set_status(&mut self, text: impl Into<String>, kind: StatusKind) {
        self.status = Some(Status {
            text: text.into(),
            kind,
            expires_at: Instant::now() + STATUS_TTL,
        });
    }

    fn current_status(&self) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| Instant::now() < s.expires_at)
    }

    /// Toggle collapse on the folder under the file-panel cursor.
    /// No-op when cursor is on a file row.
    pub fn toggle_folder(&mut self) {
        let Some(i) = self.list_state.selected() else {
            return;
        };
        let Some(row) = self.visible_rows.get(i).cloned() else {
            return;
        };
        let VisibleItem::Folder { path, .. } = row.item else {
            return;
        };
        if let Some(folder) = self.file_tree.folder_at_mut(&path) {
            folder.collapsed = !folder.collapsed;
        }
        self.visible_rows = self.file_tree.visible_rows();
        let new_idx = self.visible_rows.iter().position(|r| match &r.item {
            VisibleItem::Folder { path: p, .. } => p == &path,
            _ => false,
        });
        if let Some(idx) = new_idx {
            self.list_state.select(Some(idx));
        }
    }

    /// Return the GitHub-side anchor (path, side, line) for the row under the cursor,
    /// preferring HEAD when the row has both lines (Context). Returns None for rows
    /// with no commentable content (HunkHeader, comment rows, Empty).
    /// Return the thread node ID for the row under the cursor, if it sits inside
    /// a rendered comment thread (header, body, or terminator row). Used by `r`.
    pub fn reply_target(&self) -> Option<String> {
        let i = self.selected_idx()?;
        let cur = self.cursor[i] as usize;
        self.laid[i].rows.get(cur)?.thread_id.clone()
    }

    pub fn comment_target(&self) -> Option<(String, CommentSide, u32)> {
        let i = self.selected_idx()?;
        let cur = self.cursor[i] as usize;
        let row = self.laid[i].rows.get(cur)?;
        let path = self.diffs[i].path.clone();
        if let Some(line) = row.head_line {
            Some((path, CommentSide::Head, line))
        } else {
            row.base_line.map(|line| (path, CommentSide::Base, line))
        }
    }

    /// Replace metadata + threads after a refetch and rebuild the laid-out diff.
    /// Newly-appearing threads (e.g. one the user just posted) are auto-expanded
    /// so the caller doesn't need to hunt them down.
    pub fn apply_refresh(&mut self, meta: PrMetadata, threads: Vec<CommentThread>) {
        self.meta = meta;
        let known: HashSet<String> = self
            .threads_by_file
            .iter()
            .flat_map(|tt| tt.iter().map(|t| t.id.clone()))
            .collect();
        for thread in &threads {
            if !known.contains(&thread.id) {
                self.expanded_threads.insert(thread.id.clone());
            }
        }
        self.set_threads(threads);
    }

    /// Replace threads and rebuild the laid-out diff. Cursor and scroll offsets are
    /// preserved by row index, but the rows underneath may have shifted (a new
    /// comment thread inserts rows). That's acceptable — the cursor row identity
    /// changes silently after a post.
    pub fn set_threads(&mut self, threads: Vec<CommentThread>) {
        let mut threads_by_file: Vec<Vec<CommentThread>> = vec![Vec::new(); self.diffs.len()];
        for thread in threads {
            if let Some(idx) = self.diffs.iter().position(|d| d.path == thread.path) {
                threads_by_file[idx].push(thread);
            }
        }
        self.threads_by_file = threads_by_file;
        self.relayout();
    }

    pub fn pr_number(&self) -> u64 {
        self.session.pr_number
    }

    pub fn pr_node_id(&self) -> &str {
        &self.meta.node_id
    }

    pub fn pending_review_id(&self) -> Option<&str> {
        self.meta.pending_review_id.as_deref()
    }

    pub fn cursor_on_folder(&self) -> bool {
        let Some(i) = self.list_state.selected() else {
            return false;
        };
        matches!(
            self.visible_rows.get(i).map(|r| &r.item),
            Some(VisibleItem::Folder { .. })
        )
    }

    pub fn cursor_on_thread(&self) -> bool {
        let Some(i) = self.selected_idx() else {
            return false;
        };
        let cur = self.cursor[i] as usize;
        self.laid[i]
            .rows
            .get(cur)
            .map(|r| r.thread_id.is_some())
            .unwrap_or(false)
    }

    pub fn cursor_on_code_line(&self) -> bool {
        let Some(i) = self.selected_idx() else {
            return false;
        };
        let cur = self.cursor[i] as usize;
        let Some(row) = self.laid[i].rows.get(cur) else {
            return false;
        };
        row.thread_id.is_none() && (row.base_line.is_some() || row.head_line.is_some())
    }

    /// How many of the visible (non-outdated) comments belong to a pending review.
    pub fn pending_comment_count(&self) -> usize {
        self.laid
            .iter()
            .flat_map(|laid| laid.rows.iter())
            .filter_map(|row| match (&row.base, &row.head) {
                (
                    crate::tui::diff_view::Cell::CommentHeader { is_pending, .. },
                    _,
                )
                | (
                    _,
                    crate::tui::diff_view::Cell::CommentHeader { is_pending, .. },
                ) => Some(*is_pending),
                _ => None,
            })
            .filter(|p| *p)
            .count()
    }

    fn file_status(&self, path: &str) -> FileStatus {
        self.session
            .files
            .get(path)
            .copied()
            .unwrap_or(FileStatus::Unviewed)
    }

    fn set_file_status(&mut self, path: String, status: FileStatus) {
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
        self.set_file_status(path, next);
    }

    pub fn toggle_skipped(&mut self) {
        let Some(i) = self.selected_idx() else { return };
        let path = self.diffs[i].path.clone();
        let next = match self.file_status(&path) {
            FileStatus::Skipped => FileStatus::Unviewed,
            _ => FileStatus::Skipped,
        };
        self.set_file_status(path, next);
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

    pub fn cycle_focus_back(&mut self) {
        self.focus = match self.focus {
            Focus::Files => Focus::Head,
            Focus::Base => Focus::Files,
            Focus::Head => Focus::Base,
        };
    }

    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
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
        if self.visible_rows.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.visible_rows.len() - 1);
        self.list_state.select(Some(next));
    }

    fn prev_file(&mut self) {
        if self.visible_rows.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
    }

    /// Index into `self.diffs` for the file under the file-panel cursor.
    /// `None` when the cursor is on a folder row.
    fn selected_idx(&self) -> Option<usize> {
        let i = self.list_state.selected()?;
        match self.visible_rows.get(i)?.item {
            VisibleItem::File { diff_idx, .. } => Some(diff_idx),
            VisibleItem::Folder { .. } => None,
        }
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

    /// Make the file at `diff_idx` selected in the file panel, expanding any
    /// collapsed ancestor folders so the file row is visible.
    fn select_file(&mut self, diff_idx: usize) {
        let Some(path) = self.file_tree.find_file(diff_idx) else {
            return;
        };
        for i in 1..path.len() {
            if let Some(folder) = self.file_tree.folder_at_mut(&path[..i]) {
                folder.collapsed = false;
            }
        }
        self.visible_rows = self.file_tree.visible_rows();
        let pos = self.visible_rows.iter().position(|r| {
            matches!(r.item, VisibleItem::File { diff_idx: d, .. } if d == diff_idx)
        });
        if let Some(idx) = pos {
            self.list_state.select(Some(idx));
        }
    }

    /// Jump to the next comment-thread row across the whole PR (wraps around).
    /// Search order: from cursor+1 in the current file, then later files, then
    /// earlier files, then back to start of current file up to the cursor.
    pub fn goto_next_thread(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let current_file = self.selected_idx().unwrap_or(0);
        let current_row = self
            .selected_idx()
            .map(|f| self.cursor[f] as usize + 1)
            .unwrap_or(0);
        let n = self.diffs.len();
        for offset in 0..=n {
            let file_idx = (current_file + offset) % n;
            let start = if offset == 0 { current_row } else { 0 };
            let rows = &self.laid[file_idx].rows;
            if start >= rows.len() {
                continue;
            }
            if let Some(rel) = rows[start..].iter().position(|r| r.thread_id.is_some()) {
                let target = start + rel;
                self.select_file(file_idx);
                self.cursor[file_idx] = target as u16;
                self.ensure_cursor_visible(file_idx);
                if self.focus == Focus::Files {
                    self.focus = Focus::Head;
                }
                return;
            }
        }
    }

    /// Jump to the previous comment-thread row across the whole PR (wraps around).
    pub fn goto_prev_thread(&mut self) {
        if self.diffs.is_empty() {
            return;
        }
        let current_file = self.selected_idx().unwrap_or(0);
        let current_row = self
            .selected_idx()
            .map(|f| self.cursor[f] as usize)
            .unwrap_or(0);
        let n = self.diffs.len();
        for offset in 0..=n {
            // Walk backwards: current file then earlier (with wrap).
            let file_idx = (current_file + n - (offset % n)) % n;
            let rows = &self.laid[file_idx].rows;
            let end = if offset == 0 {
                current_row.min(rows.len())
            } else {
                rows.len()
            };
            if end == 0 {
                continue;
            }
            if let Some(rel) = rows[..end].iter().rposition(|r| r.thread_id.is_some()) {
                self.select_file(file_idx);
                self.cursor[file_idx] = rel as u16;
                self.ensure_cursor_visible(file_idx);
                if self.focus == Focus::Files {
                    self.focus = Focus::Head;
                }
                return;
            }
        }
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
    render_footer(frame, outer[2], state);
}

fn render_footer(frame: &mut Frame, area: Rect, state: &ReviewState) {
    if let Some(status) = state.current_status() {
        let (prefix, color) = match status.kind {
            StatusKind::Success => ("\u{2713} ", Color::Green),
            StatusKind::Error => ("\u{2717} ", Color::Red),
            StatusKind::Info => ("\u{2022} ", Color::Cyan),
        };
        let line = Line::from(vec![
            Span::styled(
                prefix,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(status.text.clone(), Style::default().fg(color)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    } else {
        render_hotkeys(frame, area, state);
    }
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
    state.relayout_for_width(cols[1].width);

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
        .visible_rows
        .iter()
        .map(|row| match &row.item {
            VisibleItem::Folder {
                name, collapsed, ..
            } => {
                let indent = "  ".repeat(row.depth);
                let chevron = if *collapsed { "\u{25B8}" } else { "\u{25BE}" };
                ListItem::new(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(
                        format!("{chevron} {name}/"),
                        Style::default().fg(Color::Cyan),
                    ),
                ]))
            }
            VisibleItem::File { diff_idx, name } => {
                let d = &state.diffs[*diff_idx];
                let status = state.file_status(&d.path);
                let (marker, marker_style) = match status {
                    FileStatus::Unviewed => (" ", Style::default()),
                    FileStatus::InProgress => ("*", Style::default().fg(Color::Cyan)),
                    FileStatus::Viewed => ("\u{2713}", Style::default().fg(Color::Green)),
                    FileStatus::Skipped => ("~", Style::default().fg(Color::Yellow)),
                };
                let name_style = if status == FileStatus::Viewed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let indent = "  ".repeat(row.depth);
                ListItem::new(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(marker, marker_style),
                    Span::raw(" "),
                    Span::styled(name.clone(), name_style),
                    Span::raw("  "),
                    Span::styled(format!("+{}", d.added), Style::default().fg(Color::Green)),
                    Span::raw(" "),
                    Span::styled(format!("-{}", d.removed), Style::default().fg(Color::Red)),
                ]))
            }
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

fn render_hotkeys(frame: &mut Frame, area: Rect, state: &ReviewState) {
    let mut groups: Vec<(&str, &str)> = Vec::new();
    match state.focus {
        Focus::Files => {
            groups.push(("j/k", " move  "));
            if state.cursor_on_folder() {
                groups.push(("Space", " fold  "));
            } else {
                groups.push(("v/s", " view/skip  "));
            }
        }
        Focus::Base | Focus::Head => {
            groups.push(("j/k", " scroll  "));
            groups.push(("]/[", " hunk  "));
            if state.cursor_on_thread() {
                groups.push(("Enter", " expand  "));
                groups.push(("r", " reply  "));
            } else if state.cursor_on_code_line() {
                groups.push(("e/E", " edit  "));
                groups.push(("c", " comment  "));
            }
        }
    }
    groups.push(("Tab", " panel  "));
    groups.push(("S", " submit  "));
    groups.push(("q", " quit"));

    let mut spans = Vec::with_capacity(groups.len() * 2);
    for (k, label) in groups {
        spans.push(key(k));
        spans.push(Span::raw(label));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().fg(Color::DarkGray)),
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
