mod dashboard;
mod diff_view;
mod editor;
mod file_tree;
mod review;
mod syntax;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::diff::FileDiff;
use crate::github::{CommentThread, PrMetadata};
use crate::session::Session;
use crate::tui::diff_view::Side;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

pub fn run(
    meta: PrMetadata,
    diffs: Vec<FileDiff>,
    threads: Vec<CommentThread>,
    session: Session,
    repo_root: PathBuf,
    token: String,
    owner: String,
    repo: String,
) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(
        &mut terminal,
        meta,
        diffs,
        threads,
        session,
        repo_root,
        token,
        owner,
        repo,
    );
    ratatui::restore();
    result
}

pub async fn run_dashboard(
    token: String,
    owner: String,
    repo: String,
    repo_root: PathBuf,
) -> Result<()> {
    let data = crate::github::fetch_dashboard(&token, &owner, &repo)
        .await
        .context("failed to fetch dashboard")?;
    let mut state = dashboard::DashboardState::new(data, owner.clone(), repo.clone(), &repo_root)?;

    let mut terminal = ratatui::init();
    let result = dashboard_loop(&mut terminal, &mut state, &token, &owner, &repo, &repo_root);
    ratatui::restore();
    result
}

fn dashboard_loop(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut dashboard::DashboardState,
    token: &str,
    owner: &str,
    repo: &str,
    repo_root: &Path,
) -> Result<()> {
    loop {
        terminal
            .draw(|frame| dashboard::render(frame, state))
            .context("failed to draw dashboard frame")?;

        if !event::poll(Duration::from_millis(250)).context("failed to poll terminal events")? {
            continue;
        }
        let Event::Key(key) = event::read().context("failed to read terminal event")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let Some(outcome) = state.apply_key(key.code) else {
            continue;
        };

        match outcome {
            dashboard::DashboardOutcome::Quit => return Ok(()),
            dashboard::DashboardOutcome::Refresh => {
                let res = tokio::task::block_in_place(|| {
                    Handle::current().block_on(crate::github::fetch_dashboard(token, owner, repo))
                });
                match res {
                    Ok(data) => {
                        state.set_data(data);
                        state.set_success("Refreshed");
                    }
                    Err(e) => state.set_error(format!("Refresh failed: {e:#}")),
                }
            }
            dashboard::DashboardOutcome::Open(pr) | dashboard::DashboardOutcome::OpenLocal(pr) => {
                let res = open_pr_review(terminal, token, owner, repo, repo_root, pr);
                if let Err(e) = res {
                    state.set_error(format!("Open #{pr} failed: {e:#}"));
                }
                if let Err(e) = state.reload_sessions() {
                    state.set_error(format!("Reload sessions: {e:#}"));
                }
            }
            dashboard::DashboardOutcome::Cleanup(pr) => {
                match cleanup_session(repo_root, pr) {
                    Ok(()) => state.set_success(format!("Cleaned up session #{pr}")),
                    Err(e) => state.set_error(format!("Cleanup #{pr} failed: {e:#}")),
                }
                if let Err(e) = state.reload_sessions() {
                    state.set_error(format!("Reload sessions: {e:#}"));
                }
            }
        }
    }
}

