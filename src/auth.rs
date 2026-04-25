use anyhow::{bail, Context, Result};
use std::process::Command;

pub fn resolve_token() -> Result<String> {
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // gh ≥ 2.7.0: `gh auth token`
    if let Some(token) = try_gh_auth_token() {
        return Ok(token);
    }

    // gh < 2.7.0: `gh auth status --show-token`
    gh_auth_status_token()
}

fn try_gh_auth_token() -> Option<String> {
    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if token.is_empty() { None } else { Some(token) }
}

fn gh_auth_status_token() -> Result<String> {
    let output = Command::new("gh")
        .args(["auth", "status", "--show-token"])
        .output()
        .context("failed to run `gh auth status` — is the gh CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`gh auth status` failed (exit {}): {}\nSet GITHUB_TOKEN or run `gh auth login` first.",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }

    // gh writes status output to stderr
    let text = String::from_utf8(output.stderr)
        .context("`gh auth status` output was not valid UTF-8")?;

    // Output contains a line like: "  ✓ Token: ghp_xxxx"
    text
        .lines()
        .find_map(|line| {
            let rest = line.split_once("Token:")?.1.trim();
            if rest.is_empty() { None } else { Some(rest.to_owned()) }
        })
        .context("could not find token in `gh auth status --show-token` output\nSet GITHUB_TOKEN or run `gh auth login` first.")
}
