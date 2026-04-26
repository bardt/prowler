# prowler

A terminal UI for deep GitHub PR review. Inspired by lazygit's UX philosophy.

## What it does

`prowler review --pr 123` opens a TUI for reviewing a pull request:

- Side-by-side diff (Base | Head) with syntax highlighting
- File panel listing every changed file with lines added/removed
- Open either side in `$EDITOR` at the current line with full LSP (`e` / `E`)
- Inline GitHub review comments rendered next to their anchor lines
- Per-file viewed state, synced bidirectionally with GitHub

The differentiator: a `git worktree` is spun up for the PR branch so the
reviewer can edit freely without touching their current working branch. The
worktree survives TUI quit and is reused on the next open.

## Install

```sh
cargo install --path .
```

Requires Rust 1.95+ and the `gh` CLI for authentication.

## Usage

From inside any clone of a GitHub-hosted repository:

```sh
prowler review --pr 123
```

To remove the worktree and clear the local session:

```sh
prowler review --pr 123 --cleanup
```

## Authentication

Resolved at startup, in priority order:

1. `GITHUB_TOKEN` env var
2. `gh auth token` subprocess

No credentials are stored by the tool.

## Key bindings

| Key | Action |
|-----|--------|
| `j` / `k` | Move cursor |
| `]` / `[` | Next / prev hunk |
| `Tab` | Cycle panel focus |
| `e` | Open HEAD in `$EDITOR` at the cursor line |
| `E` | Open BASE in `$EDITOR` at the cursor line |
| `v` | Mark current file viewed |
| `s` | Skip current file |
| `c` | Post a new inline comment |
| `q` | Quit |

## Status

Early development. Posting a comment creates a draft review under your account
on GitHub; submitting the review (Approve / Comment / Request changes) is not
yet implemented and is the next planned milestone.