/// Fetch a PR, set up the worktree, and run the review event loop on the same
/// terminal as the dashboard. When the user quits the review (`q`), control
/// returns here.
fn open_pr_review(
    terminal: &mut ratatui::DefaultTerminal,
    token: &str,
    owner: &str,
    repo: &str,
    repo_root: &Path,
    pr_number: u64,
) -> Result<()> {
    let (mut meta, threads) = tokio::task::block_in_place(|| {
        Handle::current().block_on(crate::github::fetch_pr(token, owner, repo, pr_number))
    })?;

    let desired_path = crate::git::worktree_path(repo, pr_number, &meta.head_sha);
    let base_path = crate::git::base_worktree_path(repo, pr_number, &meta.base_sha);

    let session = Session::load(repo_root, pr_number)?;
    let reused = desired_path.exists();
    if !reused {
        crate::git::fetch_pr_head(repo_root, pr_number)?;
        crate::git::add_worktree(repo_root, &desired_path, &crate::git::pr_local_ref(pr_number))?;
    }
    crate::git::ensure_sha(repo_root, &meta.base_sha)?;
    if !base_path.exists() {
        crate::git::add_worktree(repo_root, &base_path, &meta.base_sha)?;
    }

    let renames = crate::git::detect_renames(repo_root, &meta.base_sha, &meta.head_sha)?;
    for file in &mut meta.files {
        if let Some(old) = renames.get(&file.path) {
            file.previous_path = Some(old.clone());
        }
    }

    let mut files = session.map(|s| s.files).unwrap_or_default();
    for pr_file in &meta.files {
        let github_state = pr_file.viewer_viewed_state.as_str();
        let local = files.get(&pr_file.path).copied();
        match (local, github_state) {
            (Some(crate::session::FileStatus::Skipped), _) => {}
            (_, "DISMISSED") => {
                files.insert(pr_file.path.clone(), crate::session::FileStatus::Dismissed);
            }
            (None, "VIEWED") => {
                files.insert(pr_file.path.clone(), crate::session::FileStatus::Viewed);
            }
            _ => {}
        }
    }
    let session = Session {
        pr_number,
        branch: meta.head_branch.clone(),
        worktree_path: desired_path.clone(),
        base_worktree_path: base_path.clone(),
        base_sha: meta.base_sha.clone(),
        head_sha: meta.head_sha.clone(),
        files,
    };
    session.save(repo_root)?;

    let diffs = crate::diff::compute_diffs(repo_root, &desired_path, &meta.base_sha, &meta.files)?;

    // Reuse the dashboard's terminal — the review event loop handles its own
    // editor handoffs internally.
    terminal.clear().ok();
    event_loop(
        terminal,
        meta,
        diffs,
        threads,
        session,
        repo_root.to_path_buf(),
        token.to_owned(),
        owner.to_owned(),
        repo.to_owned(),
    )?;
    terminal.clear().ok();
    Ok(())
}

fn cleanup_session(repo_root: &Path, pr_number: u64) -> Result<()> {
    let Some(s) = Session::load(repo_root, pr_number)? else {
        return Ok(());
    };
    if s.worktree_path.exists() {
        crate::git::remove_worktree(repo_root, &s.worktree_path)?;
    }
    if !s.base_worktree_path.as_os_str().is_empty() && s.base_worktree_path.exists() {
        crate::git::remove_worktree(repo_root, &s.base_worktree_path)?;
    }
    Session::delete(repo_root, pr_number)?;
    Ok(())
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    meta: PrMetadata,
    diffs: Vec<FileDiff>,
    threads: Vec<CommentThread>,
    session: Session,
    repo_root: PathBuf,
    token: String,
    owner: String,
    repo: String,
) -> Result<()> {
    let (status_tx, mut status_rx) = mpsc::unbounded_channel::<review::StatusMessage>();
    let mut state = review::ReviewState::new(
        meta, diffs, threads, session, repo_root, token, owner, repo, status_tx,
    );

    loop {
        // Drain any pending background status messages (e.g. async viewed-state
        // sync errors) before drawing.
        while let Ok(msg) = status_rx.try_recv() {
            state.set_status(msg.text, msg.kind);
        }

        terminal
            .draw(|frame| review::render(frame, &mut state))
            .context("failed to draw frame")?;

        if !event::poll(Duration::from_millis(250)).context("failed to poll terminal events")? {
            continue;
        }

        let Event::Key(key) = event::read().context("failed to read terminal event")? else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Side-effectful keys that need terminal/runtime access stay here;
        // everything else is handled by the pure `review::apply_key`.
        match key.code {
            KeyCode::Char('e') => open_in_editor(terminal, &mut state, Side::Head)?,
            KeyCode::Char('E') => open_in_editor(terminal, &mut state, Side::Base)?,
            KeyCode::Char('c') => post_comment(terminal, &mut state)?,
            KeyCode::Char('r') => reply_to_comment(terminal, &mut state)?,
            KeyCode::Char('S') => submit_review(terminal, &mut state)?,
            KeyCode::Char('o') => toggle_thread_resolved(&mut state)?,
            KeyCode::Char('M') => edit_own_comment(terminal, &mut state)?,
            KeyCode::Char('X') => delete_own_comment(&mut state)?,
            KeyCode::Char('a') => apply_suggestion(&mut state)?,
            other => {
                if review::apply_key(&mut state, other) {
                    return Ok(());
                }
            }
        }
    }
}

