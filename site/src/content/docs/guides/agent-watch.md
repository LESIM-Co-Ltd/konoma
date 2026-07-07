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
automatically shows **the diff of that file** (hunk-level before/after, the
same full-screen diff as the git suite). The agent saves a file → you're
looking at exactly what changed, hands off.

Details that make it comfortable:

- **Any other key takes control back** (you grabbed the keyboard, so konoma
  stops driving — Zed-style). Press `F` again to resume.
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

## A typical loop

1. `F` — follow on. Ask the agent for a change.
2. Watch the diffs land as the agent works; `n`/`N` to flip between files.
3. Spot something off? `Y` the exact lines, paste the `@path#L12-34` reference
   into the chat with your comment.
4. Happy? `o` → stage → `c` commit, all inside konoma.
