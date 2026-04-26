# Changelog

All notable user-facing changes will be tracked here. The project is in early
development — every entry is subject to change until a tagged release.

## Unreleased

### Added

- Inline review-comment posting via `c` (M9). Opens `$EDITOR`, submits the
  body via the `addPullRequestReviewThread` GraphQL mutation, then re-fetches
  threads so the new comment appears inline.
- Inline review-comment reading (M8). Threads render under their anchor line
  on the BASE or HEAD pane with box-drawing connectors.
- Per-file viewed state with bidirectional GitHub sync (M7).
- Editor handoff (`e` / `E`) to `$EDITOR` for HEAD and BASE worktrees, with
  full LSP on both sides (M6).
- Side-by-side diff rendering with syntect-based syntax highlighting (M5).
- TUI shell with a file panel, BASE / HEAD diff panes, hotkey footer (M4).
- Diff data model and computation (M3).
- Worktree lifecycle and `.review/sessions/{pr}/state.toml` persistence (M2).
- GitHub authentication and PR metadata fetch (M1).