fn toggle_thread_resolved(state: &mut review::ReviewState) -> Result<()> {
    let Some((thread_id, was_resolved)) = state.current_thread_resolution() else {
        state.set_status(
            "No thread under cursor (or no permission)",
            review::StatusKind::Error,
        );
        return Ok(());
    };
    let target_resolved = !was_resolved;
    let token = state.token.clone();
    let owner = state.owner.clone();
    let repo = state.repo.clone();
    let pr_number = state.pr_number();
    let result = tokio::task::block_in_place(|| {
        Handle::current().block_on(async {
            crate::github::set_thread_resolved(&token, &thread_id, target_resolved).await?;
            crate::github::fetch_pr(&token, &owner, &repo, pr_number).await
        })
    });
    match result {
        Ok((meta, threads)) => {
            state.apply_refresh(meta, threads);
            state.set_status(
                if target_resolved {
                    "Thread resolved"
                } else {
                    "Thread reopened"
                },
                review::StatusKind::Success,
            );
        }
        Err(e) => {
            log_post_error(&format!(
                "[FAIL] resolve thread {thread_id} (target={target_resolved}): {e:#}\n"
            ));
            state.set_status(format!("Resolve failed: {e}"), review::StatusKind::Error);
        }
    }
    Ok(())
}

fn edit_own_comment(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut review::ReviewState,
) -> Result<()> {
    let Some((comment_id, body)) = state.current_own_comment() else {
        state.set_status("Cursor not on your comment", review::StatusKind::Error);
        return Ok(());
    };

    let prompt = format!(
        "# Editing your comment.\n\
         # Lines starting with `#` are ignored. Save & exit to update; abort to cancel.\n\n{body}\n"
    );

    ratatui::restore();
    let new_body = editor::compose(&prompt);
    *terminal = ratatui::init();
    terminal.clear().ok();

    let new_body = match new_body {
        Ok(b) if !b.is_empty() => b,
        _ => return Ok(()),
    };
    if new_body == body {
        state.set_status("Edit cancelled (no changes)", review::StatusKind::Success);
        return Ok(());
    }

    let token = state.token.clone();
    let owner = state.owner.clone();
    let repo = state.repo.clone();
    let pr_number = state.pr_number();
    let result = tokio::task::block_in_place(|| {
        Handle::current().block_on(async {
            crate::github::update_review_comment(&token, &comment_id, &new_body).await?;
            crate::github::fetch_pr(&token, &owner, &repo, pr_number).await
        })
    });
    match result {
        Ok((meta, threads)) => {
            state.apply_refresh(meta, threads);
            state.set_status("Comment updated", review::StatusKind::Success);
        }
        Err(e) => {
            log_post_error(&format!("[FAIL] edit comment {comment_id}: {e:#}\n"));
            state.set_status(format!("Edit failed: {e}"), review::StatusKind::Error);
        }
    }
    Ok(())
}

fn delete_own_comment(state: &mut review::ReviewState) -> Result<()> {
    let Some((comment_id, _)) = state.current_own_comment() else {
        state.set_status("Cursor not on your comment", review::StatusKind::Error);
        return Ok(());
    };
    if !state.arm_or_confirm_delete(&comment_id) {
        state.set_status(
            "Press X again to confirm delete",
            review::StatusKind::Error,
        );
        return Ok(());
    }
    let token = state.token.clone();
    let owner = state.owner.clone();
    let repo = state.repo.clone();
    let pr_number = state.pr_number();
    let result = tokio::task::block_in_place(|| {
        Handle::current().block_on(async {
            crate::github::delete_review_comment(&token, &comment_id).await?;
            crate::github::fetch_pr(&token, &owner, &repo, pr_number).await
        })
    });
    match result {
        Ok((meta, threads)) => {
            state.apply_refresh(meta, threads);
            state.set_status("Comment deleted", review::StatusKind::Success);
        }
        Err(e) => {
            log_post_error(&format!("[FAIL] delete comment {comment_id}: {e:#}\n"));
            state.set_status(format!("Delete failed: {e}"), review::StatusKind::Error);
        }
    }
    Ok(())
}

