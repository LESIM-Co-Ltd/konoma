<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/hero-image.png" alt="konoma — 木の間 — between the trees · full-screen preview" width="860">
</p>

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
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/markdown.png" alt="Inline images, a Mermaid diagram, and LaTeX math rendered in a Markdown preview" width="860">
</p>
<p align="center"><b>Markdown</b> — inline images, Mermaid diagrams, and LaTeX math, rendered right in the preview</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/markdown-richtext.png" alt="Tables with inline styling and alignment, a horizontal rule, and interactive task lists in a Markdown preview" width="860">
</p>
<p align="center"><b>Rich text</b> — tables with inline styling and alignment, a horizontal rule, and interactive task lists</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/markdown-alerts.png" alt="GitHub-style alerts, autolinks, emoji, and footnotes in a Markdown preview" width="860">
</p>
<p align="center"><b>GitHub-flavored</b> — alerts, autolinks, emoji, and footnotes</p>

## Agent watch — konoma follows your AI's edits

Press `F` and konoma stops being a browser you drive: whenever a file changes on
disk, it jumps there by itself and shows a diff of **what changed since you pressed
`F`** — not the full `git diff`. That distinction is the point. A working tree you
hand to an agent usually already has your own edits in it; follow mode hides those
and shows you only what the agent just did. Press `f` to swap between the two views,
`n`/`N` to cycle the files touched in this session, and `q` to stop.

<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/follow.gif" alt="Follow mode — the file already has an uncommitted edit; after F, konoma shows only the lines the agent adds, and f reveals the full git diff" width="860">
</p>
<p align="center">
  <b>Follow mode</b> — the file already has an uncommitted line (<code>// TODO: handle retries</code>).
  After <code>F</code>, the agent's new function is the only thing highlighted; <code>f</code> switches to
  the full <code>git diff</code>, where both show up.
</p>

## Worktrees — one checkout per agent

Agents work best with a checkout of their own, so `git worktree` has become the way
to run several at once. `o` then `w` lists the repository's worktrees; `d` shows what
one of them has added since its base branch — **committed and uncommitted together**,
because an agent that commits mid-task would otherwise show you an empty diff. `Enter`
moves in, `Ctrl-t` opens it in a new tab, and the `WT` chip keeps naming the repository
you came from, since a worktree's directory rarely does.

<p align="center">
  <img src="https://raw.githubusercontent.com/LESIM-Co-Ltd/konoma/main/assets/worktrees.gif" alt="Worktrees — listing the repository's worktrees, showing what an agent's worktree added since its base branch, and stepping into it with the WT chip naming the repository" width="860">
</p>
<p align="center">
  <b>Worktrees</b> — the agent's checkout has a landed commit <i>and</i> an unfinished line;
  <code>d</code> shows both. After <code>Enter</code>, <code>WT demo</code> says which repository
  <code>add-retry</code> belongs to.
</p>

## Features

- **Full-screen preview**: images, Markdown, Mermaid, code, SVG, video thumbnails, and
  **PDF** (multi-page, navigate with `J`/`K`) rendered to fill the screen.
- **Table preview**: CSV/TSV render as an aligned, rainbow-column table with a cell cursor;
  **archives** (`.zip`/`.tar`/`.tar.gz`) list their entries — name, size, modified date — in the
  same grid, without extracting anything.
- **Config-driven delegation**: declare how each format is previewed in TOML — delegate to a
  built-in renderer or an external command. Unsupported formats safely show `[can not preview]`
  full-screen instead of crashing.
- **kitty graphics**: on a kitty-graphics terminal konoma transmits images itself, zlib-compressed, so
  a full-screen image or a zoom step appears immediately instead of streaming megabytes of escape
  codes. Other terminals (sixel, iTerm2, half-blocks) fall back to ratatui-image.
- **Agent watch**: `F` follows whatever your AI edits — konoma jumps to each file on its own and shows
  the diff *since you pressed `F`*, so pre-existing changes stay out of the way.
- **Git suite**: status, diff, log, a custom commit-graph renderer, branches, commits, and
  **worktrees** — list them, diff one against its base branch, and switch or open it in a tab.
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

The gate for the full experience is the **terminal**, not the OS:

- A terminal that speaks the **[kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)**
  — [Ghostty](https://ghostty.org), [kitty](https://sw.kovidgoyal.net/kitty/), [WezTerm](https://wezterm.org),
  or Konsole — for the **image / PDF / SVG / video previews**. **Text previews** (Markdown, code, git diffs)
  work in **any** terminal.
- **OS**: konoma runs on **macOS and Linux** (Unix). Of the combinations, **macOS on Apple Silicon** is the
  most battle-tested; Intel macOS works too, and **Linux (`x86_64`)** builds and passes the full test suite
  in CI, ships prebuilt binaries, and has had its previews verified rendering via kitty graphics — still
  **beta**, as it is newer than the macOS path. **Windows is not supported** (Unix-only APIs; no kitty
  graphics in Windows terminals).
- **Fonts**: **icons** need [Nerd Font](https://www.nerdfonts.com/) glyphs (or set `ui.icons = false`), and
  **CJK text** (the `jp` UI, CJK filenames/contents) needs the terminal font to include **CJK glyphs** or it
  shows as tofu (□) — konoma computes the widths correctly regardless. A Nerd-Font-patched CJK font like
  **HackGen Console NF** covers both in one font.
- **Optional tools** (all degrade gracefully): `ffmpegthumbnailer`/`ffmpeg` (video formats konoma
  cannot decode itself — see below), `git` (git suite), `lazygit` (external git tool on `!`).
  **Images, SVG, Mermaid, LaTeX math, CSV, PDF and H.264/HEVC video thumbnails need nothing installed
  at all** — konoma renders them itself, in pure Rust.

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

```bash
brew install ffmpeg git           # macOS
sudo apt install ffmpeg git       # Debian / Ubuntu
```

- **ffmpeg** or **ffmpegthumbnailer** — thumbnails for the video formats konoma cannot decode itself:
  **VP9**, **AV1** and the older codecs, the `.mkv`/`.webm`/`.avi` containers, and the uncommon
  profiles (H.264 in 10-bit / 4:2:2 / 4:4:4 / monochrome; HEVC outside Main / Main 10 4:2:0).
  **H.264 and HEVC inside `.mp4`/`.m4v`/`.mov` — the ordinary case, including what an iPhone records
  by default — need nothing**: konoma decodes that keyframe itself, in pure Rust.
- **git** — the in-app git suite (status / diff / log / graph / branches). Enabled by default;
  build with `--no-default-features` to drop it.

Images, **PDF**, SVG, Markdown, Mermaid, LaTeX math, CSV and code need nothing extra — konoma renders
them itself, in pure Rust. (On macOS only, a PDF the built-in renderer cannot draw falls back to the
system's own `qlmanage`/`sips` for its first page — already installed, nothing to add.) The picture
formats do need a terminal that speaks the kitty graphics protocol; text previews work anywhere.

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
