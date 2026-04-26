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
  config.rs        # ~/.config/prowler/config.toml loader
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

## Configuration

Optional file at `~/.config/prowler/config.toml` (or
`$XDG_CONFIG_HOME/prowler/config.toml`). Missing fields use defaults.
See `docs/config-sample.toml` for the schema.

Fields:
- `editor.command` — overrides `$VISUAL` / `$EDITOR` for `e` / `E` and
  comment-compose buffers.
- `dashboard.scope` — `"current_repo"` (default) or `"all"` for cross-repo.
- `review.hide_resolved_default` — default for `Session.hide_resolved`.
- `review.poll_interval_secs` — background poller cadence (default 60).
- `review.confirm_delete_ttl_secs` — `X X` arming window (default 3).
- `review.cursor_sync_modes` — placeholder for future cursor-sync toggle.

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

Always 3 columns: FILES | LEFT | RIGHT.

`L` toggles between two modes:

- **`BaseHead`** (default): LEFT = BASE, RIGHT = HEAD. What the PR proposes.
- **`HeadLocal`**: LEFT = HEAD (head_sha), RIGHT = WORK (your worktree). What
  you'd suggest on top of the PR. In this mode, `V` + `c` posts a
  ` ```suggestion ` comment on the corresponding HEAD line range.

Cursor and scroll are tracked per-mode so toggling preserves your
position in each.

## Key bindings

### Review view
| Key | Action |
|-----|--------|
| `j`/`k` | Scroll diff |
| `]`/`[` | Next/prev hunk |
| `Tab` / `Shift+Tab` | Cycle panel focus |
| `1`/`2`/`3`/`4` | Jump to panel |
| `e` | Open HEAD in $EDITOR at current line |
| `E` | Open BASE in $EDITOR at current line |
| `v` | Mark file viewed |
| `s` | Skip file |
| `c` | Post inline comment (single line, or multi-line if `V`-selection active) |
| `V` | Start multi-line selection at cursor (j/k extends; Esc cancels) |
| `r` | Reply to thread under cursor |
| `o` | Resolve / unresolve thread under cursor |
| `M` | Edit your own comment under cursor |
| `X` `X` | Delete your own comment (two presses to confirm) |
| `a` | Apply ` ```suggestion ` block at cursor to worktree file |
| `H` | Toggle hide-resolved threads (persisted in session) |
| `/` | Filter file panel; type to narrow, Enter to commit, Esc to clear |
| `n`/`N` | Next/prev comment thread (cross-file) |
| `Enter` | Expand/collapse comment thread (or fold folder in Files panel) |
| `?` | Toggle keymap help overlay |
| `D` | Toggle PR description / conversation panel |
| `Esc` | Close help / description overlay |
| `L` | Toggle diff mode: PR (base→head) ↔ Local (head→work) |
| `R` | Refresh local diff for current file |
| `c` (mode 2 + `V` selection) | Post selection as ` ```suggestion ` comment |
| `F5` / `Ctrl+R` | Re-fetch PR from GitHub (comments, viewed states) |
| `S` | Submit review (verdict + summary) |
| `q` | Back to dashboard |

### Dashboard view
| Key | Action |
|-----|--------|
| `j`/`k` | Move selection (skips section headers) |
| `g`/`G` | Jump to first/last selectable row |
| `Enter` | Open the PR / session under cursor in review view |
| `r` | Refresh GitHub data |
| `d` | Delete a local session (worktree + state) |
| `q` | Quit |

## Current milestone

**M18 — 1.0 polish**

## Milestones overview

### v1 (target: 1.0 release)

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
| M9 | Inline comments (post, single-line) | ✅ |
| M10 | Submit review | ✅ |
| M11 | Local diff panel | ✅ |
| M12 | Dashboard | ✅ |
| M13 | Missing review actions | ✅ |
| M14 | Local hunk → suggestion comment | ✅ |
| M15 | LOCAL/HEAD pane alignment | ✅ (via BaseHead/HeadLocal modes) |
| M16 | Hide-resolved toggle + file-panel fuzzy filter | ✅ |
| M17 | Configuration file (`~/.config/prowler/config.toml`) | ✅ |
| M18 | 1.0 polish: empty states, loading hints, persisted UI prefs | 🔲 next |

### v2 (post-1.0, opt-in / experimental)

| # | Name | Status |
|---|------|--------|
| V2.1 | Impact-based file sorting (was M14) | 🔲 |
| V2.2 | AI-assisted review (summarise PR, draft comments) | 🔲 |
| V2.3 | Stacked-PR support | 🔲 |
| V2.4 | Inter-diff mode (changes since last review) | 🔲 |
| V2.5 | CI status integration (`statusCheckRollup`) | 🔲 |
| V2.6 | Non-GitHub remotes (GitLab, Gitea) | 🔲 |
| V2.7 | Reactions | 🔲 |
| V2.8 | Themes | 🔲 |

**M10 scope:** A panel listing all comments in the current pending review, with a verdict
selector (Approve / Comment / Request changes) and an optional summary body. Submits via
GraphQL `submitPullRequestReview` (or creates the review with `addPullRequestReview` if
no pending one exists). Without this, M9's `c` accumulates Pending comments forever.

**M13 scope:** GitHub-supported review actions that prowler didn't expose
before this milestone.

**Shipped:**

- **Resolve / unresolve threads** (`o`). Uses `resolveReviewThread` /
  `unresolveReviewThread` GraphQL mutations; respects `viewerCanResolve` /
  `viewerCanUnresolve` (no-ops with an error toast if the viewer lacks
  permission).
- **Edit own comments** (`M`). Opens the existing body in `$EDITOR`; on save
  calls `updatePullRequestReviewComment`. Detected via
  `comment.viewerDidAuthor`. Preserves cancellation semantics
  (empty/abort = no-op; same body = "no changes" toast).
- **Delete own comments** (`X`, two-step). First press arms; second press
  within 3 s confirms and calls `deletePullRequestReviewComment`. Same
  arming TTL as the rest of the status row.
- **Render suggestion blocks distinctly.** Lines inside a
  ` ```suggestion ` … ` ``` ` fence are highlighted with a green background
  (`BG_SUGGESTION`).
- **Apply suggested changes** (`a`). Detects the suggestion block in the
  comment under the cursor (HEAD-side only — suggestions replace new code,
  not old) and writes it into the worktree file at the thread's anchor line.
  Single-line replacement for v1; multi-line ranges (when GitHub provides
  `original_start_line`) is a follow-up.

**Deferred** (carried into the backlog / future milestones):

- **Post local hunks as suggestion comments.** Depends on the LOCAL-pane
  alignment fix; without alignment we can't reliably map a local hunk row
  to its HEAD anchor line. Tracked in the backlog.
- **Multi-line comments.** Selection mode (`V` to start, `j`/`k` to extend,
  `c` to post) — tracked in the backlog so we can implement it after the
  local-hunk-to-suggestion feature, since both want a similar selection
  primitive.

**M14 scope:** Local hunk → ` ```suggestion ` comment. The flagship "edit
and suggest" loop:

- User opens a file with `e`, edits it, saves. The LOCAL pane shows their
  edits.
- In the LOCAL pane, the user selects rows with `V` (already shipped).
- `c` posts the selection as a ` ```suggestion ` block on the
  corresponding HEAD line(s), via `addPullRequestReviewThread`.

Depends on **M15** for clean line-anchor mapping. Without M15 we have to
parse hunk headers + walk rows; with M15 the cursor row already knows
its HEAD line.

**M15 scope:** Align LOCAL pane rows with HEAD pane.

Today the LOCAL pane is rendered as a unified diff (single column,
`+`/`-` markers). HEAD is rendered as side-by-side rows where row Y
corresponds to a specific line on either side. The two are unrelated —
line 42 in HEAD may sit at row 12 while the LOCAL diff for line 42 sits
at some unrelated row.

Approach: lay out LOCAL using the HEAD pane's row geometry. For each
HEAD row with `head_line == N`, look up local-diff content anchored at
line N and render it on the same row (or blank). Locally-deleted lines
become extra rows on both sides.

Side benefit: M14's suggestion-comment anchor becomes "the cursor row's
HEAD line" — no offset math.

**M16 scope:** Two daily-use UX additions.

- **Hide-resolved threads (`H`).** Toggleable filter that drops
  `is_resolved == true` threads from the layout. State stored in
  `Session` so it persists across opens. Default: hidden (matches
  GitHub's web UI).
- **Fuzzy file filter (`/`).** Live filter on the file panel. Type to
  narrow; Esc / Enter clears. Useful in PRs with > 30 files. Reuse the
  selected-row mechanic; the tree collapses to leaves matching the
  query.

**M17 scope:** Configuration file at `~/.config/prowler/config.toml`.

Replaces hard-coded values:

- `editor` — overrides `$EDITOR` (default `nvim`).
- `dashboard.scope` — `"current_repo"` (default) or `"all"` for
  cross-repo dashboard.
- `review.hide_resolved_default` — bool, default true.
- `review.local_panel_default` — bool, default false.
- `review.poll_interval_secs` — bg poll cadence (default 60).
- `review.confirm_delete_ttl_secs` — `X X` arming window (default 3).

Loaded on startup. Future entries (theme, key remap) layer onto the
same struct. In v1 because the maintainer wants to experiment with
defaults without recompiling.

**M18 scope:** Final 1.0 polish pass.

- Empty-state hints on dashboard / file panel / LOCAL pane.
- Loading status during dashboard refresh and PR open (today they
  freeze the UI for ~1 s).
- Persist expanded-thread state in `Session`.
- Persist last-cursor-position-per-file in `Session` so reopening a PR
  drops you where you were.
- Post-`q` cleanup: release stale worktrees from PRs that were merged /
  closed.
- Cargo release setup: `cargo install --git`, README, basic install
  instructions, demo gif.

### v2 details

**V2.1 — Impact-based file sorting** (was M14). Optional file-panel
ordering by dependency-graph "impact" — core modules imported by many
others sort first, leaf files last. Per design.md: "core modules
imported by many others go first, lockfiles last."

- Compute an import graph from changed files plus their direct
  importers in the worktree. Per-language parser: Rust `use`, JS/TS
  `import` / `require`, Python `import`. Start with Rust; alphabetical
  fallback for others.
- For each file, count in-degree (how many others import it). Higher =
  more central.
- Sort flat file list (or tree leaves) by impact desc; lockfiles,
  generated paths, unknown languages tie-break to the bottom.
- Toggle via `o`: tree → flat-impact → flat-alpha. Tree stays the
  default since it's fastest to scan on small PRs.

Trade-off: parsing imports for every file is non-trivial on
monorepos. Cache on disk under `.review/impact-graph.json` keyed by
HEAD SHA.

Moved to v2 because it's speculative (depends on PR shape) and the
parser surface area dwarfs the rest of v1 combined.

Update the **Current milestone** section and the status column above at the start of each
new milestone.

## Backlog

- **Cursor sync between BaseHead and HeadLocal modes** *(blocked on
  config/prefs to make it toggleable)*. Today each mode keeps its own
  cursor; switching `L` returns you to where you were in the new mode.
  Alternative: sync the cursor by HEAD line so toggling lands on the
  corresponding row in the target mode. Caveats:
  - Removed-only rows (mode 1) and Added-only rows (mode 2) have no
    HEAD-line counterpart — fall back to the nearest neighbouring row
    that has one.
  - Comment-thread rows have no `head_line` directly — fall back to
    `thread.line`.
  - HEAD line may not exist in the target mode at all (e.g. you didn't
    edit the line locally) — leave the target cursor unchanged, or
    surface a hint.
  - Synthetic hunks in mode 1 don't have counterparts in mode 2.
  - Toggling should clear any active `V`-selection (anchor row index
    won't translate).
  - Behaviour change vs current; ship as a config toggle so users can
    pick continuity-vs-independence.
- **Mockable GitHub client for end-to-end tests.** The headless harness
  (state + render + event layers) is in place — `apply_key` is pure,
  `ReviewState::for_test` builds a state from fixtures, and `TestBackend`
  snapshots the rendered frame. What's still missing is the API layer:
  `github::fetch_pr`, `set_viewed`, `post_thread`, etc. each build their own
  `Octocrab` client and hit the network. To exercise post-then-refresh flows
  without GitHub, refactor those into a trait
  (`trait GitHubClient { async fn fetch_pr(...); async fn post_thread(...); }`)
  and inject either the real `Octocrab`-backed impl or a `MockGitHubClient`
  returning canned responses. Test handlers like `post_comment` /
  `submit_review` then become drivable end-to-end.

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
