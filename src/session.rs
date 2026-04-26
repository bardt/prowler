use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    #[default]
    Unviewed,
    InProgress,
    Viewed,
    Skipped,
    /// You marked this file viewed earlier, but GitHub auto-cleared the mark
    /// because the head SHA changed. Functionally equivalent to Unviewed for
    /// keybind behaviour, but rendered distinctly so you notice.
    Dismissed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub pr_number: u64,
    pub branch: String,
    pub worktree_path: PathBuf,
    #[serde(default)]
    pub base_worktree_path: PathBuf,
    pub base_sha: String,
    pub head_sha: String,
    #[serde(default)]
    pub files: HashMap<String, FileStatus>,
    /// Hide threads where `is_resolved == true` from the diff layout. Defaults
    /// to false (show all). Toggled with `H` in the review TUI; persists across
    /// sessions so the choice sticks.
    #[serde(default)]
    pub hide_resolved: bool,
    /// Comment thread node IDs that are currently expanded. Anything not in
    /// here renders collapsed. Persisted so re-opening the PR keeps your
    /// in-progress reading position on long threads.
    #[serde(default)]
    pub expanded_threads: Vec<String>,
    /// Last cursor row per file, keyed by file path. Persisted so re-opening a
    /// PR returns the cursor to where you left off.
    #[serde(default)]
    pub cursors: HashMap<String, u16>,
}

impl Session {
    pub fn load(repo_root: &Path, pr_number: u64) -> Result<Option<Session>> {
        let path = state_path(repo_root, pr_number);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let session = toml::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(Some(session))
    }

    pub fn save(&self, repo_root: &Path) -> Result<()> {
        let path = state_path(repo_root, self.pr_number);
        std::fs::create_dir_all(path.parent().unwrap())
            .with_context(|| format!("failed to create session directory for PR #{}", self.pr_number))?;
        let text = toml::to_string_pretty(self).context("failed to serialize session")?;
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn delete(repo_root: &Path, pr_number: u64) -> Result<()> {
        let path = state_path(repo_root, pr_number);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to delete {}", path.display()))?;
        }
        Ok(())
    }
}

/// Ensure `.review/` is listed in `.git/info/exclude` so session state is
/// never accidentally committed.
pub fn ensure_excluded(repo_root: &Path) -> Result<()> {
    let exclude_path = repo_root.join(".git").join("info").join("exclude");
    std::fs::create_dir_all(exclude_path.parent().unwrap())
        .context("failed to create .git/info directory")?;

    let existing = if exclude_path.exists() {
        std::fs::read_to_string(&exclude_path).context("failed to read .git/info/exclude")?
    } else {
        String::new()
    };

    if existing.lines().any(|l| l.trim() == ".review/") {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(".review/\n");
    std::fs::write(&exclude_path, updated).context("failed to write .git/info/exclude")?;
    Ok(())
}

fn state_path(repo_root: &Path, pr_number: u64) -> PathBuf {
    repo_root
        .join(".review")
        .join("sessions")
        .join(pr_number.to_string())
        .join("state.toml")
}
