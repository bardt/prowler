use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

use crate::diff::FileDiff;
use crate::github::{CommentSide, CommentThread, PrMetadata};
use crate::session::{FileStatus, Session};
use crate::tui::diff_view::{LaidOutDiff, Side, render_pane};
use crate::tui::file_tree::{FileTree, VisibleItem, VisibleRow};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Files,
    Base,
    Head,
    Local,
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

/// Message sent from background tasks (e.g. spawned viewed-state sync) to the
/// event loop, which converts it into a `Status` for the footer.
pub struct StatusMessage {
    pub text: String,
    pub kind: StatusKind,
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
    /// Channel for messages from background tasks (currently the viewed-state
    /// sync). The event loop drains this and turns each into a status row.
    status_tx: UnboundedSender<StatusMessage>,
    /// Whether the LOCAL pane is currently displayed.
    local_panel: bool,
    /// Per-file local diff (HEAD → worktree), lazily computed when L is toggled
    /// on or R is pressed. `Some(diff)` means computed (might be empty);
    /// `None` means not yet computed.
    local_diffs: Vec<Option<FileDiff>>,
    /// Viewport scroll offset for the LOCAL pane, per file.
    local_scroll: Vec<u16>,
    /// When true, the body area is replaced by a full-width description page
    /// (PR body + top-level conversation). Toggled with `?`.
    show_description: bool,
    description_scroll: u16,
    /// Two-step delete confirmation: first `X` press records the comment id and
    /// timestamp; a second `X` press on the same comment within `STATUS_TTL`
    /// triggers the actual delete.
    pending_delete: Option<(String, Instant)>,
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
        status_tx: UnboundedSender<StatusMessage>,
    ) -> Self {
        let n = diffs.len();
        let mut threads_by_file: Vec<Vec<CommentThread>> = vec![Vec::new(); n];
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
            status_tx,
            local_panel: false,
            local_diffs: (0..n).map(|_| None).collect(),
            local_scroll: vec![0; n],
            show_description: false,
            description_scroll: 0,
            pending_delete: None,
        }
    }

    /// Toggle the full-width PR description / conversation page.
    pub fn toggle_description(&mut self) {
        self.show_description = !self.show_description;
        self.description_scroll = 0;
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

    /// Toggle the LOCAL diff panel. On first show (or after the cursor moves
    /// to a file with no cached local diff), the diff is computed lazily.
    pub fn toggle_local_panel(&mut self) {
        self.local_panel = !self.local_panel;
        if self.local_panel {
            self.ensure_local_for_current();
        } else if self.focus == Focus::Local {
            self.focus = Focus::Head;
        }
    }

    /// Recompute the local diff for the file under the cursor.
    pub fn refresh_local(&mut self) {
        let Some(i) = self.selected_idx() else {
            return;
        };
        self.compute_local_for(i);
    }

    fn ensure_local_for_current(&mut self) {
        let Some(i) = self.selected_idx() else {
            return;
        };
        if self.local_diffs[i].is_none() {
            self.compute_local_for(i);
        }
    }

    fn compute_local_for(&mut self, file_idx: usize) {
        // Constructs a single PrFile so we can reuse `diff::compute_diffs` to
        // produce a HEAD → worktree diff. status="modified" works for the
        // common case; locally-added or locally-deleted files render as a full
        // additions/removals diff respectively. Acceptable for v1.
        let pr_file = crate::github::PrFile {
            path: self.diffs[file_idx].path.clone(),
            previous_path: None,
            status: "modified".into(),
            viewer_viewed_state: String::new(),
        };
        let diff = match crate::diff::compute_diffs(
            &self.repo_root,
            &self.session.worktree_path,
            &self.meta.head_sha,
            std::slice::from_ref(&pr_file),
        ) {
            Ok(mut v) => v.pop(),
            Err(e) => {
                self.set_status(
                    format!("Local diff failed: {e}"),
                    StatusKind::Error,
                );
                None
            }
        };
        self.local_diffs[file_idx] = diff;
        self.local_scroll[file_idx] = 0;
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

    /// The thread under the cursor, if any.
    pub fn current_thread(&self) -> Option<&CommentThread> {
        let i = self.selected_idx()?;
        let cur = self.cursor[i] as usize;
        let row = self.laid[i].rows.get(cur)?;
        let tid = row.thread_id.as_deref()?;
        self.threads_by_file[i].iter().find(|t| t.id == tid)
    }

    /// The (thread_id, is_resolved) of the thread under the cursor.
    pub fn current_thread_resolution(&self) -> Option<(String, bool)> {
        let t = self.current_thread()?;
        let toggleable = if t.is_resolved {
            t.viewer_can_unresolve
        } else {
            t.viewer_can_resolve
        };
        if !toggleable {
            return None;
        }
        Some((t.id.clone(), t.is_resolved))
    }

    /// First press of `X` arms a delete; the second press within `STATUS_TTL`
    /// for the same comment confirms. Returns true when the caller should
    /// actually run the mutation.
    pub fn arm_or_confirm_delete(&mut self, comment_id: &str) -> bool {
        let now = Instant::now();
        if let Some((id, t)) = &self.pending_delete {
            if id == comment_id && now.duration_since(*t) <= STATUS_TTL {
                self.pending_delete = None;
                return true;
            }
        }
        self.pending_delete = Some((comment_id.to_owned(), now));
        false
    }

    /// The (id, body) of the comment under the cursor, but only when the viewer
    /// authored it (i.e. the viewer can edit / delete it).
    pub fn current_own_comment(&self) -> Option<(String, String)> {
        let i = self.selected_idx()?;
        let cur = self.cursor[i] as usize;
        let row = self.laid[i].rows.get(cur)?;
        let cid = row.comment_id.as_deref()?;
        let tid = row.thread_id.as_deref()?;
        let thread = self.threads_by_file[i].iter().find(|t| t.id == tid)?;
        let comment = thread.comments.iter().find(|c| c.id == cid)?;
        if !comment.viewer_did_author {
            return None;
        }
        Some((comment.id.clone(), comment.body.clone()))
    }

    /// Extract a `\`\`\`suggestion ... \`\`\`` block from the comment under the
    /// cursor, plus the file path and 1-indexed HEAD line where it should
    /// apply. Only matches threads on the HEAD side, since suggestions replace
    /// the new code, not the old.
    pub fn current_suggestion_target(
        &self,
    ) -> Option<(String, std::path::PathBuf, u32, u32)> {
        let i = self.selected_idx()?;
        let cur = self.cursor[i] as usize;
        let row = self.laid[i].rows.get(cur)?;
        let cid = row.comment_id.as_deref()?;
        let tid = row.thread_id.as_deref()?;
        let thread = self.threads_by_file[i].iter().find(|t| t.id == tid)?;
        if thread.side != CommentSide::Head {
            return None;
        }
        let comment = thread.comments.iter().find(|c| c.id == cid)?;
        let suggestion = extract_suggestion(&comment.body)?;
        Some((
            suggestion,
            self.session.worktree_path.join(&thread.path),
            thread.line,
            thread.line,
        ))
    }

    /// Apply a suggestion text to a worktree file at the given 1-indexed line range
    /// (inclusive). Replaces the line in place, preserving the trailing newline if
    /// the original file had one.
    pub fn apply_suggestion(
        &self,
        file: &std::path::Path,
        start_line: u32,
        end_line: u32,
        suggestion: &str,
    ) -> anyhow::Result<()> {
        use anyhow::Context;
        let original = std::fs::read_to_string(file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        let trailing_nl = original.ends_with('\n');
        let mut lines: Vec<&str> = original.split('\n').collect();
        if trailing_nl {
            // split('\n') on text ending in \n gives a trailing empty entry; drop it.
            lines.pop();
        }
        let start = start_line.saturating_sub(1) as usize;
        let end = (end_line as usize).min(lines.len());
        if start > lines.len() {
            anyhow::bail!("line {start_line} is past EOF");
        }
        let suggestion_lines: Vec<&str> = suggestion.split('\n').collect();
        // GitHub's suggestion blocks include one trailing newline that makes the
        // `\`\`\`` close-fence its own line; strip a trailing empty entry if present.
        let mut suggestion_lines = suggestion_lines;
        if suggestion_lines.last().map(|s| s.is_empty()).unwrap_or(false) {
            suggestion_lines.pop();
        }
        lines.splice(start..end, suggestion_lines);
        let mut out = lines.join("\n");
        if trailing_nl {
            out.push('\n');
        }
        std::fs::write(file, out)
            .with_context(|| format!("failed to write {}", file.display()))?;
        Ok(())
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
        let status_tx = self.status_tx.clone();
        tokio::spawn(async move {
            let result = crate::github::set_viewed(&token, &node_id, &path, viewed).await;
            let line = match &result {
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
            if let Err(e) = result {
                let action = if viewed { "mark viewed" } else { "unmark viewed" };
                let _ = status_tx.send(StatusMessage {
                    text: format!("Sync failed ({action} {path}): {e}"),
                    kind: StatusKind::Error,
                });
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
        self.focus = match (self.focus, self.local_panel) {
            (Focus::Files, _) => Focus::Base,
            (Focus::Base, _) => Focus::Head,
            (Focus::Head, true) => Focus::Local,
            (Focus::Head, false) => Focus::Files,
            (Focus::Local, _) => Focus::Files,
        };
    }

    pub fn cycle_focus_back(&mut self) {
        self.focus = match (self.focus, self.local_panel) {
            (Focus::Files, true) => Focus::Local,
            (Focus::Files, false) => Focus::Head,
            (Focus::Base, _) => Focus::Files,
            (Focus::Head, _) => Focus::Base,
            (Focus::Local, _) => Focus::Head,
        };
    }

    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
    }

    pub fn move_down(&mut self) {
        if self.show_description {
            self.description_scroll = self.description_scroll.saturating_add(1);
            return;
        }
        match self.focus {
            Focus::Files => self.next_file(),
            Focus::Base | Focus::Head => self.move_cursor(1),
            Focus::Local => self.scroll_local(1),
        }
    }

    pub fn move_up(&mut self) {
        if self.show_description {
            self.description_scroll = self.description_scroll.saturating_sub(1);
            return;
        }
        match self.focus {
            Focus::Files => self.prev_file(),
            Focus::Base | Focus::Head => self.move_cursor(-1),
            Focus::Local => self.scroll_local(-1),
        }
    }

    fn scroll_local(&mut self, delta: i32) {
        let Some(i) = self.selected_idx() else {
            return;
        };
        let cur = self.local_scroll[i] as i32;
        let next = (cur + delta).max(0) as u16;
        self.local_scroll[i] = next;
    }

    fn next_file(&mut self) {
        if self.visible_rows.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        let next = (i + 1).min(self.visible_rows.len() - 1);
        self.list_state.select(Some(next));
        if self.local_panel {
            self.ensure_local_for_current();
        }
    }

    fn prev_file(&mut self) {
        if self.visible_rows.is_empty() {
            return;
        }
        let i = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(i.saturating_sub(1)));
        if self.local_panel {
            self.ensure_local_for_current();
        }
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

/// Apply a pure (non-terminal-touching) key to the state. Returns `true` for
/// `q` (quit), `false` otherwise. Side-effectful keys (`c`, `r`, `S`, `e`, `E`)
/// are NOT handled here — the event loop dispatches them itself because they
/// suspend the TUI / spawn editors / await network I/O.
/// Extract the body of a ` ```suggestion ` … ` ``` ` block from a comment body.
/// Returns the inner text (lines between fences, joined with `\n`). If multiple
/// blocks exist, only the first is returned.
pub fn extract_suggestion(body: &str) -> Option<String> {
    let mut iter = body.lines();
    let mut content = Vec::new();
    let mut in_block = false;
    for line in iter.by_ref() {
        let trimmed = line.trim_start();
        if !in_block {
            if let Some(rest) = trimmed.strip_prefix("```") {
                if rest.trim() == "suggestion" {
                    in_block = true;
                }
            }
            continue;
        }
        if trimmed.starts_with("```") {
            return Some(content.join("\n"));
        }
        content.push(line);
    }
    None
}

pub fn apply_key(state: &mut ReviewState, key: KeyCode) -> bool {
    match key {
        KeyCode::Char('q') => return true,
        KeyCode::Tab => state.cycle_focus(),
        KeyCode::BackTab => state.cycle_focus_back(),
        KeyCode::Char('1') => state.set_focus(Focus::Files),
        KeyCode::Char('2') => state.set_focus(Focus::Base),
        KeyCode::Char('3') => state.set_focus(Focus::Head),
        KeyCode::Char('4') => {
            if state.local_panel {
                state.set_focus(Focus::Local);
            }
        }
        KeyCode::Char('L') => state.toggle_local_panel(),
        KeyCode::Char('R') => state.refresh_local(),
        KeyCode::Char('?') => state.toggle_description(),
        KeyCode::Char('j') | KeyCode::Down => state.move_down(),
        KeyCode::Char('k') | KeyCode::Up => state.move_up(),
        KeyCode::Char(']') => state.next_hunk(),
        KeyCode::Char('[') => state.prev_hunk(),
        KeyCode::Char('v') => state.toggle_viewed(),
        KeyCode::Char('s') => state.toggle_skipped(),
        KeyCode::Char(' ') => state.toggle_folder(),
        KeyCode::Enter => match state.focus {
            // Files panel: Enter folds/unfolds the folder under the cursor.
            Focus::Files if state.cursor_on_folder() => state.toggle_folder(),
            // Diff panes: Enter expands/collapses the comment thread under the cursor.
            Focus::Base | Focus::Head => state.toggle_thread(),
            _ => {}
        },
        KeyCode::Char('n') => state.goto_next_thread(),
        KeyCode::Char('N') => state.goto_prev_thread(),
        _ => {}
    }
    false
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
    if state.show_description {
        render_description(frame, outer[1], state);
    } else {
        render_body(frame, outer[1], state);
    }
    render_footer(frame, outer[2], state);
}

fn render_description(frame: &mut Frame, area: Rect, state: &ReviewState) {
    let mut spans_lines: Vec<Line> = Vec::new();

    // PR description
    spans_lines.push(Line::styled(
        "## Description",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    spans_lines.push(Line::raw(""));
    if state.meta.body.is_empty() {
        spans_lines.push(Line::styled(
            "(no description)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for line in state.meta.body.lines() {
            spans_lines.push(Line::raw(line.to_owned()));
        }
    }
    spans_lines.push(Line::raw(""));

    // Conversation
    spans_lines.push(Line::styled(
        format!("## Conversation ({})", state.meta.conversation.len()),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    spans_lines.push(Line::raw(""));
    if state.meta.conversation.is_empty() {
        spans_lines.push(Line::styled(
            "(no top-level comments)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for c in &state.meta.conversation {
            spans_lines.push(Line::from(vec![
                Span::styled(
                    format!("@{}", c.author),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(c.created_at.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            for line in c.body.lines() {
                spans_lines.push(Line::raw(format!("  {line}")));
            }
            spans_lines.push(Line::raw(""));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title("DESCRIPTION  (?: close · j/k: scroll)")
        .border_style(Style::default().fg(Color::Cyan));
    let para = Paragraph::new(spans_lines)
        .block(block)
        .scroll((state.description_scroll, 0));
    frame.render_widget(para, area);
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
    let (badge_text, badge_color) = pr_state_badge(&state.meta.state, state.meta.is_draft);
    let total = state.meta.files.len();
    let viewed = state
        .meta
        .files
        .iter()
        .filter(|f| {
            matches!(
                state.session.files.get(&f.path).copied().unwrap_or(FileStatus::Unviewed),
                FileStatus::Viewed | FileStatus::Skipped
            )
        })
        .count();
    let progress_color = if total > 0 && viewed == total {
        Color::Green
    } else {
        Color::Cyan
    };
    let title = Line::from(vec![
        Span::styled(
            format!(" {badge_text} "),
            Style::default()
                .fg(Color::Black)
                .bg(badge_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!("#{}: ", state.meta.pr_number()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(state.meta.title.clone()),
        Span::raw("   "),
        Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(Color::Red)),
        Span::raw("   "),
        Span::styled(
            format!("{viewed}/{total} viewed"),
            Style::default().fg(progress_color),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

/// `state` is GraphQL `PullRequestState`: `OPEN | CLOSED | MERGED`. A draft PR
/// has `state == "OPEN"` plus `is_draft == true`; we render that as `DRAFT` so
/// the reviewer can tell at a glance.
fn pr_state_badge(state: &str, is_draft: bool) -> (&'static str, Color) {
    if is_draft && state == "OPEN" {
        ("DRAFT", Color::Gray)
    } else {
        match state {
            "OPEN" => ("OPEN", Color::Green),
            "MERGED" => ("MERGED", Color::Magenta),
            "CLOSED" => ("CLOSED", Color::Red),
            _ => ("?", Color::DarkGray),
        }
    }
}

fn render_body(frame: &mut Frame, area: Rect, state: &mut ReviewState) {
    let constraints: Vec<Constraint> = if state.local_panel {
        vec![
            Constraint::Length(36),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Min(20),
        ]
    } else {
        vec![
            Constraint::Length(36),
            Constraint::Percentage(50),
            Constraint::Min(20),
        ]
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
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

    if state.local_panel {
        let local_file = i.and_then(|i| state.local_diffs.get(i).and_then(|d| d.as_ref()));
        let local_scroll = i.map(|i| state.local_scroll[i]).unwrap_or(0);
        render_local_pane(
            frame,
            cols[3],
            state.focus == Focus::Local,
            local_file,
            local_scroll,
        );
    }
}

fn render_local_pane(
    frame: &mut Frame,
    area: Rect,
    focused: bool,
    file: Option<&FileDiff>,
    scroll: u16,
) {
    use crate::diff::DiffLine;
    use crate::tui::syntax;

    let block = Block::default()
        .borders(Borders::ALL)
        .title("LOCAL [4]")
        .border_style(if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        });

    let lines: Vec<Line> = match file {
        None => vec![Line::styled(
            "(no local diff yet — press R to refresh)",
            Style::default().fg(Color::DarkGray),
        )],
        Some(f) if f.added == 0 && f.removed == 0 => vec![Line::styled(
            "(no local changes)",
            Style::default().fg(Color::DarkGray),
        )],
        Some(f) => {
            let syn = syntax::highlighter();
            let syntax_ref = syn.syntax_for(&f.path);
            let mut out = Vec::new();
            for hunk in &f.hunks {
                out.push(Line::from(Span::styled(
                    hunk.header.clone(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )));
                for line in &hunk.lines {
                    let (text, marker, fg) = match line {
                        DiffLine::Added(t) => (t.as_str(), "+ ", Color::Green),
                        DiffLine::Removed(t) => (t.as_str(), "- ", Color::Red),
                        DiffLine::Context(t) => (t.as_str(), "  ", Color::Reset),
                        DiffLine::Moved(t) => (t.as_str(), "~ ", Color::Blue),
                    };
                    let trimmed = text.strip_suffix('\n').unwrap_or(text);
                    let segs = syn.highlight_line(syntax_ref, trimmed);
                    let mut spans = vec![Span::styled(marker, Style::default().fg(fg))];
                    spans.extend(syntax::to_spans(&segs, None));
                    out.push(Line::from(spans));
                }
            }
            out
        }
    };

    let para = Paragraph::new(lines).block(block).scroll((scroll, 0));
    frame.render_widget(para, area);
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
                    FileStatus::Dismissed => (
                        "!",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                };
                let name_style = if status == FileStatus::Viewed {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                let indent = "  ".repeat(row.depth);
                let mut spans = vec![
                    Span::raw(indent),
                    Span::styled(marker, marker_style),
                    Span::raw(" "),
                    Span::styled(name.clone(), name_style),
                    Span::raw("  "),
                    Span::styled(format!("+{}", d.added), Style::default().fg(Color::Green)),
                    Span::raw(" "),
                    Span::styled(format!("-{}", d.removed), Style::default().fg(Color::Red)),
                ];
                // Unresolved comment-thread count (resolved threads silently
                // counted out so the badge tracks attention demand).
                let unresolved = state
                    .threads_by_file
                    .get(*diff_idx)
                    .map(|tt| tt.iter().filter(|t| !t.is_resolved).count())
                    .unwrap_or(0);
                if unresolved > 0 {
                    spans.push(Span::raw("  "));
                    spans.push(Span::styled(
                        format!("\u{1F4AC}{unresolved}"),
                        Style::default().fg(Color::Cyan),
                    ));
                }
                ListItem::new(Line::from(spans))
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
                groups.push(("Enter/Space", " fold  "));
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
                groups.push(("o", " resolve  "));
                if state.current_own_comment().is_some() {
                    groups.push(("M/X", " edit/del  "));
                }
                if state.current_suggestion_target().is_some() {
                    groups.push(("a", " apply  "));
                }
            } else if state.cursor_on_code_line() {
                groups.push(("e/E", " edit  "));
                groups.push(("c", " comment  "));
            }
        }
        Focus::Local => {
            groups.push(("j/k", " scroll  "));
            groups.push(("R", " refresh  "));
        }
    }
    groups.push(("Tab", " panel  "));
    if state.local_panel {
        groups.push(("L", " hide local  "));
    } else {
        groups.push(("L", " local  "));
    }
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

#[cfg(test)]
impl ReviewState {
    /// Construct a `ReviewState` with stub I/O fields suitable for unit tests.
    /// `Session` and worktree paths are dummies; the status channel's receiver
    /// is dropped, so any background sends are best-effort.
    pub fn for_test(
        meta: PrMetadata,
        diffs: Vec<FileDiff>,
        threads: Vec<CommentThread>,
    ) -> Self {
        use std::collections::HashMap;
        let session = Session {
            pr_number: meta.pr_number,
            branch: "test".into(),
            worktree_path: PathBuf::new(),
            base_worktree_path: PathBuf::new(),
            base_sha: meta.base_sha.clone(),
            head_sha: meta.head_sha.clone(),
            files: HashMap::new(),
        };
        let (status_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        Self::new(
            meta,
            diffs,
            threads,
            session,
            PathBuf::new(),
            String::new(),
            String::new(),
            String::new(),
            status_tx,
        )
    }

    /// Test-only accessor: index into `diffs` for the file under the
    /// file-panel cursor (mirrors the private `selected_idx`).
    #[cfg(test)]
    pub fn selected_file_idx(&self) -> Option<usize> {
        self.selected_idx()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffLine, FileDiff, Hunk};
    use crate::github::{CommentSide, CommentThread, PrFile, PrMetadata, ReviewComment};

    fn meta(files: &[&str]) -> PrMetadata {
        PrMetadata {
            pr_number: 1,
            node_id: "PR_test".into(),
            title: "Test PR".into(),
            base_branch: "main".into(),
            base_sha: "base_sha".into(),
            head_branch: "feature".into(),
            head_sha: "head_sha".into(),
            files: files
                .iter()
                .map(|p| PrFile {
                    path: (*p).to_owned(),
                    previous_path: None,
                    status: "modified".into(),
                    viewer_viewed_state: "UNVIEWED".into(),
                })
                .collect(),
            pending_review_id: None,
            state: "OPEN".into(),
            is_draft: false,
            body: String::new(),
            conversation: Vec::new(),
            viewer_login: "viewer".into(),
        }
    }

    fn diff(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_owned(),
            previous_path: None,
            hunks: vec![Hunk {
                header: "@@ -1,3 +1,3 @@".to_owned(),
                lines: vec![
                    DiffLine::Context("ctx 1\n".into()),
                    DiffLine::Removed("old\n".into()),
                    DiffLine::Added("new\n".into()),
                    DiffLine::Context("ctx 2\n".into()),
                ],
            }],
            added: 1,
            removed: 1,
        }
    }

    fn thread(id: &str, path: &str, line: u32, body: &str) -> CommentThread {
        CommentThread {
            id: id.to_owned(),
            path: path.to_owned(),
            side: CommentSide::Head,
            line,
            is_resolved: false,
            is_outdated: false,
            viewer_can_resolve: true,
            viewer_can_unresolve: false,
            comments: vec![ReviewComment {
                id: format!("{id}-c0"),
                author: "alice".into(),
                body: body.into(),
                created_at: "2026-04-26 10:00".into(),
                is_pending: false,
                viewer_did_author: false,
            }],
        }
    }

    fn state(files: &[&str], threads: Vec<CommentThread>) -> ReviewState {
        let m = meta(files);
        let diffs: Vec<FileDiff> = files.iter().map(|p| diff(p)).collect();
        ReviewState::for_test(m, diffs, threads)
    }

    #[test]
    fn cursor_lands_on_first_file_at_startup() {
        let s = state(&["a.rs", "b.rs"], vec![]);
        assert_eq!(s.selected_file_idx(), Some(0));
    }

    #[test]
    fn j_moves_down_through_visible_rows() {
        let mut s = state(&["a.rs", "b.rs"], vec![]);
        let before = s.list_state.selected();
        apply_key(&mut s, KeyCode::Char('j'));
        let after = s.list_state.selected();
        assert!(after > before, "j should advance the file panel cursor");
    }

    #[test]
    fn k_moves_up_and_clamps_at_zero() {
        let mut s = state(&["a.rs", "b.rs"], vec![]);
        apply_key(&mut s, KeyCode::Char('k'));
        assert_eq!(s.list_state.selected(), Some(0));
    }

    #[test]
    fn space_toggles_folder_collapse() {
        // Two files in the same folder so a folder row exists.
        let mut s = state(&["src/a.rs", "src/b.rs"], vec![]);
        // Cursor lands on the first file row, not the folder. Move up onto the folder.
        apply_key(&mut s, KeyCode::Char('k'));
        assert!(s.cursor_on_folder(), "expected cursor on the src/ folder");
        let visible_before = s.visible_rows.len();
        apply_key(&mut s, KeyCode::Char(' '));
        let visible_after = s.visible_rows.len();
        assert!(
            visible_after < visible_before,
            "collapsing the folder should hide its children: {visible_before} -> {visible_after}"
        );
        apply_key(&mut s, KeyCode::Char(' '));
        assert_eq!(s.visible_rows.len(), visible_before, "second toggle re-expands");
    }

    #[test]
    fn focus_keys_jump_to_panels() {
        let mut s = state(&["a.rs"], vec![]);
        apply_key(&mut s, KeyCode::Char('2'));
        assert_eq!(s.focus, Focus::Base);
        apply_key(&mut s, KeyCode::Char('3'));
        assert_eq!(s.focus, Focus::Head);
        apply_key(&mut s, KeyCode::Char('1'));
        assert_eq!(s.focus, Focus::Files);
    }

    #[test]
    fn tab_cycles_focus_forward() {
        let mut s = state(&["a.rs"], vec![]);
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Base);
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Head);
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Files);
    }

    #[test]
    fn shift_tab_cycles_focus_backward() {
        let mut s = state(&["a.rs"], vec![]);
        apply_key(&mut s, KeyCode::BackTab);
        assert_eq!(s.focus, Focus::Head);
    }

    #[test]
    fn q_signals_quit() {
        let mut s = state(&["a.rs"], vec![]);
        let quit = apply_key(&mut s, KeyCode::Char('q'));
        assert!(quit);
    }

    #[test]
    fn enter_toggles_thread_when_cursor_on_it() {
        let t = thread("T1", "a.rs", 1, "hello");
        let mut s = state(&["a.rs"], vec![t]);
        // Move into the diff pane and onto the thread row.
        apply_key(&mut s, KeyCode::Char('3'));
        // Walk down until we're on a thread row.
        let mut steps = 0;
        while !s.cursor_on_thread() && steps < 50 {
            apply_key(&mut s, KeyCode::Char('j'));
            steps += 1;
        }
        assert!(s.cursor_on_thread(), "expected to find a thread row by scrolling");
        let i = s.selected_file_idx().unwrap();
        let rows_before = s.laid[i].rows.len();
        apply_key(&mut s, KeyCode::Enter);
        let rows_after = s.laid[i].rows.len();
        assert!(
            rows_after > rows_before,
            "expanding a collapsed thread should add rows: {rows_before} -> {rows_after}"
        );
    }

    /// Render the TUI to an in-memory `TestBackend` and return one `String` per row.
    fn render_to_lines(state: &mut ReviewState, w: u16, h: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut lines = Vec::with_capacity(buffer.area.height as usize);
        for y in 0..buffer.area.height {
            let mut line = String::new();
            for x in 0..buffer.area.width {
                line.push_str(
                    buffer[(x, y)]
                        .symbol(),
                );
            }
            lines.push(line.trim_end().to_owned());
        }
        lines
    }

    #[test]
    fn file_row_shows_unresolved_comment_count() {
        let t = thread("T1", "a.rs", 1, "needs work");
        let mut s = state(&["a.rs", "b.rs"], vec![t]);
        let lines = render_to_lines(&mut s, 60, 12);
        // Find the row that contains a.rs's filename. Wide emoji + `1` may
        // render with a filler cell between, so just assert both are present.
        let a_row = lines
            .iter()
            .find(|l| l.contains("a.rs"))
            .expect("a.rs row not found");
        assert!(
            a_row.contains("\u{1F4AC}") && a_row.contains('1'),
            "a.rs row should include the speech-bubble badge and `1`: {a_row:?}"
        );
        let b_row = lines
            .iter()
            .find(|l| l.contains("b.rs"))
            .expect("b.rs row not found");
        assert!(
            !b_row.contains("\u{1F4AC}"),
            "b.rs has no threads — no badge expected: {b_row:?}"
        );
    }

    #[test]
    fn file_row_omits_count_when_no_threads() {
        let mut s = state(&["a.rs"], vec![]);
        let lines = render_to_lines(&mut s, 60, 12);
        let joined = lines.join("\n");
        assert!(
            !joined.contains("\u{1F4AC}"),
            "no threads → no badge"
        );
    }

    #[test]
    fn pr_state_badge_resolves_correctly() {
        assert_eq!(super::pr_state_badge("OPEN", false).0, "OPEN");
        assert_eq!(super::pr_state_badge("OPEN", true).0, "DRAFT");
        assert_eq!(super::pr_state_badge("MERGED", false).0, "MERGED");
        assert_eq!(super::pr_state_badge("CLOSED", false).0, "CLOSED");
        assert_eq!(super::pr_state_badge("WAT", false).0, "?");
        // is_draft is meaningless when state != OPEN — we render the literal state.
        assert_eq!(super::pr_state_badge("MERGED", true).0, "MERGED");
    }

    #[test]
    fn render_shows_pr_state_badge_in_header() {
        let mut s = state(&["a.rs"], vec![]);
        let lines = render_to_lines(&mut s, 120, 20);
        assert!(
            lines[0].contains("OPEN"),
            "header should include the state badge: {:?}",
            lines[0]
        );
    }

    #[test]
    fn question_mark_toggles_description_panel() {
        let mut s = state(&["a.rs"], vec![]);
        // Stuff a body and a conversation comment so the page has content.
        s.meta.body = "Hello body line one.\nLine two.".into();
        s.meta.conversation.push(crate::github::ConversationComment {
            author: "alice".into(),
            body: "lgtm".into(),
            created_at: "2026-04-26 10:00".into(),
        });
        assert!(!s.show_description);
        apply_key(&mut s, KeyCode::Char('?'));
        assert!(s.show_description);

        let lines = render_to_lines(&mut s, 100, 20);
        let joined = lines.join("\n");
        assert!(joined.contains("Description"));
        assert!(joined.contains("Hello body line one"));
        assert!(joined.contains("Conversation"));
        assert!(joined.contains("@alice"));

        apply_key(&mut s, KeyCode::Char('?'));
        assert!(!s.show_description);
    }

    #[test]
    fn render_includes_pr_title_and_file_panel() {
        let mut s = state(&["src/a.rs", "src/b.rs"], vec![]);
        let lines = render_to_lines(&mut s, 120, 20);
        let joined = lines.join("\n");
        assert!(joined.contains("Test PR"), "header should contain PR title");
        assert!(joined.contains("FILES"), "files panel title");
        assert!(joined.contains("BASE"), "base pane title");
        assert!(joined.contains("HEAD"), "head pane title");
        assert!(joined.contains("src/"), "file tree should show the folder row");
    }

    #[test]
    fn enter_folds_folder_when_focus_is_files() {
        let mut s = state(&["src/a.rs", "src/b.rs"], vec![]);
        // Move cursor up onto the folder row.
        apply_key(&mut s, KeyCode::Char('k'));
        assert!(s.cursor_on_folder());
        let visible_before = s.visible_rows.len();
        apply_key(&mut s, KeyCode::Enter);
        assert!(
            s.visible_rows.len() < visible_before,
            "Enter on a folder row in Files focus should collapse it"
        );
    }

    #[test]
    fn enter_does_not_toggle_thread_when_focus_is_files() {
        // Even if the diff cursor would be on a thread row, Enter routes to
        // folder-toggle when focus is Files.
        let t = thread("T1", "a.rs", 1, "hi");
        let mut s = state(&["a.rs"], vec![t]);
        // Walk diff cursor onto a thread row, but stay focused on Files.
        let i = s.selected_file_idx().unwrap();
        let mut steps = 0;
        while s.cursor[i] < 50 && !s.cursor_on_thread() {
            // Manually advance diff cursor without changing focus.
            s.cursor[i] += 1;
            steps += 1;
            if steps > 50 {
                break;
            }
        }
        assert_eq!(s.focus, Focus::Files);
        let rows_before = s.laid[i].rows.len();
        apply_key(&mut s, KeyCode::Enter);
        assert_eq!(
            s.laid[i].rows.len(),
            rows_before,
            "Enter under Files focus must not expand a thread"
        );
    }

    #[test]
    fn capital_l_toggles_local_panel() {
        let mut s = state(&["a.rs"], vec![]);
        assert!(!s.local_panel, "local panel off by default");
        apply_key(&mut s, KeyCode::Char('L'));
        // We can't actually run `git show` against a fake worktree, so the
        // toggle still flips the boolean — the diff just stays None and the
        // pane shows the placeholder.
        assert!(s.local_panel, "L should turn the pane on");
        apply_key(&mut s, KeyCode::Char('L'));
        assert!(!s.local_panel, "L again should turn the pane off");
    }

    #[test]
    fn key_4_switches_focus_to_local_only_when_panel_on() {
        let mut s = state(&["a.rs"], vec![]);
        apply_key(&mut s, KeyCode::Char('4'));
        assert_ne!(s.focus, Focus::Local, "panel off — 4 is a no-op");
        apply_key(&mut s, KeyCode::Char('L'));
        apply_key(&mut s, KeyCode::Char('4'));
        assert_eq!(s.focus, Focus::Local, "panel on — 4 jumps to Local");
    }

    #[test]
    fn tab_includes_local_when_panel_on() {
        let mut s = state(&["a.rs"], vec![]);
        apply_key(&mut s, KeyCode::Char('L'));
        // Files -> Base -> Head -> Local -> Files
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Base);
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Head);
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Local);
        apply_key(&mut s, KeyCode::Tab);
        assert_eq!(s.focus, Focus::Files);
    }

    #[test]
    fn n_jumps_to_next_thread_across_files() {
        let mut s = state(
            &["a.rs", "b.rs"],
            vec![thread("T1", "b.rs", 1, "hi")],
        );
        // Cursor starts on file a.rs which has no threads.
        assert_eq!(s.selected_file_idx(), Some(0));
        apply_key(&mut s, KeyCode::Char('n'));
        assert_eq!(s.selected_file_idx(), Some(1), "n should jump to the file with the thread");
        assert!(s.cursor_on_thread(), "cursor should land on the thread row");
    }

    #[test]
    fn extract_suggestion_picks_first_block() {
        let body = "comment text\n```suggestion\nlet x = 1;\nlet y = 2;\n```\nmore text";
        assert_eq!(
            super::extract_suggestion(body).as_deref(),
            Some("let x = 1;\nlet y = 2;")
        );
    }

    #[test]
    fn extract_suggestion_returns_none_when_no_block() {
        assert!(super::extract_suggestion("just a regular comment").is_none());
    }

    #[test]
    fn extract_suggestion_ignores_plain_code_blocks() {
        let body = "```rust\nfn x() {}\n```";
        assert!(super::extract_suggestion(body).is_none());
    }

    #[test]
    fn arm_or_confirm_delete_requires_two_presses() {
        let mut s = state(&["a.rs"], vec![]);
        let id = "C1";
        assert!(!s.arm_or_confirm_delete(id), "first press should arm only");
        assert!(s.arm_or_confirm_delete(id), "second press confirms");
        assert!(
            !s.arm_or_confirm_delete(id),
            "third press re-arms (confirmation was consumed)"
        );
    }

    #[test]
    fn arm_or_confirm_delete_resets_on_different_comment() {
        let mut s = state(&["a.rs"], vec![]);
        s.arm_or_confirm_delete("C1");
        assert!(
            !s.arm_or_confirm_delete("C2"),
            "different comment id resets arming"
        );
    }

    #[test]
    fn current_thread_resolution_returns_none_when_cursor_not_on_thread() {
        let s = state(&["a.rs"], vec![]);
        assert!(s.current_thread_resolution().is_none());
    }

    #[test]
    fn apply_suggestion_replaces_target_line_in_file() {
        let s = state(&["a.rs"], vec![]);
        let dir = std::env::temp_dir();
        let path = dir.join(format!("prowler-suggest-test-{}.txt", std::process::id()));
        std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
        s.apply_suggestion(&path, 2, 2, "REPLACED").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert_eq!(result, "line1\nREPLACED\nline3\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn apply_suggestion_handles_multi_line_replacement() {
        let s = state(&["a.rs"], vec![]);
        let dir = std::env::temp_dir();
        let path = dir.join(format!("prowler-suggest-multi-{}.txt", std::process::id()));
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        s.apply_suggestion(&path, 2, 3, "X\nY\nZ").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert_eq!(result, "a\nX\nY\nZ\nd\n");
        std::fs::remove_file(&path).ok();
    }
}
