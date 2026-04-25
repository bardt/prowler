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
