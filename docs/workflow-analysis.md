# Workflow analysis — propositions for review

Audit of mental-model gaps and broken / confusing workflows in v0.1.0,
written before serious dogfooding so we can spot what the user *won't*
catch from a few sessions of normal use. Each item: what it is, why
it's a problem, and a proposed fix. Severity is a rough sort key, not
a release-blocker label.

## High severity — data-loss or correctness

### W1. Force-push abandons local edits

**Scenario.** User opens PR #123, edits a file with `e`, saves but
doesn't commit. PR author force-pushes. User presses F5; we detect
`head_sha` changed and refuse to refresh: *"PR moved (abc1234 →
def5678). Quit (q) and reopen to fetch new commits."* User quits.
On re-open, the new worktree path is
`/tmp/prowler-{repo}-{pr}-{def5678}` — different from the old one.
Their edits are stranded in the old worktree dir, never surfaced
again.

**Why it matters.** Silent data loss. The user thinks they're
following a guided flow; their work is on disk but invisible.

**Proposal.** On reopen, after computing the new worktree path, scan
for any sibling `/tmp/prowler-{repo}-{pr}-*` directories that aren't
the current path. For each, check `git -C {old} status --porcelain`
— if non-empty, surface a warning in the status row: *"Stranded edits
at /tmp/prowler-...-{abc1234}. Discard with `prowler review --pr
{N} --cleanup`, or `cd` and `git diff` to recover."* Don't auto-merge
— that's risky. Just don't lose track.

**Severity.** High.

### W2. Concurrent prowler instances on the same PR

**Scenario.** User has two terminals open. Both run
`prowler review --pr 1`. Both load `.review/sessions/1/state.toml`.
Both periodically save it (cursor moves, expanded threads, viewed
state). Last writer wins; the other instance overwrites silently.

**Why it matters.** Surprising loss of state. Edge case but easy to
hit (e.g. tmux split, accidental second open).

**Proposal.** On Session save, lock-and-write atomically: write to
`state.toml.tmp`, rename. (Already mostly atomic on POSIX, but make
explicit.) Beyond that, advisory PID-locking via `state.toml.lock`
on the first session load — if a lock exists and the PID is alive,
either refuse or show a warning. Probably overkill; the atomic-rename
fix alone is fine for v0.1.x.

**Severity.** Medium-high.

### W3. Worktree drift not detected on reuse

**Scenario.** Worktree at `/tmp/prowler-{repo}-{pr}-{sha}` exists.
We reuse it without verifying `git -C {worktree} rev-parse HEAD`
matches `sha`. If something else moved the worktree's HEAD (manual
git ops, prowler crashed mid-`worktree add`), the user sees a diff
against the wrong content.

**Why it matters.** Silent corruption — the diff is wrong but the
TUI looks healthy.

**Proposal.** On reuse, verify `rev-parse HEAD` matches the expected
sha. If mismatch, log to status row and offer to recreate the
worktree (or run `git -C {worktree} reset --hard {sha}`).

**Severity.** Medium.

## Medium severity — confusing UX

### W4. Multi-line selection across hunk boundaries

**Scenario.** User in mode 1, presses `V`, then `j` enough times to
extend selection past a hunk header into the next hunk. `c` posts
the comment with `startLine` from the first hunk and `line` from the
second. GitHub renders the comment as anchored to a span; the
intervening hunk header is "between" them on GitHub's UI, which
looks weird and makes the comment span unclear.

