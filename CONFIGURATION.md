# Configuring konoma

konoma reads a single TOML file:

```
~/.config/konoma/config.toml
```

Everything is optional — **no config file is required**. A missing or broken file never
prevents startup; konoma falls back to the defaults described below (invalid values fall
back per-key). A fully commented example lives in
[`config.example.toml`](config.example.toml) (Japanese inline comments); copy it as a
starting point:

```sh
mkdir -p ~/.config/konoma
cp config.example.toml ~/.config/konoma/config.toml
```

Contents:

- [Quick start](#quick-start)
- [`[ui]` — appearance & behavior](#ui--appearance--behavior)
- [`[ui.sort]` — default tree order](#uisort--default-tree-order)
- [`[ui.theme]` — colors](#uitheme--colors)
- [`[[preview.rules]]` — what renders each file type](#previewrules--what-renders-each-file-type)
- [`[editor]` — external editor](#editor--external-editor)
- [`[git]` — git integration](#git--git-integration)
- [`[keys]` — keybindings](#keys--keybindings)
- [Data files](#data-files)
- [Fonts & terminal requirements](#fonts--terminal-requirements)

## Quick start

```toml
[ui]
lang = "en"                 # UI language ("auto" follows the OS)
wrap = false                # no soft-wrap; h/l scroll long lines
line_numbers = true
details = ["size", "modified"]

[ui.theme]
bg = "#282c34"

[keys]
copy_prefix = "y"
```

## `[ui]` — appearance & behavior

| Key | Default | Description |
|---|---|---|
| `show_hidden` | `false` | Show dotfiles at startup (toggle at runtime with `.`). |
| `tabbar` | `"auto"` | Tab bar visibility: `"always"` / `"auto"` (only with 2+ tabs) / `"hidden"`. |
| `icons` | `true` | Nerd Font icons in the tree, Markdown links and task checkboxes. Set `false` on terminals without a Nerd Font — konoma falls back to plain ASCII symbols (no tofu). |
| `wrap` | `true` | Soft-wrap text previews. `false` = no wrap + horizontal scrolling (`h`/`l`, `0`/`$`). |
| `line_numbers` | `false` | Line-number gutter in code/text previews. |
| `git_gutter` | `true` | Editor-style git change gutter (green added / blue modified / red deleted) in code/text previews of files with uncommitted changes. |
| `tab_width` | `4` | Tab stop width in code/text previews (`0` keeps raw tabs). |
| `syntax_highlight` | `true` | Syntax highlighting for code previews (`false` = plain text, fastest). |
| `preview_loading` | `"indicator"` | First-open wait for heavy code previews: `"indicator"` (loading screen) or `"progressive"` (plain text first, colors swap in when ready). |
| `path_style` | `"relative"` | Title-bar path form: `"relative"` / `"home"` (`~/...`) / `"full"`. Cycle at runtime with `p`. |
| `keys` | `"vim"` | Paging-key scheme for previews: `"vim"` (`Ctrl-f/b`, `Ctrl-d/u`) or `"less"` (`f`/`b`, `d`/`u`, `Space`). |
| `lang` | `"auto"` | UI language for help/hints/messages: `"auto"` (OS language) / `"en"` / `"jp"`. |
| `statusbar` | `"split"` | Status chrome layout: `"split"` (context top, hints bottom) / `"bottom"` / `"top"`. |
| `image_render_scale` | `1.0` | Image display scale (0.1–1.0). Smaller = fewer pixels sent to the terminal = faster draws, smaller image. |
| `svg_max_px` | `800` | Max rasterization side (px) for SVG previews. Larger = crisper but heavier. |
| `details` | `[]` | Metadata columns on tree rows, in order. Available: `"size"`, `"modified"`, `"perm"`, `"type"`, `"items"` (directory entry count). |
| `graph_max_branches` | `12` | Cap on branches drawn simultaneously in the commit graph (`o` → `g`). `0` = unlimited. Toggle branches at runtime with `b` inside the graph. |
| `graph_base_branches` | `[]` | Ordered list of preferred base branches for the graph, e.g. `["main", "develop"]`. The first one that exists becomes the base (pinned to lane 0); the array order becomes the display priority. |
| `commit_meta_align` | `"right"` | Author/date column in git log & graph: `"right"` (aligned right-edge column) or `"inline"` (directly after the subject). |
| `confirm_quit` | `true` | Ask before quitting (`q`/`y`/`Enter` = quit, `n`/`Esc` = cancel; `qq` quits quickly). `false` = quit immediately. |
| `confirm_bookmark_overwrite` | `true` | Ask before a bookmark key (`m`) overwrites a **different** existing path (`y`/`Enter` = overwrite, `n`/`Esc` = cancel). Re-registering the same path or an unused key never prompts. `false` = overwrite silently. |
| `csv_rainbow` | `true` | Rainbow column colors in CSV/TSV table previews. `false` = monochrome (alignment and navigation unchanged). |
| `follow_view` | `"diff"` | How follow mode (`F`) opens a changed file: `"diff"` (full-screen git diff; untracked files show as all-added) or `"file"` (normal preview scrolled to the first changed hunk). Files without a diff and media always open as `"file"`. |
| `busy_indicator` | `true` | Small spinner + job label at the top-right while background work runs (git-ignored scan, media decode, highlight warm-up, image fetches). Idle shows nothing and costs nothing. |
| `mermaid` | `"image"` | How mermaid diagrams render. `"image"` rasterizes them in-process (pure Rust, mermaid.js-quality — no browser/Node): standalone `.mmd` files show full screen with zoom/pan (`+`/`-`/`hjkl`; zooming re-rasterizes so it stays sharp), and ```mermaid fences in Markdown render inline (`Tab` focuses a diagram — it gets a cyan frame and the view scrolls to show it whole; `+`/`-` zoom **in place** without moving the layout, `hjkl` pan while zoomed, `0` fits, `Enter` opens it full screen, `q` returns). `"text"` keeps the legacy Unicode box-drawing rendering. Unsupported diagrams, render failures, and non-graphics terminals degrade to text automatically. |
| `mermaid_theme` | `"dark"` | Color theme for image-mode diagrams: `"dark"` (matches dark terminals), `"light"`, `"classic"` (mermaid.js default), `"forest"`, `"neutral"`. The background is always transparent, so diagrams blend with the terminal. |
| `mermaid_rows` | `24` | Target height (rows) of an inline mermaid diagram inside Markdown — **scales up too**: diagrams are vector-backed, so growing beyond the base raster re-rasterizes at the needed density and stays sharp (width-capped, aspect-preserving). The initial view also **fits the viewport**: when the window is shorter than the target, the diagram shrinks so the whole block is visible without scrolling. 0/invalid falls back to the default. |
| `restore_tabs` | `true` | Restore the previous tab set **per start directory**: each tab's root, tree cursor, and open preview come back on the next launch in the same directory. Saved on tab open/close/switch and on quit under `~/.config/konoma/sessions/`. `false` = always start fresh (nothing is read or written). |
| `md_task_states` | `[" ", "x"]` | Task-checkbox states cycled by `Space` in a Markdown preview, in order. Each entry is exactly one character. E.g. `[" ", "/", "x"]` adds an Obsidian-style in-progress state (shown as `[/]`). Invalid configs fall back to the default. |

## `[ui.sort]` — default tree order

| Key | Default | Description |
|---|---|---|
| `key` | `"name"` | Sort key: `"name"` / `"size"` / `"modified"` / `"ext"`. |
| `reverse` | `false` | Descending order. |
| `dirs_first` | `true` | Group directories before files. |

Change at runtime with the `s` menu (`n`/`s`/`m`/`e`, `r` = reverse, `.` = dirs first).

## `[ui.theme]` — colors

Colors accept `"#rrggbb"`, color names (`"black"`, `"lightblue"`, …), terminal indexes
(`"8"`), or `"none"`.

| Key | Default | Description |
|---|---|---|
| `bg` | `"none"` | App background. `"none"` keeps the terminal's default background (terminal transparency keeps working). |
| `code_bg` | `"#2b303b"` | Background band for Markdown code (inline + blocks). `"none"` removes it. |
| `code_label_align` | `"right"` | Code-block language badge position: `"right"` or `"left"`. |
| `code_label_bg` | `"auto"` | Language badge background: `"auto"` (brightened `code_bg`) / `"none"` / any color. |
| `code_theme` | `"TwoDark"` | Syntax-highlight theme (shared by code files and Markdown fences). Others: `"OneHalfDark"`, `"Dracula"`, `"Nord"`, `"gruvbox-dark"`, `"Catppuccin Mocha"`, `"Monokai Extended"`, `"Solarized (dark)"`, `"GitHub"`, … Separators/case are ignored; unknown names fall back to TwoDark. |

## `[[preview.rules]]` — what renders each file type

konoma's core model: **file format → viewer is declared in TOML**. Rules are evaluated
top to bottom; the first match wins. A rule matches by `glob` (file name,
case-insensitive) or `mime` (content sniffing, e.g. `"image/*"`), and renders via either
a built-in renderer or an external command.

> **Note:** if your config defines any `[[preview.rules]]`, your list **replaces** the
> built-in defaults — copy the full rule list from `config.example.toml` and edit, rather
> than adding a single rule.

Built-in renderers (`builtin = "..."`):

| Name | Renders |
|---|---|
| `markdown` | Decorated Markdown (headings, tables, links, task checkboxes, inline images, ```` ```mermaid ```` fences as diagrams). |
| `mermaid` | Standalone `.mmd`/`.mermaid` files as Unicode box-drawing diagrams (pure Rust, no external tools). |
| `image` | Full-screen image via kitty graphics (zoom/pan; GIFs animate automatically). |
| `svg` | Rasterized in-process (resvg; pure Rust) and shown as an image. |
| `video` | A representative frame extracted with `ffmpegthumbnailer`/`ffmpeg` (optional tools; a hint is shown if absent). No in-terminal playback — delegate to `mpv` via `command` if you want playback. |
| `pdf` | Pages rasterized one at a time (`pdftocairo`/`pdftoppm`/`qlmanage`/`sips`; on macOS the last two are always present). `J`/`K` turn pages (multi-page needs poppler). |
| `csv` / `tsv` | Aligned table with rainbow columns and a cell cursor (`hjkl` moves, `y →` copies cell/row/column). |
| `code` | Syntax-highlighted source (grammar resolved by extension → file name → first line). |
| `text` | Plain text. Also the automatic fallback for anything that looks like text. |

External command delegation:

```toml
[[preview.rules]]
glob = "*.{mp4,mov}"
command = "mpv {path}"      # {path} = the file, {out} = a temp output path
detached = true             # don't block the TUI (opens in a separate process)

[[preview.rules]]
glob = "*.mmd"
command = "merman -i {path} -o {out}.png --scale 2"
render_as = "image"         # treat the command's output as an image
```

Anything that matches no rule and doesn't look like text shows a safe
`[can not preview: <ext>]` screen — konoma never crashes on unknown input, and missing
optional tools degrade to a hint.

## `[editor]` — external editor

konoma never edits file contents itself; `e` delegates to your editor.

```toml
[editor]
command = "nvim"            # global default
[editor.ext]
md = "code -w"              # per-extension override (extension without the dot)
rs = "nvim +{line} {path}"  # {line} = the preview line you were on
```

Resolution order: `[editor.ext]` match → `editor.command` → `$VISUAL` → `$EDITOR` →
`vim`. Values are command + args, whitespace-separated; `{path}` is substituted if
present, otherwise the file path is appended.

**Opening at the preview line.** Pressing `e` from a windowed preview (plain text, code,
or raw Markdown via `R`) opens the editor at the caret line. Use a `{line}` token to place
it explicitly (`code -g {path}:{line}`, `hx {path}:{line}`, `nvim +{line} {path}`). Without
a `{line}` token, common editors are handled automatically — vim family (`+N`, plus `zt`
to scroll that line to the top of the window), VS Code (`-g path:N`), and Sublime/Helix/Zed
(`path:N`); other editors open at the top. Rendered Markdown reflows the source, so `e` opens
at the line whose text is at the top of your view — it searches the source for the on-screen
text and lands on it (block-structured docs land right on the section on screen; `R` gives an
exact caret open). If you have `Tab`-focused an item (link, checkbox, code block) that is on
screen, `e` opens at that item's line instead. Mermaid and images always open at the top.

## `[git]` — git integration

| Key | Default | Description |
|---|---|---|
| `tool` | `"lazygit"` | External git tool launched with `O` (command + args). |
| `diff` | `"unified"` | Initial diff layout: `"unified"` (vertical) / `"split"` (side by side) / `"auto"` (by width). Cycle at runtime with `s` while viewing a diff. |

## `[keys]` — keybindings

Every command is rebindable. The model: **each screen ("surface") maps keys to actions**,
helix-style:

```toml
[keys.tree]
"J" = "navigate:half_down"     # capital = Shift included
"ctrl-g" = "open_git_view"     # ctrl-x / c-x
"space d" = "file_delete"      # two tokens = chord (leader + key)
"o" = "noop"                   # disable a default binding

[keys.global]                  # inherited by every non-input surface
"Q" = "quit"
```

Surface names: `global`, `tree`, `tree_visual`, `preview_text`, `preview_text_visual`,
`preview_image`, `preview_table`, `sort`, `bookmarks`, `info`, `help`, and (git builds)
`preview_git_diff`, `git_changes`, `git_log`, `git_graph`, `git_branches`, `git_detail`.

Key tokens: single characters (uppercase = Shift), `space`, literals like `0 $ ! + - = . / '`,
modifiers `ctrl-<k>` (alias `c-<k>`), named keys `tab enter esc backspace delete up down
left right home end pageup pagedown`. Two whitespace-separated tokens form a chord
(`"y f"` = `y` then `f`). `Esc`/`Enter`/`Tab`/arrows and text-input keys are fixed and
cannot be rebound.

Action names are snake_case strings — the full annotated list is in
[`config.example.toml`](config.example.toml). The main groups:

- **Movement**: `navigate:down|up|top|bottom|page_down|page_up|half_down|half_up|left|right|line_home|line_end`
- **Tree**: `quit`, `close_tab_or_quit`, `tree_descend`, `tree_leave`, `tree_activate`, `filter_start`, `toggle_hidden`, `refresh`, `open_sort_menu`, `toggle_info`, `request_edit`, `cycle_path_style`, `set_anchor`, `reset_anchor`, `enter_visual`, `toggle_select`, `open_in_new_tab` (`Ctrl-t`: open the entry under the cursor in a new foreground tab)
- **Bookmarks**: `mark_set` (`m`), `mark_jump` (`'` — opens the list; plain letters inside it jump), `bookmark_edit` (`ctrl-e`), `bookmark_delete` (`ctrl-d`), `bookmark_close`. `m`/`'` are bound in both the tree and previews (a preview bookmarks the shown file).
- **Path copy** (`y` leader): `copy_name`, `copy_relative`, `copy_full`, `copy_parent`, `copy_at_ref` (`@relative/path` for AI chats)
- **File management** (`Space` leader): `file_create`, `file_rename`, `file_delete`, `file_copy`, `file_cut`, `file_paste`, `file_duplicate` (`Space→D`: duplicate the cursor/selection in place, e.g. `note copy.md`)
- **Preview**: `preview_back`, `search_start`, `search_next`, `search_prev`, `preview_enter_visual` (`v`), `preview_enter_visual_line` (`V`), `preview_copy_selection`, `preview_copy_selection_ref` (`Y` = `@path#L12-34`), `toggle_markdown_raw` (`R`), `link_focus_next/prev`, `link_open` (`Enter` = current tab), `open_link_new_tab` (`Ctrl-t` = new tab), `image_zoom_in/out/reset`, `pdf_next_page`, `pdf_prev_page`, `preview_next_file` / `preview_prev_file` (`Ctrl-n` / `Ctrl-p` — page to the next/previous file in tree order, skipping directories, wrapping at the ends), `table_copy_cell/row/column`
- **Agent Watch**: `toggle_follow` (`F`), `toggle_changed_filter` (`C`), `jump_next_change` (`n`), `jump_prev_change` (`N`)
- **Paste-jump** (`global`): `paste_jump` (`P`) — reads a path or GitHub link from the clipboard and jumps there (reveal + preview). Understands local absolute/relative paths, GitHub `blob`/`raw` URLs, and `#L123` / `:123` line anchors; switches root to the target's repository when it lies outside the current root.
- **Git**: `open_git_view` (`o`), `open_git_diff_cursor` (`d`), `git_stage`, `git_unstage`, `git_stage_all`, `git_unstage_all`, `git_discard`, `git_commit`, `git_open_log`, `git_open_graph`, `git_open_branches`, `git_launch_tool` (`O`), `cycle_diff_layout`, `git_copy_*`, `branch_*`
- **Tabs / app** (`global`): `tab_new` (`t`), `toggle_tab_list` (`T` — tab list; `tab_list_close` = `d` inside it), `tab_prev`/`tab_next` (`[`/`]`), `quit` (`Q`), `toggle_help` (`?`). `tab_close` has no default key (closing is `q` on the tree; rebind with `"w" = "tab_close"` if you want it back)
- `noop` (alias `disabled`) removes a default binding.

Conflicting bindings that would break essential defaults (stealing a leader prefix, tab
keys, …) are detected at startup, reported in the footer, and reverted — a bad config
never bricks the UI.

Backward-compatible aliases for path copy also exist at the `[keys]` top level
(`copy_prefix`, `copy_name`, `copy_relative`, `copy_full`, `copy_parent`).

## Data files

| Path | Contents |
|---|---|
| `~/.config/konoma/config.toml` | This configuration. |
| `~/.config/konoma/bookmarks.toml` | Global (uppercase) bookmarks — absolute paths. |
| `~/.config/konoma/bookmarks/<dir>.toml` | Local (lowercase) bookmarks, one file per start directory. |
| `~/.config/konoma/sessions/<dir>.toml` | Tab session (`restore_tabs`), one file per start directory. |
| `~/.cache/konoma/remote-images/` | Cache for remote images embedded in Markdown. |

## Fonts & terminal requirements

- **Images / SVG / video thumbnails / PDF pages** need a terminal with the kitty
  graphics protocol (Ghostty, kitty, WezTerm).
- **Icons** (`ui.icons = true`, the default) need Nerd Font glyphs: either add
  `Symbols Nerd Font Mono` as a fallback font in your terminal, or use an NF-bundled
  font (HackGen Console NF, UDEV Gothic NF, …). Without one, set `ui.icons = false`
  for plain-symbol fallbacks.
- **Optional tools**: `poppler` (multi-page PDF), `ffmpegthumbnailer`/`ffmpeg` (video
  thumbnails), `git` + `lazygit` (git suite / external tool). Everything degrades
  gracefully when absent.
