---
title: Default keymap
description: The default key bindings by layer and by screen. Every key is rebindable via [keys] in the config.
sidebar:
  order: 2
---

Every binding below is a default — all of them can be changed per context via
`[keys.<context>]` in the config (see
[Configuration](../configuration/)). `?` shows this information contextually inside the app, always with the
keys as they are currently set.

## How the keys are arranged

The default keys are arranged in layers, so **the same key means the same thing wherever it works**:

| Layer | What it holds |
|---|---|
| 0. The app | `?` `Q` `F` `P` `t` `T` `[` `]` `1`-`9` — the same on every screen |
| 1. The basics | movement, `/` `n` `N` `r` `y` `Y` `v` `V` `m` `'` `Ctrl-t` `,` `Space` `Esc` `Enter` `q` — the same in every view that has a cursor |
| 2. One kind of view | zoom for images, `J` `K` for pages, `O` for the outline, `R` for the representation, `s` for a diff's layout |
| 3. Verbs | `e` edit · `o` open in another app · `p` the project screen · `!` the project tool |

The tree has a few keys of its own (`d` `C` `a` `A` `.` `i`), and each version-control screen has its list keys.
Outside the tree, **`Space` opens a menu of every operation that acts on the screen you are looking at** — each
under the key it already has, or a letter of its own — so nothing is ever more than one keystroke away even
when it has no key of its own.

## Layer 0 — the app (every screen)

