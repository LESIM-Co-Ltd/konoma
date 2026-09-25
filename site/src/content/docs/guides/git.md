---
title: The git suite
description: Changes hub, full-screen diffs, log, a custom-rendered commit graph, and branch management — without leaving the browser.
sidebar:
  order: 4
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

From the hub, from the tree (`d` on any changed file), or from follow mode. A diff has up to 3
presentations, cycled with `R` (`[ui] diff_view`, default `rendered`):

- **`rendered`** — a Markdown file's diff drawn as decorated blocks: removed content in red/dim,
  added in green, changed-to in amber — so an edit's *deletions* stay visible, not only its final
  result. Falls back to `source` for a non-Markdown file, which has no block-diff renderer. **For a
  changed image or PDF, `rendered` is the side-by-side old/new picture** (see "Media diffs" below);
  those two kinds have no plain-text `source` to fall back to.
- **`source`** — the classic unified/split diff (an SVG's raw markup diff, for the one kind that is
  both a picture and real text).
- **`preview`** — the file's ordinary preview, with a change gutter on the current content (for an
  image/PDF, just the new version, no overlay); `R` there returns to the diff instead of toggling
  raw source.

`R` cycles between whichever presentations the file being diffed actually has: all 3 for Markdown
and SVG, `source ⇄ preview` for any other text file, `rendered ⇄ preview` for an image/PDF, none
for a summary-only binary (no hint is shown for those — see the media section below). For a file
over 5,000 lines or 1 MiB, `rendered`/`preview` stop at the same position an ordinary preview
would — switch to `source` (`R`) to see changes past that point.

- `s` cycles the `source` layout: unified (vertical) → split (side by side) → auto. On a media
  diff, `s` instead cycles *that* view's own layout (below).
- `n` / `N` jump straight to the next/previous changed file's diff without
  leaving the view — the title shows `(2/5)`. Reviewing a whole change set is
  one keystroke per file; the current presentation is kept as you move between files.
- `x` discards the whole file's changes (confirmed).

## Media diffs

A changed image, GIF, SVG, or PDF page shows its old and new versions **next to each other**
instead of a useless "binary files differ" line — no pixel diffing or highlighting, just the two
pictures at one shared scale, so a resize stays visible and nothing is ever enlarged past its
natural size.

- **Layout**: `[git] media_diff` picks the initial layout — `"auto"` (default) lays the pair
  side by side or stacked, whichever lets the picture draw larger; `"side"` always left/right,
  `"stack"` always top/bottom. `s` cycles auto → side → stack → auto at runtime.
- **Captions**: each pane names the base it's actually comparing against — `HEAD` / jj's `@-` / (in
  a follow-opened diff) the follow-session snapshot taken when `F` was pressed — plus dimensions,
  file size, and the PDF page. A follow session with no usable snapshot for that file (over the
  5 MiB per-file cap) falls back to the committed base and the caption says so honestly, rather
  than claiming "since follow-start" for a comparison it didn't actually make.
- **PDF paging**: `J` / `K` (also `PageDown` / `PageUp`) turn both sides' page together — only
  while a multi-page PDF diff is showing.
- **New / deleted / identical / undecodable**: a brand-new file shows only its new side (old pane:
  "new file"); a deleted one shows only its old side; byte-identical content says so plainly; a
  side that fails to decode (corrupt image, encrypted/unparsable PDF) shows "cannot display: …" in
  its own pane while the other side still draws.
- **Everything else** — video, archives, other binaries, or either side over the 64 MiB cap — has
  no picture to draw, so `rendered` degrades to a **one-line summary**: sizes and the byte delta
  (`(+3 B)` / `(-3 B)` / "same size, content differs"), or the honest "(no changes)" when the bytes
  really are identical. This replaced a real bug: git/jj always return an *empty* line diff for
  binary content, which konoma used to read at face value as "no changes" even when the file had
  in fact changed.
- `R`, `n`/`N`, and the presentation-cycling rules above all apply the same way to a media diff as
  to a text one.

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
(confirmed), `/` filters. Branches checked out in another worktree appear here
too, though checking one out fails — git keeps a branch in one worktree at a
time, and says which one holds it.

## Worktrees — `w`

Lists the repository's worktrees with their branch, short hash and path, marking
the one you are in. `Enter` makes it this tab's root, `Ctrl-t` opens it in a new
tab, and `d` diffs it against its base branch — committed and uncommitted work
together. See [Working with an AI agent](../agent-watch/) for why that matters when an
agent is doing the committing.

## jj (Jujutsu) repositories — preview

**This is a preview.** The surface below is complete and was checked against jj's own output in
every layout jj can produce — colocated, non-colocated, and a workspace — but it has not lived
through long everyday use yet. `jj workspace` has no list of its own, and jj is pre-1.0: it ships breaking changes monthly, and
konoma probes the `jj` on your machine rather than pinning a version — if it cannot answer, konoma
falls back to git and nothing else changes.

**Nothing changes for a repository that already worked.** By default git keeps answering wherever it
can, so a colocated repository (`.git` and `.jj` side by side) looks exactly as it did. jj answers
where there is no git repository to ask: one created with `jj git init --no-colocate`, and every
`jj workspace`. Set `[external] vcs = "jj"` to use jj in a colocated repository too.

Where jj does answer, it answers for everything this page describes: the tree's markers and
dimming, the diffs, the changed-file list, follow mode, and the hub.

What you see is jj's, not git's wearing jj's data:

- The chip names the working-copy commit (`@ owyqxpku side B`) rather than a
  branch. jj tracks none, and in a colocated repository git itself is left
  detached — "HEAD" would be a lie.
- A new file reads as added, not untracked: jj has no untracked state.
- The graph uses jj's own range and symbols — `@` working copy, `○` commit,
  `◆` immutable, `×` conflict — and names rows by change ID, the name a commit
  keeps when jj rewrites it. `a` widens it to every revision. Another
  workspace's checkout appears as an ordinary commit labelled `ws-second@`.
- `b` lists bookmarks and marks none of them current, because a bookmark does
  not follow the working copy the way a branch follows HEAD.
- `!` launches `[jj] tool` (default `lazyjj`).

**konoma only reads a jj repository.** Every call it makes carries
`--ignore-working-copy`, so it never snapshots your working copy — without
that, a status refresh would write a commit, and konoma refreshes on every
file change. Keys that would write are not offered.

Because it never writes, konoma tracks changes itself rather than asking jj,
which means a file an agent wrote a second ago shows up even though jj has not
noticed it yet. The one thing that escapes this is a file whose contents
changed while its timestamp did not; `R` in the hub asks jj for a snapshot to
settle it, after a confirmation (`[ui] confirm_jj_sync`).

Not there yet: `jj workspace` has no list of its own (`w` says so), and staging
has no counterpart — jj has no index, and `jj split` selects after the fact
rather than before.

## External tool — `!` (inside the changes hub)

For anything beyond the built-ins (rebase, stash surgery…), `!` (inside the changes hub, `o`) suspends
konoma and launches your configured tool (`git.tool`, default `lazygit`),
returning to the same spot when you exit.
