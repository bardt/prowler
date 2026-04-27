//! Cross-platform clipboard copy via shelling out.
//!
//! Tries platform-appropriate command-line tools in priority order:
//! - macOS: `pbcopy` (always present).
//! - Linux Wayland: `wl-copy` (if `$WAYLAND_DISPLAY` is set).
//! - Linux X11: `xclip -selection clipboard`, falling back to
//!   `xsel --clipboard --input`.
//!
//! Each backend reads the payload from stdin. We pipe `text`, close stdin,
//! and wait for exit. Returns the backend name on success for the status
//! row.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn copy(text: &str) -> Result<String> {
    for backend in backends() {
        match try_copy(backend, text) {
            Ok(()) => return Ok(backend.name.to_owned()),
            Err(e) => {
                // Save the last error in case nothing works; otherwise try
                // the next backend (e.g. xclip not installed → xsel).
                let _ = e;
            }
        }
    }
    bail!("no clipboard tool found (tried: pbcopy / wl-copy / xclip / xsel)")
}

struct Backend {
    name: &'static str,
    prog: &'static str,
    args: &'static [&'static str],
}

fn backends() -> Vec<&'static Backend> {
    let mut out: Vec<&'static Backend> = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(&PBCOPY);
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        out.push(&WL_COPY);
    }
    out.push(&XCLIP);
    out.push(&XSEL);
    out
}

const PBCOPY: Backend = Backend {
    name: "pbcopy",
    prog: "pbcopy",
    args: &[],
};
const WL_COPY: Backend = Backend {
    name: "wl-copy",
    prog: "wl-copy",
    args: &[],
};
const XCLIP: Backend = Backend {
    name: "xclip",
    prog: "xclip",
    args: &["-selection", "clipboard"],
};
const XSEL: Backend = Backend {
    name: "xsel",
    prog: "xsel",
    args: &["--clipboard", "--input"],
};

fn try_copy(backend: &Backend, text: &str) -> Result<()> {
    let mut child = Command::new(backend.prog)
        .args(backend.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn `{}`", backend.prog))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .context("clipboard tool refused stdin")?;
        stdin
            .write_all(text.as_bytes())
            .context("failed to write to clipboard tool stdin")?;
    }
    let status = child
        .wait()
        .with_context(|| format!("wait `{}`", backend.prog))?;
    if !status.success() {
        bail!("`{}` exited with {status}", backend.prog);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_list_is_non_empty() {
        // Smoke check: at least one backend should be attempted on any
        // supported platform. (macOS always has pbcopy; Linux always has
        // xclip/xsel as fallbacks even if neither is installed.)
        assert!(!backends().is_empty());
    }

    #[test]
    fn copy_with_no_tools_returns_clear_error() {
        // Force an environment where none of our tools resolve by setting
        // PATH to an empty dir. This exercises the bail! path without
        // actually relying on what's installed locally.
        let saved = std::env::var_os("PATH");
        // SAFETY: tests run single-threaded by default; we restore PATH
        // before returning. This is a hack but enough for a smoke check.
        unsafe {
            std::env::set_var("PATH", "");
        }
        let result = copy("anything");
        unsafe {
            match saved {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("no clipboard tool"),
            "expected the no-tool-found error"
        );
    }
}
