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
| `Tab` / `Shift+Tab` | Cycle panel focus |
| `1`/`2`/`3`/`4` | Jump to panel |
| `e` | Open HEAD in $EDITOR at current line |
| `E` | Open BASE in $EDITOR at current line |
| `v` | Mark file viewed |
| `s` | Skip file |
| `c` | Post inline comment |
| `r` | Reply to thread under cursor |
| `o` | Resolve / unresolve thread under cursor |
| `M` | Edit your own comment under cursor |
| `X` `X` | Delete your own comment (two presses to confirm) |
| `a` | Apply ` ```suggestion ` block at cursor to worktree file |
| `n`/`N` | Next/prev comment thread (cross-file) |
| `Enter` | Expand/collapse comment thread (or fold folder in Files panel) |
| `?` | Toggle PR description / conversation panel |
| `L` | Toggle local diff panel |
| `R` | Refresh local diff |
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

**M14 — Impact-based file sorting**

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
| M11 | Local diff panel | ✅ |
| M12 | Dashboard | ✅ |
| M13 | Missing review actions (partial — see scope) | ✅ |
| M14 | Impact-based file sorting | 🔲 next |

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

**M14 scope:** Optional file-panel ordering by dependency-graph "impact" — core
modules imported by many others sort first, leaf files last. Per design.md:
"core modules imported by many others go first, lockfiles last."

- Compute an import graph from the set of changed files plus their direct
  importers in the worktree. Per-language parser: Rust `use` statements, JS/TS
  `import` / `require`, Python `import`. Start with one language and let the
  rest fall back to alphabetical.
- For each file, count how many other files import it (in-degree). That's
  the impact score. Higher = more central.
- Sort the flat file list (or the leaf level of the tree) by impact
  descending; lockfiles, generated paths, and unknown languages tie-break to
  the bottom alphabetically.
- Make it a toggle (e.g. `o` cycles ordering: tree → flat-impact → flat-alpha)
  rather than a hard-coded mode, since for small PRs the tree is still
  faster to scan.

**Tradeoff:** computing imports requires parsing every file in the repo (or
at least every reverse-dependency); on big monorepos this is non-trivial.
Cache the graph on disk under `.review/impact-graph.json` keyed by HEAD SHA
to avoid re-parsing on every prowler open.

Update the **Current milestone** section and the status column above at the start of each
new milestone.

## Backlog

- **Multi-line review comments.** `c` and the M13 `a` suggestion-apply both
  assume a single-line anchor. Adding a selection mode (e.g. `V` enters
  visual-line, `j`/`k` extends, `c` posts) would let the user post threads
  that anchor to a span. GraphQL field: `addPullRequestReviewThread.input`
  takes optional `startLine` + `startSide`; the same selection primitive
  feeds local-hunk-to-suggestion below.
- **Local hunk → suggestion comment.** With LOCAL aligned to HEAD (next item),
  pressing `c` while focused on a LOCAL hunk should wrap the new lines in
  a ` ```suggestion ` fence and post via `addPullRequestReviewThread`. The
  HEAD line at the cursor row becomes the anchor line; if the hunk spans
  multiple HEAD lines, we use `startLine`/`line` from the multi-line work
  above.
- **Align LOCAL pane rows with HEAD pane.** Today the LOCAL pane is rendered as a
  unified diff (single column with `+`/`-` markers, walking the local hunks in
  order). HEAD is rendered as side-by-side rows where row Y corresponds to a
  specific line number on either side. The two are unrelated — line 42 in HEAD
  may sit at row 12, while the LOCAL diff for line 42 sits at some unrelated
  row. To align: lay out LOCAL using the HEAD pane's row geometry — for each
  HEAD row with `head_line == N`, look up local-diff content anchored at line
  N and render it in the same row (or blank). Lines that don't appear in HEAD
  (e.g. locally-deleted) get inserted as extra rows on both sides. This
  enables the "convert hunk to suggestion" feature in M13 to map cleanly: the
  cursor row's HEAD line is the anchor for the suggestion comment.
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
