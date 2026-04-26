# Changelog

All notable changes to prowler are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
While the major version is `0.x`, breaking changes may land in any release.

## [Unreleased]

## [0.1.0] - 2026-04-26

Initial release.

### Added

**Dashboard** (`prowler` with no args).

- Three sections fetched from GitHub: *Awaiting your review*, *Opened
  by you*, *Assigned to you*. Plus a *Local sessions* section listing
  in-progress reviews on disk.
- Sort by `updatedAt` desc — most-recently-active PRs at the top.
- `Enter` opens a PR into the review TUI on the same terminal handle;
  `q` returns. `r` refreshes; `d` cleans up a local session
  (worktree + state). Loading status while fetches are in flight.

**Review view** (`prowler review --pr N`).

- Side-by-side BASE / HEAD diff with syntax highlighting
  (syntect, full RGB).
- File panel with per-file line counts (`+N -M`), unresolved-comment
  badge, viewed-state marker, rename source (`docs/foo.rs ← foo.rs`).
- Tree-style file panel with collapsible folders.
- Header shows PR state badge (OPEN/DRAFT/MERGED/CLOSED), title,
  total `+/-`, and `N/M viewed` progress (cyan → green when complete).
- Cursor + per-file scroll preserved across files. Cursor and
  expanded-thread state persist in `Session` across opens.

**Diff modes** (`L` toggles).

- `BaseHead` (default): LEFT=BASE, RIGHT=HEAD. What the PR proposes.
- `HeadLocal`: LEFT=HEAD, RIGHT=WORK (your worktree). What you'd
  suggest on top of the PR.
- Cursor and scroll tracked per mode so toggling preserves your
  position in each.

**Editing & worktrees** (`e` / `E`).

- `e` opens HEAD-side file in `$EDITOR` at the cursor line, full LSP.
- `E` opens BASE-side file (separate worktree at the base SHA).
- Worktrees live at `/tmp/prowler-{repo}-{pr}-{sha}` and are reused
  across opens; `git worktree prune` runs at startup for crash recovery.
- Diffs auto-refresh after `$EDITOR` exits; status row hints toward
  mode 2 to see local edits.

**Inline comments.**

- Read review threads inline at their anchor line, side-by-side with
  the diff. Collapsed by default (`▸ N comments • @author: preview`);
  `Enter` expands.
- Outdated / off-diff threads get a synthetic context hunk (±3 lines
  pulled from the appropriate SHA) so they always have somewhere to
  show, mirroring GitHub's web UI behaviour.
- `c` posts a single-line comment; `V`+`j`/`k`+`c` posts a multi-line
  comment via `addPullRequestReviewThread` with `startLine` /
  `startSide`.
- `r` replies; `o` toggles resolve / unresolve; `M` opens your own
  comment in `$EDITOR` for editing; `X` `X` (two-press, 3-second
  arming window) deletes your own comment.
- `a` applies a ` ```suggestion ` block from a comment to the
  worktree file at the thread's anchor line.
- Compose buffers seed with code-context lines (`# > NN: text`) so
  you can see what you're commenting on while typing.

**Local hunks → suggestion comments.**

- In `HeadLocal` mode, `V`+`c` posts the selected rows as a
  ` ```suggestion ` comment on the corresponding HEAD line range.
  The "edit and suggest" loop runs entirely from the terminal:
  `e` → `nvim` → save → `L` → select → `c`.

**Submit review** (`S`).

- Compose-buffer flow with explicit verdict line (`APPROVE` /
  `COMMENT` / `REQUEST_CHANGES`) and an optional summary body below
  a `--- summary below ---` marker. Parse failures surface in the
  status row instead of being silent.

**Refresh.**

- `F5` / `Ctrl+R` re-fetches the PR; same SHAs merge in updated
  comments / viewed states; SHA changes are surfaced as "Quit and
  reopen" instead of silently swapping the worktree.
- Background poller every 60s notifies you ("+2 threads, +1 comment
  on GitHub") via the status row when activity arrives. Notify
  only — never auto-applies, so rows don't shift under your fingers.

**Daily-use UX.**

- `H` toggles hide-resolved threads (persists in `Session`).
- `/` filters the file panel by case-insensitive substring; folders
  auto-expand to reveal matches.
- `n` / `N` jumps to next / previous comment thread cross-file.
- `?` opens a categorized keymap overlay; `D` toggles the PR
  description / conversation panel; `Esc` closes either.

**Configuration** (`~/.config/prowler/config.toml`).

- Optional file. Missing → defaults. Schema:
  - `editor.command` — overrides `$VISUAL` / `$EDITOR`.
  - `dashboard.scope` — `"current_repo"` (default) or `"all"`.
  - `review.hide_resolved_default` — default for `Session.hide_resolved`.
  - `review.poll_interval_secs` — background poller cadence.
  - `review.confirm_delete_ttl_secs` — `X X` arming window.
  - `review.cursor_sync_modes` — placeholder for future cursor-sync.
- Sample at `docs/config-sample.toml`.

**Auth.**

- Resolves `GITHUB_TOKEN`, falls back to `gh auth token`. No
  credentials stored by prowler.

### Stack

Rust 2024 (rust-version 1.85) · ratatui 0.30 · octocrab 0.44
(GraphQL) · git2 0.19 · similar 2 · syntect 5 · toml 0.8 ·
tokio 1 · clap 4.

### Tests

52 unit + integration tests covering state transitions, key
dispatch, render snapshots, diff layout, file-tree filtering,
config parsing, and the multi-line / suggestion build paths.

[Unreleased]: https://github.com/bardt/prowler/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/bardt/prowler/releases/tag/v0.1.0
