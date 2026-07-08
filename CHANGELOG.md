# Changelog

All notable changes to konoma are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0] - 2026-07-08

### Added
- Markdown preview: **code blocks are now `Tab`-focusable and copy with `y`**. `Tab`/`⇧Tab`
  cycles through links, checkboxes and fenced code blocks in document order; focusing a code
  block reverses its header line, and `y` (the same copy key used everywhere else) copies the
  block's **raw source** (unhighlighted, fence contents only) to the clipboard — no which-key
  menu, just one press. The footer shows `y copy code` while a block is focused. No mouse capture
  is used, so the terminal's own text selection keeps working.

## [0.9.1] - 2026-07-08

### Fixed
- With soft-wrap on, `Tab` focus in a Markdown preview did not scroll the view when
  the next link/checkbox was off-screen. The renderer clamps scrolling in visual
  (post-wrap) rows, but the focus-follow compared the item's logical line against
  that visual offset — once long wrapped paragraphs pushed the two apart, the check
  always thought the item was still visible. Focus-follow now converts the item's
  position with the exact same reflow the renderer uses.

## [0.9.0] - 2026-07-07

### Fixed
- The panic guard added in 0.8.0 was too blunt: one problematic construct (e.g. a
  loose list containing task items, which panics the underlying tui-markdown) made
  konoma render the **whole document section** as plain undecorated text. The guard
  now retries by bisecting the section at blank lines (never inside code fences), so
  in practice the entire document renders decorated and at worst only the single
  offending paragraph degrades.
- Running `git pull` (or any locking git command) in a repository konoma was watching
  could fail with `Unable to create .git/index.lock: File exists`. konoma refreshes
  its status on every file-system event, and plain `git status` takes the optional
  index lock to write back the refreshed stat cache — during a pull's burst of file
  events the two raced. All of konoma's background reads now pass
  `--no-optional-locks` (the git facility built for background tooling, git 2.15+),
  so they never take the index lock; a regression test pins that reads leave
  `.git/index` untouched.

### Changed
- `w` no longer closes the tab (it is unbound by default). Closing a tab had two
  keys, and for vim users `w` is word-motion muscle memory — an accidental press
  closed a tab. Closing is unified on `q` in the tree (the last tab quits, behind
  the usual confirmation); inside the tab list the close key is now `d`. Rebind
  with `[keys.global] w = "tab_close"` if you want the old behavior back.

### Added
- Tab list (`T`, from any screen): every tab on one popup — number, name and root
  path, the active tab marked. `1`-`9`/`Enter` switch, `w` closes the **selected**
  tab (the list stays open; the last tab refuses), `T`/`q`/`Esc` close the list.
- The tab bar now handles overflow: when tabs don't fit, it shows a window centered
  on the active tab with `‹n` / `n›` markers for the tabs hidden on each side — the
  active tab can no longer scroll out of sight.

### Fixed
- The tab bar no longer runs under the top-right context/status area on the shared
  top row (its layout budget now excludes that width).
- Keymap validation: a built-in per-screen specialization of a global key (like `w`
  in the tab list) is no longer flagged/stripped; a user override of such a key now
  falls back to the built-in specialization instead of the global action.

## [0.8.0] - 2026-07-07

### Fixed
- With soft-wrap on (`ui.wrap = true`), a Markdown code-block line longer than the
  screen broke the block's left `▎` gutter and background band: wrapping was left to
  the terminal paragraph, so continuation rows started bare. Code-block lines are now
  pre-wrapped by the renderer itself (CJK-aware, syntax colors preserved across the
  split), so every visual row carries the gutter and the full-width band. `ui.wrap =
  false` keeps long lines intact for horizontal scrolling, as before.

### Added
- A background-activity indicator (`ui.busy_indicator`, default on): while something
  runs off the UI thread — the git-ignored scan, media decoding, syntax-highlight
  warm-up, inline-image fetches — the top-right shows a small spinner with the job
  name (`⠋ git scan`, plus `+n` when several run at once) and disappears when done.
  The indicator is derived from the jobs' own state (nothing to leak or get stuck)
  and only schedules animation frames while active, so an idle konoma still costs
  zero redraws and 0.0% CPU.

### Added
- Documentation site at <https://lesim-co-ltd.github.io/konoma/> (English and Japanese):
  getting started, scenario guides (AI-agent workflow, previews, git, files), and full
  configuration/keymap references. Built with Astro Starlight from `site/`, deployed by
  the `Docs` workflow.
- A hands-on tutorial designed to be read inside konoma — `samples/tutorial.md` /
  `samples/tutorial.ja.md` — with links you can follow and checkboxes you can toggle.
- `CONFIGURATION.md` — a full configuration reference (every `[ui]` option, colors,
  preview rules, editor/git integration, and the complete keybinding model), linked
  from the README.

