use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Spawn `$EDITOR` (default `vi`) on `file` at `line`, inheriting the terminal.
/// Uses the POSIX `+N` convention which is honoured by vim/nvim/nano/emacs.
pub fn open(file: &Path, line: u32) -> Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    let status = Command::new(&editor)
        .arg(format!("+{line}"))
        .arg(file)
        .status()
        .with_context(|| format!("failed to spawn editor `{editor}`"))?;

    if !status.success() {
        // Many editors exit non-zero on user actions (e.g. :cq). Don't propagate.
        eprintln!("editor `{editor}` exited with {}", status);
    }
    Ok(())
}
