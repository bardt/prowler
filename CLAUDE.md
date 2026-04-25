# prowler

A terminal UI for deep GitHub PR review. Inspired by lazygit's UX philosophy.

## What it does

`prowler review --pr 123` opens a full TUI for reviewing a PR:
- Side-by-side diff (Base | Head) with syntax highlighting
- File panel with lines added/removed per file, viewed state
- Toggleable Local diff panel (`git diff HEAD` in worktree)
- Open either side in `$EDITOR` at the current line with full LSP (`e` / `E`)
- Inline GitHub comments — read and post without leaving the TUI
- `prowler` with no args opens the dashboard (PRs opened, assigned, in-progress, stale)

The core differentiator: a `git worktree` is spun up for the PR branch so the reviewer
can edit freely without touching their current working branch. The worktree survives TUI
quit and is reused on the next open.

## Stack

- **Language:** Rust
- **TUI:** ratatui
- **Git:** git2
- **GitHub API:** octocrab (REST only in v1)
- **Diff:** similar
- **Syntax highlighting:** syntect
- **Config/state:** toml + serde
- **Async:** tokio (GitHub API calls only)
- **Args:** clap

## GitHub auth

Resolved at startup in priority order:
1. `GITHUB_TOKEN` env var
2. `gh auth token` subprocess

No credentials stored by the tool.

## Project structure

```
src/
  main.rs          # entry point, arg parsing (clap)
  auth.rs          # GitHub token resolution
  github.rs        # octocrab API calls
  git.rs           # git2 + worktree lifecycle
  diff.rs          # diff computation (similar) + data model
  session.rs       # session state (.review/ read/write)
  tui/
    mod.rs         # ratatui bootstrap, event loop
    dashboard.rs   # dashboard view
    review.rs      # review view (file panel + diff panels)
    diff.rs        # diff rendering, syntax highlighting
    comments.rs    # inline comment display + compose
```

## Session state

Stored in `.review/sessions/{pr_number}/` at repo root.
Added to `.git/info/exclude` automatically on first run.
Never committed.

Key files:
- `state.toml` — worktree path, SHAs, viewed file status, cursor
- `notes.toml` — local per-line notes (never posted to GitHub)

## Core data model

```rust
struct FileDiff { path: String, hunks: Vec<Hunk>, added: usize, removed: usize }
struct Hunk     { header: String, lines: Vec<DiffLine> }
enum DiffLine   { Added(String), Removed(String), Context(String), Moved(String) }

enum FileStatus { Unviewed, InProgress, Viewed, Skipped }

struct Session {
    pr_number: u64,
    branch: String,
    worktree_path: PathBuf,
    base_sha: String,
    head_sha: String,
    files: HashMap<String, FileStatus>,
}
```

## Worktree lifecycle

- First open: `git worktree add /tmp/prowler-{repo}-{pr}-{sha} {branch}`
- Subsequent opens: reuse if path exists, recreate if gone
- TUI quit: worktree stays alive (editor may still be open)
- Explicit teardown: `d` on stale session in dashboard
- Crash recovery: `git worktree prune` runs on startup

## Layout

**Wide (≥ 200 cols), local panel off:** FILES | BASE | HEAD  
**Wide (≥ 200 cols), local panel on:** FILES | BASE | HEAD | LOCAL  
**Narrow (< 200 cols), local panel on:** FILES | HEAD | LOCAL  

Toggle local panel with `L`.

## Key bindings

### Review view
| Key | Action |
|-----|--------|
| `j`/`k` | Scroll diff |
| `]`/`[` | Next/prev hunk |
| `Tab` | Cycle panel focus |
| `1`/`2`/`3`/`4` | Jump to panel |
| `e` | Open HEAD in $EDITOR at current line |
| `E` | Open BASE in $EDITOR at current line |
| `v` | Mark file viewed |
| `s` | Skip file |
| `c` | Post inline comment |
| `Enter` | Expand/collapse comment thread |
| `L` | Toggle local diff panel |
| `R` | Refresh local diff |
| `q` | Back to dashboard |

## Current milestone

**M11 — Local diff panel**

## Milestones overview

