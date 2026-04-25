use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prune stale worktree metadata left behind after `/tmp` entries are cleared.
pub fn prune_worktrees(repo_root: &Path) -> Result<()> {
    let status = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "worktree", "prune"])
        .status()
        .context("failed to run `git worktree prune`")?;
    if !status.success() {
        bail!("`git worktree prune` failed");
    }
    Ok(())
}

/// Canonical worktree path for a PR: `/tmp/prowler-{repo}-{pr}-{short_sha}`.
pub fn worktree_path(repo_name: &str, pr_number: u64, head_sha: &str) -> PathBuf {
    let short_sha = &head_sha[..head_sha.len().min(7)];
    PathBuf::from(format!("/tmp/prowler-{repo_name}-{pr_number}-{short_sha}"))
}

/// Fetch the PR head ref so the branch is available for worktree creation.
pub fn fetch_pr_head(repo_root: &Path, pr_number: u64) -> Result<()> {
    let refspec = format!("refs/pull/{pr_number}/head:refs/prowler/pr/{pr_number}");
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "fetch",
            "origin",
            &refspec,
        ])
        .output()
        .context("failed to run `git fetch`")?;

    if !output.status.success() {
        bail!(
            "`git fetch` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Ensure a commit SHA is locally available, fetching it from origin if needed.
pub fn ensure_sha(repo_root: &Path, sha: &str) -> Result<()> {
    let exists = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "cat-file",
            "-e",
            &format!("{sha}^{{commit}}"),
        ])
        .status()
        .context("failed to run `git cat-file`")?;
    if exists.success() {
        return Ok(());
    }

    let output = Command::new("git")
        .args(["-C", &repo_root.to_string_lossy(), "fetch", "origin", sha])
        .output()
        .context("failed to run `git fetch` for base SHA")?;
    if !output.status.success() {
        bail!(
            "`git fetch origin {sha}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Local ref written by `fetch_pr_head`.
pub fn pr_local_ref(pr_number: u64) -> String {
    format!("refs/prowler/pr/{pr_number}")
}

/// Create a new worktree at `path` checked out to `ref_or_branch`.
pub fn add_worktree(repo_root: &Path, path: &Path, ref_or_branch: &str) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "worktree",
            "add",
            &path.to_string_lossy(),
            ref_or_branch,
        ])
        .output()
        .with_context(|| format!("failed to run `git worktree add` for {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "`git worktree add` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Remove a worktree by path, discarding any local changes.
pub fn remove_worktree(repo_root: &Path, path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "worktree",
            "remove",
            "--force",
            &path.to_string_lossy(),
        ])
        .output()
        .with_context(|| format!("failed to run `git worktree remove` for {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "`git worktree remove` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
