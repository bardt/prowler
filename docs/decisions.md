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