| # | Name | Status |
|---|------|--------|
| M1 | GitHub auth + PR metadata | ✅ |
| M2 | Worktree lifecycle | ✅ |
| M3 | Diff data model | ✅ |
| M4 | Basic TUI shell | ✅ |
| M5 | Diff rendering | ✅ |
| M6 | Editor handoff | ✅ |
| M7 | Viewed state + session persistence | ✅ |
| M8 | Inline comments (read) | ✅ |
| M9 | Inline comments (post) | ✅ |
| M10 | Submit review | ✅ |
| M11 | Local diff panel | 🔲 next |
| M12 | Dashboard | 🔲 |

**M10 scope:** A panel listing all comments in the current pending review, with a verdict
selector (Approve / Comment / Request changes) and an optional summary body. Submits via
GraphQL `submitPullRequestReview` (or creates the review with `addPullRequestReview` if
no pending one exists). Without this, M9's `c` accumulates Pending comments forever.

Update the **Current milestone** section and the status column above at the start of each
new milestone.

## Backlog

- **Pull GitHub viewed state on PR open.** Today the sync is push-only: `v` writes to
  GitHub via the `markFileAsViewed` GraphQL mutation, but we never read back. Open the
  same PR on a second machine (or after marking files in the web UI) and prowler shows
  everything as Unviewed. Fetch `pullRequest.files.nodes.viewerViewedState` (GraphQL,
  enum `VIEWED` / `DISMISSED` / `UNVIEWED`) at startup and seed `Session.files` from it.
  Note: `DISMISSED` means GitHub auto-cleared a viewed mark because the head changed —
  we'll likely want to surface that distinctly in the UI rather than treating it as
  Unviewed.
- **Render outdated comments.** `fetch_comments` drops comments whose `line` is null
  (GitHub marks them outdated when the head moves). They're still meaningful — show
  them somewhere (file-level pinned panel, or anchored to the original line on the
  base side) rather than silently discarding.
- **Wrap long comment bodies.** Comment body lines are rendered verbatim and overflow
  the pane width. Hard-wrap at the pane width (or use ratatui's `Wrap`) — needs a
  layout pass that knows the pane width.
- **Collapse/expand threads.** Spec calls for `Enter` to toggle a thread; today every
  thread is fully expanded. Add per-thread collapsed state, default to collapsed
  (one-line summary), expand the thread under the cursor on `Enter`.
- **Status feedback for synchronous posts (`c`, `r`, `e`, `E`).** Add `status:
  Option<Status>` to `ReviewState` with `{ text, kind: Success | Error | Info,
  expires_at: Instant }`. Render in the bottom row, replacing the hotkey footer
  while active. Auto-clear is free — the event loop already redraws every 250ms
  via the `event::poll` timeout, so checking `Instant::now() >= expires_at` on
  each draw works without extra plumbing. Wire `set_status()` calls at the end
  of `post_comment` / `reply_to_comment` / `open_in_editor`.
- **Status feedback for async syncs (`v`, `s`).** Viewed-state sync uses
  `tokio::spawn` fire-and-forget — errors only land in `/tmp/prowler-sync.log`.
  To surface them in the UI, add an `mpsc::UnboundedSender<StatusMessage>` to
  the spawned task and drain it from the event loop on each tick. Lower
  priority than the sync-post variant since it's lossier and rarer.
- **Cross-file comment navigation.** Add `gN` / `gP` (or similar) to jump to the
  next/prev comment thread across the whole PR — selects the file, scrolls cursor
  to the thread anchor. Today `]` / `[` only navigate hunks within the current file.

## Conventions

- No `.unwrap()` in library code — use `?` and `anyhow::Result`
- GitHub API errors should surface with context (which PR, which endpoint)
- Worktree paths use the pattern `/tmp/prowler-{repo}-{pr_number}-{short_sha}`
- All file I/O for session state goes through `session.rs` — never read/write `.review/`
  directly from other modules
- TUI code lives entirely under `tui/` — no ratatui imports outside that module
- Keep `tokio` usage minimal: only for octocrab calls, never in the TUI event loop

## Design doc

Full design document: `design.md` in the repo root.
