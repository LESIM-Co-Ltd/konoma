# konoma tutorial — learn by doing

[日本語版はこちら](./tutorial.ja.md) ← focus this link with `Tab`, open it with `Enter`.

You are reading this *inside* konoma, which means every feature this file describes
can be tried right here. Keys to remember while reading: `j`/`k` scroll, `q` goes back,
`?` opens help for whatever screen you are on.

## 1. Scroll and leave

Scroll this document with `j`/`k` (half page: `Ctrl-d`/`Ctrl-u`, ends: `g`/`G`).
Press `q` once to go back to the file tree — then come back by selecting
`tutorial.md` and pressing `Enter`. That round trip (tree ⇄ full-screen preview)
is the core of konoma: no split panes, always the full screen.

## 2. Follow links with Tab

Markdown links are focusable. Press `Tab` repeatedly — the focus (inverted block)
walks through every link and checkbox in this document. Try it:

- [markdown.md](./markdown.md) — the Markdown showcase (tables, code, Mermaid, images)
- [sample.csv](./sample.csv) — opens as an aligned table: move cells with `h j k l`,
  copy a cell/row/column with `y`
- [sample.png](./sample.png) — image preview: zoom with `+`/`-`, pan with `h j k l`
- [sample.pdf](./sample.pdf) — PDF pages: turn with `J`/`K`

Focus one with `Tab`, press `Enter` to open it, then `q` to come back here.

## 3. Toggle these checkboxes

Checkboxes are interactive: `Tab` to focus, `Space` (or `Enter`) to toggle.
The change is written back to this file — you are editing real state, safely
(konoma verifies the file on disk before writing a single character).

- [ ] I scrolled with `j`/`k`
- [ ] I opened a link with `Tab` + `Enter` and came back with `q`
- [ ] I toggled this checkbox with `Space`

## 4. Search and copy

- `/` searches inside a text preview; `n`/`N` jump between matches.
- `R` shows this file's raw Markdown source. In raw view (and any code/text file),
  `v` starts a character selection and `V` a line selection — move with `h j k l`,
  copy with `y`. `Y` copies an `@path#L12` reference for AI chats.
- From the tree, `y` opens a copy menu: file name, relative/full path, or an
  `@relative/path` reference.

## 5. The tree is a file manager

Back in the tree (`q`):

- `/` filters entries as you type; `Esc` clears.
- `Space` opens the file-management menu: create (`n`), rename (`r`), delete (`d` —
  to the trash, with confirmation), copy/cut/paste (`c`/`x`/`p`).
- `.` shows dotfiles, `s` opens the sort menu, `i` shows file info.
- `m` + a letter bookmarks the cursor item (lowercase = this project, uppercase =
  global). `'` opens the bookmark list — press the letter to jump. Both work from
  previews too, bookmarking the file you are viewing.

## 6. Git, live

If this directory is inside a git repository: `o` opens the changes hub
(stage/unstage/commit), `d` on a changed file shows its full-screen diff,
and inside the hub `l` is the log and `g` the commit graph. Modified files
are colored in the tree, and code previews get an editor-style change gutter.

## 7. Watch an AI agent work

konoma's flagship trick, for the "konoma on the left, coding agent on the right"
layout:

- `F` — follow mode: whenever a file changes on disk, konoma automatically shows
  its diff. Press any key to take back control; `F` again to resume.
- `C` — flat list of every file with uncommitted changes; `n`/`N` jump through them.
- `y` → `@` / `Y` — copy `@path` / `@path#L12-34` references to paste into the
  agent conversation.

## Where to go next

- Press `?` on any screen — every view documents its own keys.
- The full manual (guides, configuration, keymap) lives at the documentation site
  linked from the README, and `config.example.toml` documents every setting inline.

One last box to tick:

- [ ] Done! Enjoy konoma.
