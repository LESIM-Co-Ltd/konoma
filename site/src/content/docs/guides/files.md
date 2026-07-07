---
title: Files, bookmarks & tabs
description: Filtering, file operations with a safety net, bookmarks that work from previews, tabs, and path copying.
sidebar:
  order: 4
---

## Moving around

- `j` / `k` move; `l` descends into a directory (it becomes the new root),
  `h` goes to the parent; `Enter` expands/collapses in place.
- `/` filters the tree as you type — `Enter` keeps the filter, `Esc` clears it.
- `.` toggles dotfiles; `s` opens the sort menu (name/size/modified/extension,
  reverse, dirs-first); `i` pops up file info; `r` refreshes.
- `ui.details = ["size", "modified"]` adds metadata columns to every row.

## File operations — `Space`

All file management sits behind the `Space` leader (a menu shows the options):

- `n` create (end with `/` for a directory), `r` rename, `d` delete.
- `c` copy / `x` cut / `p` paste.
- **Deletions go to the system trash** after a confirmation dialog — nothing
  is ever destroyed silently. Renames of multiple selected files open a batch
  preview before applying.
- `v` starts a visual range selection; `V` toggles selection on one entry.
  Operations then apply to the whole selection.

## Bookmarks — `m` and `'`

vim-style marks with two scopes: **lowercase = local** to the directory you
launched in, **uppercase = global** across everything.

- `m` + a letter bookmarks the tree cursor item — or, **inside a preview, the
  file you are viewing**.
- `'` opens the bookmark list immediately (local and global sections; global
  entries show absolute `~/...` paths). Press a letter to jump: directories
  become the new root, files open as previews. `Ctrl-e` edits the selected
  entry's file directly, `Ctrl-d` deletes the bookmark, `q` closes.
- Anchors: `a` re-anchors relative-path display to the current root, `A`
  resets it to the launch directory.

## Tabs

- `t` new tab, `w` close, `[` / `]` switch, `1`-`9` jump by number.
- Each tab keeps its own root, cursor, preview and scroll state.
- `q` on the tree closes the current tab (quitting the app when it's the last
  one); `Q` quits from anywhere, with a confirmation (`ui.confirm_quit`).

## Copying paths — `y`

`y` opens the copy menu for the selected entry (or the previewed file):

| Key | Copies |
|---|---|
| `n` | file name |
| `r` | relative path |
| `f` | full path |
| `p` | parent directory |
| `@` | `@relative/path` — an AI-agent reference |

## Delegating edits — `e`

konoma deliberately has no built-in editor. `e` opens the file in your editor
(`[editor]` config → `$VISUAL` → `$EDITOR` → `vim`), per-extension overrides
included, and returns to the preview with the content refreshed when you quit.
