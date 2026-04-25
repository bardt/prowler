mod auth;
mod git;
mod github;
mod session;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use git2::Repository;
use session::Session;

#[derive(Parser)]
#[command(name = "prowler", about = "Terminal UI for GitHub PR review")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Review a pull request
    Review {
        #[arg(long)]
        pr: u64,
        /// Remove the worktree and session for this PR
        #[arg(long)]
        cleanup: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Review { pr, cleanup } => review(pr, cleanup).await?,
    }
    Ok(())
}

async fn review(pr_number: u64, cleanup: bool) -> Result<()> {
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
    let meta = github::fetch_pr(&token, &owner, &repo_name, pr_number).await?;

    println!("Title:      {}", meta.title);
    println!("Base:       {}", meta.base_branch);
    println!("Head SHA:   {}", meta.head_sha);
    println!("Files:      {}", meta.file_count);

    let desired_path = git::worktree_path(&repo_name, pr_number, &meta.head_sha);

    let session = Session::load(&repo_root, pr_number)?;

    if desired_path.exists() {
        if session.is_none() {
            // Path exists but no session — save one so future runs track it.
            Session {
                pr_number,
                branch: meta.head_branch.clone(),
                worktree_path: desired_path.clone(),
                base_sha: meta.head_sha.clone(), // base SHA fetched in M3
                head_sha: meta.head_sha.clone(),
            }
            .save(&repo_root)?;
        }
        println!("Worktree:   {} (reused)", desired_path.display());
        return Ok(());
    }

    // Worktree doesn't exist — check if an old session points elsewhere (SHA changed).
    if let Some(old) = &session {
        if old.worktree_path.exists() {
            // A different worktree exists for another SHA; leave it alone.
            // git worktree prune will clean it up if the branch is gone.
        }
    }

    git::fetch_pr_head(&repo_root, pr_number)?;
    git::add_worktree(&repo_root, &desired_path, &git::pr_local_ref(pr_number))?;

    Session {
        pr_number,
        branch: meta.head_branch,
        worktree_path: desired_path.clone(),
        base_sha: meta.head_sha.clone(),
        head_sha: meta.head_sha,
    }
    .save(&repo_root)?;

    println!("Worktree:   {} (created)", desired_path.display());
    Ok(())
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
