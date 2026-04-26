mod auth;
mod config;
mod diff;
mod git;
mod github;
mod session;
mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use git2::Repository;
use session::Session;

#[derive(Parser)]
#[command(name = "prowler", about = "Terminal UI for GitHub PR review")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Review a pull request (default mode if --pr is supplied)
    Review {
        #[arg(long)]
        pr: u64,
        /// Remove the worktree and session for this PR
        #[arg(long)]
        cleanup: bool,
        /// Emit diff as JSON instead of setting up the worktree interactively
        #[arg(long)]
        json: bool,
    },
    /// Open the PR dashboard for the current repo (default if no subcommand)
    Dashboard,
}

#[tokio::main]
async fn main() -> Result<()> {
    config::init();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Dashboard) {
        Commands::Review { pr, cleanup, json } => review(pr, cleanup, json).await?,
        Commands::Dashboard => dashboard().await?,
    }
    Ok(())
}

async fn dashboard() -> Result<()> {
    let repo = Repository::discover(".")
        .context("not inside a git repository (could not find .git)")?;
    let repo_root = repo
        .workdir()
        .context("repository has no working directory")?
        .to_path_buf();
    let (owner, repo_name) = extract_owner_repo(&repo)?;

    git::prune_worktrees(&repo_root)?;
    session::ensure_excluded(&repo_root)?;

    let token = auth::resolve_token().context("could not resolve a GitHub token")?;

    tui::run_dashboard(token, owner, repo_name, repo_root).await
}

async fn review(pr_number: u64, cleanup: bool, json: bool) -> Result<()> {
    let repo = Repository::discover(".")
        .context("not inside a git repository (could not find .git)")?;

    let repo_root = repo
        .workdir()
        .context("repository has no working directory")?
        .to_path_buf();

    let (owner, repo_name) = extract_owner_repo(&repo)?;

    // Prune stale worktree metadata left from previous /tmp cleanups.
    git::prune_worktrees(&repo_root)?;

    if cleanup {
        return do_cleanup(&repo_root, pr_number);
    }

    session::ensure_excluded(&repo_root)?;

    let token = auth::resolve_token().context("could not resolve a GitHub token")?;
    let (mut meta, threads) = github::fetch_pr(&token, &owner, &repo_name, pr_number).await?;

    let desired_path = git::worktree_path(&repo_name, pr_number, &meta.head_sha);
    let base_path = git::base_worktree_path(&repo_name, pr_number, &meta.base_sha);
    let session = Session::load(&repo_root, pr_number)?;

    let reused = desired_path.exists();

    if !reused {
        git::fetch_pr_head(&repo_root, pr_number)?;
        git::add_worktree(&repo_root, &desired_path, &git::pr_local_ref(pr_number))?;
    }

    git::ensure_sha(&repo_root, &meta.base_sha)?;

    if !base_path.exists() {
        git::add_worktree(&repo_root, &base_path, &meta.base_sha)?;
    }

    // GraphQL doesn't expose previous_filename for renames — detect locally.
    let renames = git::detect_renames(&repo_root, &meta.base_sha, &meta.head_sha)?;
    for file in &mut meta.files {
        if let Some(old) = renames.get(&file.path) {
            file.previous_path = Some(old.clone());
        }
    }

    let hide_resolved = session
        .as_ref()
        .map(|s| s.hide_resolved)
        .unwrap_or(config::get().review.hide_resolved_default);
    let expanded_threads = session
        .as_ref()
        .map(|s| s.expanded_threads.clone())
        .unwrap_or_default();
    let cursors = session
        .as_ref()
        .map(|s| s.cursors.clone())
        .unwrap_or_default();
    let mut files = session.map(|s| s.files).unwrap_or_default();
    // Merge GitHub's per-viewer state with the local session map.
    //
    // - DISMISSED (GitHub auto-cleared a viewed mark because the head moved)
    //   wins over a local Viewed: the user should know their review is stale.
    //   It does NOT override Skipped — that's an explicit "not going to review".
    // - Otherwise the local state wins, since it can carry user actions whose
    //   sync to GitHub may not yet have completed.
    for pr_file in &meta.files {
        let github_state = pr_file.viewer_viewed_state.as_str();
        let local = files.get(&pr_file.path).copied();
        match (local, github_state) {
            (Some(session::FileStatus::Skipped), _) => {}
            (_, "DISMISSED") => {
                files.insert(pr_file.path.clone(), session::FileStatus::Dismissed);
            }
            (None, "VIEWED") => {
                files.insert(pr_file.path.clone(), session::FileStatus::Viewed);
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
        hide_resolved,
        expanded_threads,
        cursors,
    };
    session.save(&repo_root)?;

    let diffs = diff::compute_diffs(&repo_root, &desired_path, &meta.base_sha, &meta.files)?;

    if json {
        let output = serde_json::json!({
            "pr_number": pr_number,
            "title": meta.title,
            "base_branch": meta.base_branch,
            "base_sha": meta.base_sha,
            "head_sha": meta.head_sha,
            "worktree": desired_path,
            "base_worktree": base_path,
            "reused": reused,
            "files": diffs,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    tui::run(meta, diffs, threads, session, repo_root, token, owner, repo_name)
}

fn do_cleanup(repo_root: &std::path::Path, pr_number: u64) -> Result<()> {
    let session = Session::load(repo_root, pr_number)?;
    match session {
        None => {
            println!("No session found for PR #{pr_number}.");
        }
        Some(s) => {
            if s.worktree_path.exists() {
                git::remove_worktree(repo_root, &s.worktree_path)?;
                println!("Removed worktree at {}", s.worktree_path.display());
            } else {
                println!("Worktree path no longer exists, skipping removal.");
            }
            if !s.base_worktree_path.as_os_str().is_empty() && s.base_worktree_path.exists() {
                git::remove_worktree(repo_root, &s.base_worktree_path)?;
                println!(
                    "Removed base worktree at {}",
                    s.base_worktree_path.display()
                );
            }
            Session::delete(repo_root, pr_number)?;
            println!("Cleaned up session for PR #{pr_number}.");
        }
    }
    Ok(())
}

fn extract_owner_repo(repo: &Repository) -> Result<(String, String)> {
    let remote = repo
        .find_remote("origin")
        .context("no remote named `origin` found")?;
    let url = remote.url().context("origin remote has no URL")?;
    parse_github_url(url)
        .with_context(|| format!("could not parse owner/repo from origin URL: {url}"))
}

fn parse_github_url(url: &str) -> Option<(String, String)> {
    let url = url.strip_suffix('/').unwrap_or(url);
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return split_owner_repo(rest);
    }
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        return split_owner_repo(rest);
    }
    if let Some(rest) = url.strip_prefix("http://github.com/") {
        return split_owner_repo(rest);
    }
    None
}

fn split_owner_repo(path: &str) -> Option<(String, String)> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_owned(), repo.to_owned()))
}
