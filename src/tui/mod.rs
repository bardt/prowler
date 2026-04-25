mod diff_view;
mod editor;
mod review;
mod syntax;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::path::PathBuf;
use std::time::Duration;

use crate::diff::FileDiff;
use crate::github::{CommentThread, PrMetadata};
use crate::session::Session;
use crate::tui::diff_view::Side;

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
    let mut state = review::ReviewState::new(
        meta, diffs, threads, session, repo_root, token, owner, repo,
    );

    loop {
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

        match key.code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Tab => state.cycle_focus(),
            KeyCode::BackTab => state.cycle_focus_back(),
            KeyCode::Char('1') => state.set_focus(review::Focus::Files),
            KeyCode::Char('2') => state.set_focus(review::Focus::Base),
            KeyCode::Char('3') => state.set_focus(review::Focus::Head),
            KeyCode::Char('j') | KeyCode::Down => state.move_down(),
            KeyCode::Char('k') | KeyCode::Up => state.move_up(),
            KeyCode::Char(']') => state.next_hunk(),
            KeyCode::Char('[') => state.prev_hunk(),
            KeyCode::Char('e') => open_in_editor(terminal, &state, Side::Head)?,
            KeyCode::Char('E') => open_in_editor(terminal, &state, Side::Base)?,
            KeyCode::Char('v') => state.toggle_viewed(),
            KeyCode::Char('s') => state.toggle_skipped(),
            KeyCode::Char('c') => post_comment(terminal, &mut state)?,
            KeyCode::Char('r') => reply_to_comment(terminal, &mut state)?,
            KeyCode::Char('S') => submit_review(terminal, &mut state)?,
            _ => {}
        }
    }
}

fn open_in_editor(
    terminal: &mut ratatui::DefaultTerminal,
    state: &review::ReviewState,
    side: Side,
) -> Result<()> {
    let Some(target) = state.editor_target(side) else {
        return Ok(());
    };
    ratatui::restore();
    let result = editor::open(&target.file, target.line);
    *terminal = ratatui::init();
    terminal.clear().ok();
    result
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
        Ok((meta, threads)) => state.apply_refresh(meta, threads),
        Err(e) => log_post_error(&format!(
            "[FAIL] PR #{pr_number} {path}:{line} {side_label}: {e:#}\n"
        )),
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
        Ok((meta, threads)) => state.apply_refresh(meta, threads),
        Err(e) => log_post_error(&format!(
            "[FAIL] reply PR #{pr_number} thread {thread_id}: {e:#}\n"
        )),
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
        Ok((meta, threads)) => state.apply_refresh(meta, threads),
        Err(e) => log_post_error(&format!("[FAIL] submit PR #{pr_number}: {e:#}\n")),
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