**Why it matters.** User intent ("comment on these contiguous
lines") doesn't match the result ("comment on a span across
unrelated regions"). GitHub may even reject some such anchors.

**Proposal.** In `start_selection` or `move_cursor` while selection
active, refuse to extend across a hunk boundary — clamp the
selection within the hunk that contains the anchor. Status row
hint when clamped: *"Selection limited to the current hunk."*

**Severity.** Medium.

### W5. Suggestion in mode 2 with all-Context selection

**Scenario.** User in mode 2 (`HeadLocal`), selects rows with `V`
that contain only `Context` lines (i.e., lines unchanged between
HEAD and worktree). `c` posts a "suggestion" whose body is the
unchanged code. GitHub accepts it, displaying as a suggestion that
changes nothing.

**Why it matters.** Wastes the reviewer's time on a no-op
suggestion. Easy to do by accident if the user just wants to
*comment* on a HEAD-side line in mode 2.

**Proposal.** Detect: if `body_lines` from `suggestion_from_selection`
exactly equals the head_sha content for those lines, refuse with a
status hint: *"Suggestion is identical to HEAD — comment without `V`
to comment instead, or extend selection to include changed lines."*
Cheap to compare since we have both texts.

**Severity.** Medium.

### W6. `c` from a Removed line in BASE pane

**Scenario.** User in mode 1, BASE pane focused, cursor on a
`Removed` row (code that's gone in HEAD). `c` posts a comment on the
BASE side at the original line. GitHub renders it as a comment on
deleted code.

**Why it matters.** Reviewers often comment on removed code to
explain *why* the deletion is wrong. So it's legitimate. But for new
users it might feel like the comment "vanishes" because GitHub's
default view collapses BASE-side comments.

**Proposal.** Status hint on first BASE-side comment per session:
*"Posted on BASE — visible on GitHub via the 'show outdated' toggle
unless the reviewer expands it."* One-shot; later occurrences
silent.

**Severity.** Low-medium.

### W7. Filter `/` orphans an active selection

**Scenario.** User in file A with a `V`-selection. Presses `/`,
filters file panel to exclude A. Selection state stays in
`ReviewState.selection` (with `file_idx` pointing to A) but A isn't
visible. Cursor moves to first match. Selection appears inactive
visually because the file isn't on screen, but `selection_active()`
still returns true. If user types `c` they'd post on A's selection
without seeing it.

**Why it matters.** User confusion + accidental posts.

**Proposal.** When `start_file_filter` runs, clear any active
selection. Status hint: *"Selection cleared — filter active."*

**Severity.** Medium.

### W8. Status messages collide and the older one is lost

**Scenario.** Background poller surfaces *"+2 threads on GitHub"*.
Three seconds later (or earlier if they fire close together), a
user-action status replaces it. User saw the poller's hint only if
they looked between polls.

**Why it matters.** Poller info is the only feedback that GitHub
state has changed; missing it means F5 never gets pressed.

**Proposal.** Either (a) queue status messages with sticky
behaviour — info stays until next user action, success/error TTL
out as today; or (b) add a small persistent "activity" indicator in
the header showing pending threads/comment counts since last
refresh. (b) is cleaner.

**Severity.** Medium.

### W9. Discoverability of `?` (help)

**Scenario.** New user opens prowler. The footer hotkeys are
context-aware and don't always include `?`. A first-time user might
never find the help overlay.

**Why it matters.** TUIs live or die by discoverability.

**Proposal.** Add `?` to the footer at all times, or show a
first-run tip *"Press ? for help"* on the first open of each
session, dismissed after the first `?` press.

**Severity.** Low-medium (matters for adoption, not for a happy
existing user).

## Low severity — corner cases & polish

### W10. PR closes/merges while open

**Scenario.** User reviewing PR #123. Author merges. Background
poller detects nothing (it diffs counts, not state). User submits
`APPROVE`. GraphQL returns an error or no-op. Status row shows
failure.

**Why it matters.** Wasted typing.

**Proposal.** On `apply_refresh`, compare PR state against the
known one. If it changed (OPEN → MERGED/CLOSED), surface a one-time
banner: *"PR merged — submitting a review now is no-op."*

**Severity.** Low.

### W11. PR description renders as plain text

**Scenario.** `D` opens the description panel. PR body has
markdown — bullet lists, code blocks, links. We render as plain
text. Long PRs are hard to scan.

**Why it matters.** Cosmetic, but the description is where context
lives.

**Proposal.** Use `pulldown-cmark` (or similar) to parse markdown,
render with limited formatting (bold/italic/code ↔ ratatui spans).
~150 LOC; the cost / value is good.

**Severity.** Low.

### W12. No way to jump to PR on GitHub

**Scenario.** User wants to open the PR in their browser
(screenshot, share link, leave a reaction). Nothing in prowler does
this.

**Why it matters.** Friction for cross-team workflow.

**Proposal.** Bind `O` (uppercase, capital-O for "Open in browser")
to `Command::new("open").arg(pr_url)` (macOS) /
`xdg-open` (Linux). The PR URL is already in `DashboardPr.url` and
can be derived from `owner/repo/pr_number` for the review view.

**Severity.** Low.

### W13. Long file names truncate awkwardly

**Scenario.** File panel is fixed at 36 cols. Long paths overflow.
The list widget clips silently; nothing tells the user the row
content is incomplete.

**Why it matters.** Hard to identify deeply-nested files at a
glance.

**Proposal.** Detect overflow per row; replace the middle of the
path with `…/` (similar to what `previous_path_label` already does
for renames). Or right-align the leaf so the basename is always
visible.

**Severity.** Low.

### W14. No `Esc → focus Files` shortcut

**Scenario.** User mid-diff, wants to switch files. They press
`Tab`, `Tab`, `Tab` to cycle through to Files. Or `1`. `Esc`
currently only clears overlays / selection.

**Why it matters.** Vim-trained muscle memory.

**Proposal.** When no overlay / selection is active, `Esc` goes to
`Focus::Files`. Cheap and intuitive.

**Severity.** Low.

### W15. Vim users expect `h` / `l` to move

**Scenario.** Currently `h` is unbound (could conflict with `H` =
hide-resolved? No — case-sensitive in `KeyCode::Char`). `l` is
unbound too (was `L` for toggle local; lowercase `l` free).

**Why it matters.** Cheap addition to feel more vim-like.

**Proposal.** Bind lowercase `h` / `l` as aliases for `←` / `→`
horizontal scroll.

**Severity.** Low.

### W16. Compose buffer lost on network failure

**Scenario.** User types a long comment, saves and exits, prowler
posts → network error. Compose buffer was deleted. User has to
retype.

**Why it matters.** Annoying. Not data loss in the worktree sense
but real lost typing.

**Proposal.** On post failure, *don't* delete the temp file;
leave it at `/tmp/prowler-compose-{pid}.md` and surface its path in
the status row: *"Post failed; draft kept at /tmp/prowler-compose-
{pid}.md."*

**Severity.** Low-medium.

### W17. Help overlay scrolls off-screen on small terminals

**Scenario.** Help has ~40 lines. Terminal is < 30 rows tall. The
last sections are hidden with no scroll indicator.

**Why it matters.** Help that's incomplete is misleading.

**Proposal.** Make help scrollable with `j`/`k`. Add a scroll
indicator in the title (`Keymap [1/3]`).

**Severity.** Low.

### W18. `--json` flag is undocumented

**Scenario.** `prowler review --pr 1 --json` exists, was used during
M3 / M4 development as a debug-export. Not in README or `--help`.

**Why it matters.** Either it's a public feature (then document it),
or it's dev-only (then drop it or hide behind `--debug`).

**Proposal.** Either remove or move behind a `--debug` subcommand.
Probably remove for v0.1.x simplicity.

**Severity.** Low.

### W19. Dashboard "Local sessions" can include merged/closed PRs

**Scenario.** Worktree from a merged PR sticks around in
`/tmp/prowler-...`. Dashboard "Local sessions" lists it forever
(until user `d`s it). After many PRs, this list balloons.

**Why it matters.** Clutter; inviting users to open stale work.

**Proposal.** On dashboard fetch, cross-reference local sessions
against the PR list — if a session's PR is merged/closed, dim it
and add a `[merged]` / `[closed]` tag. Keep `d` for cleanup; maybe
prompt to bulk-clean closed sessions.

**Severity.** Low.

### W20. Comment context preview clips long lines

**Scenario.** I added `# > NN: text` lines to the compose prompt
(M18) so the user sees what they're commenting on. Long lines run
off the editor width. Most editors soft-wrap; some don't.

**Why it matters.** Cosmetic.

**Proposal.** Truncate context lines to ~80 cols with `…` suffix.
Or wrap with continuation prefix `# >    {wrapped}`.

**Severity.** Low.

### W21. Refusing the file `git show` for renamed-on-rename

**Scenario.** A file was renamed multiple times (a → b → c). We
record only one `previous_path` (b → c) but `compute_diffs` does
`git show base_sha:b`, where `base_sha` only knows the file as `a`.
The git command fails.

**Why it matters.** Edge case, but renames-of-renames happen in
long-lived branches.

**Proposal.** When `git show {base_sha}:{previous_path}` fails,
fall back to `git log --follow` or to plain `git show
{base_sha}:{path}` (same name on both sides). Worth investigating
how often this hits.

**Severity.** Low.

## Summary

The two real risks are **W1 (stranded edits on force-push)** and
**W3 (worktree drift on reuse)** — both involve diverging git state
that prowler doesn't notice. **W7 (selection-orphan-on-filter)** and
**W8 (status collision)** are the most likely to feel buggy in
normal use. Everything else is polish.

Recommended order if I were ranking by *fix value vs effort*:
W7 → W14 → W15 → W4 → W1 → W3 → W12 → rest.
