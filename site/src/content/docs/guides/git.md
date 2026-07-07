---
title: The git suite
description: Changes hub, full-screen diffs, log, a custom-rendered commit graph, and branch management — without leaving the browser.
sidebar:
  order: 3
---

Inside a git repository, the tree already shows status: changed files are
colored, gitignored entries are dimmed, and code previews carry a change
gutter. The full suite lives one key away. (Requires `git`; konoma works fine
without it in non-repos.)

## Changes hub — `o`

The staging area. Each uncommitted file on one line with its status:

- `s` / `u` stage / unstage the selected file; `S` / `U` all files.
- `Enter` opens the file's **full-screen diff**.
- `c` commits (message prompt in-app); `x` discards a file's changes
  (confirmed).
- `l` opens the log, `g` the commit graph, `b` the branch list.
- `y` copies the selected file's path.

## Diffs

From the hub, from the tree (`d` on any changed file), or from follow mode:

- `s` cycles the layout: unified (vertical) → split (side by side) → auto.
- `n` / `N` jump straight to the next/previous changed file's diff without
  leaving the view — the title shows `(2/5)`. Reviewing a whole change set is
  one keystroke per file.
- `x` discards the whole file's changes (confirmed).

## Log — `l`

Commit list with right-aligned author/date columns. `Enter` shows the full
commit: complete message (multi-paragraph preserved) plus its diff, scrolling
as one document. `y` copies the hash / subject / full message / author / date.

## Commit graph — `g`

konoma renders the DAG itself — square box-drawing corners instead of
`git log --graph`'s diagonals, one color per lane, a legend of branch heads at
the bottom.

- `s` on a commit pins its branch as the **base**: its first-parent chain
  locks to lane 0 as a straight line and everything else folds to the right —
  ideal for reading feature branches against `main`. `x` unpins.
- `b` opens a branch picker (toggle visibility, reorder with `J`/`K`); at most
  `ui.graph_max_branches` (default 12) branches draw at once.
- `ui.graph_base_branches = ["main", "develop"]` pre-pins your convention.
- `Enter` opens the commit's detail, `y` copies commit info.

## Branches — `b`

List with ahead/behind info. `Enter` checks out, `n` creates, `d` deletes
(confirmed), `/` filters. Worktrees are listed too.

## External tool — `O`

For anything beyond the built-ins (rebase, stash surgery…), `O` suspends
konoma and launches your configured tool (`git.tool`, default `lazygit`),
returning to the same spot when you exit.
