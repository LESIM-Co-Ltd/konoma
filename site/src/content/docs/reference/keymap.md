---
title: Default keymap
description: The default key bindings per screen. Every key is rebindable via [keys] in the config.
sidebar:
  order: 2
---

Every binding below is a default — all of them can be changed per screen via
`[keys.<surface>]` in the config (see
[Configuration](../configuration/)). `?` shows this
information contextually inside the app.

## Global (every screen)

| Key | Action |
|---|---|
| `?` | help for the current screen |
| `Q` | quit (confirmation unless `ui.confirm_quit = false`; `qq` is quick) |
| `t` | new tab (closing is `q` on the tree; `w` is deliberately unbound — vim word-motion muscle memory) |
| `[` / `]` / `1`-`9` | previous / next / numbered tab |
| `F` | follow mode (auto-show whatever changes on disk) |
| `T` | tab list (switch / close tabs; the tab bar shows `‹n / n›` when tabs overflow) |

## Tree

| Key | Action |
|---|---|
| `j` `k` / `g` `G` | move / first / last |
| `l` | descend into directory (files: preview) |
| `h` | parent directory |
| `Enter` | expand/collapse in place (files: preview) |
| `/` | filter as you type (`Esc` clears) |
| `.` | toggle dotfiles |
| `s` / `i` / `r` | sort menu / file info / refresh |
| `e` | open in external editor |
| `p` | cycle path display (relative / `~` / full) |
| `v` / `V` | visual range selection / toggle one selection |
| `Space` → `n r d c x p` | create / rename / delete / copy / cut / paste |
| `y` → `n r f p @` | copy name / relative / full / parent / `@ref` |
| `m` + letter / `'` | set bookmark / bookmark list |
| `a` / `A` | anchor display root here / reset anchor |
| `o` / `d` | git changes hub / diff of the cursor file (the external git tool |
| `C` / `n` `N` | changed-files view / jump between changed files |
| `q` | close tab (last tab: quit) |

## Text / Markdown preview

| Key | Action |
|---|---|
| `j` `k`, `Ctrl-d` `Ctrl-u`, `g` `G` | scroll / half page / ends |
| `h` `l`, `0` `$` | horizontal scroll & line ends (wrap off) |
| `/` `n` `N` | search / next / previous match |
| `Tab` / `Shift-Tab` | focus next / previous link or checkbox (Markdown) |
| `Enter` | open focused link / toggle focused checkbox |
| `Space` | toggle focused checkbox |
| `R` | toggle rendered ⇄ raw source (Markdown/Mermaid) |
| `o` | heading outline of a Markdown preview (`Enter` jumps to a heading) |
| `v` / `V` | select by character / by line, then `y` copies |
| `Y` | copy `@path#L12-34` reference |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file (tree order, wraps) |
| `m` + letter / `'` | bookmark the previewed file / bookmark list |
| `e` / `q` | external editor / back to tree |

## Image / PDF preview

| Key | Action |
|---|---|
| `+` `-` / `0` `=` | zoom / reset to fit |
| `h j k l` | pan |
| `J` `K` | PDF page down / up |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file (tree order, wraps) |
| `m` `'` / `e` / `q` | bookmark / editor / back |

## CSV / TSV table preview

| Key | Action |
|---|---|
| `h j k l` | move by cell |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file (tree order, wraps) |
| `g` `G` / `0` `$` | first/last row / first/last column |
| `/` / `n` `N` | search cells (case-insensitive) / next, previous match (wraps) |
| `y` → `c r C f` | copy cell / row / column / full path |
| `Enter` | Open the cursor cell in a popup — the full, untruncated value, wrapped and scrollable (`Enter` / `q` / `Esc` closes). Also works on `.zip` / `.tar` listings. |
| `m` `'` / `e` / `q` | bookmark / editor / back |

## Git

| Key | Action |
|---|---|
| Hub (`o`): `s` `u` / `S` `U` | stage/unstage file / all |
| Hub: `c` / `x` / `Enter` | commit / discard file / open diff |
| Hub: `l` / `g` / `b` / `w` | log / commit graph / branches / worktrees |
| Diff: `s` / `n` `N` / `x` | layout unified⇄split⇄auto / next/prev changed file / discard |
| Follow diff: `f` | toggle scope: since follow-start (default) ⇄ full git diff |
| Log & graph: `Enter` / `y` | commit detail (full message + diff) / copy commit info |
| Graph: `s` / `x` `0` / `b` | pin base branch / unpin / branch picker |
| Branches: `Enter` / `n` / `d` / `/` | checkout / create / delete / filter |
| Worktrees (`w`): `Enter` / `Ctrl-t` | switch this tab to it / open it in a new tab |
| Worktrees: `d` / `/` | diff it against its base branch / filter |
| Worktrees: `n` | create one — asks for a branch name, then switches into it |

## Tab list (`T`)

| Key | Action |
|---|---|
| `1`-`9` / `Enter` | switch to that tab / to the selection |
| `j` `k` | move |
| `d` | close the **selected** tab (list stays open; the last tab refuses) |
| `T` `q` `Esc` | close the list |

## Bookmark list (`'`)

| Key | Action |
|---|---|
| any letter | jump to that bookmark (a-z local / A-Z global) |
| `j` `k` + `Enter` | select + jump |
| `Ctrl-e` / `Ctrl-d` | edit target in editor / delete bookmark |
| `'` `q` `Esc` | close |

Letters used by the list or global keys (`j` `k` `q` `t` `T` `F` `Q`) can't
letter-jump — select those with `j`/`k` + `Enter`, or rebind.