| Key | Action |
|---|---|
| `?` | help for the current screen |
| `Q` | quit (confirmation unless `ui.confirm_quit = false`; `qq` is quick) |
| `t` | new tab, inserted right after the current one (like vim's `:tabnew`; closing is `q` on the tree; `w` is deliberately unbound — vim word-motion muscle memory) |
| `[` / `]` / `1`-`9` | previous / next / numbered tab |
| `F` | follow mode (auto-show whatever changes on disk) |
| `T` | tab list (switch / close tabs; the tab bar shows `‹n / n›` when tabs overflow) |
| `P` | jump to the path or GitHub link on the clipboard (reveal + preview; `#L12` / `:12` line anchors work) |

## Layer 1 — the basics

| Key | Action |
|---|---|
| `j` `k` `h` `l`, arrows | move (in the tree and in the lists — the Git screens, the outline / page list and the tab list — `l` opens / descends and `h` goes back; the bookmark list keeps `h` and `l` as bookmark names) |
| `g` `G`, `Ctrl-d` `Ctrl-u`, `Ctrl-f` `Ctrl-b`, `0` `$` | top / bottom, half page, page, line start / end (`ui.keys = "less"`: `d` `u`, `f` `b`) |
| `q` | go back / close (the tree: close the tab, the last tab quits) |
| `Esc` | cancel (an input, a selection, a search; otherwise go back) |
| `Enter` | the default operation of the screen (open, expand, jump, toggle a focused checkbox) |
| `/` | search (the tree and the lists filter as you type; `Esc` clears) — also on the changes list, the log, the graph, the outline / page list, the tab list and the bookmark list |
| `n` / `N` | next / previous match; **with nothing searched, next / previous change** (the tree: changed file · a code or text preview: change-gutter block · a rendered document: changed block · a diff: change block) |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file in tree order (in a diff: the next / previous changed file) |
| `r` | read again (the tree, a preview, a diff, every list of the Git screens) |
| `v` / `V` | select by character / by line (the tree: range selection / toggle one entry) |
| `y` | copy menu of the screen (see below) · `Y` copy `@path#L12-34` reference |
| `m` + letter / `'` | set bookmark (in a preview: the shown file) / bookmark list |
| `Ctrl-t` | open the thing under the cursor or focus in a new tab (tree entry · local link · changes-list row · worktree) |
| `,` | sort and display options menu (see below) |
| `Space` | operations menu (see below) |

## Layer 3 — verbs

| Key | Action |
|---|---|
| `e` | edit in the external editor, at the line you are looking at (an Office file: in an Office app — `[external] office_apps`). Works in the tree, every preview, a diff and on a row of the changes list |
| `o` | open in another app: an Office file in Microsoft Office / LibreOffice / the OS default, anything else in the OS default (`open` / `xdg-open`). Runs in the background; `[external] office_apps` / `open_links` switch it off. A directory is not opened |
| `p` | the project screen: the Git (or jj) changes list, from the tree, a preview or a diff |
| `!` | the project tool: lazygit / lazyjj (`[git] tool` / `[jj] tool`), from the tree, a preview, a diff and the changes list |

## Tree

| Key | Action |
|---|---|
| `j` `k` / `g` `G` | move / first / last |
| `l` | descend into directory (files: preview) |
| `h` | parent directory |
| `Enter` | expand/collapse in place (files: preview) |
| `/` | filter as you type (`Esc` clears) |
| `.` / `i` / `r` | toggle dotfiles / file info / refresh |
| `,` | sort and display menu: `n` name · `s` size · `m` modified · `e` extension · `r` reverse · `.` folders first · `p` path style (relative / `~` / full) · `h` hidden files |
| `v` / `V` | visual range selection / toggle one selection (`a` selects the siblings, `A` all; `q` or `Esc` cancels) |
| `Space` → `n r d c x p D` | create / rename / delete / copy / cut / paste / duplicate in place |
| `y` → `n r f p @` | copy name / relative / full / parent / `@ref` |
| `Ctrl-t` | open the entry under the cursor in a new tab (file: preview · directory: its root) |
| `m` + letter / `'` | set bookmark / bookmark list |
| `a` / `A` | anchor display root here / reset anchor |
| `d` | diff of the cursor file (git) |
| `C` / `n` `N` | changed-files view / jump between changed files |
| `e` / `o` / `p` / `!` | edit / open in another app / the Git hub / the external git tool |
| `q` | close tab (last tab: quit) |

## Text / Markdown / document preview

| Key | Action |
|---|---|
| `j` `k`, `Ctrl-d` `Ctrl-u`, `g` `G` | scroll / half page / ends |
| `h` `l`, `0` `$` | horizontal scroll & line ends (wrap off) |
| `/` `n` `N` | search / next / previous match (nothing searched: next / previous change) |
| `Tab` / `Shift-Tab` | focus next / previous link, checkbox, `<details>` or diagram (documents) |
| `Enter` | open focused link / toggle focused checkbox or `<details>` / open a focused diagram full screen |
| `Ctrl-t` | open the focused **local** link in a new tab (anchors jump in place instead; URLs open in the browser either way) |
| `J` `K` | presentations: next / previous slide, in the picture view and the text view alike (no-op elsewhere and on a one-slide deck; actions `page_next` / `page_prev`) |
| `R` | cycle the representation: rendered ⇄ raw source (Markdown/Mermaid, and Word/OpenDocument documents — raw is the converted Markdown); on a presentation, slide pictures ⇄ text view, keeping the current slide; while this preview *is* a diff's `preview` presentation, returns to that diff instead |
| `O` | heading outline of a Markdown preview (`Enter` jumps to a heading); on a PDF, a workbook or a presentation with 2+ pages, the list of its pages (`Enter` goes to the page) |
| `+` `-` `=` | zoom a focused diagram in place / zoom out / fit |
| `v` / `V` | select by character / by line, then `y` copies (`q` or `Esc` leaves the selection) |
| `y` → `n r f p @ c` | copy name / relative / full / parent / `@ref` / the focused Markdown code block (`Tab` focuses one) |
| `Y` | copy `@path#L12-34` reference |
| `,` | `p` path style · `w` line wrapping |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file (tree order, wraps) |
| `m` + letter / `'` | bookmark the previewed file / bookmark list |
| `e` / `o` / `p` / `!` / `r` / `q` | edit / open in another app / the Git hub / the git tool / read again / back to tree |

## Image / PDF / SVG / video preview

| Key | Action |
|---|---|
| `+` `-` / `=` | zoom / reset to fit |
| `h j k l` | pan |
| `J` `K` (also `PageDown` `PageUp`) | PDF page down / up (actions `page_next` / `page_prev`; the old `pdf_next_page` / `pdf_prev_page` still work) |
| `R` | back to the diff — only while this preview *is* a diff's `preview` representation (opened from there, or via `Ctrl-n`/`Ctrl-p`); a no-op, and hidden, on an ordinary image/PDF/SVG preview |
| `,` | `p` path style |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file (tree order, wraps) |
| `m` `'` / `e` `o` `p` `!` / `r` `q` | bookmark / the verbs / read again, back |

## CSV / TSV / spreadsheet / archive table preview

| Key | Action |
|---|---|
| `h j k l` | move by cell |
| `Ctrl-n` / `Ctrl-p` | preview the next / previous file (tree order, wraps) |
| `g` `G` / `0` `$` | first/last row / first/last column |
| `J` `K` | spreadsheets only: next / previous sheet (needs 2+ visible sheets; actions `page_next` / `page_prev`; the old `sheet_next` / `sheet_prev` still work) |
| `/` / `n` `N` | search cells (case-insensitive) / next, previous match (wraps) |
| `y` → `c r C f` | copy cell / row / column / full path |
| `Enter` | Open the cursor cell in a popup — the full, untruncated value, wrapped and scrollable (`Enter` / `q` / `Esc` closes). Also works on `.zip` / `.tar` listings and spreadsheets (address, displayed text, raw value, type, formula, format). |
| `,` | `p` path style |
| `m` `'` / `e` `o` `p` `!` / `r` `q` | bookmark / the verbs (`e` and `o` on an Office document open it in an Office app — `[external] office_apps`) / read again, back |

## Diff (a file's git / jj diff)

| Key | Action |
|---|---|
| `j` `k` `h` `l`, `g` `G`, `0` `$`, `Ctrl-d` `Ctrl-u` `Ctrl-f` `Ctrl-b` | scroll |
| `n` / `N` | next / previous **change block** of the file |
| `Ctrl-n` / `Ctrl-p` | next / previous **changed file** |
| `s` | layout unified ⇄ split ⇄ auto — or, for a changed image/PDF/SVG (`[ui] diff_view = "rendered"` shows it side by side), auto ⇄ side ⇄ stack instead (`[git] media_diff`) |
| `R` | cycle the presentations that file actually has (source ⇄ rendered ⇄ preview for Markdown/SVG, rendered ⇄ preview for an image/PDF — `[ui] diff_view`) |
| `J` `K` (also `PageDown` `PageUp`, in place of scrolling) | image/PDF/SVG diff: turn a multi-page PDF's page, both sides together (no-op elsewhere; actions `page_next` / `page_prev`, the old `media_diff_page_next` / `_prev` still work) |
| `,` | `f` follow scope (since follow-start ⇄ the full git diff) · `p` path style |
| `x` | discard this file's changes (confirmed first) |
| `e` | edit at the line of the change block on screen |
| `o` / `p` / `!` / `r` / `q` | open in another app / the Git hub / the git tool / read again / back |
| `y` | copy menu |

## Git screens (and jj)

`p` opens the changes list from the tree, a preview or a diff. **`Tab` / `Shift-Tab` walk the screens** — changes,
log, graph, branches, worktrees (jj has no worktree screen, so its cycle skips it). Every list shares `j` `k` `g` `G`,
`l` / `Enter` (open), `h` / `q` (back), `/` (filter), `r` (read again), `y` (copy menu), `Space` (operations menu), `Ctrl-t`
(open in a new tab, where it applies).

| Screen | Keys |
|---|---|
| Changes (`p`) | `s` `u` stage / unstage the file · `S` `U` all · `c` commit · `x` discard (confirmed) · `d` diff of all changes · `l` / `Enter` open the diff · `b` branches · `w` worktrees · `e` edit the row's file · `o` open it in another app · `!` the git tool · `Ctrl-t` open the file in a new tab |
| Log / graph | `l` / `Enter` commit detail (full message + diff) · `y` copy commit info (`s h t m a d`) |
| Graph | `s` pin base branch · `x` unpin · `a` show every revision (jj) · `b` branch panel (`x` toggle · `a` all · `c` current only · `J` `K` reorder · `l` / `Enter` apply) |
| Commit detail | `s` layout · scroll keys |
| Branches | `l` / `Enter` checkout · `c` create · `d` delete · `/` filter · `y` → `n` copy the branch name |
| Worktrees (`w`) | `l` / `Enter` switch this tab to it · `Ctrl-t` open it in a new tab · `d` diff it against its base branch · `c` create one (asks for a branch name, then switches into it) · `/` filter |
| **jj** — changes | `s` ask jj to snapshot the working copy (the only write; confirmed first) |
| **jj** — branches | bookmarks (jj's pointers; none is marked current) |
| **jj** — changes | no `u` `S` `U` `c` `x` `w`: jj has no index, and konoma only reads |

## Menus: `y`, `,` and `Space`

| Key | What it opens |
|---|---|
| `y` | the copy menu of the screen: the tree and previews copy paths (`n r f p @`, and `c` for a focused code block) · a table copies `c` cell · `r` row · `C` column · `f` path · a commit `s h t m a d` · a branch `n` name |
| `,` | the sort and display options of the screen (see the tree, text and diff tables above) |
| `Space` | the tree: the file menu (`n r d c x p D`). Everywhere else: every operation that acts on the screen now, built automatically — an operation you rebound or that has no single key is still here. The items come in the order of the screen's footer (the operations used most first) and carry the footer's short labels |

A chord in `[keys.<context>]` (`"y x"`, `", x"`, `"space x"`) changes that context's menu only; see
[Configuration](../configuration/#menus-and-chords).

## Tab list (`T`)

| Key | Action |
|---|---|
| `1`-`9` / `Enter` / `l` | switch to that tab / to the selection |
| `j` `k` | move |
| `/` | filter the rows by the text they show (`Enter` keeps the query, `Esc` clears it; then `Esc` closes) |
| `d` | close the **selected** tab (list stays open; the last tab refuses) |
| `T` `q` `h` `Esc` | close the list |

## Bookmark list (`'`)

| Key | Action |
|---|---|
| any letter | jump to that bookmark (a-z local / A-Z global) |
| `j` `k` + `Enter` | select + jump |
| `/` | filter the rows by the text they show; while you type, letters are text, not bookmark names (`Enter` keeps the query, `Esc` clears it; then `Esc` closes) |
| `Ctrl-e` | edit target in editor |
| `Space` `d` | delete the bookmark |
| `'` `q` `Esc` | close |

Letters used by the list or global keys (`F` `P` `Q` `T` `j` `k` `q` `t`) can't
letter-jump — select those with `j`/`k` + `Enter`, or rebind. `h` and `l` are *not* back / open
here, unlike every other list: they are bookmark names like the other letters.

## Outline (`O`), file info (`i`), help (`?`)

| Screen | Keys |
|---|---|
| Outline (the page list on a PDF, workbook or presentation) | `j` `k` `g` `G` move · `Enter` / `l` jump to the heading (or go to the page) · `/` filter the rows (`Enter` keeps the query, `Esc` clears it) · `O` `q` `h` `Esc` close |
| File info | `i` `q` `Esc` close |
| Help | `j` `k` `g` `G` scroll · `?` `q` `Esc` close |

## What changed from earlier versions

| Before | Now |
|---|---|
| tree `o` (Git hub) | `p` (the project screen — also from previews and diffs); `o` now opens the file in another app |
| preview `o` (outline) | `O` |
| tree `s` (sort menu) and `p` (path style) | the `,` menu |
| `t` (append at the end) and `I` (insert after the current tab) | `t` inserts after the current tab; `I` and the appending `tab_new` have no default key |
| diff `n` / `N` (changed file) | `Ctrl-n` / `Ctrl-p`; `n` / `N` step through the change blocks of the file |
| image `0` (fit) | `=` |
| range selection `q` (quit the app) | cancels the selection (`Q` still quits) |
| hub `g` / `l` (graph, log) | `Tab` / `Shift-Tab` walk the screens (`b` and `w` still jump); `l` opens the selected row |
| jj `R` (sync) | `s` on the jj changes list |
| branch list `y` (direct copy) | `y` menu → `n` |
| branch / worktree lists `n` (create) | `c` (`n` is the next match) |
| graph `0` (unpin base) | `x` only; the branch panel's `n` is `c`, apply is `l` |
| preview `Space` (toggle a checkbox / `<details>`) | `Enter` (it already worked) |
| `ui.keys = "less"` `Space` (page down) | `f` (`Space` is the operations menu) |
| diff `f` (follow scope) | the `,` menu → `f` |
| the sort overlay (`open_sort_menu`) | `open_sort_menu` opens the `,` menu; the `,` menu closes after one choice (the overlay stayed open for `r` and `.`) |
| *(new)* | `o` open in another app · `e` in a diff and on the changes list · `/` on the changes list, log and graph, and on the outline / page list, the tab list and the bookmark list · `l` / `h` open / back on the outline, page list and tab list · `r` everywhere · `n` / `N` step through changes · `,` and `Space` menus · `Tab` walks the Git screens |

Any of these can be put back in `[keys]` — see [Configuration](../configuration/#action-names).
