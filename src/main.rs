mod auth;
mod github;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use git2::Repository;

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
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Review { pr } => review(pr).await?,
    }
    Ok(())
}

async fn review(pr_number: u64) -> Result<()> {
    let repo = Repository::discover(".")
        .context("not inside a git repository (could not find .git)")?;

    let (owner, repo_name) = extract_owner_repo(&repo)?;
    let token = auth::resolve_token().context("could not resolve a GitHub token")?;
    let meta = github::fetch_pr(&token, &owner, &repo_name, pr_number).await?;

    println!("Title:      {}", meta.title);
    println!("Base:       {}", meta.base_branch);
    println!("Head SHA:   {}", meta.head_sha);
    println!("Files:      {}", meta.file_count);

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
