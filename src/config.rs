use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::OnceLock;

/// User configuration loaded from `~/.config/prowler/config.toml` (or
/// `$XDG_CONFIG_HOME/prowler/config.toml`). Missing file → all defaults.
/// Missing fields → field defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub editor: EditorConfig,
    pub dashboard: DashboardConfig,
    pub review: ReviewConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Command to run on `e` / `E` and the compose buffers. Empty (default)
    /// falls back to `$VISUAL`, then `$EDITOR`, then `vi`.
    pub command: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DashboardConfig {
    /// `"current_repo"` (default) — scope dashboard search to the current
    /// repo. `"all"` — fetch PRs across all repos the viewer can see.
    pub scope: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            scope: "current_repo".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Default for `Session.hide_resolved` on a fresh session. Once a session
    /// is saved, this flag's local value wins.
    pub hide_resolved_default: bool,
    /// Background-poller cadence for new comments / threads.
    pub poll_interval_secs: u64,
    /// Two-step `X X` arming window (delete confirmation).
    pub confirm_delete_ttl_secs: u64,
    /// On `L` (toggle diff mode), sync cursor to the corresponding HEAD line
    /// in the target mode. False = each mode keeps its own cursor.
    pub cursor_sync_modes: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            hide_resolved_default: false,
            poll_interval_secs: 60,
            confirm_delete_ttl_secs: 3,
            cursor_sync_modes: false,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(cfg)
    }
}

/// Resolve where the config file lives. XDG_CONFIG_HOME wins; otherwise
/// `$HOME/.config/prowler/config.toml`. Falls back to `./prowler-config.toml`
/// only if neither is set (rare; used as a last resort, never auto-created).
pub fn path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("prowler/config.toml");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config/prowler/config.toml");
    }
    PathBuf::from("./prowler-config.toml")
}

/// Process-global config. Initialized once at startup; defaults if loading
/// fails so a malformed user config doesn't crash the binary.
static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn init() {
    let cfg = Config::load().unwrap_or_else(|e| {
        eprintln!("warning: failed to load config ({e:#}); using defaults");
        Config::default()
    });
    let _ = CONFIG.set(cfg);
}

pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_baked_in_constants() {
        let cfg = Config::default();
        assert_eq!(cfg.review.poll_interval_secs, 60);
        assert_eq!(cfg.review.confirm_delete_ttl_secs, 3);
        assert!(!cfg.review.hide_resolved_default);
        assert_eq!(cfg.dashboard.scope, "current_repo");
        assert_eq!(cfg.editor.command, "");
    }

    #[test]
    fn parses_partial_toml_with_field_defaults() {
        let toml_src = r#"
            [review]
            poll_interval_secs = 120
        "#;
        let cfg: Config = toml::from_str(toml_src).unwrap();
        assert_eq!(cfg.review.poll_interval_secs, 120);
        // unspecified fields fall back to defaults
        assert_eq!(cfg.review.confirm_delete_ttl_secs, 3);
        assert_eq!(cfg.dashboard.scope, "current_repo");
    }
}
