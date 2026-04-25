# Trade-offs and design notes

This file collects design decisions made during autonomous work that the user
should review later. Each entry: what was chosen, what the alternative was, why.

---

## Status feedback for synchronous actions (2026-04-26)

**Choice:** Single transient row at the bottom of the TUI, replacing the hotkey
footer when active. Uses `StatusKind::{Success, Error, Info}` with a 3-second
TTL. Auto-cleared on the next redraw after expiry — no extra timer plumbing
because the event loop already redraws every 250ms via the poll timeout.

**Alternative considered:** A dedicated status row alongside the hotkey footer
(no replacement). Rejected because vertical space is precious; replacing the
hotkey row for 3s is unobtrusive.

**Wired:** `c` (post), `r` (reply), `S` (submit), `e`/`E` (editor errors only —
success is implicit when the editor closes normally).

**Not wired:** `v`/`s` viewed-state sync — that's `tokio::spawn` fire-and-forget
and would need an `mpsc::UnboundedSender` from the spawned task back to the
event loop. Tracked as a separate backlog item (see CLAUDE.md).

---

## Comment body wrapping (2026-04-26)

**Choice:** Pre-wrap comment body lines at layout time using the pane's content
width (`pane.width - 5` for borders + gutter + `│ ` prefix). Layout is rebuilt
when the pane width changes (detected in `render_body`). Each wrapped chunk
becomes a separate `Cell::CommentBody` row. `ReviewState` now stores
`threads_by_file` so we can re-layout without re-grouping.

**Alternative considered:** ratatui's `Paragraph::wrap(Wrap { trim: false })`.
Rejected because it wraps visually but `scroll((n, 0))` then counts visual
lines, which would break our cursor / hunk-start row math. Pre-wrapping keeps
logical row count == visual row count.

**Width math:** simple byte-length wrapping with UTF-8 boundary safety. Wide
chars (CJK / emoji) over-flow because we don't measure visual columns. If
that becomes a real problem, swap in `unicode-width`.

**Resize behavior:** every resize triggers a full layout rebuild. Cost is small
(walks each hunk once), and `last_layout_width` short-circuits when width
hasn't changed. Cursor position is preserved by row index, so the cursor may
land on a different content row after a resize — acceptable for v1.

---

## Pull viewed state on PR open (2026-04-26)

**Choice:** On every PR fetch, walk `meta.files` and seed `Session.files` for
any path that doesn't already have a local entry: if GitHub says `VIEWED`, mark
it `FileStatus::Viewed` locally; otherwise leave it absent (treated as
Unviewed). Local entries always win over GitHub state.

**Rationale for local-wins:** if a user marks a file viewed locally and the
sync to GitHub fails (network blip), the next open should still reflect their
intent. Locally-set `Skipped` should also survive across opens.

**`DISMISSED` handled as Unviewed.** GitHub returns this when the head SHA
changed after a viewed-mark, indicating the previous review is stale. We
silently treat as Unviewed — backlog tracks distinguishing it as a separate
state.

**No write-back from GitHub.** The fetch only seeds; we never overwrite local
`Viewed`/`Skipped` based on GitHub saying `UNVIEWED`. That'd be the wrong
direction for divergence.

---

## Collapsible comment threads (2026-04-26)

