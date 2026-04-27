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
    /// True for hunks synthesised by `enrich_with_orphan_context` to give
    /// off-diff comment threads a row to attach to. The renderer dims the
    /// header text and tags it with "(outdated context)".
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_synthetic: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "text")]
pub enum DiffLine {
    Added(String),
    Removed(String),
    Context(String),
    Moved(String),
}

/// Compute the PR's base→head diff. Both sides are read from the git object
/// database — the worktree is intentionally NOT consulted, so local edits do
/// not leak into the "what the PR proposes" view. For HEAD→worktree (your
/// uncommitted edits on top of the PR), use [`compute_local_diffs`].
pub fn compute_pr_diffs(
    repo_root: &Path,
    base_sha: &str,
    head_sha: &str,
    files: &[crate::github::PrFile],
) -> Result<Vec<FileDiff>> {
    files
        .iter()
        .map(|f| diff_pr_file(repo_root, base_sha, head_sha, f))
        .collect()
}

fn diff_pr_file(
    repo_root: &Path,
    base_sha: &str,
    head_sha: &str,
    file: &crate::github::PrFile,
) -> Result<FileDiff> {
    let base_path = file.previous_path.as_deref().unwrap_or(&file.path);
    let base_content = if file.status == "added" {
        String::new()
    } else {
        read_blob(repo_root, base_sha, base_path)?
    };
    let head_content = if file.status == "removed" {
        String::new()
    } else {
        read_blob(repo_root, head_sha, &file.path)?
    };
    let mut diff = diff_texts(&file.path, &base_content, &head_content);
    diff.previous_path = file.previous_path.clone();
    Ok(diff)
}

/// Compute head→worktree diff for the HeadLocal mode. Old side is the PR's
/// HEAD blob (read from `head_sha`); new side is the worktree file. This is
/// where local edits should appear.
pub fn compute_local_diffs(
    repo_root: &Path,
    worktree_path: &Path,
    head_sha: &str,
    files: &[crate::github::PrFile],
) -> Result<Vec<FileDiff>> {
    files
        .iter()
        .map(|f| diff_local_file(repo_root, worktree_path, head_sha, f))
        .collect()
}

fn diff_local_file(
    repo_root: &Path,
    worktree_path: &Path,
    head_sha: &str,
    file: &crate::github::PrFile,
) -> Result<FileDiff> {
    // Tolerate a missing blob at head_sha (file added locally); tolerate a
    // missing worktree file (file removed locally). Either way an empty side
    // produces a clean additions/removals diff.
    let base_content = read_at_sha(repo_root, head_sha, &file.path).unwrap_or_default();
    let head_content = read_worktree(worktree_path, &file.path)?;
    let mut diff = diff_texts(&file.path, &base_content, &head_content);
    diff.previous_path = file.previous_path.clone();
    Ok(diff)
}

