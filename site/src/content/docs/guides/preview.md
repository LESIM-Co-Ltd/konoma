---
title: Previews in depth
description: Markdown with interactive checkboxes, tables, images, PDF, CSV, code — and how to search, select and copy from all of them.
sidebar:
  order: 2
---

Select a file, press `Enter`, and konoma picks the right renderer from its
rule table (see [Configuration](../../reference/configuration/)). `q` always
goes back. `?` shows the keys for the current preview type.

## Markdown

Rendered with headings, full-width rules, tables (with column alignment and
inline styles), task lists, code fences (syntax-highlighted), Mermaid fences
(rendered as real diagram images), and inline images (local and remote, drawn
as real pixels).

- **Links**: `Tab` / `Shift-Tab` walk every link and checkbox in the document;
  `Enter` opens the focused link — local paths open inside konoma, URLs open in
  the browser.
- **Task checkboxes**: focus with `Tab`, toggle with `Space`. The state
  character is written back to the file — verified against the file on disk
  first, so it never clobbers a concurrent edit (e.g. by an AI agent). The
  states cycle through `ui.md_task_states` (default `[ ]` ⇄ `[x]`; add a
  custom in-progress state like `[/]` if you like).
- **Mermaid diagrams**: ```` ```mermaid ```` fences render inline as real
  images — laid out and rasterized fully in-process (no browser or Node),
  dark-themed with a transparent background so they blend into the terminal.
  `Tab` focuses a diagram; `+` / `-` zoom it in place (`h j k l` pan, `0`
  fits), and `Enter` opens it full screen (`q` returns to the same spot).
  Standalone `.mmd` files open full screen directly. `ui.mermaid = "text"`
  keeps the legacy Unicode rendering; unsupported diagrams fall back to it
  automatically.
- **Raw source**: `R` toggles the decorated view against the raw Markdown
  source, where precise line/column selection works.

## Code and text

Syntax highlighting resolves the grammar by extension, then file name
(`.bashrc`, `Makefile`, `Dockerfile`, …), then first line. Large files stream
through a windowed reader, so multi-hundred-MB logs open instantly.

- `/` searches; `n` / `N` jump between matches.
- A 2D caret moves with `h j k l`; `v` selects by character, `V` by line;
  `y` copies the selection. `0` / `$` jump within the line, `g` / `G` to the
  ends.
- `Y` copies an `@path#L12-34` reference for the caret or selection.
- Files with uncommitted changes get an editor-style git gutter (green added /
  blue modified / red deleted).
- Wrap is configurable (`ui.wrap`); with wrap off, long lines scroll
  horizontally.

## CSV / TSV tables

Comma- and tab-separated files render as an aligned grid with rainbow column
colors and a cell cursor:

- `h j k l` move by cell; `g`/`G` first/last row; `0`/`$` first/last column.
- `y` opens a copy menu: cell, row, column, or full path.
- Quoted fields, CJK widths, ragged rows and non-UTF-8 input are all handled;
  parsing failures fall back to plain text.

## Images, SVG, GIF, video, PDF

Drawn as real pixels via the kitty graphics protocol:

- `+` / `-` zoom, `0`/`=` reset to fit, `h j k l` pan.
- GIFs animate automatically. SVGs rasterize in-process (no external tools).
- Videos show a representative frame via `ffmpegthumbnailer`/`ffmpeg`
  (optional; a hint appears if neither is installed). Want playback? Delegate
  to `mpv` with one config rule.
- PDFs render page by page — `J` / `K` (or PageDown/PageUp) turn pages
  (multi-page needs poppler; the first page works with macOS built-ins alone).

## Everything else

Files matching no rule that look like text open as text; anything else shows a
safe `[can not preview]` screen. konoma never crashes on unknown input —
add a rule in the config to teach it new formats, including delegation to any
external command.
