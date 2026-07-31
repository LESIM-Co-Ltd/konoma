---
title: Working with an AI agent
description: Follow mode, the changed-files view, and @path references — konoma as the review pane next to your coding agent.
sidebar:
  order: 1
---

konoma was built for one layout: **konoma on the left, an AI coding agent
(Claude Code, etc.) on the right**. The agent edits; you watch, review, and
steer — without ever leaving the keyboard.

## Follow mode — `F`

Press `F` anywhere. From now on, whenever a file changes on disk, konoma
automatically shows a diff of what changed **since you pressed `F`**
(hunk-level before/after, the same full-screen diff as the git suite) — any
changes the file already had before you started following stay hidden, so
what you see is exactly the agent's new work. The agent saves a file → you're
looking at exactly that edit, hands off.

<img src="/konoma/follow.gif" alt="Follow mode — the file already has an uncommitted edit; after F, konoma shows only the lines the agent adds, and f reveals the full git diff" width="1268" height="768" style="width:100%;height:auto;border-radius:8px" />

*`src/main.rs` starts with an uncommitted line of your own (`// TODO: handle
retries`) — the plain git diff shows it. After `F`, the agent appends a function
and konoma opens the file by itself, with that new function as the only
highlighted change: your earlier edit has dropped back to context. `f` switches
to the full git diff, where both changes are highlighted. The agent then edits
`docs/guide.md` and konoma follows again.*

Details that make it comfortable:

- **Follow is sticky.** Reading the diff — scrolling, `n`/`N` (cycling
  changed files), `f` (below) — doesn't turn it off. Only `q` (leaving the
  diff), entering a text-input/confirm prompt, or pressing `F` again stops
  following.
- Press `f` inside a follow diff to switch between the diff **since
  follow-start** (default) and the **full git diff** for that file — the
  title shows which one you're looking at (`· since follow start` /
  `· full diff`).
- Untracked (new) files show as an all-added diff. Files with no diff and
  media files (images, PDFs, …) open as a normal preview instead, scrolled to
  the first change.
- Rapid multi-file edits are rate-limited to about one view-switch per second
  (the latest change wins), so bursts don't thrash the screen.
- While a diff is on screen, `n` / `N` cycle **through the files changed in
  this follow session** — the title shows your position like `(2/5)`. Files
  that were already dirty before you pressed `F` don't clutter the loop.
- `.git` internals, gitignored and hidden files are never followed.
- `ui.follow_view = "file"` switches the default presentation from diff to a
  normal preview with the git gutter, scrolled to the first changed hunk.

## Changed-files view — `C`

`C` flattens the tree into just the files with uncommitted changes (relative
paths, live-updating, status markers). Review an agent's work top-to-bottom:

- `Enter` previews, `d` shows the diff.
- `n` / `N` also work from the normal tree — they jump to the next/previous
  changed file, expanding collapsed directories as needed.
- `C` or `h` returns to the normal tree. The view exits by itself when the
  change set becomes empty (e.g. after a commit).

## Point the agent at things — `@` references

Coding agents accept `@path` references. konoma copies them ready-to-paste:

- Tree: `y` → `@` copies `@relative/path` for the selected entry.
- Text preview: `Y` copies `@path#L12` for the caret line, or `@path#L12-34`
  for a `v`/`V` selection.

Paste into the conversation and the agent knows exactly which lines you mean.

## Worktrees — one checkout per agent — `o` `w`

Agents work best with a checkout of their own, which is why `git worktree` has
become the usual way to run several at once. konoma lists them: `o` then `w`.

<img src="/konoma/worktrees.gif" alt="Worktrees — listing the repository's worktrees, showing what an agent's worktree added since its base branch, and stepping into it with the WT chip naming the repository" width="1268" height="768" style="width:100%;height:auto;border-radius:8px" />

*Your own checkout has an uncommitted edit of your own. `o` `w` lists the
worktrees; the agent has one to itself. `d` shows what that worktree added since
its base branch — the commit it landed **and** the line it is still writing. After
`Enter` you are inside it, and `WT demo` names the repository `add-retry` belongs
to.*

- **`d` spans committed and uncommitted work.** An agent usually commits as it
  goes, so a diff of uncommitted changes alone would go blank exactly when it got
  something done. konoma diffs from the merge base instead, and includes untracked
  files, so a new file the agent just wrote is visible too.
- **The base branch is the nearest one.** Among the branches in
  `graph_base_branches` and the one checked out in the main worktree, konoma picks
  whichever shares the most recent common ancestor — the branch the worktree
  actually grew out of, so unrelated work on other branches stays out of the diff.
- **`Enter` moves this tab, `Ctrl-t` opens another.** Switching also re-bases
  `@` references on the worktree you are now in, so what you paste into the chat
  points at the right file.
- **The `WT` chip is always there.** A worktree's directory is named after the
  branch, not the project, so nothing on screen would otherwise say which
  repository you are in. In an ordinary checkout no chip appears.
- Worktrees git will not let you enter — a bare repository, or one whose directory
  has been deleted — say so instead of failing quietly. A locked one can still be
  entered: the lock guards moving and deleting it, not reading it.

## A typical loop

1. `F` — follow on. Ask the agent for a change.
2. Watch the diffs land as the agent works; `n`/`N` to flip between files.
3. Spot something off? `Y` the exact lines, paste the `@path#L12-34` reference
   into the chat with your comment.
4. Happy? `o` → stage → `c` commit, all inside konoma.