fn apply_suggestion(state: &mut review::ReviewState) -> Result<()> {
    let Some((suggestion, file, start_line, end_line)) = state.current_suggestion_target() else {
        state.set_status(
            "No suggestion at cursor (HEAD-side comments only)",
            review::StatusKind::Error,
        );
        return Ok(());
    };
    match state.apply_suggestion(&file, start_line, end_line, &suggestion) {
        Ok(()) => state.set_status(
            format!("Applied suggestion to {}", file.display()),
            review::StatusKind::Success,
        ),
        Err(e) => state.set_status(format!("Apply failed: {e:#}"), review::StatusKind::Error),
    }
    Ok(())
}

fn open_in_editor(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut review::ReviewState,
    side: Side,
) -> Result<()> {
    let Some(target) = state.editor_target(side) else {
        return Ok(());
    };
    ratatui::restore();
    let result = editor::open(&target.file, target.line);
    *terminal = ratatui::init();
    terminal.clear().ok();
    match result {
        Err(e) => {
            state.set_status(format!("Editor failed: {e}"), review::StatusKind::Error);
        }
        Ok(()) => {
            state.refresh_after_edit(side);
            if matches!(side, Side::Head) {
                let hint = if state.local_panel_visible() {
                    "Diff refreshed"
                } else {
                    "Diff refreshed — press L to see your local edits"
                };
                state.set_status(hint, review::StatusKind::Success);
            }
        }
    }
    Ok(())
}

fn post_comment(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut review::ReviewState,
) -> Result<()> {
    let Some((path, side, line)) = state.comment_target() else {
        return Ok(());
    };
    let side_label = match side {
        crate::github::CommentSide::Base => "BASE",
        crate::github::CommentSide::Head => "HEAD",
    };
    let prompt = format!(
        "# Posting comment on `{path}` line {line} ({side_label}).\n\
         # Lines starting with `#` are ignored. Save and exit to post; abort the editor to cancel.\n"
    );

    ratatui::restore();
    let body = editor::compose(&prompt);
    *terminal = ratatui::init();
    terminal.clear().ok();

    let body = match body {
        Ok(b) if !b.is_empty() => b,
        Ok(_) => return Ok(()), // empty → silent cancel
        Err(_) => return Ok(()), // editor exited non-zero → silent cancel
    };

    let pr_node_id = state.pr_node_id().to_owned();
    let token = state.token.clone();
    let owner = state.owner.clone();
    let repo = state.repo.clone();
    let pr_number = state.pr_number();
    let path_for_post = path.clone();

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            crate::github::post_thread(&token, &pr_node_id, &path_for_post, side, line, &body)
                .await?;
            crate::github::fetch_pr(&token, &owner, &repo, pr_number).await
        })
    });

    match result {
        Ok((meta, threads)) => {
            state.apply_refresh(meta, threads);
            state.set_status(
                format!("Comment posted on {path}:{line}"),
                review::StatusKind::Success,
            );
        }
        Err(e) => {
            log_post_error(&format!(
                "[FAIL] PR #{pr_number} {path}:{line} {side_label}: {e:#}\n"
            ));
            state.set_status(format!("Post failed: {e}"), review::StatusKind::Error);
        }
    }
    Ok(())
}

