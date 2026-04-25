use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, DiffOp, TextDiff};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    pub hunks: Vec<Hunk>,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "text")]
pub enum DiffLine {
    Added(String),
    Removed(String),
    Context(String),
    Moved(String),
}

pub fn compute_diffs(
    repo_root: &Path,
    worktree_path: &Path,
    base_sha: &str,
    files: &[crate::github::PrFile],
) -> Result<Vec<FileDiff>> {
    files
        .iter()
        .map(|f| diff_file(repo_root, worktree_path, base_sha, f))
        .collect()
}

fn diff_file(
    repo_root: &Path,
    worktree_path: &Path,
    base_sha: &str,
    file: &crate::github::PrFile,
) -> Result<FileDiff> {
    let base_path = file.previous_path.as_deref().unwrap_or(&file.path);
    let base_content = if file.status == "added" {
        String::new()
    } else {
        base_content(repo_root, base_sha, base_path)?
    };
    let head_content = if file.status == "removed" {
        String::new()
    } else {
        head_content(worktree_path, &file.path)?
    };
    let mut diff = diff_texts(&file.path, &base_content, &head_content);
    diff.previous_path = file.previous_path.clone();
    Ok(diff)
}

fn base_content(repo_root: &Path, base_sha: &str, path: &str) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "show",
            &format!("{base_sha}:{path}"),
        ])
        .output()
        .context("failed to run `git show`")?;
    if !output.status.success() {
        anyhow::bail!(
            "`git show {base_sha}:{path}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn head_content(worktree_path: &Path, path: &str) -> Result<String> {
    let file_path = worktree_path.join(path);
    if !file_path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&file_path)
        .with_context(|| format!("failed to read {}", file_path.display()))
}

fn diff_texts(path: &str, old: &str, new: &str) -> FileDiff {
    let diff = TextDiff::from_lines(old, new);
    let mut hunks = Vec::new();
    let mut total_added = 0usize;
    let mut total_removed = 0usize;

    for group in diff.grouped_ops(3) {
        let mut lines = Vec::new();
        let mut added = 0usize;
        let mut removed = 0usize;

        for op in &group {
            for change in diff.iter_changes(op) {
                match change.tag() {
                    ChangeTag::Insert => {
                        lines.push(DiffLine::Added(change.value().to_owned()));
                        added += 1;
                    }
                    ChangeTag::Delete => {
                        lines.push(DiffLine::Removed(change.value().to_owned()));
                        removed += 1;
                    }
                    ChangeTag::Equal => {
                        lines.push(DiffLine::Context(change.value().to_owned()));
                    }
                }
            }
        }

        total_added += added;
        total_removed += removed;
        hunks.push(Hunk {
            header: hunk_header(&group),
            lines,
        });
    }

    FileDiff {
        path: path.to_owned(),
        previous_path: None,
        hunks,
        added: total_added,
        removed: total_removed,
    }
}

fn hunk_header(ops: &[DiffOp]) -> String {
    let old_start = ops.first().map(|op| op.old_range().start).unwrap_or(0);
    let old_end = ops.last().map(|op| op.old_range().end).unwrap_or(0);
    let new_start = ops.first().map(|op| op.new_range().start).unwrap_or(0);
    let new_end = ops.last().map(|op| op.new_range().end).unwrap_or(0);
    format!(
        "@@ -{},{} +{},{} @@",
        old_start + 1,
        old_end.saturating_sub(old_start),
        new_start + 1,
        new_end.saturating_sub(new_start),
    )
}