**Choice:** Threads default to collapsed (single `▸ N comments • @author: …`
row); `Enter` toggles. State lives in `ReviewState.expanded_threads:
HashSet<String>` keyed by GraphQL thread node ID. New threads (those that
appear on `apply_refresh` but weren't in the previous snapshot) auto-expand so
the user sees what they just posted.

**Persistence:** none — collapsed/expanded state is in-memory only and resets
on prowler restart. Storing in `Session` would also work but adds churn for a
mostly-ergonomic preference. Revisit if users complain.

**Cursor behaviour on toggle:** the layout rebuild keeps the cursor at the
same numeric row index. When collapsing, cursor stays on the (now-summary)
thread row. When expanding, cursor lands on the thread's header row. Rows in
later threads / hunks shift in either direction; users may need to scroll.
Acceptable for v1.

**Preview width:** `wrap_width - 40` chars of the root comment's first line,
ellipsised. The 40-char reserve covers the `▸ N comments • @author: ` prefix
on a typical pane; if it's too tight, tune later.

---

## Cross-file comment navigation (2026-04-26)

**Choice:** `n` / `N` jump to next / previous comment thread across the whole
PR (wraps around). On jump, ancestors of the target file are auto-expanded in
the file tree so the file row is visible. If focus was on the Files panel,
it switches to Head so the diff pane is in front (since that's where the
cursor jumped). The cursor lands on the thread row on whichever side
(Base/Head pane) the thread is anchored on — but the user may need to switch
focus to that side themselves to see the diff-side highlight clearly.

**Alternative considered:** `gN` / `gP` two-key sequences. Rejected as too
fiddly without a leader-key state machine.

**Footer:** doesn't currently advertise `n` / `N`. Always-available cross-file
navigation feels like vim's `n` / `N` — assumed knowledge for terminal users.
Reconsider if first-time users miss it.

---

## Async sync errors surfaced via mpsc channel (2026-04-26)

**Choice:** `tui::run` creates a `tokio::sync::mpsc::unbounded_channel` and
hands the sender to `ReviewState`. The spawned `set_viewed` task sends a
`StatusMessage` on error (success is silent — `v` toggles dozens of files
during a review and a "Synced" banner on each would be noise). The event loop
drains the receiver before each draw and converts every message into
`state.set_status`.

**Alternative considered:** Polling a shared `Mutex<VecDeque>` from the event
loop. Rejected — mpsc is the idiomatic Tokio answer and avoids lock
contention.

**Success silenced.** Only errors hit the status row. Successful viewed-state
syncs are logged to `/tmp/prowler-sync.log` only (existing behaviour). If a
user ever wants positive confirmation, a verbose-mode flag could re-enable.

---

## DISMISSED viewed state surfaced as `FileStatus::Dismissed` (2026-04-26)

**Choice:** New `FileStatus::Dismissed` variant rendered with a bold yellow
`!` marker in the file panel. Triggered when GitHub returns
`viewerViewedState == "DISMISSED"` (auto-cleared because the head SHA moved
since the user marked the file viewed).

**Merge precedence:** DISMISSED is the *only* GitHub state that overrides a
local entry — and even then, not Skipped. The rationale: dismissal is a
positive signal "your previous review is stale, look again", which is more
valuable than preserving a local Viewed mark. Skipped means "I've decided
not to review", which doesn't get invalidated by code churn.

**Keybind behaviour:** `v` on a Dismissed row marks Viewed (re-review); `s`
marks Skipped. Same code path as Unviewed because the existing `_ =>
FileStatus::Viewed` arm already covers Dismissed.

---

## Headless TUI testing harness (2026-04-26)

**Choice:** Three-layer split — pure key handler, state constructor, render
inspection.

- `pub fn apply_key(state: &mut ReviewState, key: KeyCode) -> bool` is a free
  function in `review.rs` that handles every key that doesn't touch the
  terminal or runtime. Returns `true` for `q` (quit signal). The event loop
  matches the side-effectful keys (`c`, `r`, `S`, `e`, `E`) explicitly and
  delegates everything else to `apply_key`.
- `ReviewState::for_test(meta, diffs, threads)` (gated `#[cfg(test)]`) stubs
  Session, repo paths, and tokens, and creates an mpsc channel whose
  receiver is dropped (background sends are best-effort and panic-free under
  a Tokio runtime; tests that don't trigger them work without
  `#[tokio::test]`).
- `TestBackend` from ratatui plus a small `render_to_lines` helper let
  snapshot tests grep the rendered buffer for known content.

**Skipped: API trait.** `fetch_pr`, `post_thread`, etc. still build their own
`Octocrab` clients and hit the network. End-to-end tests of post + refresh
flows would need a `GitHubClient` trait with `Mock` and `Octocrab` impls.
Documented as a follow-up backlog item; current tests cover state +
navigation + render layers, which catches the regressions most likely to be
shipped during autonomous work (cursor math, layout, key dispatch).

**Why no `#[tokio::test]`:** the existing tests don't trigger any spawned
tasks. `toggle_viewed` would call `tokio::spawn` and panic without a
runtime; tests that need that path can opt into `#[tokio::test]` later.

---
