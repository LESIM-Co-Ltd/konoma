# Changelog

All notable changes to konoma are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- CSV/TSV table preview: `.csv` / `.tsv` files render as an aligned grid with a fixed
  header row, rainbow (per-column) colors, and a movable cell cursor — the way csvlens
  and Rainbow CSV present tabular data. Parsing goes through the `csv` crate, so quoted
  commas, embedded newlines, ragged (variable-column) rows, and full-width (CJK) cells
  are handled correctly; a file that fails to parse degrades to a raw-text preview.
  Navigate cells with `h`/`j`/`k`/`l` (`g`/`G` = first/last row, `0`/`$` = first/last
  column), and copy with `y` → `c` (cell) / `r` (row) / `C` (column) / `f` (full path).
- `[ui] csv_rainbow` config option (default `true`) to toggle the rainbow column colors.
- Range selection + copy in code/text previews with a vim-style 2D caret: a block caret
  moves by line (`j`/`k`, the window following at the edges) and by column (`h`/`l`,
  `0`/`$` for line start/end, the view following horizontally when not wrapping). `v`
  starts a **charwise** selection (an exact character range across lines) and `V` a
  **linewise** selection (whole logical lines); the caret extends the range and `y`
  copies it to the clipboard — the real file text, not the wrapped display, ideal for
  pasting a precise snippet elsewhere. `Esc`/`v`/`V`/`q` cancel. Applies to windowed
  code/text previews (Markdown/Mermaid are reflowed and excluded).

### Changed
- At the tree top level, `q` now closes the current tab when more than one tab is open,
  and only quits the app once the last tab remains (with the usual quit confirmation).
  `Q` still quits the whole app from anywhere. The tree footer reflects this — it shows
  `q: close tab` plus `Q: quit` while multiple tabs are open, and `q: quit` otherwise.

## [0.3.0]

### Added
- Editor-style git change gutter in code/text previews (Zed/VS Code style): a
  one-cell marker to the left of the line-number column shows added, modified,
  and deleted lines at a glance. Distinct from the full-screen `d` diff view.
  Green `▌` marks added lines, amber `▌` modified lines, and a red `▔` sits on
  the top edge of the line just below a removed block, so a deletion reads as
  "removed between these rows" without disturbing line spacing. A deletion that
  is contiguous with an add/modify folds into the modified marker (matching Zed).
- `[ui] git_gutter` config option (default `true`) to toggle the gutter. Files
  with no changes and non-repositories keep their previous layout unchanged.

## [0.2.0]

### Added
- Inline image preview inside Markdown: block-level images (Markdown `![](…)` and
  HTML `<img>`) render in the flow of the document via kitty graphics, decoded off
  the UI thread. A dim `🖼 alt` placeholder reserves the space until the image is ready.
- Remote images: `http(s)://` images are downloaded with the system `curl` into an
  on-disk cache (`~/.cache/konoma/remote-images`) and then rendered like local files —
  the kind of screenshots and badges READMEs show on GitHub. SVG badges/logos are
  rasterized with resvg. A `loading…` line shows while fetching; unreachable hosts,
  non-image responses, and missing files degrade to a text placeholder (principle #3).
- Partially-scrolled inline images are drawn clipped to the viewport (their visible
  band is cropped and encoded) instead of being hidden, so large/stacked images stay
  visible while scrolling.
- `samples/images.md` demonstrating local, HTML, remote, and fallback cases.

### Changed
- Inline-image encoding (resize + protocol) runs on a dedicated worker thread, so the
  UI never blocks while an image is prepared or re-clipped during scrolling.

## [0.1.1]

### Added
- Prebuilt binaries for macOS (Apple Silicon / Intel) and Linux (`x86_64`) attached
  to each GitHub Release, with `cargo binstall konoma` support.
- CI verifies builds on Linux and Windows in addition to macOS.

### Notes
- Windows is intentionally not built: konoma uses Unix-only standard-library APIs,
  and Windows terminals lack the kitty graphics protocol the previews rely on.
- Linux support is experimental — it builds in CI, but its runtime (previews,
  clipboard, trash) is not yet verified.

## [0.1.0]

Initial release.

### Added
- Tree view and full-screen preview with mode transitions (Tree ⇄ Preview), and a
  safe `[can not preview]` fallback for unsupported formats.
- Config-driven preview delegation (built-in renderers or external commands).
- Full-screen image preview via the kitty graphics protocol with zoom/pan, GIF
  animation, and SVG rendering.
- Markdown / Mermaid rendering and syntax-highlighted code preview.
- Video thumbnails (representative frame; no in-terminal playback) and multi-page
  PDF preview (one page at a time; `J`/`K` to navigate).
- In-app git suite: status, diff, log, a custom commit-graph renderer, branches,
  and commits.
- File manager: create / rename / delete (trash by default) / copy / move, plus
  search, bookmarks, and sorting, with confirmation dialogs for destructive actions.
- Tabs, path copy, a fully configurable keymap with conflict detection, and an
  optional quit-confirmation dialog.

[Unreleased]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/LESIM-Co-Ltd/konoma/releases/tag/v0.1.0
