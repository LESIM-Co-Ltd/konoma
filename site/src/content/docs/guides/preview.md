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
- **LaTeX math**: `$…$` and `$$…$$` (or `\(…\)` / `\[…\]`) are typeset in pure
  Rust and drawn as images — no browser, no Node, no TeX installation. A
  terminal cannot place a picture inside a line of text, so an inline formula is
  lifted onto a line of its own and a display formula is centered. `$5` and
  anything inside code stays literal. Set `ui.math = "text"` to keep the raw
  LaTeX instead, and `ui.math_color` to match your terminal's foreground.
- **Block alignment**: `ui.md_table_align` (default `"left"`) and
  `ui.md_image_align` (default `"center"`) place a table's box and a block image
  — a standalone `![alt](url)`, a row of badges, a Mermaid diagram — at the
  left, the center, or the right of the preview. The table setting moves the
  whole grid, including any picture inside a cell; it does not touch alignment
  *within* a cell, which a column's `:---:` still decides.
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
- `Enter` opens the cursor cell in a popup — the full, untruncated value,
  wrapped and scrollable (`Enter` / `q` / `Esc` closes it).
- Quoted fields, CJK widths, ragged rows and non-UTF-8 input are all handled;
  parsing failures fall back to plain text.

### Archives

`.zip`, `.tar` and `.tar.gz` open in the same grid: one row per entry with its
name, size and modified date. **Nothing is extracted** — konoma only reads the
archive's index — so cell search (`/`), the cell cursor and the `y` copy menu
all work exactly as they do for CSV, on a listing that never touches your disk.

## Images, SVG, GIF, video, PDF

Drawn as real pixels in any terminal that speaks a graphics protocol — kitty
graphics (Ghostty, kitty, WezTerm, Konsole), iTerm2, or sixel. konoma has its
own compressed transfer for the kitty protocol, so those terminals are the
fastest; anywhere else the picture degrades to a coarse but visible half-block
approximation.

- `+` / `-` zoom, `0`/`=` reset to fit, `h j k l` pan.
- GIFs animate automatically. SVGs rasterize in-process (no external tools).
- Videos show a representative frame. **H.264 and HEVC inside
  `.mp4`/`.m4v`/`.mov` and `.mkv`/`.webm` are decoded natively in Rust —
  nothing to install**, which covers an iPhone's default recording format as
  well as the container most ripped or re-encoded files arrive in. VP9, AV1,
  the older codecs (Xvid, MPEG-2, WMV, …), the `.avi` container, and the
  uncommon profiles (H.264 in 10-bit / 4:2:2 / 4:4:4 / monochrome, HEVC outside
  Main / Main 10 4:2:0) use `ffmpegthumbnailer`/`ffmpeg` if installed, and show
  a hint if not. Want playback? Delegate to `mpv` with one config rule.
- PDFs render page by page, natively in Rust (`hayro`, no external tool
  needed) — `J` / `K` (or PageDown/PageUp) turn any page. On macOS only, the
  rare PDF `hayro` can't render (encrypted, corrupt, or otherwise unsupported)
  falls back to the system's own `qlmanage`/`sips` — already installed —
  though those can only produce the **first** page.

## Everything else

Files matching no rule that look like text open as text; anything else shows a
safe `[can not preview]` screen. konoma never crashes on unknown input —
add a rule in the config to teach it new formats, including delegation to any
external command.
