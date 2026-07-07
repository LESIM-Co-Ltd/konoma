---
title: Getting started
description: Install konoma, learn the two-screen model, and take the five-minute tour.
---

## Install

```sh
cargo binstall konoma        # prebuilt binary (install cargo-binstall via Homebrew first)
# or
cargo install konoma         # build from source with the Rust toolchain
```

Prebuilt tarballs are also on the
[GitHub releases page](https://github.com/LESIM-Co-Ltd/konoma/releases) if you
don't use cargo at all.

**Requirements**

- macOS on Apple Silicon is the primary target (Intel macOS works; Linux x86_64
  builds are experimental; no Windows).
- Image / SVG / video-thumbnail / PDF previews need a terminal with the **kitty
  graphics protocol** — [Ghostty](https://ghostty.org) or kitty. Text previews
  work in any terminal.
- Icons need Nerd Font glyphs: add `Symbols Nerd Font Mono` as a fallback font,
  or use an NF-bundled font (HackGen Console NF, UDEV Gothic NF, …). No Nerd
  Font? Set `ui.icons = false` in the config for plain symbols.

Optional tools, all degrading gracefully when absent: `git` (git suite),
`poppler` (multi-page PDF), `ffmpegthumbnailer`/`ffmpeg` (video thumbnails),
`lazygit` (external git tool on `O`).

## The two-screen model

```sh
konoma            # browse the current directory
konoma ~/work     # or any directory
```

konoma has exactly two main screens and no split panes:

1. **Tree** — the full-screen file tree. Move with `j`/`k`, expand/enter with
   `l` or `Enter`, go to the parent with `h`.
2. **Preview** — the full-screen view of the selected file. `q` (or `Esc`)
   returns to the tree.

Everything else (git views, bookmark list, help) layers on top of these two.
Two habits carry you everywhere:

- **`?` shows help for the screen you are on.** Every view documents its own keys.
- **`q` goes back one level.** `Q` quits from anywhere (with a confirmation).

## Five-minute tour

1. Launch `konoma` in a project directory.
2. Type `/` and a few letters — the tree filters as you type. `Esc` clears.
3. Select a Markdown file, press `Enter` — it renders with headings, tables and
   links. Press `Tab` to focus a link, `Enter` to follow it, `q` to come back.
4. Select an image or PDF — full-screen pixels; `+`/`-` zooms, `J`/`K` turns
   PDF pages.
5. In a git repository, press `o` — the changes hub. `Enter` on a file shows its
   full-screen diff; `l` is the log; `g` is the commit graph. `q` backs out.
6. Press `m` then `a` to bookmark where you are; press `'` to see the bookmark
   list and jump.

## The built-in tutorial

The repository ships a hands-on tour designed to be read **inside konoma** —
links you can follow, checkboxes you can actually toggle:

```sh
git clone https://github.com/LESIM-Co-Ltd/konoma
konoma konoma/samples        # then open tutorial.md
```

## Where next

- [Working with an AI agent](../guides/agent-watch/) — konoma's flagship workflow.
- [Previews in depth](../guides/preview/) — Markdown, tables, media, copying.
- [The git suite](../guides/git/) — hub, diffs, log, graph, branches.
- [Files, bookmarks & tabs](../guides/files/) — the file-manager side.
- [Configuration](../reference/configuration/) — every option, one page.