fn read_blob(repo_root: &Path, sha: &str, path: &str) -> Result<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "show",
            &format!("{sha}:{path}"),
        ])
        .output()
        .context("failed to run `git show`")?;
    if !output.status.success() {
        anyhow::bail!(
            "`git show {sha}:{path}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn read_worktree(worktree_path: &Path, path: &str) -> Result<String> {
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
            is_synthetic: false,
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

/// Append synthetic context hunks for review threads whose anchor line isn't
/// inside any rendered hunk. Without this, the badge on the file panel says
/// "💬 N" but the diff has nowhere to show the thread.
///
/// Strategy: pull `±CONTEXT` lines around the anchor from the appropriate SHA
/// (`base_sha` for BASE-side threads, `head_sha` for HEAD-side), and append
/// them as a hunk with `is_synthetic = true`. The renderer tags these so
/// users see "(outdated context)" or similar.
///
/// Idempotent: existing synthetic hunks are stripped before re-enrichment, so
/// it's safe to re-run on every `set_threads`.
pub fn enrich_with_orphan_context(
    repo_root: &Path,
    base_sha: &str,
    head_sha: &str,
    diffs: &mut [FileDiff],
    threads_by_file: &[Vec<crate::github::CommentThread>],
) {
    use crate::github::CommentSide;
    use std::collections::BTreeSet;

    const CONTEXT: u32 = 3;

    for (file_idx, file_threads) in threads_by_file.iter().enumerate() {
        let Some(diff) = diffs.get_mut(file_idx) else {
            continue;
        };
        // Strip previous synthetic hunks before recomputing.
        diff.hunks.retain(|h| !h.is_synthetic);

        let (base_set, head_set) = collect_hunk_lines(&diff.hunks);

        // Group orphan anchors so we don't synthesise overlapping windows for
        // multiple threads on the same line.
        let mut orphans: BTreeSet<(u8, u32)> = BTreeSet::new();
        for thread in file_threads {
            let in_diff = match thread.side {
                CommentSide::Base => base_set.contains(&thread.line),
                CommentSide::Head => head_set.contains(&thread.line),
            };
            if !in_diff {
                let side_key = match thread.side {
                    CommentSide::Base => 0,
                    CommentSide::Head => 1,
                };
                orphans.insert((side_key, thread.line));
            }
        }
        if orphans.is_empty() {
            continue;
        }

        let base_text = read_at_sha(repo_root, base_sha, &diff.path);
        let head_text = read_at_sha(repo_root, head_sha, &diff.path);

        for (side_key, anchor) in orphans {
            let (text, side) = match side_key {
                0 => (&base_text, CommentSide::Base),
                _ => (&head_text, CommentSide::Head),
            };
            if let Some(hunk) = synthesise_orphan_hunk(text.as_deref(), anchor, side, CONTEXT) {
                diff.hunks.push(hunk);
            }
        }
    }
}

fn collect_hunk_lines(
    hunks: &[Hunk],
) -> (
    std::collections::HashSet<u32>,
    std::collections::HashSet<u32>,
) {
    use std::collections::HashSet;
    let mut base = HashSet::new();
    let mut head = HashSet::new();
    for hunk in hunks {
        let Some((mut old_line, mut new_line)) = parse_hunk_header(&hunk.header) else {
            continue;
        };
        for line in &hunk.lines {
            match line {
                DiffLine::Context(_) | DiffLine::Moved(_) => {
                    base.insert(old_line);
                    head.insert(new_line);
                    old_line += 1;
                    new_line += 1;
                }
                DiffLine::Removed(_) => {
                    base.insert(old_line);
                    old_line += 1;
                }
                DiffLine::Added(_) => {
                    head.insert(new_line);
                    new_line += 1;
                }
            }
        }
    }
    (base, head)
}

/// Parse `@@ -X,Y +A,B @@` → `(X, A)` (the starting old / new line numbers).
pub fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let rest = header.strip_prefix("@@ -")?;
    let (old_part, after) = rest.split_once(' ')?;
    let new_part = after.strip_prefix('+')?.split(' ').next()?;
    let old_start: u32 = old_part.split(',').next()?.parse().ok()?;
    let new_start: u32 = new_part.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

fn read_at_sha(repo_root: &Path, sha: &str, path: &str) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "show",
            &format!("{sha}:{path}"),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn synthesise_orphan_hunk(
    text: Option<&str>,
    anchor_line: u32,
    side: crate::github::CommentSide,
    context: u32,
) -> Option<Hunk> {
    use crate::github::CommentSide;

    let header = format!(
        "@@ -{anchor_line},1 +{anchor_line},1 @@ {}",
        match side {
            CommentSide::Base => "outdated context",
            CommentSide::Head => "off-diff context",
        }
    );

    let placeholder = || Hunk {
        header: header.clone(),
        is_synthetic: true,
        lines: vec![DiffLine::Context(format!(
            "(line {anchor_line} not available at this SHA)\n"
        ))],
    };

    let Some(text) = text else {
        return Some(placeholder());
    };
    let lines: Vec<&str> = text.split('\n').collect();
    if anchor_line == 0 || (anchor_line as usize) > lines.len() {
        return Some(placeholder());
    }
    let anchor_idx = (anchor_line - 1) as usize;
    let start = anchor_idx.saturating_sub(context as usize);
    let end = (anchor_idx + context as usize + 1).min(lines.len());
    let start_line = (start + 1) as u32;
    let count = (end - start) as u32;

    let header = format!(
        "@@ -{start_line},{count} +{start_line},{count} @@ {}",
        match side {
            CommentSide::Base => "outdated context",
            CommentSide::Head => "off-diff context",
        }
    );

    let mut hunk_lines = Vec::with_capacity(count as usize);
    for line in &lines[start..end] {
        hunk_lines.push(DiffLine::Context(format!("{line}\n")));
    }
    Some(Hunk {
        header,
        is_synthetic: true,
        lines: hunk_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{CommentSide, CommentThread, ReviewComment};

    fn thread(line: u32, side: CommentSide) -> CommentThread {
        CommentThread {
            id: "T1".into(),
            path: "f".into(),
            side,
            line,
            is_resolved: false,
            is_outdated: true,
            viewer_can_resolve: false,
            viewer_can_unresolve: false,
            comments: vec![ReviewComment {
                id: "C1".into(),
                author: "alice".into(),
                body: "x".into(),
                created_at: "now".into(),
                is_pending: false,
                viewer_did_author: false,
            }],
        }
    }

    #[test]
    fn collect_hunk_lines_walks_old_and_new_counters() {
        let hunks = vec![Hunk {
            header: "@@ -10,3 +20,3 @@".into(),
            is_synthetic: false,
            lines: vec![
                DiffLine::Context("a\n".into()),
                DiffLine::Removed("b\n".into()),
                DiffLine::Added("c\n".into()),
                DiffLine::Context("d\n".into()),
            ],
        }];
        let (base, head) = collect_hunk_lines(&hunks);
        assert!(base.contains(&10) && base.contains(&11) && base.contains(&12));
        assert!(head.contains(&20) && head.contains(&21) && head.contains(&22));
    }

    #[test]
    fn synthesise_orphan_hunk_pulls_three_lines_around_anchor() {
        let text = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n";
        let hunk = synthesise_orphan_hunk(Some(text), 4, CommentSide::Base, 3).unwrap();
        assert_eq!(hunk.lines.len(), 7); // all 7 lines fit within ±3 of line 4
        assert!(hunk.is_synthetic);
        assert!(hunk.header.contains("outdated context"));
    }

    #[test]
    fn synthesise_orphan_hunk_returns_placeholder_when_text_missing() {
        let hunk = synthesise_orphan_hunk(None, 4, CommentSide::Base, 3).unwrap();
        assert!(hunk.is_synthetic);
        assert_eq!(hunk.lines.len(), 1);
    }

    #[test]
    fn enrich_strips_existing_synthetic_hunks() {
        let mut diffs = vec![FileDiff {
            path: "f".into(),
            previous_path: None,
            hunks: vec![
                Hunk {
                    header: "@@ -1,1 +1,1 @@".into(),
                    is_synthetic: false,
                    lines: vec![DiffLine::Context("real\n".into())],
                },
                Hunk {
                    header: "@@ -50,1 +50,1 @@ outdated context".into(),
                    is_synthetic: true,
                    lines: vec![DiffLine::Context("stale\n".into())],
                },
            ],
            added: 0,
            removed: 0,
        }];
        // No threads = no orphans = synthetic hunk should be stripped.
        let tmp = std::env::temp_dir();
        enrich_with_orphan_context(&tmp, "deadbeef", "deadbeef", &mut diffs, &[vec![]]);
        assert_eq!(diffs[0].hunks.len(), 1);
        assert!(!diffs[0].hunks[0].is_synthetic);
    }

    #[test]
    fn enrich_skips_threads_already_anchored_inside_a_hunk() {
        let mut diffs = vec![FileDiff {
            path: "f".into(),
            previous_path: None,
            hunks: vec![Hunk {
                header: "@@ -1,3 +1,3 @@".into(),
                is_synthetic: false,
                lines: vec![
                    DiffLine::Context("a\n".into()),
                    DiffLine::Context("b\n".into()),
                    DiffLine::Context("c\n".into()),
                ],
            }],
            added: 0,
            removed: 0,
        }];
        let tx = vec![vec![thread(2, CommentSide::Head)]];
        let tmp = std::env::temp_dir();
        enrich_with_orphan_context(&tmp, "x", "x", &mut diffs, &tx);
        // Thread is inside the hunk, so no synthetic hunk should be added.
        assert_eq!(diffs[0].hunks.len(), 1);
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
