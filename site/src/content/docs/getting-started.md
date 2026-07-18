---
title: Getting started
description: Install konoma, learn the two-screen model, and take the five-minute tour.
---

Already have a terminal and Rust? This is the whole install:

```sh
cargo binstall konoma        # prebuilt binary (install cargo-binstall via Homebrew first)
# or
cargo install konoma         # build from source with the Rust toolchain
```

Prebuilt tarballs are also on the
[GitHub releases page](https://github.com/LESIM-Co-Ltd/konoma/releases) if you
don't use cargo at all. Starting from a fresh machine? See
[Set up from scratch](#set-up-from-scratch) below.

## Requirements

The gate for konoma's full experience is the **terminal**, not the OS:

- konoma runs on **macOS and Linux** (Unix). Windows is not supported (it uses
  Unix-only APIs, and Windows terminals do not speak the kitty graphics protocol).
- The full-screen **image / PDF / SVG / video previews need a terminal that speaks
  the [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)**
  — [Ghostty](https://ghostty.org), [kitty](https://sw.kovidgoyal.net/kitty/),
  [WezTerm](https://wezterm.org), or Konsole. **Text previews** (Markdown, code,
  git diffs) work in **any** terminal.
- Of the OS/arch combinations, **macOS on Apple Silicon is the most battle-tested**.
  Intel macOS works too, and **Linux x86_64** builds and passes the full test suite
  in CI, ships prebuilt binaries, and has had its previews verified rendering via
  kitty graphics — still **beta**, as it is newer than the macOS path.

**Fonts** — two glyph coverages matter:

- **Icons** (`ui.icons = true`, the default) need **Nerd Font** glyphs. Add
  `Symbols Nerd Font Mono` as a fallback font, or use a Nerd-Font-patched font.
  No Nerd Font? Set `ui.icons = false` for plain ASCII symbols (no tofu).
- **CJK text** (the `jp` UI, or CJK filenames / file contents) needs the terminal
  font to include **CJK glyphs** — otherwise CJK shows as tofu (□). konoma computes
  the display widths correctly regardless; the glyphs come from the font. A
  **Nerd-Font-patched CJK font** such as **HackGen Console NF** or **UDEV Gothic NF**
  covers both needs (icons *and* CJK) in a single font.

**Optional tools**, all degrading gracefully when absent: `git` (git suite),
`poppler` (multi-page PDF), `ffmpegthumbnailer` / `ffmpeg` (video thumbnails),
`lazygit` (external git tool on `O`).

## Set up from scratch

Starting from a machine with nothing installed, here is the full path to a working
konoma with image previews.

### macOS (Apple Silicon)

1. **Homebrew** (skip if you have it) — the package manager:
   ```sh
   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
   ```
2. **A kitty-graphics terminal** — Ghostty:
   ```sh
   brew install --cask ghostty
   ```
3. **A font with Nerd Font + CJK glyphs** (covers icons and Japanese in one):
   ```sh
   brew install --cask font-hackgen-console-nf
   ```
   Then in Ghostty's config (`~/.config/ghostty/config`) set:
   ```
   font-family = "HackGen Console NF"
   ```
4. **konoma** — the quickest is the prebuilt binary:
   ```sh
   brew install cargo-binstall
   cargo binstall konoma
   ```
   Prefer to build from source? Install Rust first, then `cargo install konoma`:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   cargo install konoma
   ```
5. **Optional tools** for richer previews:
   ```sh
   brew install poppler ffmpeg git lazygit
   ```
6. **Run it** — open Ghostty, then:
   ```sh
   konoma            # the current directory
   konoma ~/work     # or any directory · press ? for help
   ```

### Linux (x86_64 · beta)

Commands below use `apt` (Ubuntu/Debian); adapt for your package manager.

1. **A kitty-graphics terminal** — kitty is the simplest on Linux:
   ```sh
   sudo apt install kitty
   ```
   (Ghostty and WezTerm work too.)
2. **Fonts** — CJK glyphs plus a Nerd Font:
   ```sh
   sudo apt install fonts-noto-cjk        # CJK glyphs
   ```
   For a single font covering both Nerd Font icons and CJK, download a
   Nerd-Font-patched CJK font (e.g. HackGen NF) into `~/.local/share/fonts/`, run
   `fc-cache -f`, then set it as kitty's `font_family`.
3. **konoma** — the quickest is the prebuilt binary from the
   [releases page](https://github.com/LESIM-Co-Ltd/konoma/releases)
   (`konoma-x86_64-unknown-linux-gnu.tar.gz`); extract it onto your `PATH`.
   To build from source instead, install Rust and the C-library headers konoma's
   dependencies need:
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   sudo apt install pkg-config cmake libssl-dev libssh2-1-dev zlib1g-dev \
     libdbus-1-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev git
   cargo install konoma
   ```
4. **Optional tools**:
   ```sh
   sudo apt install poppler-utils ffmpeg git
   ```
5. **Run it** inside your kitty-graphics terminal:
   ```sh
   konoma            # press ? for help
   ```

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

- [Tutorial](../tutorial/) — the same tour as above, in seven guided steps.
- [Working with an AI agent](../guides/agent-watch/) — konoma's flagship workflow.
- [Previews in depth](../guides/preview/) — Markdown, tables, media, copying.
- [The git suite](../guides/git/) — hub, diffs, log, graph, branches.
- [Files, bookmarks & tabs](../guides/files/) — the file-manager side.
- [Configuration](../reference/configuration/) — every option, one page.
