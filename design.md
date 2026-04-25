# prowler — Design Document

> **Status:** Draft  
> **Name:** prowler  
> **Stack:** Rust · ratatui · git2 · octocrab · similar · syntect  

---

## Problem

Code review in the terminal is stuck at reading diffs. The moment you want to understand
*why* something is written a certain way — check a function's callers, trace a type through
the codebase, try an alternative before suggesting it — you have to leave the review context
entirely. You either checkout the branch (losing your working state) or switch to GitHub's
web UI (losing your editor).

The result: reviewers stay shallow. They skim rather than understand.

---

## Core Differentiator

**Git worktree-backed editor handoff with local diff visibility.**

Every file in the diff can be opened in `$EDITOR` at the exact line — with full LSP, full
branch state, and zero disruption to the reviewer's current working branch. A `git worktree`
is spun up for the PR branch when a review session starts. The reviewer can edit freely in
that worktree, and their local changes appear in a third panel alongside the PR diff — laying
the groundwork for converting those changes into GitHub code suggestions.

---

## Design Principles

- **Local-first.** Everything computed from git. Works offline except GitHub API calls.
- **Worktree-safe.** Never touches the reviewer's current branch or working tree.
- **Editor-native.** The TUI is a navigation and annotation layer. Deep reading and editing
  happen in `$EDITOR`.
- **Explicit over clever.** No opaque heuristics. Sorting and scoring are transparent and
  overridable.
- **Zero mandatory config.** Running the binary in a git repo does something useful
  immediately.
- **Composable.** Every view supports `--json` for piping to other tools or CI.
- **Lazygit-inspired UX.** Hotkey hints always visible on screen. Keyboard-driven throughout.

---

## V1 Feature Scope

### Must have
- **Dashboard** — default entry point showing: PRs I opened, PRs I'm assigned/mentioned in,
  PRs I've started reviewing but aren't merged yet, stale sessions (merged/closed PRs with
  live worktrees)
- **Review session** — enter via `prowler review --pr 123` or by selecting from dashboard
- **Worktree lifecycle** — spin up a `git worktree` for the PR branch on first open; re-use
  on subsequent opens; survive TUI quit; explicit teardown from dashboard
- **Side-by-side diff** — Base (target branch) | Head (PR branch), syntax-highlighted,
  three-color (added / removed / moved)
