---
title: Tutorial
description: A guided tour of konoma in seven steps — follow along in your terminal.
sidebar:
  order: 2
---

This is a follow-along tour: keep konoma open in a terminal and try each step
as you read. Two keys carry you through everything — **`?` shows help for the
current screen, `q` goes back one level.**

:::tip[Best experienced inside konoma]
The repository ships this tour as an *interactive* document —
[`samples/tutorial.md`](https://github.com/LESIM-Co-Ltd/konoma/blob/main/samples/tutorial.md)
— with links you can actually follow and checkboxes you can actually toggle:

```sh
git clone https://github.com/LESIM-Co-Ltd/konoma
konoma konoma/samples        # then open tutorial.md
```
:::

## 1. Scroll and leave

Open any directory (`konoma ~/some/project`), select a text file and press
`Enter`. Scroll with `j`/`k` (half page: `Ctrl-d`/`Ctrl-u`, ends: `g`/`G`),
then press `q` to return to the tree. That round trip — full-screen tree ⇄
full-screen preview — is the core of konoma. There are no split panes.

## 2. Follow links with Tab

Open a Markdown file with links. Press `Tab` repeatedly: the focus (an
inverted block) walks through every link and checkbox in the document.
`Enter` opens the focused link — local paths open inside konoma (and `q`
brings you back), URLs open in your browser.

## 3. Toggle checkboxes

In a Markdown file with a task list (`- [ ]` items), `Tab` onto a checkbox
and press `Space`. The checkbox toggles **and the state is written back to
the file** — verified against the file on disk first, so a concurrently
editing agent is never clobbered. `R` shows the raw source if you want to
see the change.

## 4. Search and copy

- `/` searches inside any text preview; `n`/`N` jump between matches.
- In code/text (and raw Markdown), move the caret with `h j k l`, select with
  `v` (characters) or `V` (lines), copy with `y`.
- `Y` copies an `@path#L12-34` reference — paste it into an AI-agent chat to
  point at exact lines.
- From the tree, `y` opens a copy menu: name, relative/full path, parent, or
  an `@relative/path` reference.

## 5. The tree is a file manager

- `/` filters as you type; `Esc` clears.
- `Space` opens the file menu: create (`n`), rename (`r`), delete (`d` — to
  the trash, after a confirmation), copy/cut/paste (`c`/`x`/`p`).
- `.` shows dotfiles, `s` sorts, `i` shows file info.
- `m` + a letter bookmarks the cursor item (lowercase = this project,
  uppercase = global); `'` opens the bookmark list — press a letter to jump.
  Both work from previews too, where they act on the file being shown.

## 6. Git, live

In a git repository: `o` opens the changes hub (stage with `s`, commit with
`c`), `Enter` on a file shows its full-screen diff (`s` cycles
unified/split), `l` is the log and `g` the commit graph — try `s` on a
commit there to pin its branch straight along lane 0. Changed files are
colored in the tree, and code previews get a change gutter.

## 7. Watch an AI agent work

The flagship workflow — konoma on the left, a coding agent on the right:

- `F` turns on **follow mode**: every file the agent changes appears as a
  diff, automatically. Any key takes control back; `F` resumes.
- While a diff is shown, `n`/`N` cycle through the files changed in this
  follow session.
- `C` lists every uncommitted file; `y` → `@` and `Y` copy references for
  the conversation.

More depth: [Working with an AI agent](../guides/agent-watch/) ·
[Previews](../guides/preview/) · [Git](../guides/git/) ·
[Files & bookmarks](../guides/files/) ·
[Configuration](../reference/configuration/)
