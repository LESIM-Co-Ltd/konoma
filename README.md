# konoma

> Terminal file browser built for AI pair-programming — full-screen previews
> (Markdown, images, PDF, CSV), a git suite, and an agent-watch mode that
> follows your AI's edits.
> macOS & Linux · Rust · MIT

[![CI](https://github.com/LESIM-Co-Ltd/konoma/actions/workflows/ci.yml/badge.svg)](https://github.com/LESIM-Co-Ltd/konoma/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/konoma.svg)](https://crates.io/crates/konoma)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/LESIM-Co-Ltd/konoma/blob/main/LICENSE)

<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/demo.gif" alt="konoma tour — tree, full-screen image preview, Markdown, code with a git gutter, and the git graph" width="860">
</p>

Pick something in the tree, and preview it **full-screen**. konoma is a terminal
file browser built around that single idea. It is made for a side-by-side workflow
where you keep konoma on one half of the screen and work on the other.

The name "konoma" (木の間, "between the trees") comes from the tool's character:
peering through the gaps between the trees to look into the contents of the file tree.

**📖 Documentation: [lesim-co-ltd.github.io/konoma](https://lesim-co-ltd.github.io/konoma/)**
— getting started, guides and full reference, in English and
[日本語](https://lesim-co-ltd.github.io/konoma/ja/).

## Why it exists

When you want a tree + preview pane filling one half of the screen while you work on the
other, existing TUI file managers (such as yazi) cannot structurally remove the file-list
panel, so they cannot show *only the selected file* full-screen. konoma solves that one
thing with **mode transitions**: Tree (full-screen) ⇄ Preview (full-screen), with no
in-between split view.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/tree.png" alt="Tree view with git status colors"></td>
    <td width="50%"><img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/git-graph.png" alt="Custom git commit-graph renderer"></td>
  </tr>
  <tr>
    <td align="center"><b>Tree view</b> — git status colors</td>
    <td align="center"><b>Git graph</b> — custom commit-graph renderer</td>
  </tr>
</table>

<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/markdown.png" alt="Markdown and Mermaid rendering" width="860">
</p>
<p align="center"><b>Markdown &amp; Mermaid</b></p>

## Features

- **Full-screen preview**: images, Markdown, Mermaid, code, SVG, video thumbnails, and
  **PDF** (multi-page, navigate with `J`/`K`) rendered to fill the screen.
- **Config-driven delegation**: declare how each format is previewed in TOML — delegate to a
  built-in renderer or an external command. Unsupported formats safely show `[can not preview]`
  full-screen instead of crashing.
- **kitty graphics**: images use the kitty graphics protocol via ratatui-image for high quality.
- **Git suite**: status, diff, log, a custom commit-graph renderer, branches, and commits, all in-app.
- **File manager**: create / rename / delete (trash by default) / copy / move, plus search,
  bookmarks, and sorting. Destructive actions require a confirmation dialog.
- **Optional dependencies**: the app never breaks when an external tool (mpv, etc.) is missing.
  It runs from a plain `cargo install`.

## Status

Pre-release (feature-complete). The milestones below track what is implemented.

- [x] Tree view & navigation, mode transitions, `can not preview` fallback (M0/M1)
- [x] Full-screen images with zoom/pan (M2)
- [x] Markdown / Mermaid rendering (M3)
- [x] Tabs and path copy (M4)
- [x] Git integration: status, diff, log, graph, branches, commits (M5)
- [x] Video thumbnails (representative frame; no in-terminal playback) and GIF/SVG preview (M6)
- [x] PDF preview (multi-page, one page at a time)
- [x] File manager: create / rename / delete / copy / move, search, bookmarks, sorting (M7)
- [x] Configurable keymap with conflict detection
- [x] crates.io publish
- [x] Prebuilt binaries and `cargo binstall` (macOS, Linux `x86_64`)

## Requirements

- **macOS on Apple Silicon** is the primary, most battle-tested target. macOS on Intel (`x86_64`) has
  prebuilt binaries too. **Linux (`x86_64`)** builds and passes the full clippy + test suite in CI on every
  push, ships prebuilt binaries, and its previews — images, PDF, and Markdown — are verified to render via
  the kitty graphics protocol on Linux. It is newer than the macOS path (clipboard and trash use the
  standard freedesktop mechanisms but have had less real-world use), so still consider it **beta**. Windows
  is not supported.
- A terminal that supports the **kitty graphics protocol** (e.g. [Ghostty](https://ghostty.org), which runs
  on macOS and Linux) for image, SVG, video-thumbnail, and PDF previews. Without it, text-based previews still work.

## Install

Prebuilt binaries (fastest — no compilation) via [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```bash
cargo binstall konoma
```

Or compile and install from crates.io:

```bash
cargo install konoma
```

Prebuilt archives for macOS (Apple Silicon / Intel) and Linux (`x86_64`) are also attached to each
[GitHub Release](https://github.com/LESIM-Co-Ltd/konoma/releases).

Or build from source:

```bash
cargo build --release
```

## Usage

```bash
konoma [DIR]     # opens DIR (defaults to the current directory)
```

Press `?` in the app for the full, context-sensitive key reference.

**Take the tour**: open [`samples/tutorial.md`](samples/tutorial.md)
([日本語](samples/tutorial.ja.md)) *inside konoma* — a hands-on walkthrough with
links you can follow and checkboxes you can actually toggle.

## Optional tools

konoma never breaks when an external tool is missing — the relevant preview just degrades to a hint
(principle: "unsupported is shown safely, never a crash"). Install these to enable richer previews:

- **poppler** (`pdftoppm` / `pdftocairo` / `pdfinfo`) — PDF rendering and multi-page navigation.
  Without it, macOS falls back to `qlmanage`/`sips` for the **first page only**.
- **ffmpeg** or **ffmpegthumbnailer** — video thumbnail frames.
- **git** — the in-app git suite (status / diff / log / graph / branches). Enabled by default;
  build with `--no-default-features` to drop it.

## Configuration

`~/.config/konoma/config.toml` (works with defaults if absent).

- **[CONFIGURATION.md](CONFIGURATION.md)** — full reference: every `[ui]` option,
  colors/themes, preview rules (built-in renderers & external-command delegation),
  external editor, git integration, and the complete keybinding model.
- [`config.example.toml`](config.example.toml) — a fully commented example config;
  copy it as a starting point. A Japanese-annotated copy is at
  [`config.example.ja.toml`](config.example.ja.toml).

## License

[MIT](LICENSE) © LESIM
