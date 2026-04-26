use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

fn resolve_editor() -> String {
    let cfg = crate::config::get().editor.command.trim();
    if !cfg.is_empty() {
        return cfg.to_owned();
    }
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.is_empty() {
            return v;
        }
    }
    std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string())
}

/// Spawn the user's editor on `file` at `line`, inheriting the terminal.
/// Uses the POSIX `+N` convention which is honoured by vim/nvim/nano/emacs.
///
/// Resolution order: `[editor].command` from the config file, then `$VISUAL`,
/// then `$EDITOR`, then `vi` as a final fallback.
pub fn open(file: &Path, line: u32) -> Result<()> {
    let editor = resolve_editor();

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

    let editor = resolve_editor();
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
