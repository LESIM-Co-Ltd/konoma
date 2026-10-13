# Configuring konoma

konoma reads a single TOML file:

```
~/.config/konoma/config.toml
```

Everything is optional — **no config file is required**. A missing or broken file never
prevents startup; konoma falls back to the defaults described below (invalid values fall
back per-key). A fully commented example lives in
[`config.example.toml`](config.example.toml); copy it as a starting point (a
Japanese-annotated copy is at [`config.example.ja.toml`](config.example.ja.toml)):

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
- [`[plugins.<id>]` — plugin switches and settings](#pluginsid--plugin-switches-and-settings)
- [`[editor]` — external editor](#editor--external-editor)
- [`[git]` — git integration](#git--git-integration)
- [`[external]` — external process on/off switches](#external--external-process-onoff-switches)
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

[plugins.video]
enabled = false             # no video thumbnails

[keys.global]
"I" = "tab_new_after"       # put an old key back (the new tab is `t` now)
```

## `[ui]` — appearance & behavior

| Key | Default | Description |
|---|---|---|
| `show_hidden` | `false` | Show dotfiles at startup (toggle at runtime with `.`). |
| `tabbar` | `"auto"` | Tab bar visibility: `"always"` / `"auto"` (only with 2+ tabs) / `"hidden"`. |
| `icons` | `true` | Nerd Font icons in the tree, Markdown links and task checkboxes. Set `false` on terminals without a Nerd Font — konoma falls back to plain ASCII symbols (no tofu). |
| `wrap` | `true` | Soft-wrap text previews. `false` = no wrap + horizontal scrolling (`h`/`l`, `0`/`$`). Switch at runtime in the `,` menu (`w`). |
| `line_numbers` | `false` | Line-number gutter in code/text previews. |
| `git_gutter` | `true` | Editor-style git change gutter (green added / blue modified / red deleted) in code/text previews of files with uncommitted changes. |
| `tab_width` | `4` | Tab stop width in code/text previews (`0` keeps raw tabs). Values outside the realistic range `1..=16` fall back to `4`. |
| `syntax_highlight` | `true` | Syntax highlighting for code previews (`false` = plain text, fastest). Also `[plugins.code] syntax_highlight`, which wins. |
| `preview_loading` | `"indicator"` | First-open wait for heavy code previews: `"indicator"` (loading screen) or `"progressive"` (plain text first, colors swap in when ready). |
| `path_style` | `"relative"` | Title-bar path form: `"relative"` / `"home"` (`~/...`) / `"full"`. Cycle at runtime in the `,` menu (`p`). |
| `keys` | `"vim"` | Paging-key scheme for previews: `"vim"` (`Ctrl-f/b`, `Ctrl-d/u`) or `"less"` (`f`/`b`, `d`/`u`; `Space` stays the operations menu). |
| `lang` | `"auto"` | UI language for help/hints/messages: `"auto"` (OS language) / `"en"` / `"jp"`. |
| `statusbar` | `"split"` | Status chrome layout: `"split"` (context top, hints bottom) / `"bottom"` / `"top"`. |
| `image_render_scale` | `1.0` | Image display scale (0.1–1.0). Smaller = fewer pixels sent to the terminal = faster draws, smaller image. |
| `svg_max_px` | `800` | Max rasterization side (px) for SVG previews. Larger = crisper but heavier. Also `[plugins.svg] max_px`, which wins. |
| `details` | `[]` | Metadata columns on tree rows, in order. Available: `"size"`, `"modified"`, `"perm"`, `"type"`, `"items"` (directory entry count). |
| `graph_max_branches` | `12` | Cap on branches drawn simultaneously in the commit graph (`p`, then `Tab` to the graph). `0` = unlimited. Toggle branches at runtime with `b` inside the graph. |
| `graph_base_branches` | `[]` | Ordered list of preferred base branches for the graph, e.g. `["main", "develop"]`. The first one that exists becomes the base (pinned to lane 0); the array order becomes the display priority. Also used as the candidate pool for a linked worktree's diff (`d` in the worktree list, `git.worktrees`): the candidates are the entries here that exist plus the main worktree's branch, and whichever candidate's merge-base (common ancestor) with the diffed worktree's HEAD is **newest** becomes the base — i.e. the branch it actually diverged from, not necessarily the first one listed; ties fall back to this array's order. |
| `commit_meta_align` | `"right"` | Author/date column in git log & graph: `"right"` (aligned right-edge column) or `"inline"` (directly after the subject). |
| `confirm_quit` | `true` | Ask before quitting (`q`/`y`/`Enter` = quit, `n`/`Esc` = cancel; `qq` quits quickly). `false` = quit immediately. |
| `confirm_jj_sync` | `true` | Ask before `s` on a jj changes screen lets jj snapshot the working copy — the one thing konoma writes to a jj repository, and only when asked. It exists for what konoma cannot see on its own: a file whose contents changed while its timestamp did not. `false` = snapshot without asking. |
| `confirm_bookmark_overwrite` | `true` | Ask before a bookmark key (`m`) overwrites a **different** existing path (`y`/`Enter` = overwrite, `n`/`Esc` = cancel). Re-registering the same path or an unused key never prompts. `false` = overwrite silently. |
| `csv_rainbow` | `true` | Rainbow column colors in CSV/TSV table previews. `false` = monochrome (alignment and navigation unchanged). Also `[plugins.csv] rainbow` (and `[plugins.spreadsheet] rainbow`, which follows it), which win. |
| `filter_mode` | `"fuzzy"` | How the `/` tree filter matches: `"fuzzy"` (ranked subsequence match, fzf-style; space-separated words are AND-ed) or `"substring"` (the older plain substring test, pool order preserved). Case is ignored either way. |
| `follow_view` | `"diff"` | How follow mode (`F`) opens a changed file: `"diff"` (full-screen diff **since follow-start** — pre-existing uncommitted changes stay hidden; open the `,` menu in the diff and press `f` to switch to the full git diff) or `"file"` (normal preview scrolled to the first changed hunk). Files without a diff and media always open as `"file"`. |
| `diff_view` | `"rendered"` | Which of the full-screen diff's presentations it opens showing. `"rendered"` draws a Markdown file's diff as decorated blocks — removed content red/dim, added green, changed-to amber — so an edit's deletions stay visible, not only its final result (falls back to `"source"` for a non-Markdown file, which has no block-diff renderer of its own). For a changed image or PDF, `"rendered"` **is** the side-by-side old/new view (layout picked by `[git] media_diff`, below) — those two kinds have no plain-text `"source"` to show instead. SVG has all three (it is both a picture and real text): `"source"` is its raw markup diff, `"rendered"` the side-by-side pictures. `"source"` (everywhere else) is the classic unified/split diff. `"preview"` is the file's ordinary preview with a change gutter (for an image/PDF, just the new file, no overlay). A kind that can be neither line-diffed nor drawn side by side (video, archives, other binaries) shows a one-line size/byte-count summary instead — never the old, misleading "(no changes)" for a binary that did change. `R` cycles between whichever presentations the file being diffed actually has (a plain text file: `"source"` ⇄ `"preview"` only; a summary-only binary: none, and no hint is shown). An unrecognized value falls back to `"rendered"`. |
| `busy_indicator` | `true` | Small spinner + job label at the top-right while background work runs (git-ignored scan, media decode, highlight warm-up, image fetches). Idle shows nothing and costs nothing. A **file operation in progress** (copy / move / duplicate / delete, with its `N/M` progress) is always shown regardless of this setting — it is what you are waiting on, not background bookkeeping. |
| `mermaid` | `"image"` | How mermaid diagrams render. `"image"` rasterizes them in-process (pure Rust, mermaid.js-quality — no browser/Node): standalone `.mmd` files show full screen with zoom/pan (`+`/`-`/`hjkl`; zooming re-rasterizes so it stays sharp), and ```mermaid fences in Markdown render inline (`Tab` focuses a diagram — it gets a cyan frame and the view scrolls to show it whole; `+`/`-` zoom **in place** without moving the layout, `hjkl` pan while zoomed, `=` fits, `Enter` opens it full screen, `q` returns). `"text"` keeps the legacy Unicode box-drawing rendering. Unsupported diagrams, render failures, and non-graphics terminals degrade to text automatically. Also `[plugins.mermaid] mode`, which wins. |
| `mermaid_theme` | `"dark"` | Color theme for image-mode diagrams: `"dark"` (matches dark terminals), `"light"`, `"classic"` (mermaid.js default), `"forest"`, `"neutral"`. The background is always transparent, so diagrams blend with the terminal. Has no effect when `mermaid_routing = "konoma-orthogonal"` — that mode always uses konoma's own designed palette instead. Also `[plugins.mermaid] theme`. |
| `mermaid_curve` | `"basis"` | Curve a flowchart's edges are drawn in — mermaid's `flowchart.curve` (mermaid-js#2817 / #2549, mermaid's most-requested flowchart feature: a straight/right-angle line instead of a spline). All 13 of mermaid's own curve names are implemented; an unrecognized value falls back to `"basis"`. `"basis"` (default) is a smooth spline — mermaid's own default, and what every diagram drew before this setting existed, so the look never changes unless you set this. `"linear"` draws straight segments through every waypoint; `"step"` / `"stepBefore"` / `"stepAfter"` draw a right-angle line, differing in where the corner sits (midpoint / vertical-first / horizontal-first). `"natural"` / `"cardinal"` / `"catmullRom"` are smooth splines that pass through every waypoint (unlike `"basis"`), each a different d3-shape interpolation. `"monotoneX"` / `"monotoneY"` are a smooth spline that never overshoots a waypoint, along the named axis. `"bumpX"` / `"bumpY"` draw an S-curve between every pair of waypoints, offset on the named axis. `"rounded"` draws straight segments with every corner cut to a small rounded curve — right angles like `"linear"`, without the sharp corner (mermaid's own curve, not part of d3-shape). Three ways to set the curve, weakest first: this setting < the diagram's own `%%{init: {"flowchart": {"curve": "..."}}}%%` directive < a `linkStyle <n> interpolate <curve>` naming one specific edge — each one, if present, overrides this setting and every weaker source. Only a flowchart's own edges read any of this — every other diagram kind always draws `"basis"`. Also `[plugins.mermaid] curve`. |
| `mermaid_routing` | `"splines"` | How a flowchart's edges are routed — a konoma-only extension, independent of `mermaid_curve` above (which only picks the curve shape, not the route). `"splines"` (default) is the classic mermaid layout: edges follow the diagram's own curve (`mermaid_curve`) — the look never changes unless you set this. `"konoma-orthogonal"` is konoma's own orthogonal router (flowchart and stateDiagram-v2; every other diagram kind is unaffected): every edge bends only at 90° with sharp corners (zero radius) and always meets a node or subgraph border head-on, never at an angle. Decision nodes (`{}`) draw as a chamfered rectangle (6px cut corners) instead of a diamond, so every edge lands on a flat connection face. Multiple edges on the same side of a node spread across 16px-spaced ports, widening the node on that axis if there isn't room. A straight-through edge between adjacent ranks aligns to a shared centerline — a true zero-bend straight lane. Where edges cross, the one passing over (a back-edge or detour) opens a 12px gap so the stacking order reads at a glance. Back-edges and detours route around the diagram's outer lane (16px outside every element; multiple such edges offset 8px apart). A label sits on a plate on the line itself, and its segment is padded to at least the label's width plus margin — widening the rank gap automatically when it doesn't fit. Arrowheads are painted the edge's own color (after `linkStyle`/`classDef`) and stop 1px short of the node border. Named after ELK's `edgeRouting: ORTHOGONAL` and Graphviz's `splines=ortho`; the `konoma-` prefix marks it as konoma's own value so it can never collide with a future mermaid-defined `"orthogonal"`. `mermaid_curve` has no effect in this mode — there is no curve, only right-angle segments. This mode also switches the diagram to konoma's own designed palette and style — dark node fill with rounded corners and thin strokes, dashed subgraph frames, and `classDef`/`style` colors applied consistently to a node's border, its edges, arrowheads, and label — so `mermaid_theme` has no effect here either. An unrecognized value falls back to `"splines"`. Also `[plugins.mermaid] routing`. |
| `mermaid_rows` | `24` | Upper limit on the height (rows) of an inline mermaid diagram inside Markdown. A diagram is shown at its natural size — matching the surrounding body text — and is never enlarged; it's only shrunk (aspect preserved) when that natural size exceeds this cap or the available width. Diagrams re-rasterize at the needed density on zoom and stay sharp (the display size itself doesn't change). The initial view also fits the viewport: shorter windows shrink the diagram further so the whole block is visible. 0/invalid falls back to the default. Also `[plugins.mermaid] rows`. |
| `math` | `"image"` | How LaTeX math renders. `"image"` (default) rasterizes `$…$` / `$$…$$` in-process via RaTeX (pure Rust, KaTeX quality, no browser/Node) and shows each as an inline image (inline math is always drawn in the running text, like ordinary Markdown, also inside table cells, headings, emphasis and quotes — an expression too wide for the line shrinks to fit instead of being lifted onto its own line; math in a link label stays literal; display math is centered). `"text"` leaves the raw LaTeX as plain text. Non-graphics terminals and render failures degrade to the raw LaTeX automatically. Also `[plugins.math] mode`, which wins. |
| `math_color` | `"#d0d0d0"` | Glyph color for image-mode math. RaTeX paints equations pure black, invisible on a dark terminal; konoma is dark-terminal-first, so equations are recolored to this over a transparent background (the terminal shows through, like mermaid). On a light terminal set a dark color (e.g. `"#202020"`). Accepts any color usvg parses (`#hex`, `rgb(…)`, or a CSS color name); an unrecognized or fully-transparent value falls back to the default (a typo can't silently blank equations). Also `[plugins.math] color`, which wins. |
| `restore_tabs` | `true` | Restore the previous tab set **per start directory**: each tab's root, tree cursor, and open preview come back on the next launch in the same directory. Saved on tab open/close/switch and on quit under `~/.config/konoma/sessions/`. `false` = always start fresh (nothing is read or written). |
| `restore_single_tab` | `true` | When `restore_tabs` is on: also persist/restore a session that has only one tab. `false` = don't save single-tab sessions — quitting with a lone tab deletes that directory's session file, so the next launch starts fresh; sessions with two or more tabs still save/restore. |
| `tree_cursor` | `"origin"` | Where the cursor lands after moving to the parent directory (`h`). `"origin"` puts it back on the directory you just left (the same spirit as `q` returning from a preview — you land where you came from, not at the top); `"top"` always lands on the first row (the older behavior). An unrecognized value is treated as `"origin"`. If the left directory isn't in the parent's listing (e.g. it's hidden and `show_hidden` is off), this falls back to the top. Descending into a directory (`l`) always starts at the top either way — there is no "came from" row there. |
| `md_task_states` | `[" ", "x"]` | Task-checkbox states cycled by `Enter` in a Markdown preview, in order. Each entry is exactly one character. E.g. `[" ", "/", "x"]` adds an Obsidian-style in-progress state (shown as `[/]`). Invalid configs fall back to the default. |
| `md_autolink` | `true` | Auto-link bare URLs and emails in Markdown previews (GFM autolink), like GitHub: a plain `https://…`, `www.…`, or `foo@bar.com` becomes a focusable link (`Tab` to it, `Enter` opens it). Never applied inside code spans / code fences. `false` = leave them as plain text. |
| `md_alerts` | `true` | Render GitHub-style alerts as colored callout boxes (icon + label) instead of plain blockquotes. Markers (case-insensitive, plus common aliases): `> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]`. `false` = ordinary blockquote (marker kept literal). |
| `md_emoji` | `true` | Convert `:shortcode:` emoji to real Unicode in Markdown previews, like GitHub (`:rocket:` → 🚀). Shortcodes with no Unicode equivalent (GitHub-custom like `:shipit:`) and shortcodes inside code stay literal. `false` if emoji width upsets column alignment. |
| `md_frontmatter` | `true` | Recognize leading YAML front matter (`---` … `---` at the very start) and show it as a compact dim metadata block instead of a rule + raw YAML. `false` renders it as ordinary Markdown. |
| `md_footnotes` | `true` | Render GFM footnotes: `text[^1]` references become superscript numbers and the `[^1]: …` definitions are collected into a numbered footnotes section at the end. `false` leaves them literal. |
| `md_inline_html` | `true` | Render common inline HTML the Markdown engine strips: `<del>`/`<s>`/`<strike>` as strikethrough, `<kbd>` as an inline-code keycap, `<sup>`/`<sub>` as Unicode (when the text maps), `<br>` as a hard line break. (`<mark>`/`<ins>` keep only their text either way.) `false` strips all of them. |
| `md_details` | `"auto"` | How `<details>` blocks start out. `"auto"` honors the `open` attribute like GitHub (`<details>` collapsed, `<details open>` expanded); `"open"` always expanded; `"closed"` always collapsed. Either way `Tab` focuses the `<summary>` and `Enter` toggles it. |
| `md_table_align` | `"left"` | Where a table sits in a Markdown preview: `"left"` (the grid starts at the left edge), `"center"`, `"right"`. Moves the whole box — GFM pipe tables and HTML `<table>` alike, and any image inside a cell travels with it. It does not change alignment *within* a cell (a column's `:---:` and an HTML cell's `align=` still decide that). A table as wide as the pane stays flush left. |
| `md_image_align` | `"center"` | Where a block image sits in a Markdown preview: `"left"`, `"center"`, `"right"`. Covers a standalone `![alt](url)`, a packed row of badges, and a mermaid fence's diagram (its caption and focus frame follow it). It does not cover an image inside a table cell (its cell's alignment places that) or display math (centered by typesetting convention). An image as wide as the pane stays flush left. |

## `[ui.sort]` — default tree order

| Key | Default | Description |
|---|---|---|
| `key` | `"name"` | Sort key: `"name"` / `"size"` / `"modified"` / `"ext"`. |
| `reverse` | `false` | Descending order. |
| `dirs_first` | `true` | Group directories before files. |

Change at runtime in the tree's `,` menu (`n`/`s`/`m`/`e`, `r` = reverse, `.` = dirs first).

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

> **Note:** your `[[preview.rules]]` are tried first; a file none of them matches goes on to
> the **standard plugins**, which carry the default assignments (markdown, mermaid, svg, image,
> csv, spreadsheet, document, presentation, archive, code, video, pdf). Writing one rule therefore
> adds to the defaults — it no longer replaces them. Run `konoma --plugins` to list every assignment
> (yours first), and switch a standard plugin off with `[plugins.<id>] enabled = false` (below). The old
> `builtin = "tsv"` is read as the CSV viewer with a tab separator, and `builtin = "document"` as the Word or the
> presentation viewer by file extension.

Built-in renderers (`builtin = "..."`):

| Name | Renders |
|---|---|
| `markdown` | Decorated Markdown (headings, tables, links, task checkboxes, inline images — animated GIFs cycle in place — and ```` ```mermaid ```` fences as diagrams). |
| `mermaid` | Standalone `.mmd`/`.mermaid` files as diagrams. Renders as a real image by default (pure Rust, full-screen zoom/pan); set `[ui] mermaid = "text"` for Unicode box-drawing. Controlled by the `[ui] mermaid` option above. |
| `image` | Full-screen image drawn with the terminal's graphics protocol (zoom/pan; GIFs animate automatically). |
| `svg` | Rasterized in-process (resvg; pure Rust) and shown as an image. |
| `video` | A single representative frame, shown as a thumbnail. **H.264 and HEVC inside `.mp4`/`.m4v`/`.mov` and `.mkv`/`.webm` are decoded natively in Rust — no external tool needed** (HEVC covers what an iPhone records by default). Everything else (VP9, AV1, older codecs, the `.avi` container, and the uncommon profiles — H.264 in 10-bit / 4:2:2 / 4:4:4 / monochrome, HEVC outside Main / Main 10 4:2:0) is extracted with `ffmpegthumbnailer`/`ffmpeg` if installed, and shows a hint if not. No in-terminal playback either way — delegate to `mpv` via `command` if you want playback. |
| `pdf` | Pages rasterized natively in Rust (`hayro`; one page at a time, no external tool needed) — `J`/`K` turn any page. On macOS only, a PDF `hayro` can't render (encrypted, corrupt, or otherwise unsupported) falls back to the system's own `qlmanage`/`sips` — always present, nothing to install — but those can only produce the **first** page. |
| `csv` / `tsv` | Aligned table with rainbow columns and a cell cursor (`hjkl` moves, `y →` copies cell/row/column). |
| `archive` | Lists entries (name / size / modified) for `.zip`/`.tar`/`.tar.gz`/`.tgz` in the same aligned-table renderer as CSV/TSV — metadata only, nothing is extracted (`hjkl` / `y →` c/r/C copy work the same). |
| `spreadsheet` | `.xlsx`/`.xlsm`/`.xltx`/`.xltm`/`.xlsb`/`.xls`/`.ods` as a table in the same renderer as CSV (pure Rust, no external tool): each cell is shown in its Excel number format (thousands separators, percent, currency, dates, times, Japanese eras), with column letters and row numbers. Rows and columns hidden in Excel (or hidden by a filter) are still shown; hidden *sheets* are not. `J`/`K` switch sheets (only when there are 2+ visible sheets; the cursor goes back to A1 on each sheet). Formulas are never evaluated and macros never run (an `.ods` formula is shown in Excel's notation; Excel 365 error values such as `#SPILL!` are shown as they are; a pivot table shows the totals Excel saved, not recalculated); the cell popup (`Enter`) shows the displayed text, raw value, type, formula and format. `/` search covers the sheet on screen only (at most 5,000 hits; the footer reads `[1/5000+]` when cut), and a cell's displayed text is cut at 1,024 characters. A sheet beyond 100,000 rows / 4M cells / 128 MiB of text shows its beginning marked `(capped)` (only the sheet on screen is read; `J`/`K` read the next in the background). Password-protected, oversized (file > 256 MiB, > 1 GiB expanded, > 10,000 zip entries, > 4,096 sheets, XML nested deeper than 256 levels, an `.xls` whose sheets share the same records, or an `.ods`/`.xls` too large to open safely) or corrupt files show a reason instead. Merged cells show their value only in the top-left cell. |
| `document` | `.docx`/`.docm`/`.dotx`/`.dotm`/`.odt`/`.ott` (Word / OpenDocument text) converted to Markdown by konoma itself (pure Rust, no external tool) and drawn by the Markdown renderer, so `O` (outline) / `Tab` / `/` and `$…$` math all work. Headings, bold / italic / strikethrough, bullets and numbering as the document shows them (`(a)`, `ア`, `①`, `第1条` ...), tables (a merged cell shows its value in the top-left cell), pictures, footnotes / endnotes, links (external URLs and to headings in the document), text boxes, and the saved text of fields such as a table of contents. Tracked changes show the final text; comments and headers / footers are not shown. Equations (Word's OMML, ODF's MathML) are converted to LaTeX and drawn as math; one that cannot be converted stays as text. `R` shows the converted Markdown (`v`/`V` select, `y` copies); the document file itself is never written, so checkbox toggling is off. `.doc` is not previewed (`e` opens it in an Office app). Password-protected, damaged or too-large files (same package limits as spreadsheets) show a reason instead; a conversion cut at its limits (1,000,000 bytes / 5,000 lines of Markdown, 300 pictures, 16 MiB per picture / 96 MiB in total, 20,000 table cells) shows the start and says so in the title. Known limit: inside a table cell, `*`, `` ` `` and `~~` that appear two or more times are replaced by look-alike characters (konoma's table layout does not understand escapes). Checked against LibreOffice-made files, not yet against files made by Microsoft Word. **Presentations** (`.pptx`/`.pptm`/`.ppsx`/`.ppsm`/`.potx`/`.potm`/`.odp`/`.otp`) are not converted to a page of text but **drawn as pictures of the slides** by konoma itself (its own DrawingML / ODF to SVG renderer, drawn by the supervised SVG process): shapes, text, pictures including EMF / WMF, gradients and effects, groups, connectors, tables, charts (3-D ones with depth, no perspective) and SmartArt (from the saved drawing part). Each slide sits under a `## Slide 3: Title` heading (hidden slides marked `(hidden)`) with the speaker notes as a quote labelled `Notes`; comments are not shown. `J`/`K` (`page_next` / `page_prev`) jump to the next / previous slide (nothing happens on a one-slide deck) and the status line shows `slide 3/12`. `R` switches between the pictures and a **text view** (the slides as Markdown: title first, then text, bullets, tables (merged cells), pictures, equations and `[chart: title]` markers in reading order — by position on the slide, a two-column slide column by column; date / footer / slide-number placeholders left out), keeping the current slide; without kitty / iTerm2 / sixel graphics (half-block terminals) the text view is the default. Office fonts are substituted by faces that exist on macOS / Linux, so line breaks can differ from PowerPoint. Limits: 1,000 slides, 5,000 shapes per slide, plus the limits above. Not drawn: 3-D shape effects, SmartArt without a saved drawing part (its text is in the text view), LibreOffice's `svm` pictures, EMF+-only pictures. The old binary `.ppt` is not previewed (`e` opens it). Presentations have been compared against LibreOffice's renderings, not against PowerPoint itself. |
| `code` | Syntax-highlighted source (grammar resolved by extension → file name → first line). |
| `text` | Plain text. Also the automatic fallback for anything that looks like text. |

External command delegation:

```toml
[[preview.rules]]
glob = "*.{mp4,mov}"
command = "mpv {path}"      # {path} = the file, {out} = a temp output path
detached = true             # don't block the TUI (opens in a separate process)

[[preview.rules]]
glob = "*.{dot,gv}"         # No builtin for Graphviz -- delegate it. (Mermaid needs no rule: konoma
render_as = "image"         # renders it natively.) render_as = treat the command's output as an image.
command = "dot -Tpng -o {out}.png {path}"   # No shell: write the command as argv, not as a pipeline.
```

Omitting `render_as` (or setting it to anything other than `"image"`) captures the
command's output and shows it as plain text through the ordinary windowed reader — a
missing/failing command (binary not found, non-zero exit, `{out}` never produced)
degrades safely to `[can not preview: <ext>]` with the reason attached, instead of
crashing. `command = ""` (or a whitespace-only value) is treated the same way — no
command to delegate to — never as "run the previewed file itself".

**Mermaid needs no rule of its own.** konoma renders `.mmd`/`.mermaid` files and
```` ```mermaid ```` fences itself — pure Rust, no Node, no browser — and the `[ui] mermaid`
options above control it. If what you specifically want is the output of *mermaid.js itself*
(the same engine behind mermaid.live and GitHub), you can delegate standalone files to
[`mmdc`](https://github.com/mermaid-js/mermaid-cli), the official mermaid CLI:

```toml
[[preview.rules]]
glob = "*.{mmd,mermaid}"    # tried before the standard mermaid plugin
render_as = "image"
command = "mmdc -q -i {path} -o {out}.png -t dark -b transparent -s 3"
```

`-i`/`-o` are the input and output files (mmdc picks svg/png/pdf from the output
extension), `-t` is the theme (`default` / `forest` / `dark` / `neutral`), `-b` the
background color, `-s` the Puppeteer scale factor (`3` renders at 3×, so the PNG still has
pixels to spare when you zoom in), and `-q` keeps mmdc's progress log out of the error
line. `{out}` deliberately has no extension, so the `.png` you append in `{out}.png` is
what tells mmdc which format to write — konoma accepts the suffixed file it produces.

**Unlike everything else konoma leans on, `mmdc` is a heavyweight dependency.** You install
it with `npm install -g @mermaid-js/mermaid-cli`; it wants Node 18.19+ / 20+, and it drives
a real browser through Puppeteer, so the install brings a bundled Chromium with it and every
preview starts one — expect a second or two per diagram rather than an instant one (konoma
gives any delegated command 30 seconds before it gives up). konoma's own mermaid path needs
none of that. Take the trade deliberately.

Two limits of this rule:

- It covers **standalone files only**. ```` ```mermaid ```` fences inside a Markdown
  document are composited by the `markdown` renderer and always take konoma's built-in
  path — a `[[preview.rules]]` entry matches whole files, so it cannot reach inside one.
- Don't aim it at `*.md`. Given a Markdown input, `mmdc` rewrites the whole document
  (replacing each fence with a generated image file) instead of producing one diagram —
  not what a preview wants.

If `mmdc` isn't installed, the rule degrades exactly like any other missing command, and
`[external] preview_commands = false` switches it off (along with every other delegation)
without touching your rule list.

Anything that matches no rule and doesn't look like text shows a safe
`[can not preview: <ext>]` screen — konoma never crashes on unknown input, and missing
optional tools degrade to a hint.

## `[plugins.<id>]` — plugin switches and settings

Each preview is made by a plugin built into konoma. The standard ones are `markdown`, `mermaid`, `math` (formulas
inside Markdown), `svg`, `image`, `csv` (CSV and TSV), `spreadsheet`, `document` (Word), `presentation`,
`archive`, `code`, `video` and `pdf`. `konoma --plugins` prints which plugin shows which files, in the order a
file is decided: your `[[preview.rules]]` first, then the standard plugins, then plain text, then
`[can not preview]`.

```toml
[plugins.video]
enabled = false      # its files go to the next step: plain text if they read as text, else [can not preview]

[plugins.csv]
rainbow = false      # monochrome CSV / TSV columns
```

- `enabled` (`true` by default) — `false` switches the plugin off. An unknown id, a value that is not `true`/`false`
  and a plugin that cannot be switched off are reported at startup.
- Every other key of the table is that plugin's own setting. They **replace the matching `[ui]` keys**: the old
  `[ui]` name is still read and is used when the plugin table does not set the value; if both are set, the plugin
  table wins. A key the plugin does not have (`[plugins.markdown] foo = 1`) is reported at startup and ignored.
- **Old names are announced once, at start.** When the config still uses one of the `[ui]` names in the table
  below and the plugin's own key is not set, the footer says `config: [ui] md_alerts is an old name — write
  [plugins.markdown] alerts instead` (and `(+n more)` for the rest). The old name keeps working; the message goes
  away when you move the value. What is wrong (an unreadable config, key conflicts, a bad `[plugins.*]` table) is
  listed first and these hints last; if they do not all fit the width of the terminal, the footer ends with
  `(+n more)` instead of cutting a message off.

| Plugin | Setting | Default | Replaces |
|---|---|---|---|
| `csv` | `rainbow` | `true` | `[ui] csv_rainbow` |
| `spreadsheet` | `rainbow` | follows `csv` | `[plugins.csv] rainbow`, `[ui] csv_rainbow` |
| `markdown` | `task_states` | `[" ", "x"]` | `[ui] md_task_states` |
| `markdown` | `autolink` | `true` | `[ui] md_autolink` |
| `markdown` | `alerts` | `true` | `[ui] md_alerts` |
| `markdown` | `emoji` | `true` | `[ui] md_emoji` |
| `markdown` | `frontmatter` | `true` | `[ui] md_frontmatter` |
| `markdown` | `footnotes` | `true` | `[ui] md_footnotes` |
| `markdown` | `inline_html` | `true` | `[ui] md_inline_html` |
| `markdown` | `details` | `"auto"` | `[ui] md_details` |
| `markdown` | `table_align` | `"left"` | `[ui] md_table_align` |
| `markdown` | `image_align` | `"center"` | `[ui] md_image_align` |
| `code` | `syntax_highlight` | `true` | `[ui] syntax_highlight` |
| `svg` | `max_px` | `800` | `[ui] svg_max_px` |
| `mermaid` | `mode` | `"image"` | `[ui] mermaid` |
| `mermaid` | `theme` | `"dark"` | `[ui] mermaid_theme` |
| `mermaid` | `curve` | `"basis"` | `[ui] mermaid_curve` |
| `mermaid` | `routing` | `"splines"` | `[ui] mermaid_routing` |
| `mermaid` | `rows` | `24` | `[ui] mermaid_rows` |
| `math` | `mode` | `"image"` | `[ui] math` |
| `math` | `color` | `"#d0d0d0"` | `[ui] math_color` |

The values and what they do are described with the `[ui]` keys above. The other plugins (`image`, `video`, `pdf`,
`archive`, `document`, `presentation`) take no setting from this table yet. A Word document is drawn with the
`markdown` plugin's options (except front matter, which a converted document does not show), and so is a
presentation's text view.
Display settings that belong to the viewer rather than to one plugin (`wrap`, `line_numbers`, `git_gutter`,
`tab_width`, `image_render_scale`) stay in `[ui]`, and `[external]` still has the switches for external processes.

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
| `tool` | `"lazygit"` | External git tool launched with `!` from the tree, a preview, a diff or the changes list (`!`). Command + args. |
| `diff` | `"unified"` | Initial diff layout: `"unified"` (vertical) / `"split"` (side by side) / `"auto"` (by width). Cycle at runtime with `s` while viewing a diff. |
| `media_diff` | `"auto"` | Initial layout for the image/PDF/SVG side-by-side "diff" (`[ui] diff_view = "rendered"` on one of those kinds — `docs/FEATURE-MEDIA-DIFF.md`). `"auto"` (default) picks whichever of side-by-side/stacked lets the picture render larger. `"side"` (aliases: split/horizontal/side-by-side) always lays the two pictures left/right; `"stack"` (aliases: vertical/stacked/unified) always lays them top/bottom. A separate setting from `diff` above on purpose — sharing one would make `"auto"` mean two different defaults for two unrelated layouts. Cycle at runtime with `s` while viewing a media diff (auto → side → stack → auto). |
| `worktree_dir` | `"../"` | Where `c` (in the worktree list, `w` from the changes list) places a newly created linked worktree. Resolved against the **main** worktree's own path (not wherever the tab's root happens to be), then joined with the branch name (its `/` replaced with `-` — otherwise `git worktree add` would create a nested directory). An absolute path is used as-is. Avoid a path inside the repository itself (e.g. `".worktrees/"`) unless you also add it to `.git/info/exclude` yourself — konoma never writes to that file for you, and every worktree created there would otherwise show up as untracked in the main worktree's own `git status`. |

## `[jj]` — jj (Jujutsu) integration

**Preview.** The surface is complete for the tree, diffs and the hub, but `jj workspace` has no list
yet, and jj is pre-1.0 and ships breaking changes monthly. konoma pins nothing: it probes the `jj` on
your machine once and falls back to git if it cannot answer. **konoma only reads a jj repository** —
every call carries `--ignore-working-copy`, so it never snapshots your working copy, and keys that
would write are hidden rather than offered. The one exception is `s` on a jj changes screen, guarded by
`[ui] confirm_jj_sync` above.

Only what has no git counterpart lives here; which backend answers is `[external] vcs` below.

| Key | Default | Description |
|---|---|---|
| `tool` | `"lazyjj"` | External jj tool launched with `!` inside the hub, the way `[git] tool` is for git. Command + args. A missing binary flashes what could not be launched. |

## `[external]` — external process on/off switches

One on/off switch per external process konoma can launch. Every key defaults to `true`,
so an absent `[external]` section (or an absent field within it) changes nothing.

| Key | Default | Description |
|---|---|---|
| `git` | `true` | git integration: status colors, the gutter, the Git views, stage/unstage/commit/checkout/branch (`src/git.rs`, via the `git` CLI and the embedded git2/libgit2). `false` behaves exactly like building with `--no-default-features` (no `git` feature) — every read returns empty/`None`, every write returns an error. `p` (open the Git view) flashes a message distinct from "not a git repo", since it may well be one. Whatever this setting says, git integration also **turns itself off automatically when no `git` executable is found** on the machine (probed once, on first use); `p` then says git is not installed rather than blaming the directory. |
| `git_tool` | `true` | The external git tool launched with `!` (`[git] tool`, default lazygit). |
| `vcs` | `"auto"` | Which version-control system answers for a directory. `"auto"` keeps git wherever git can answer, so a repository that already worked shows exactly what it showed before; jj answers only where there is no git repository to ask (one created with `jj git init --no-colocate`, or a `jj workspace`). `"git"` is always git, even where a `.jj` sits beside the `.git`. `"jj"` uses jj wherever a `.jj` exists, colocated or not — ask for this when jj is the system you actually work in. Anything unrecognised means `auto`. With no `jj` binary on the machine, konoma falls back to git. See `[jj]` above. |
| `pdf` | `true` | The **external fallback** rasterizer — macOS's bundled `qlmanage`/`sips`, tried only when the primary renderer (`hayro`, pure Rust — parses/renders in-process regardless of this flag) fails on page 1 of a given PDF (encrypted, corrupt, or otherwise unsupported). `false` never launches those tools, but PDF preview itself (page rendering and the page count) keeps working via `hayro`. **On every other platform this flag is effectively a no-op**: there is no external PDF tool to launch. |
| `video` | `true` | The **external fallback** thumbnail extractors (`ffmpegthumbnailer`/`ffmpeg`), tried only when the built-in decoder (pure Rust, in-process regardless of this flag) can't handle the file — i.e. anything that isn't H.264 or HEVC in `.mp4`/`.m4v`/`.mov` and `.mkv`/`.webm`. `false` never launches them, but H.264 and HEVC thumbnails from those containers keep working. Same relationship `pdf` has with `hayro` above. |
| `remote_images` | `true` | Fetching `http(s)://` images referenced from Markdown — the only outbound network call konoma makes. Done in-process via `ureq` (rustls), not an external tool. |
| `office_apps` | `true` | `e` or `o` on an Office document (Word `docx docm dotx dotm doc odt ott`, Excel `xlsx xlsm xltx xltm xlsb xls ods`, PowerPoint `pptx pptm ppsx ppsm potx potm ppt odp otp`) opens it in a GUI app — Microsoft Office (macOS only), then LibreOffice, then the OS default (`open` / `xdg-open`) — instead of handing the zip to `$EDITOR` (`e`) or the OS default (`o`). The launch does not block the terminal. `false` opens nothing and says so (it does not fall back to `$EDITOR`). An explicit `[editor.ext]` entry for the extension wins. |
| `open_links` | `true` | Opening URLs/files with the OS handler (`open` on macOS, `xdg-open` elsewhere) — Markdown links, pasted-path jump (`P`), and `o` (open the file in another app). |
| `preview_commands` | `true` | Running a `[[preview.rules]] command = "..."` delegation. `false` makes a matching rule behave as if it hadn't matched (falls through to `[can not preview]`); builtin renderers (`markdown`, `image`, `pdf`, ...) are unaffected. |

Disabled mechanisms degrade the same way a missing optional tool already does: PDF/video
fall back to the existing "cannot render" hint, a disabled remote image shows the text
placeholder instead of the image, and a disabled `command` rule falls through to
`[can not preview]` — nothing crashes.

To turn off one kind of preview (PDF, video, ...), use `[plugins.<id>] enabled = false`
rather than a `[[preview.rules]]` entry: `[external]` gates an external process, `[plugins.<id>]`
gates a standard plugin.

`[ui] lang` already has its own explicit-vs-`"auto"` switch for OS-language detection
(via `sys-locale`; no external process) — setting it explicitly (`"en"`/`"jp"`) skips that lookup, so there is no
separate `[external]` flag for it.

## `[keys]` — keybindings

Every command is rebindable. The model: **each context (what you are looking at) maps keys to
actions**, helix-style:

```toml
[keys.tree]
"J" = "navigate:half_down"     # capital = Shift included
"ctrl-g" = "open_git_view"     # ctrl-x / c-x
"space d" = "file_delete"      # two tokens = chord (a menu key + a key in that menu)
"o" = "noop"                   # disable a default binding

[keys.global]                  # inherited by every context
"Q" = "quit"
```

The default keys follow one design (layers), so a key means the same thing wherever it works: the app
keys (`?` `Q` `F` `P` `t` `T` `[` `]` `1`-`9`), the basic keys (`j` `k` `g` `G` `/` `n` `N` `r` `y` `Y` `v` `V` `m` `'`
`Ctrl-t` `,` `Space` `Esc` `Enter` `q`), the keys of one kind of view (zoom for images, `J` `K` for pages, `O` for the
outline, `R` for the representation), and the verbs `e` (edit), `o` (open in another app), `p` (the project screen)
and `!` (the project tool). Every one of them can be changed below; the `?` help and the footer always show the
keys as they are set. The complete default table is in the
[keymap reference](https://lesim-co-ltd.github.io/konoma/reference/keymap/).

### Contexts

A key is looked up in the context's own table, then in the keys shared by that kind of view, then in the basic
keys (`j`/`k`/`Space`), then in `global`. The context names:

| Context | What you are looking at |
|---|---|
| `global` | inherited by every context (tabs, help, quit, follow, paste-jump) |
| `tree`, `tree_visual` | the tree, and its range selection (`v`) |
| `text` | source code and plain text (also a preview that cannot be shown) |
| `document` | a decorated document: rendered Markdown, a Word file, slides, a mermaid diagram |
| `text_visual` | the character / line selection of a text or document preview (`v` / `V`) |
| `image` | images, SVG, PDF pages and video thumbnails |
| `table` | CSV / TSV / archive / spreadsheet tables; `table_cell` is the full-cell popup (`Enter`) |
| `sort` | the old sort menu (see below), `bookmarks`, `tabs` (the tab list), `outline`, `info`, `help` |
| `diff` | a file's git / jj diff (git builds) |
| `git.changes`, `git.log`, `git.graph`, `git.graph_picker`, `git.branches`, `git.worktrees`, `git.detail` | the version-control screens of a git repository (git builds). `git.worktrees` is the linked-worktree list, `git.graph_picker` the graph's branch panel, `git.detail` a commit's detail |
| `jj.changes`, `jj.log`, `jj.graph`, `jj.graph_picker`, `jj.branches`, `jj.worktrees`, `jj.detail` | the same screens opened for a jj repository; they start with the same keys as the `git.*` ones, except the few that differ (`s` syncs in `jj.changes`) |

A name with a dot needs quotes in TOML: `[keys."git.changes"]`.

The names used before contexts existed are still read: `preview_text` (= `text` + `document`),
`preview_text_visual` (= `text_visual`), `preview_image` (= `image`), `preview_table` (= `table`),
`preview_git_diff` (= `diff`) and `git_changes`, `git_log`, `git_graph`, `git_graph_picker`,
`git_branches`, `git_worktrees`, `git_detail` (= the `git.…` and `jj.…` context of that name). konoma
tells you at startup what to rename each one to. If a context is written under both names, the new
name wins. A table under an old name applies to every context it now covers, so a key you set in
`[keys.preview_text]` reaches both `text` and `document`.

### Key tokens

Single characters (uppercase = Shift), `space`, literals like `0 $ ! + - = . / ' ,`,
modifiers `ctrl-<k>` (alias `c-<k>`), named keys `tab enter esc backspace delete up down
left right home end pageup pagedown`. `Esc`, `Enter`, `Tab`, `Shift-Tab`, the arrows,
`Home`/`End` and text-input keys are fixed and cannot be rebound — but the operation each of them runs
has a name (`cancel`, `tab_list_activate`, `outline_jump`, `git_graph_picker_apply`, `toggle_focused_item`,
`link_open`, ...), so it can be given another key as well.

### Menus and chords

Three keys open a menu whose second key picks an item: `y` (copy), `,` (sort and display options) and `Space`
(operations). A chord of two tokens edits that menu: `"y x" = "copy_name"`, `", x" = "sort_size"`,
`"space x" = "file_delete"`.

- **In `[keys.<context>]` a chord changes that context's menu only.** The `y` of the tree (paths), of a table
  (cell / row / column) and of the branch list (the branch name) are different menus, and so is the `,` of the tree
  (sorting, path style, hidden files) and of a text preview (path style, wrapping). A chord whose first key
  opens no menu in that context is reported at startup and ignored.
- **`Space` is the file menu in the tree and its range selection** — `"space x"` edits it there. In every other
  context `Space` opens a menu **built from the operations that act on that screen right now**, each under the
  key it already has (or a letter of its own), so it cannot be edited as a menu: bind the operation to a key
  instead. It is how an operation whose key you moved away, or that has no single key, stays one keystroke away.
- **`[keys.global]` chords still reach every context** (`"y x"` is added to every `y` menu).
- The old `[keys.sort]` table is read as the tree's `,` menu (its sort entries move there).

### Action names

Action names are snake_case strings — the full annotated list is in
[`config.example.toml`](config.example.toml). The main groups (the default key is in parentheses; an operation
with none is reachable from a menu or by binding it):

- **Movement**: `navigate:down|up|top|bottom|page_down|page_up|half_down|half_up|left|right|line_home|line_end`
- **Tree**: `quit` (`Q`), `close_tab_or_quit` (`q`), `tree_descend` (`l`), `tree_leave` (`h`), `tree_activate` (`Enter`), `filter_start` (`/`), `toggle_hidden` (`.`), `refresh` (`r`, in every context), `toggle_info` (`i`), `request_edit` (`e`; works in a preview, a diff and on a row of the changes list too), `open_external` (`o`, see below), `set_anchor` / `reset_anchor` (`a` / `A`), `enter_visual` (`v`), `toggle_select` (`V`), `open_in_new_tab` (`Ctrl-t`: open the entry under the cursor in a new foreground tab)
- **Sort and display options** (the `,` menu): `open_sort_menu`, `sort_name`, `sort_size`, `sort_modified`, `sort_ext`, `sort_toggle_reverse`, `sort_toggle_dirs_first`, `cycle_path_style`, `toggle_wrap` — the tree's `s` and `p` are not bound by default (they are in the `,` menu); bind `open_sort_menu` / `cycle_path_style` to keys of your own if you want them back
- **Bookmarks**: `mark_set` (`m`), `mark_jump` (`'` — opens the list; plain letters inside it jump), `bookmark_edit` (`ctrl-e`), `bookmark_delete` (no single key: `d` in the bookmark list's `Space` menu), `bookmark_close`. `m`/`'` are bound in both the tree and previews (a preview bookmarks the shown file).
- **Copy** (`y` menu): `copy_name`, `copy_relative`, `copy_full`, `copy_parent`, `copy_at_ref` (`@relative/path` for AI chats), `copy_code_block` (`y c` — copy the Markdown code block currently focused with `Tab`; the entry only appears in the menu while one is focused)
- **File management** (`Space` menu of the tree): `file_create`, `file_rename`, `file_delete`, `file_copy`, `file_cut`, `file_paste`, `file_duplicate` (`Space→D`: duplicate the cursor/selection in place, e.g. `note copy.md`)
- **Open in another app** (`o`) and **edit** (`e`): `open_external` hands the file to the app made for it — an Office file to Microsoft Office / LibreOffice / the OS default, anything else to the OS default (`open` / `xdg-open`); a directory is not opened. It runs in the background. `[external] office_apps` and `[external] open_links` switch it off. `request_edit` (`e`) opens the editor (`[editor]`), at the line you are looking at; on an Office file it opens the Office app instead, as `o` does.
- **Preview**: `preview_back` (`q`), `search_start` (`/`), `search_next` / `search_prev` (`n` / `N`: the next / previous match — or, while nothing is being searched, the next / previous change of the file), `preview_enter_visual` (`v`), `preview_enter_visual_line` (`V`), `preview_copy_selection`, `preview_copy_selection_ref` (`Y` = `@path#L12-34`), `toggle_outline` (`O`: the heading outline; `Enter` jumps — `outline_jump`. On a preview made of pages — a PDF, a workbook, a presentation — with two or more pages it is the list of the pages instead: "Page n", the sheet names, "Slide n: title", the current one marked; `Enter` goes to the chosen page, and a one-page file does nothing), `toggle_markdown_raw` (`R` — cycles the representation: decorated ⇄ raw source; on a presentation, the slide pictures ⇄ the text view, keeping the current slide; while this preview *is* a diff's `"preview"` presentation, `R` instead returns to that diff), `link_focus_next` / `link_focus_prev` (`Tab` / `Shift-Tab`), `link_open` (`Enter` = current tab; on a focused checkbox or `<details>` it toggles it), `toggle_focused_item` (the same toggle as its own operation: no key, it is `t` in the `Space` menu), `open_link_new_tab` (`Ctrl-t` = new tab for a local link only; an anchor jumps in place, a URL opens externally), `image_zoom_in` / `image_zoom_out` / `image_zoom_reset` (`+` `-` `=`), `page_next` / `page_prev` (`J` / `K`, and `PageDown` / `PageUp` on an image and in an image/PDF/SVG diff (where they turn the page instead of scrolling): a PDF's page, a spreadsheet's sheet, a presentation's slide, or the PDF page of an image/PDF diff on both sides together; the old names `pdf_next_page`, `pdf_prev_page`, `sheet_next`, `sheet_prev`, `media_diff_page_next`, `media_diff_page_prev` still work as aliases), `preview_next_file` / `preview_prev_file` (`Ctrl-n` / `Ctrl-p` — page to the next/previous file in tree order, skipping directories, wrapping at the ends)
- **Selection and overlays**: `visual_select_siblings` (`a`) / `visual_select_all` (`A`) / `visual_commit` (`v`) in the tree's range selection, `preview_exit_visual` (`v` / `V` / `q` while selecting in a preview), `image_return_to_diff` (`R` on the picture of a diff's `preview` representation), `bookmark_jump` (`Enter` in the bookmark list), `info_close` (`i` / `q` in the file info), `toggle_outline` (`O`) and `outline_jump` (`Enter`) in the outline.
- **Table** (`table`): `table_copy_cell` / `table_copy_row` / `table_copy_column` (`y` menu), `toggle_table_cell` (`Enter`: a full-text popup for the cursor cell — the grid truncates wide cells with `…`, this shows the untruncated value, wrapped and scrollable with `j`/`k`/`g`/`G`/PageUp/PageDown; `q`/Esc/`Enter` close it; `[keys.table_cell]` covers the popup's own keys)
- **Agent Watch**: `toggle_follow` (`F`), `toggle_changed_filter` (`C`), `jump_next_change` / `jump_prev_change` (`n` / `N` in the tree: the next / previous changed file; `Ctrl-n` / `Ctrl-p` in a diff: the next / previous changed file), `diff_next_change` / `diff_prev_change` (`n` / `N` in a diff: the next / previous change block of the file), `toggle_follow_diff_scope` (in a follow diff: switch between "since follow start" and the full git diff — in the diff's `,` menu, key `f`)
- **Paste-jump** (`global`): `paste_jump` (`P`) — reads a path or GitHub link from the clipboard and jumps there (reveal + preview). Understands local absolute/relative paths, GitHub `blob`/`raw` URLs, and `#L123` / `:123` line anchors; switches root to the target's repository when it lies outside the current root.
- **Git**: `open_git_view` (`p` — the project screen, from the tree, a preview or a diff), `open_git_diff_cursor` (`d`, in the tree), `git_launch_tool` (`!`, from the tree, a preview, a diff and the changes list), `git_screen_next` / `git_screen_prev` (`Tab` / `Shift-Tab`: walk the changes, log, graph, branches and worktrees screens), `list_filter_start` (`/` on the changes list, the log and the graph), `git_stage`, `git_unstage`, `git_stage_all`, `git_unstage_all`, `git_discard`, `git_commit` (`c`), `git_open_selected_diff` (`l` / `Enter`), `git_open_branches` (`b`), `git_open_log` and `git_open_graph` (no default key), `cycle_diff_layout` (`s`: a text diff cycles unified/split/auto — `[git] diff`; a media diff cycles auto/side/stack instead — `[git] media_diff`), `cycle_diff_view` (`R`, in a full-screen diff: cycles the presentations available for that file — source/rendered/preview for Markdown and SVG, rendered/preview only for an image/PDF — `[ui] diff_view`), `git_open_detail` (`l` / `Enter` on the log and graph), `git_close` (`q` / `h`), `git_worktree_diff` (`d` on the changes list: the diff of all changes), `git_diff_discard` (`x` in a diff), `git_copy_short_hash` / `git_copy_full_hash` / `git_copy_subject` / `git_copy_message` / `git_copy_author` / `git_copy_date` (the commit's `y` menu), `copy_branch_name` (the branch list's `y` menu), `branch_checkout` (`l` / `Enter`), `branch_create` (`c`), `branch_delete` (`d`), `branch_filter_start` (`/`)
- **jj** (`jj.changes`): `jj_sync` (`s` — let jj snapshot the working copy; konoma's only write to a jj repository, and only after a confirmation. Does nothing in a git repository.)
- **Git graph** (`git.graph`): `git_graph_toggle_all` (`a` — show every revision instead of the range the backend considers current; only jj has a narrower one), `git_graph_set_base` (`s`), `git_graph_clear_base` (`x`), `git_graph_open_picker` (`b`, opens the branch panel). Inside the branch panel (`[keys."git.graph_picker"]`): `git_graph_picker_toggle` (`x`), `git_graph_picker_all` (`a`), `git_graph_picker_current_only` (`c`), `git_graph_picker_move_up` / `git_graph_picker_move_down` (`K` / `J`), `git_graph_picker_apply` (`l` / `Enter`)
- **Git worktrees** (`git.worktrees` — a *linked* working tree from `git worktree add`, a different concept from the "worktree" meant everywhere else on this page, which is always the uncommitted working tree): `git_open_worktrees` (`w`, on the version-control screens), `worktree_filter_start` (`/`, filter by branch name / path), `worktree_goto` (`Enter` or `l`: switch this tab's root — and `open_dir` — to the selected worktree), `worktree_goto_new_tab` (`Ctrl-t`: open the selection in a new tab, leaving this tab as-is), `worktree_create` (`c`: open an input dialog for a branch name — new vs. existing is auto-detected, no separate prompt for it — then `git worktree add` it under `[git] worktree_dir` (see above) next to the main worktree, and switch this tab into it), `worktree_show_changes` (`d`: show the selected worktree's diff since the base branch — committed and uncommitted content together, since an agent working in a worktree often commits along the way and an uncommitted-only diff would otherwise go blank; falls back to an uncommitted-only diff when there's no resolvable base or nothing has piled up — see `graph_base_branches` above), `worktree_close` (`q` / `h`). A bare main worktree, a locked/prunable one, or the currently-active one all refuse to switch (a flash explains why); `worktree_show_changes` still works on the currently-active one.
- **Tabs / app** (`global`): `tab_new_after` (`t` — a new tab immediately to the right of the current one), `tab_new` (no default key — appends a new tab at the end), `toggle_tab_list` (`T` — tab list; `tab_list_close` = `d` inside it, `tab_list_activate` = `Enter`), `tab_prev`/`tab_next` (`[`/`]`), `quit` (`Q`), `toggle_help` (`?`), `cancel` (`Esc`). `tab_close` has no default key (closing is `q` on the tree; rebind with `"w" = "tab_close"` if you want it back)
- `noop` (alias `disabled`) removes a default binding.

To put an old key back, bind the operation to it, e.g. `[keys.global] "I" = "tab_new_after"` and `"t" = "tab_new"` for the old tab keys, or
`[keys.tree] "o" = "open_git_view"` and `"s" = "open_sort_menu"` for the tree's old `o` and `s`. Each operation keeps its name.

Conflicting bindings that would break essential defaults — a context binding a key that opens a menu
(`y`, `,`, `Space`) to an operation, or a context taking one of the `global` keys for something else (write the change
in `[keys.global]` instead) — are detected at startup, reported in the footer, and reverted: a bad config
never bricks the UI.

Backward-compatible aliases for path copy also exist at the `[keys]` top level
(`copy_name`, `copy_relative`, `copy_full`, `copy_parent`); `copy_prefix` is ignored (the prefix is fixed to `y`).

## Data files

| Path | Contents |
|---|---|
| `~/.config/konoma/config.toml` | This configuration. |
| `~/.config/konoma/bookmarks.toml` | Global (uppercase) bookmarks — absolute paths. |
| `~/.config/konoma/bookmarks/<dir>.toml` | Local (lowercase) bookmarks, one file per start directory. |
| `~/.config/konoma/sessions/<dir>.toml` | Tab session (`restore_tabs`), one file per start directory. |
| `~/.cache/konoma/remote-images/` | Cache for remote images embedded in Markdown. |

## Fonts & terminal requirements

- **Images / SVG / Mermaid / LaTeX math / video thumbnails / PDF pages** are drawn as
  **real pixels** in any terminal that speaks a graphics protocol — **kitty graphics**
  (Ghostty, kitty, WezTerm, Konsole), **iTerm2**, or **sixel**. konoma has its own
  compressed transfer for the kitty protocol, so those terminals are the fastest.
  Anywhere else the picture degrades to a **half-block approximation** — coarse, but
  visible. Text previews (Markdown, code, git diffs, CSV, tables) work in any terminal.
- **Icons** (`ui.icons = true`, the default) need Nerd Font glyphs: either add
  `Symbols Nerd Font Mono` as a fallback font in your terminal, or use an NF-bundled
  font (HackGen Console NF, UDEV Gothic NF, …). Without one, set `ui.icons = false`
  for plain-symbol fallbacks.
- **CJK text** (the `jp` UI, or CJK filenames / file contents) needs the terminal
  font to include CJK glyphs, or CJK shows as tofu (□) — konoma computes the display
  widths correctly regardless; the glyphs come from the font. A Nerd-Font-patched CJK
  font like HackGen Console NF or UDEV Gothic NF covers both icons and CJK in one font.
- **Optional tools**: `ffmpegthumbnailer`/`ffmpeg` (thumbnails for video konoma can't decode
  itself — VP9, AV1, older codecs, `.avi`), `git` + `lazygit` (git suite / external tool).
  Everything degrades gracefully when absent. **PDF, and H.264/HEVC video in `.mp4`/`.m4v`/`.mov`
  and `.mkv`/`.webm`, need nothing installed** — both are decoded in pure Rust.
