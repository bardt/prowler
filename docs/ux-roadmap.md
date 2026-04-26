# UX roadmap

A running list of UX-quality items for prowler — things that won't change
*what* the tool does, but will make it feel better to live in.

Ordered roughly by impact / effort. Pick from the top.

## Top priorities

### 1. Help overlay (`?` or `K`)
A floating panel listing every keybind in the current focus, with examples
and grouping. Today the footer hints are context-aware but only show 4–6
keys at a time; a one-shot help panel would let users learn the full
surface without staring at the source.

Conflict: `?` already toggles the description panel. Either:
- Rename description toggle to `D` (might disrupt muscle memory if used).
- Use a longer key for help (`g?` two-key sequence requires a leader-key
  state machine, which we don't have yet).
- Use `K` (Vim-style help) — free, and matches the convention.

### 2. Loading indicator on dashboard refresh / PR open
Today `r` on the dashboard freezes the UI for ~1 second while the GraphQL
request runs. Same for `Enter` to open a PR (fetch + worktree set-up).
Simplest fix: render a single intermediate frame with a centered
`Loading…` line before the blocking call. No spinner needed.

Note: requires nudging the event loop so the draw fires before
`block_in_place`.

### 3. Hide resolved threads by default
Currently resolved threads render dimmed but still take vertical space.
Add a toggle (`H`?) that filters them out of the layout. Persist the
preference in `Session` so it survives re-opens. This matches GitHub's
default behavior on prs.

### 4. Confirmation status row for `X` (delete)
The two-step delete already armed the comment, but the user only sees a
red status line. Make it yellow and persistent until either cleared or
confirmed, so it's hard to miss.

### 5. Empty-state hints
- File panel with no entries: "No files in this PR — try `prowler
  dashboard` to pick another PR."
- File with no diff (rare, e.g. mode-only change): "No content changes
  for this file."
- Dashboard with no PRs in any section: "Nothing to review here. Try
  another repo."

### 6. Multi-line selection mode (`V`)
Enables both **multi-line comment posting** and **post-local-hunk-as-
suggestion** (which together unblock several backlog items). Vim-style:
- `V` enters visual-line mode on the diff panes.
- `j`/`k` extends the selection.
- `c` posts a thread anchored to the span.
- `Esc` cancels.

Render selected rows with a colored gutter.

### 7. Persist expanded-thread state across opens
`expanded_threads` is in-memory. Persisting in `Session.expanded_threads`
means re-opens preserve the user's intent (e.g. a long thread they were
mid-read on). Cost: another `HashSet<String>` in the toml.

## Quality-of-life

### 8. File panel filtering (`/`)
Vim-style fuzzy filter: `/foo` narrows the file panel to paths containing
`foo`. Useful in PRs with > 30 files.

### 9. Show diff stats on dashboard PR rows
Currently the row shows `+10 -2 💬3 @alice`. Adding number-of-files
(`5 files`) helps the user pick — a 5-file PR is reviewable in 10 min,
a 50-file PR is not. GraphQL: `pullRequest.changedFiles`.

### 10. Sort dashboard sections in a way that surfaces "needs attention"
Today: sorted by `updatedAt` desc. Better: rank by:
- PRs without a recent review by viewer first
- Then by recency

This requires the viewer's own previous reviews per PR — extra GraphQL.

### 11. Unread-comment count on file panel
Today the file row shows total unresolved count. Distinguish "has new
comments since you last opened this file" from "unread"-style. Requires
storing last-opened timestamp per file in `Session`.

### 12. Keymap discovery in status row
After landing on a context (e.g. cursor on viewer's own comment), flash
a one-second hint: "Tip: M edit, X X delete". Eventually noisy — gate
behind a `--first-run` mode that disables itself after N hits.

### 13. Diff coloring tweaks
- Use blue (instead of green) for added lines and dim red for removed,
  per IDE convention. Today's bright green-on-dark works but feels
  garish for long sessions.
- Render whitespace-only changes with a marker (`·`) so they don't blend
  into context.

### 14. PR description rendered as markdown
Today `?` shows the body as plain text. Bold/italics/lists/code blocks
would all render with a real markdown library (`pulldown-cmark` →
ratatui spans). Cost: ~150 LoC.

### 15. Smarter `q` semantics
Today `q` always returns to dashboard. When prowler was launched directly
into review (`prowler review --pr 123`), `q` quits the binary. Some
users may want `q` to always quit and `Esc` to return — make this a
config knob.

### 16. Highlight cursor's anchor line on the *opposite* pane
When the cursor is on row Y in HEAD with `head_line == 42`, dim or box
the corresponding row on BASE so the eye can track context across panes.
Particularly useful in long Removed-followed-by-Added sequences.

## Polish & infrastructure

### 17. Mock GitHub client
Already in backlog. Lets us drive end-to-end tests for post → refresh
flows without hitting the network.

### 18. `prowler review --pr 123` after closed PR
Today: errors out. Better: still open the worktree (if it exists) and
show the diff against the closed-state head, with a `CLOSED` badge.
Helps for retroactive review of merged PRs.

### 19. CI / GitHub Actions integration
Show CI status (`statusCheckRollup`) on the dashboard PR row and in the
review header. Failing checks change the badge to red.

### 20. Reaction support
Render :+1: / :rocket: counts on comments; key to add a reaction (`+`).

### 21. Configuration file
`~/.config/prowler/config.toml` for editor command, default sort mode,
hide-resolved default, color theme, etc. Most of these are currently
hard-coded.

### 22. Theme support
At minimum a high-contrast / low-contrast switch. The current dark theme
is hard-coded; some terminals invert it badly.
