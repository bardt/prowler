use anyhow::{bail, Context, Result};
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

/// Open `$EDITOR` on a temp file seeded with `prompt` (commented header). Returns the
/// user's body with `#`-prefixed lines stripped and trimmed. Returns an empty string if
/// the user wrote nothing meaningful; bails if the editor exits non-zero (treated as cancel).
pub fn compose(prompt: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!("prowler-compose-{}.md", std::process::id()));
    let initial = format!("\n{prompt}");
    std::fs::write(&path, &initial)
        .with_context(|| format!("failed to seed compose buffer at {}", path.display()))?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to spawn editor `{editor}`"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&path);
        bail!("editor `{editor}` exited with {} — comment cancelled", status);
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read compose buffer at {}", path.display()))?;
    let _ = std::fs::remove_file(&path);

    let body = raw
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    Ok(body)
}