- **Local diff panel** — toggleable third panel showing `git diff HEAD` in the worktree
  (reviewer's uncommitted edits against PR branch HEAD); navigate next/prev hunk
- **Adaptive layout** — when local panel is toggled on: all three columns on wide terminals
  (≥ 200 cols), Base column hidden on narrow terminals; when toggled off: always Base | Head
  regardless of width
- **File panel** — always visible alongside the diff; shows all changed files with lines
  added/removed per file and totals; sorted by category (source > test > config > generated >
  lockfiles)
- **Viewed state** — mark files as viewed; persisted across sessions
- **Inline comments** — display existing GitHub review comments inline in the diff; post new
  comments from within the TUI
- **Hotkey hints** — always visible at the bottom of the screen, context-sensitive,
  lazygit-style

### Future features (not in v1, but architecture must allow)
- **Code suggestions from local diff** — select lines from the Local panel (lazygit-style
  hunk/line selection) and convert to a GitHub code suggestion comment
- **Impact-based file sorting** — sort file panel by dependency graph: core modules imported
  by many others go first, lockfiles last
- **Inter-diff mode** — show only what changed since the last review session

### Explicitly out of scope for v1
- Non-GitHub remotes (GitLab, Gitea)
- AI-assisted review
- Stacked / split PR support
- Unified diff mode

---

## Views

### 1. Dashboard

Default entry point. Four sections:

```
┌─────────────────────────────────────────────────────────────────┐
│  [tool name]                               [?] help  [q] quit  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  OPENED BY ME                                                   │
│  ● #142  Add authentication         • 3 comments  2d ago       │
│  ● #138  Refactor error handling    • 0 comments  5d ago       │
│                                                                 │
│  REVIEW REQUESTED / MENTIONED                                   │
│  ● #145  Update CI pipeline         • 7 comments  1d ago       │
│  ● #141  Fix rate limiting          • 2 comments  3d ago       │
│                                                                 │
│  IN PROGRESS (started, not merged)                             │
│  ● #139  Migrate to new API         • 4 comments  4d ago       │
│                                                                 │
│  STALE SESSIONS (merged/closed, worktree still alive)          │
│  ✓ #134  Add logging middleware     merged 8d ago   [d] clean  │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│  [Enter] open review  [d] delete worktree  [r] refresh         │
└─────────────────────────────────────────────────────────────────┘
```

### 2. Review — wide layout (≥ 200 cols, local panel off)

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│  repo  ←  #123: "Add authentication"   +284 -47   [?] help           [q] quit  │
├────────────────────┬──────────────────────────────┬──────────────────────────────┤
│  FILES         [1] │  BASE                   [2]  │  HEAD                   [3] │
│                    │                              │                             │
│  ● src/auth.rs     │  fn validate(                │  fn validate(               │
│    +48 -12         │    token: &str               │    token: &str              │
│  ○ src/types.rs    │  ) -> bool {                 │  ) -> Result<Claims,        │
│    +20 -0          │    let decoded =             │             AuthError> {    │
│  ○ tests/auth.rs   │      decode(token);          │    let decoded =            │
│    +31 -0          │    decoded.is_ok()           │      decode(token)?;        │
│  ✓ Cargo.toml      │  }                           │    Ok(decoded.claims)       │
│    +3 -1           │                              │  }                          │
│  ✗ Cargo.lock      │  ● "Why bool here?"          │                             │
│    +120 -98        │                              │                             │
│                    │                              │                             │
│  5 files           │                              │                             │
│  3 remaining       │                              │                             │
├────────────────────┴──────────────────────────────┴──────────────────────────────┤
│ [e] open HEAD in editor  [E] open BASE in editor  [c] comment  [n] note         │
│ [Tab] switch panel  [v] mark viewed  [L] toggle local panel  [q] back           │
└──────────────────────────────────────────────────────────────────────────────────┘
```

### 3. Review — wide layout (≥ 200 cols, local panel on)

All three diff columns visible.

```
┌─────────────────────────────────────────────────────────────────────────────────────────────┐
│  repo  ←  #123: "Add authentication"   +284 -47   [?] help                    [q] quit    │
├────────────────────┬──────────────────────────┬─────────────────┬───────────────────────────┤
│  FILES         [1] │  BASE             [2]    │  HEAD      [3]  │  LOCAL                [4] │
│                    │                          │                 │                           │
│  ● src/auth.rs     │  fn validate(            │  fn validate(   │  fn validate(             │
│    +48 -12         │    token: &str           │    token: &str  │    token: &str            │
│  ○ src/types.rs    │  ) -> bool {             │  ) -> Result<   │  ) -> Result<             │
│    +20 -0          │    ...                   │    ...          │+   Claims, MyError> {     │
│                    │                          │                 │    ...                    │
├────────────────────┴──────────────────────────┴─────────────────┴───────────────────────────┤
│ [e] open HEAD  [E] open BASE  [c] comment  []] next hunk  [[] prev hunk  [L] toggle local  │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 4. Review — narrow layout (< 200 cols, local panel on)

Base column hidden. HEAD | LOCAL only.

```
┌─────────────────────────────────────────────────────────────────┐
│  repo  ←  #123: "Add authentication"   +284 -47      [q] quit  │
├────────────────────┬───────────────────┬────────────────────────┤
│  FILES         [1] │  HEAD        [2]  │  LOCAL             [3] │
│                    │                   │                        │
│  ● src/auth.rs     │  fn validate(     │  fn validate(          │
│    +48 -12         │    token: &str    │    token: &str         │
│                    │  ) -> Result<     │+ ) -> Result<          │
│                    │    ...            │+   Claims, MyError> {  │
│                    │                   │                        │
├────────────────────┴───────────────────┴────────────────────────┤
│ [e] open HEAD  [c] comment  []] next hunk  [[] prev hunk        │
└─────────────────────────────────────────────────────────────────┘
```

---

## Inline Comments

Existing GitHub review comments are displayed inline in the diff, anchored to their line.
Indicated with a `●` marker in the gutter. Pressing `Enter` on a marked line expands the
comment thread as an overlay. Pressing `c` on any line opens a compose buffer to post a new
comment.

Collapsed indicator in diff:

```
│  ) -> Result<Claims, AuthError> {              │
│ ● Why not return Option here?                  │  ← thread indicator, 2 replies
│    let decoded = decode(token)?;               │
```

Expanded thread overlay:

```
┌─ Comment thread: src/auth.rs:14 ──────────────────────────────┐
│                                                                │
│  alice  2d ago                                                 │
│  Why not return Option here? The error type seems redundant    │
│  given callers only check for success.                         │
│                                                                │
│  bob  1d ago                                                   │
│  We need the error variant for logging — Option loses the      │
│  failure reason downstream.                                    │
│                                                                │
├────────────────────────────────────────────────────────────────┤
│  [r] reply  [Esc] close                                        │
└────────────────────────────────────────────────────────────────┘
```

---

## GitHub Auth

Auth is resolved at startup in priority order:

1. `GITHUB_TOKEN` environment variable
2. `gh auth token` subprocess call (delegates to `gh` CLI if installed)
3. Error with a helpful message pointing to both options

No credentials are stored by the tool itself.

---

## GitHub API Strategy

**REST throughout v1** via `octocrab`. Straightforward to debug, well-documented, and
Claude Code generates correct `octocrab` REST calls reliably.

GraphQL may be introduced in a future milestone for the dashboard (where batching multiple
queries into one request becomes worthwhile), but is explicitly out of scope for v1.

---

## Worktree Lifecycle

```
First open of PR #123
  └─ git worktree add /tmp/prowler-{repo}-123-{short_sha} {pr_branch}
       └─ path stored in session state

Subsequent opens
  └─ worktree path exists? → re-use
  └─ worktree path gone?   → recreate, warn if local changes may have been lost

TUI quit
  └─ worktree stays alive (editor may still have files open)

Explicit teardown  [d] on stale session in dashboard
  └─ warn if worktree has uncommitted changes
  └─ git worktree remove {path} --force
  └─ session state deleted

Crash recovery
  └─ git worktree prune  (run automatically on startup)
```

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                    TUI Layer (ratatui)                       │
│         Dashboard · Review · Diff · Comments · Local        │
├──────────────────────────────────────────────────────────────┤
│                    Command Layer                             │
│              dashboard · review · cleanup                   │
├─────────────────────┬──────────────────┬────────────────────┤
│  Git Engine         │  Session State   │  Layout Engine     │
│  git2               │  .review/        │  width threshold   │
│  worktree mgmt      │  (local, toml)   │  column visibility │
│  diff (similar)     │                  │                    │
├─────────────────────┼──────────────────┼────────────────────┤
│  GitHub Remote      │  Editor Bridge   │  Syntax            │
│  octocrab (REST)    │  $EDITOR +{line} │  syntect           │
│  comments, PRs      │  worktree paths  │                    │
└─────────────────────┴──────────────────┴────────────────────┘
```

### Editor handoff

```rust
// Pseudocode
fn open_in_editor(side: DiffSide, file: &str, line: usize, session: &Session) {
    let path = match side {
        DiffSide::Head  => session.worktree_path.join(file),
        DiffSide::Base  => write_to_tempfile(git_show(&session.base_sha, file)),
        DiffSide::Local => session.worktree_path.join(file),
    };
    let editor = env::var("EDITOR").unwrap_or("vi");
    Command::new(editor).arg(format!("+{line}")).arg(path).spawn();
}
```

Base side uses `git show {base_sha}:{file}` into a named tempfile. No LSP on base side in
v1. Full base worktree (with LSP) is a v2 consideration.

### Local diff panel data flow

```
worktree path
  └─ git diff HEAD  (on panel focus or manual [R])
       └─ parsed into hunks via `similar`
            └─ rendered in LOCAL panel
                 └─ future: hunk/line selection → GitHub code suggestion API
```

---

## Session State

Stored in `.review/` at repo root. Added to `.git/info/exclude` automatically on first run.

```
.review/
  sessions/
    {pr_number}/
      state.toml      # worktree path, shas, viewed files, cursor position
      notes.toml      # local per-line notes (never posted to GitHub)
```

`state.toml` example:

```toml
pr_number = 123
branch = "feature/auth"
worktree_path = "/tmp/tool-myrepo-123-a1b2c3"
base_sha = "d4e5f6"
head_sha = "a1b2c3"

[files]
"src/auth.rs"   = "in_progress"
"src/types.rs"  = "unviewed"
"Cargo.toml"    = "viewed"
"Cargo.lock"    = "skipped"
```

---

## Tech Stack

| Concern | Crate |
|---------|-------|
| TUI | `ratatui` |
| Git operations | `git2` |
| GitHub API | `octocrab` (REST) |
| Diff computation | `similar` |
| Syntax highlighting | `syntect` |
| Config / state | `toml` + `serde` |
| Async runtime | `tokio` (GitHub API calls only) |
| Arg parsing | `clap` |

---

## Milestones

### M1 — GitHub auth + PR metadata
`prowler review --pr 123` in a git repo. Resolves GitHub token (`GITHUB_TOKEN` → `gh auth
token`), fetches PR metadata via `octocrab` REST (branch name, base SHA, head SHA, title),
prints to stdout. No TUI. Establishes the GitHub API wiring and auth fallback chain.

**Done when:** `prowler review --pr 123` prints PR title, base branch, head SHA, and file count
to stdout.

---

### M2 — Worktree lifecycle
Spins up a `git worktree` for the PR branch, stores session state in `.review/`, re-uses on
second run, tears down explicitly. No TUI — proves the lifecycle works in isolation. Adds
`.review/` to `.git/info/exclude`. Runs `git worktree prune` on startup.

**Done when:** Running `prowler review --pr 123` twice re-uses the same worktree. A `--cleanup`
flag removes it.

---

### M3 — Diff data model
Computes the diff between base and head using `similar`, parses into typed structs. Prints
raw hunk output to stdout. No rendering yet. This is the foundation everything visual builds
on — getting the data model right here prevents painful refactors later.

Key types:
```rust
struct FileDiff { path, hunks, added, removed }
struct Hunk     { header, lines }
enum DiffLine   { Added(String), Removed(String), Context(String), Moved(String) }
```

**Done when:** `prowler review --pr 123 --json` emits a correct JSON diff for all changed files.

---

### M4 — Basic TUI shell
`ratatui` bootstrapped. Two-panel layout: file list left, empty placeholder right. Keyboard
navigation between files (`j`/`k`), panel switching (`Tab`), hotkey hints at the bottom,
`q` to quit. No diff rendering yet — just proves the TUI skeleton and event loop work.

**Done when:** TUI opens, file list is navigable, hotkey bar visible, `q` exits cleanly.

---

### M5 — Diff rendering
Side-by-side diff rendered in the right panel for the selected file. Syntax highlighting via
`syntect`. Three-color diff (added / removed / moved). Scroll with `j`/`k`, hunk jump with
`]`/`[`. File panel shows `+48 -12` per file and totals in the header.

**Done when:** Selecting a file renders a readable, syntax-highlighted side-by-side diff.

---

### M6 — Editor handoff
`e` opens HEAD in `$EDITOR` at the current line. `E` opens BASE via `git show` into a
tempfile. TUI suspends cleanly while the editor is open and resumes on exit.

**Done when:** Pressing `e` on a diff line opens Neovim at the correct file and line in the
worktree, with LSP functional.

---

### M7 — Viewed state + session persistence
Mark files viewed (`v`), skip (`s`). State persisted to `state.toml` across restarts.
Status indicators in file panel (`●` unviewed / `○` in progress / `✓` viewed / `✗` skipped).
Remaining file count in file panel footer.

**Done when:** Marking files viewed persists after quitting and reopening the same PR.

---

### M8 — Inline comments (read)
Fetch GitHub review comments via `octocrab` REST. Display `●` markers in the diff gutter at
comment lines. `Enter` expands a thread overlay showing author, timestamp, and body. `Esc`
collapses.

**Done when:** Existing PR comments are visible inline in the diff with readable thread
expansion.

---

### M9 — Inline comments (post)
`c` opens a compose buffer at the current line. On save, posts to GitHub via `octocrab`.
Reply to existing threads from the thread overlay (`r`). Approve / request changes from the
review view.

**Done when:** A comment posted from the TUI appears on GitHub.

---

### M10 — Local diff panel
`L` toggles the local diff panel. Adaptive layout: all three columns on wide terminals
(≥ 200 cols), Base hidden on narrow. `git diff HEAD` run in the worktree on panel open and
on `R`. Hunk navigation with `]`/`[` in the local panel.

**Done when:** Local edits made in the worktree appear in the LOCAL panel alongside the PR
diff.

---

### M11 — Dashboard
No-args entry point (`prowler` with no subcommand). Four sections: opened by me, review
requested / mentioned, in progress, stale sessions. `Enter` opens a review session. `d`
tears down a stale worktree (with confirmation if uncommitted changes exist). `r` refreshes
from GitHub.

**Done when:** `prowler` with no args shows a useful PR inbox and stale session cleanup works.

---

## Keybindings (v1)

### Dashboard

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate up / down |
| `Enter` | Open review session |
| `d` | Delete worktree for selected stale session |
| `r` | Refresh from GitHub |
| `?` | Help overlay |
| `q` | Quit |

### Review

| Key | Action |
|-----|--------|
| `j` / `k` | Scroll diff |
| `]` | Next hunk |
| `[` | Previous hunk |
| `Tab` | Cycle panel focus |
| `1` / `2` / `3` / `4` | Jump to panel directly |
| `e` | Open HEAD in `$EDITOR` at current line |
| `E` | Open BASE in `$EDITOR` at current line |
| `v` | Mark current file as viewed |
| `s` | Skip current file |
| `c` | Post inline comment at current line |
| `Enter` | Expand / collapse comment thread |
| `n` | Add local note at current line |
| `R` | Refresh local diff panel |
| `L` | Toggle local diff panel |
| `?` | Help overlay |
| `q` | Back to dashboard |

---

## Open Questions

1. **Name.** Must not conflict with existing crates on crates.io.
2. **Width threshold.** 200 cols as default — should this be configurable in
   `.review/config.toml`?
3. **Teardown warning.** When deleting a worktree with uncommitted changes: hard block or
   warn-and-confirm?
4. **Base-side LSP (v2).** Full second worktree at merge-base, or stay with tempfile
   permanently?
5. **Multi-remote repos.** Assume `origin` is GitHub, or detect from remote URL?
