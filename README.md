# prowler

A terminal UI for deep GitHub PR review. Inspired by lazygit's UX
philosophy — review, edit, and suggest changes without leaving your shell.

## What it does

```sh
prowler                  # dashboard: PRs awaiting review, opened by you,
                         # assigned to you, plus local in-progress sessions

prowler review --pr 123  # review a specific PR
```

Inside a review:

- Side-by-side diff (BASE | HEAD) with syntax highlighting.
- Toggle to a HEAD | WORK view (`L`) to see your local edits as a diff
  against the PR's HEAD.
- Open either side in `$EDITOR` at the cursor line (`e` / `E`) with full
  LSP — your worktree is a real `git worktree`, not a sandbox.
- Read, post, reply to, edit, delete, and resolve inline comments.
- Apply ` ```suggestion ` blocks from comments to your worktree (`a`).
- Post your local hunks back as suggestion comments (`V` to select rows
  in the WORK column, then `c`).
- Submit your review with verdict + summary (`S`).
- Background poller notifies you when threads / comments arrive on
  GitHub while you're reviewing.

## Install

```sh
cargo install --git https://github.com/bardt/prowler
```

Requires a recent Rust toolchain. After install, the `prowler` binary
lives in `~/.cargo/bin/`.

## GitHub auth

Resolved at startup in priority order:

1. `GITHUB_TOKEN` env var.
2. `gh auth token` subprocess (the `gh` CLI must be installed and
   authenticated).

No credentials are stored by prowler.

## Configuration (optional)

Place a `config.toml` at `~/.config/prowler/config.toml` (or
`$XDG_CONFIG_HOME/prowler/config.toml`). All fields are optional; missing
ones use defaults.

See `docs/config-sample.toml` for the full schema. Common knobs:

```toml
[editor]
command = "nvim"      # default: $VISUAL → $EDITOR → vi

[dashboard]
scope = "current_repo"  # or "all"

[review]
hide_resolved_default = false
poll_interval_secs = 60
```

## Worktrees

prowler spins up a `git worktree` at `/tmp/prowler-{repo}-{pr}-{sha}` for
the PR head, plus another for the BASE side. Worktrees survive TUI quit
(your editor may still be open) and are reused on the next open. Press
`d` on a stale session in the dashboard to tear down a worktree.

## Keymap

Press `?` inside a review for the categorized keymap overlay.

Highlights:

| Key | Action |
|-----|--------|
| `j`/`k` `]`/`[` | Move cursor / next-prev hunk |
| `n`/`N` | Next/prev comment thread (cross-file) |
| `Tab` | Cycle panel focus |
| `e` / `E` | Open HEAD / BASE in `$EDITOR` |
| `c` | Comment (single line, multi-line if `V`-selection) |
| `V` | Visual-line selection (j/k extends, Esc cancels) |
| `r` `o` `M` `X X` `a` | Reply / resolve / edit / delete (×2) / apply-suggestion |
| `L` | Toggle PR diff ↔ Local diff |
| `H` `/` `?` `D` | Hide-resolved / file filter / help / description |
| `F5` / `Ctrl+R` | Re-fetch PR from GitHub |
| `S` | Submit review |
| `q` | Back to dashboard |

## Stack

- Rust 2024
- ratatui (TUI)
- octocrab (GitHub GraphQL)
- git2 (worktree lifecycle)
- similar (diffs)
- syntect (syntax highlighting)
- toml + serde (session/config persistence)
- tokio (GitHub API + background poller; never in the TUI event loop)

## Status

v1 release candidate. See `CLAUDE.md` for the full milestone breakdown.

## License

MIT.