### Fixed
- A Markdown preview could crash the app on inputs that panic the underlying
  tui-markdown renderer (e.g. a loose list followed by a task-list item, still present
  in tui-markdown 0.3.8). konoma now catches the panic and degrades that text segment
  to plain lines instead — no input can crash the preview (design principle #3).

## [0.7.0] - 2026-07-07

### Added
- Bookmarks can be set from a preview: `m` while previewing a file (text/Markdown, image,
  CSV table) bookmarks **the previewed file** (not the tree cursor, which can lag behind
  after bookmark jumps or follow mode), and `'` opens the bookmark list on top of the
  preview. Same letters, same list, same jumps as in the tree.

### Fixed
- Global (uppercase) bookmarks now display their absolute location (`~`-shortened, e.g.
  `~/.vimrc`) in the list and in the registration notice. They were shown relative to the
  current directory (`../../.vimrc`), which is unreadable for targets outside the tree.
  Storage was always absolute — this is a display fix; local bookmarks keep the contextual
  relative form.

## [0.6.0] - 2026-07-06

### Fixed
- Task checkboxes no longer render as Unicode `☐`/`☑` (added in 0.5.1). Those code points are
  East-Asian-Neutral (1 terminal cell) but CJK fallback fonts draw them double width, so the
  glyph clipped into the next cell and the new focus highlight covered only its left half.
  Checkboxes now follow the tree-icon policy: a Nerd Font checkbox icon when `ui.icons` is
  on, plain `[ ]` / `[x]` otherwise. The marker span also includes the space that follows it,
  so the focus highlight covers glyph + space — fonts that draw Nerd Font glyphs double width
  (HackGen NF and friends serve icons from the primary font at full width) get the whole
  glyph inside the highlighted area instead of a half-covered one.

### Added
- Markdown task-list checkboxes are now interactive: `Tab`/`Shift-Tab` walks links **and**
  checkboxes in one document-order cycle, and `Space` (or `Enter`) toggles the focused
  checkbox by writing the single state character back to the source file. The write is
  verified first — the file is re-scanned and the toggle is cancelled (with a notice and a
  reload) if the file changed on disk in the meantime, so it cannot clobber a concurrent
  external edit (e.g. an AI agent editing the same file). Code fences, HTML blocks and
  tables are excluded exactly like the renderer excludes them. Toggling never happens in
  raw-source (`R`) view.
- `ui.md_task_states` — configurable task states cycled by `Space`, in order (default
  `[" ", "x"]`). E.g. `[" ", "/", "x"]` adds an Obsidian-style "in progress" state:
  custom states render in bracket form (`[/]`) and are recognized as toggleable markers.
  Invalid configs (multi-char entries, fewer than two states) fall back to the default.
- The bookmark list now opens on the first `'` press (which-key style; the old invisible
  "waiting for a letter" state is gone, `''` is no longer needed). Inside the list a plain
  letter jumps straight to that bookmark (a-z local / A-Z global; dir → new root, file →
  preview; unknown letters flash and keep the list open). Edit/delete moved to `Ctrl-e` /
  `Ctrl-d` so every letter stays available as a bookmark name; `'`, `q` or `Esc` closes.
  (Letters taken by list/global keys — `j`/`k`/`q`, tab keys, `F`, `Q` — are reachable via
  `j`/`k` + `Enter`.) `m` (set bookmark) is unchanged.

## [0.5.1] - 2026-07-06

### Fixed
- Links inside Markdown tables now render as links. konoma draws GFM tables with its own
  box-drawing renderer (tui-markdown collapses them), and that renderer treated cell text
  as plain strings — so `[label](url)` showed as raw Markdown. Cells are now parsed for
  inline links: the label renders in link style (blue underline), Tab focuses it and
  Enter opens the target just like paragraph links, and column alignment stays exact
  (widths are measured on the displayed label, CJK included; labels stay atomic when a
  cell wraps). A CommonMark title (`[t](./x.md "Title")`) and `<...>`-wrapped
  destinations are reduced to the plain target so Enter opens the right file.
  Images (`![alt](url)`) and unmatched brackets are left as text.
- Markdown rendering audit fixes (a GFM sweep of the whole preview):
  - An escaped pipe (`\|`) inside a table cell no longer splits the cell (GFM treats it
    as a literal `|`; previously it grew a ghost column).
  - `**bold**`, `*italic*`, `` `code` `` and `~~strike~~` inside table cells now render
    styled instead of showing their raw markers (flat, GFM-flanking-aware — `2 * 3 * 4`
    stays literal).
  - Table alignment colons (`:---`, `:---:`, `---:`) are respected: cells pad
    left/center/right per column instead of always left.
  - HTML blocks such as `<details>` no longer disappear silently: their tag-stripped
    text is shown (entities decoded; `<!-- comments -->` stay hidden). Autolinks like
    `<https://…>` are unaffected.
  - A thematic break (`---`) renders as a full-width rule instead of literal dashes.
  - Task-list checkboxes render as `☐` / `☑` instead of raw `[ ]` / `[x]`.
  - Table-cell links get the same link icon as paragraph links when `ui.icons` is on
    (the icon is included in the column-width math, so alignment stays exact).

### Changed
- `samples/m3-demo.md` is renamed to `samples/markdown.md` (the old name referred to an
  internal milestone). The links demo (`samples/links.md`) gains a table-links section,
  and the Markdown demo gains sections for table alignment/inline styles/escaped pipes,
  horizontal rules, task lists, and HTML blocks — so every fix above can be seen in the
  samples.

## [0.5.0] - 2026-07-06

### Added
- **Agent Watch** — a set of features for konoma's core use case, sitting next to an AI
  coding agent (Claude Code) and reviewing what it does:
  - **Follow mode (`F`)**: while on, konoma automatically shows any file that changes
    on disk — watch the agent work in real time. By default the changed file opens as
    its **full-screen git diff** (`ui.follow_view = "diff"`): hunk-level before/after,
    the way dedicated agent-watching tools (hunk, livediff, diffpane) present edits;
    untracked files show as an all-added diff, and the diff refreshes in place while
    the same file keeps changing. Set `ui.follow_view = "file"` to open the normal
    content preview instead, **scrolled to the first changed hunk** (caret on the
    changed line, git gutter lighting up the edits) — files with no diff and media
    fall back to this view automatically. View switches between files are rate-limited
    (about one per second, latest change wins), so a burst of multi-file edits doesn't
    thrash the screen. Pressing any other key stops following (you took the keyboard
    back, Zed-style); one `F` re-enables it. Shown as a green `FOLLOW` chip.
    Repository internals (`.git`), gitignored and hidden files are not followed.
  - **Changed-files view (`C`)**: toggles the tree into a flat list of the files with
    uncommitted git changes (relative paths, status markers, live-updated) — review an
    agent's work top to bottom without hunting through the tree. `n` / `N` jump to the
    next/previous changed file from the normal tree too, expanding collapsed
    directories as needed and wrapping around.
  - **`n` / `N` inside the diff view**: switch straight to the next/previous changed
    file's diff without leaving the view (wraps; the title shows your position as
    `(2/5)`), so reviewing a multi-file change is one keystroke per file — the
    hunk/lazygit-style review loop. The tree cursor follows, and `q` still returns to
    wherever the diff was opened from (tree or git hub). **The navigation scope depends
    on how the diff was opened**: a follow-opened diff cycles only the files changed
    during the current follow session ("what the agent just did" — pre-existing
    uncommitted changes don't get in the way; the session resets each time `F` is
    turned on), while a diff opened from the tree (`d`) or the git hub cycles the full
    uncommitted change set.
  - **`@path` references for the conversation**: `y` → `@` copies an `@relative/path`
    reference (Claude Code's file-context syntax) for the selected entry, and `Y` in a
    text preview copies `@path#L12` / `@path#L12-34` for the caret line or the
    `v`/`V` selection — paste it into the agent chat to point at an exact spot.
  - All keys are rebindable (`toggle_follow`, `toggle_changed_filter`,
    `jump_next_change`, `jump_prev_change`, `copy_at_ref`,
    `preview_copy_selection_ref`).

## [0.4.2] - 2026-07-03

### Added
- Markdown/Mermaid raw-source toggle (`R`). Markdown and Mermaid previews are reflowed
  when rendered, so their on-screen lines don't map to the file and range selection was
  disabled for them. Pressing `R` now switches a Markdown/Mermaid preview to its raw
  source — shown windowed and syntax-highlighted like a code file, with the title marked
  `· raw source` — where the 2D caret selection/copy works against the real file text
  (`v`/`V` → `y`). Press `R` again to return to the decorated render. The mode is kept
  per tab.

## [0.4.1] - 2026-07-03

### Fixed
- Much broader syntax highlighting for the files you inspect from the CLI. Previously
  only a fixed list of extensions was colored; anything else (including many languages
  and every extensionless config file) was shown as plain text, because the syntax was
  resolved solely from the file extension — which is empty for a leading-dot name like
  `.bashrc`. The syntax is now resolved by extension, then by **file name**, then by
  first line, so dotfiles and named files are colored too: `.bashrc`, `.zshrc`,
  `.gitconfig`, `Makefile`, `Dockerfile`, `.env`, `.gitignore`, `Cargo.lock`, `go.mod`,
  logs, diffs/patches, and every language two-face knows (Ruby, Java, Kotlin, Swift,
  PHP, Lua, SQL, HTML/CSS, …). A small alias map also covers close relatives that lack a
  dedicated grammar — `.dockerignore`/`.npmignore` (→ Git Ignore) and `.jsonc`/`.json5`
  (→ JSON). Genuinely plain text (a `.txt`/`README` with no matching syntax) still
  renders without coloring.

## [0.4.0] - 2026-07-03

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

[Unreleased]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.4.2...v0.5.0
[0.4.2]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/LESIM-Co-Ltd/konoma/releases/tag/v0.1.0