fn reply_to_comment(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut review::ReviewState,
) -> Result<()> {
    let Some(thread_id) = state.reply_target() else {
        return Ok(());
    };
    let prompt = "# Replying to thread.\n# Lines starting with `#` are ignored. Save and exit to post; abort the editor to cancel.\n";

    ratatui::restore();
    let body = editor::compose(prompt);
    *terminal = ratatui::init();
    terminal.clear().ok();

    let body = match body {
        Ok(b) if !b.is_empty() => b,
        Ok(_) | Err(_) => return Ok(()),
    };

    let token = state.token.clone();
    let owner = state.owner.clone();
    let repo = state.repo.clone();
    let pr_number = state.pr_number();

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            crate::github::reply_to_thread(&token, &thread_id, &body).await?;
            crate::github::fetch_pr(&token, &owner, &repo, pr_number).await
        })
    });

    match result {
        Ok((meta, threads)) => {
            state.apply_refresh(meta, threads);
            state.set_status("Reply posted", review::StatusKind::Success);
        }
        Err(e) => {
            log_post_error(&format!(
                "[FAIL] reply PR #{pr_number} thread {thread_id}: {e:#}\n"
            ));
            state.set_status(format!("Reply failed: {e}"), review::StatusKind::Error);
        }
    }
    Ok(())
}

fn submit_review(
    terminal: &mut ratatui::DefaultTerminal,
    state: &mut review::ReviewState,
) -> Result<()> {
    let pending_id = state.pending_review_id().map(|s| s.to_owned());
    let pending_count = state.pending_comment_count();
    let pr_node_id = state.pr_node_id().to_owned();
    let pr_number = state.pr_number();

    let pending_summary = if pending_id.is_some() {
        format!(
            "# {pending_count} pending comment(s) will be published as part of this review."
        )
    } else {
        "# No pending review found — this will create a fresh, verdict-only review.".to_owned()
    };

    let prompt = format!(
        "# Submit review for PR #{pr_number}.\n\
         #\n\
         # First non-comment line: verdict — one of APPROVE, COMMENT, REQUEST_CHANGES.\n\
         # Lines after that: optional summary body.\n\
         {pending_summary}\n\
         #\n\
         # Save and exit to submit; abort the editor (e.g. `:cq`) to cancel.\n\
         \n\
         COMMENT\n\
         \n"
    );

    ratatui::restore();
    let body = editor::compose(&prompt);
    *terminal = ratatui::init();
    terminal.clear().ok();

    let buffer = match body {
        Ok(b) if !b.is_empty() => b,
        Ok(_) | Err(_) => return Ok(()),
    };

    let (event, body) = match parse_submit_buffer(&buffer) {
        Ok(parsed) => parsed,
        Err(e) => {
            log_post_error(&format!(
                "[FAIL] submit PR #{pr_number}: invalid buffer — {e:#}\n"
            ));
            return Ok(());
        }
    };

    let token = state.token.clone();
    let owner = state.owner.clone();
    let repo = state.repo.clone();

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            crate::github::submit_review(
                &token,
                &pr_node_id,
                pending_id.as_deref(),
                &event,
                &body,
            )
            .await?;
            crate::github::fetch_pr(&token, &owner, &repo, pr_number).await
        })
    });

    match result {
        Ok((meta, threads)) => {
            state.apply_refresh(meta, threads);
            state.set_status(
                format!("Review submitted: {event}"),
                review::StatusKind::Success,
            );
        }
        Err(e) => {
            log_post_error(&format!("[FAIL] submit PR #{pr_number}: {e:#}\n"));
            state.set_status(format!("Submit failed: {e}"), review::StatusKind::Error);
        }
    }
    Ok(())
}

/// Parse a submit-review compose buffer into `(event, body)`.
/// Strips `#` lines, takes the first non-empty remaining line as the verdict,
/// and joins the rest as the body.
fn parse_submit_buffer(text: &str) -> Result<(String, String)> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    let mut iter = lines.iter().skip_while(|l| l.trim().is_empty());
    let verdict_line = iter
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing verdict line"))?;
    let verdict = verdict_line.trim().to_uppercase();
    if !matches!(
        verdict.as_str(),
        "APPROVE" | "COMMENT" | "REQUEST_CHANGES"
    ) {
        anyhow::bail!(
            "invalid verdict `{verdict}` — expected APPROVE, COMMENT, or REQUEST_CHANGES"
        );
    }
    let body = iter.copied().collect::<Vec<_>>().join("\n").trim().to_owned();
    Ok((verdict, body))
}

fn log_post_error(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/prowler-sync.log")
    {
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}
