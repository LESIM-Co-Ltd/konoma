# Changelog

All notable changes to konoma are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Mermaid diagrams render as real images (`[ui] mermaid`, default `"image"`).** Diagrams are
  laid out and rasterized fully in-process (pure Rust — no browser, Node, or external tools),
  at mermaid.js quality including CJK labels. Standalone `.mmd`/`.mermaid` files open full
  screen with zoom/pan; ```mermaid fences inside Markdown render inline, join the `Tab` cycle,
  and `Enter` opens the focused diagram full screen (`q` returns to the exact spot in the
  document). Zooming re-rasterizes the diagram at the needed density on a worker thread, so it
  stays sharp instead of blowing up pixels — SVG file previews gain the same sharp zoom.
  Unsupported diagrams, render failures, and terminals without an image protocol degrade to the
  legacy Unicode text rendering automatically; `mermaid = "text"` keeps it everywhere.
  A focused inline diagram also zooms **in place**: `+`/`-` magnify within the reserved area
  (the document layout never shifts), `hjkl` pan while zoomed, `0` fits — with the same
  sharp re-rasterization, so zoomed diagrams stay crisp. The focused diagram is outlined with
  a cyan frame, the view auto-scrolls to show the whole diagram, and `[ui] mermaid_rows`
  (default 24) sets the target size of inline diagrams — including **scaling up** beyond the
  base raster (the density follows automatically, so bigger stays sharp). The initial view
  **fits the viewport**: in a window shorter than the target the diagram shrinks so the whole
  block is visible without scrolling, and inline diagrams always fill their reserved area
  centered (no off-center letterboxing). Diagrams use the
  mermaid.js **dark theme by default with a transparent background**, so they blend into the
  terminal instead of floating on a white card; `[ui] mermaid_theme` picks
  `dark`/`light`/`classic`/`forest`/`neutral`.

## [0.14.2] - 2026-07-16

### Fixed
- **Tab-session restore (`restore_tabs`) fidelity and robustness.** Restoring a saved session
  no longer:
  - focuses the wrong tab when an earlier tab's directory was deleted (the active index is now
    remapped across the dropped tabs, not just clamped);
  - reopens a tab that was showing a full-screen git diff as a plain content preview (the diff
    is persisted and reopened, falling back to a plain preview only when there is no diff);
  - loses the tree cursor on a hidden (dotfile) entry (hidden-file visibility is now persisted
    and applied before the tree is rebuilt);
  - computes `@`-references from the wrong base in the second and later restored tabs
    (each tab's start dir is persisted and restored);
  - leaves (and re-saves) a broken empty tree when a saved root exists but is unreadable
    (it rolls back to the launch directory);
  - risks losing not-yet-restored tabs if killed mid-restore (per-tab writes during restore are
    suppressed; one complete write happens at the end);
  - risks losing the entire saved session to a truncated file on a crash mid-write (the session
    file is now written atomically via a temp file + rename).

## [0.14.1] - 2026-07-15

### Changed
- **crates.io metadata repositioned for discovery.** Keywords are now
  `tui, file-manager, preview, ai, agent` (dropping the dead `kitty-graphics`
  tag and the library-oriented `ratatui`), the description leads with the
  AI pair-programming positioning and the headline features, and the crate
  is additionally listed under the `filesystem` category. The README tagline
  matches. No code changes.

## [0.14.0] - 2026-07-15

### Added
- **Page through files without leaving the preview (`Ctrl-n` / `Ctrl-p`).** While previewing,
  jump straight to the next / previous **file** in tree display order — directories are skipped,
  files inside expanded subfolders are included, the ends wrap around, and the tree cursor follows
  (so `q` drops you on the file you were looking at). Works across all preview kinds (text/code,
  Markdown, images, PDF, CSV/TSV tables — a PDF keeps `J`/`K` for its pages). Configurable as
  `preview_next_file` / `preview_prev_file`.
- **Reopen the previous tab set per project (`[ui] restore_tabs`, default on).** Launching konoma
  in a directory restores the tabs that were open when it last exited there — each tab's root,
  tree cursor, and full-screen preview (a tab left previewing a file reopens as that preview).
  Sessions are saved on every tab open/close/switch and on quit, one file per start directory
  under `~/.config/konoma/sessions/`. Deleted roots/files degrade safely to the nearest valid
  state; set `restore_tabs = false` to always start fresh (nothing is read or written).

## [0.13.0] - 2026-07-14

### Changed
- **Large performance batch — identical rendering, much less work per keypress**
  (all numbers measured on release builds):
  - **Rendered Markdown/Mermaid/code previews now draw only the visible slice.** The decorated-line
    cache precomputes link collapsing, Tab items, and the wrap layout (per-line reflow prefix sums)
    once per file/width; each frame then clones only the on-screen lines and restyles only the
    focused line, instead of deep-cloning and re-flowing the whole document on every keypress.
    Scrolling a 5,000-line Markdown document: ~4.6 ms → ~0.1 ms per frame (~45×). Focus following
    and editor line mapping now read the cached layout (O(1) instead of re-flowing).
  - **Tree rebuilds are ~5× faster on large directories** (10k files: ~34 ms → ~6 ms per rebuild,
    which runs on every file-system event). Sort keys are lowercased once per entry instead of on
    every comparison, expanded-directory lookup is a hash set, and per-entry `stat` is skipped
    unless the sort key needs it (symlinks still resolve like before).
  - **Tab switching no longer deep-clones the restored tab.** The snapshot is moved out of the slot
    (the active slot is never read while a tab is active), halving the copy cost of a switch.
  - **Animated GIFs are capped at ~128 MiB of resident frames.** A pathological GIF (e.g. 1080p ×
    hundreds of frames ≈ 500 MB+) is now downscaled in halving steps instead of ballooning memory;
    every frame is kept, so the animation stays complete. Typical GIFs are untouched.
  - **Windowed text previews cache more.** Plain-text windows are cached like highlighted ones
    (no per-frame file re-read), and the end-of-file scroll clamp is memoized (no per-frame EOF
    seek+scan).
  - **Tree detail columns (`ui.details`) cache their cells** per tree generation, so the per-row
    `stat` (and the `items` column's directory listing) no longer runs on every keypress.
  - **Fenced code blocks in Markdown cache their finished highlight** (bounded LRU), so follow-mode
    rebuilds and width changes re-highlight only fences that actually changed.
  - **Release builds use a single codegen unit** (with the existing fat LTO) for a small
    across-the-board speedup.

## [0.12.0] - 2026-07-13

### Added
- **Duplicate a file or folder in place (`Space→D`).** Duplicates the cursor entry (or the whole
  selection) next to itself with a collision-free name — `note.md` → `note copy.md`, then
  `note copy 2.md` — reusing the existing copy machinery, so folders are duplicated recursively and
  symlinks are copied as links. The new item is revealed and selected. Bound in the `Space` file
  leader as `file_duplicate`.

### Fixed
- **A stale git change marker after an external/agent commit now clears.** While you previewed a file,
  an AI agent committing in the background could leave the tree's change marker (`M`, …) stuck until
  you navigated across a directory (`h`/`l`). Two causes: (1) the watcher swallowed `.git/*.lock`-only
  events to break an old self-feedback loop where konoma's own `git status` created `.git/index.lock`
  — but since `--no-optional-locks` (0.9.0) konoma's git reads take no locks, so a `.git` lock event
  now only signals an *external* git op, and swallowing it hid the agent's commit (FSEvents can
  coalesce a commit down to lock-only churn); (2) returning to the tree did not re-check git status
  (it only refetched on a directory change). konoma now reacts to `.git` lock churn (safe — it stays
  lock-free, so no feedback loop) and re-verifies git status whenever the tree becomes visible again,
  so the marker is fresh both live and on return. Idle CPU is unchanged.
- **External/agent edits to a file shown outside the tree root are now detected.** The file watcher
  only watched `app.root` recursively, and the preview reload fires on any event under it — so a file
  displayed *outside* the root received no change events and its preview/diff went stale on an
  external (AI) edit, with no recovery. This hit a global-bookmark preview (a bookmarked file usually
  lives outside the current tree) and the repo-wide git view when the root is a repo subdirectory
  (diffing a changed file above the root). Your own `e` edit still showed up (it reloads on editor
  return), so the symptom was "my edits appear but the AI's don't". konoma now also watches the shown
  file's directory (non-recursive) whenever it lives outside the root, updating the watch as the shown
  file changes and dropping it on return to the tree. Idle CPU is unchanged (the watch is added only
  when it changes).
- **A preview reload is no longer skipped when the tree rebuild fails.** In `refresh_fs`, the preview
  (and git-view) reload used to sit behind the tree rebuild's `?`, so a transient directory-read
  failure — e.g. an expanded subdirectory briefly unreadable while an agent rewrites files — would
  drop the preview refresh for that event. The reloads now run regardless (they only need
  `preview_path` / git state, not a fresh tree); the tree error is still surfaced.

## [0.11.3] - 2026-07-10

### Added
- **Open the tree entry under the cursor in a new tab (`Ctrl-t`)**: in the tree, `Ctrl-t` opens the
  selected entry in a new foreground tab, leaving the current tab untouched — a file opens as a
  preview, a directory becomes the new tab's root. `Enter`/`l` still open in the current tab. This
  mirrors `Ctrl-t` in the Markdown preview ("open this in a new tab") and pairs with global `t` (new
  empty tab). Bound in `[keys.tree]` as `open_in_new_tab`.

## [0.11.2] - 2026-07-10

### Changed
- **`e` on a rendered-Markdown preview now opens at the Tab-focused item** (link, checkbox, or code
  block) when that focus is on screen, instead of the top of the view. So if you `Tab` to a checkbox
  and press `e`, the editor lands on that checkbox's line. If nothing is focused, or the focus has
  been scrolled out of view, it still opens at the top-of-view line as before.

## [0.11.1] - 2026-07-09

### Fixed
- **`e` on a scrolled rendered-Markdown preview now opens the editor at the spot you were reading**,
  at the top of the editor window — not buried at the top of the file. Two problems were fixed:
  - *Landing line.* Rendered Markdown reflows the source, so it is not a windowed preview and used to
    pass no line to the editor. It now starts from a proportional estimate of the source line at the
    top of the view, then refines it by content: it searches the source for the text on screen (a
    single decorated span carries no Markdown markers, so it is a verbatim substring of the source)
    and lands on the matching line closest to the estimate. On a heavily-wrapping document this is
    exact where the plain proportional estimate would undershoot by several lines. Rendering is
    untouched; if nothing matches it falls back to the estimate.
  - *Editor scroll.* The vim family now also gets `+normal! zt`, scrolling the target line to the top
    of the window so it matches konoma's top-of-view. vim otherwise leaves the window at the file top
    when the line fits on the first screen, burying the cursor mid-screen.

  Windowed previews (plain text, code, raw Markdown via `R`) still open at the exact caret line;
  Mermaid and images still open at the top.

## [0.11.0] - 2026-07-08

### Added
- **Open a Markdown link in a new tab (`Ctrl-t`)**: with a link focused (`Tab`), `Ctrl-t` opens its
  target in a new foreground tab, leaving the document you were reading intact in its own tab
  (`[`/`]` to switch back). `Enter` still opens in the current tab, and URLs still open in the
  default browser either way. `Ctrl-t` follows the TUI convention (fzf/Telescope) and konoma's
  `t`=new tab, and — unlike `Ctrl+Enter` — works reliably in every terminal and under tmux. Bound in
  `[keys.preview_text]` as `open_link_new_tab`.
- **Paste-to-jump (`P`)**: read a path or a GitHub link from the clipboard and jump straight to it —
  the tree deep-reveals it and a preview opens, so you no longer have to hand-navigate from a link
  someone shared. It understands local absolute/relative paths, GitHub `blob`/`raw` URLs, and
  `#L123` / `:123` line anchors (Code/Text and raw-Markdown previews scroll to the line). GitHub URLs
  resolve by finding the longest trailing path that exists in your checkout, so a differing
  repository name or a slashy branch name still opens the right file. When the target lies outside
  the current root, konoma switches root to the target's repository working tree. konoma's own
  `@path#L` reference copy pastes straight back in. Anything unparseable or missing degrades to a
  flash. Bound in `[keys.global]` as `paste_jump`.
- **Bookmark overwrite confirmation.** Registering a bookmark (`m`) onto a key that already points to
  a **different** path now opens a confirmation dialog (`y`/`Enter` = overwrite, `n`/`Esc` = cancel)
  showing the existing target and the new one, so a mistyped letter no longer silently clobbers a
  saved location. Re-registering the same path or an unused key never prompts. Controlled by the new
  `[ui] confirm_bookmark_overwrite` option (default `true`); set it to `false` to overwrite silently.
- **Editing from a preview opens at the on-screen line.** Pressing `e` in a windowed preview (plain
  text, code, or raw Markdown via `R`) now launches the external editor **at the caret line** instead
  of the top, so you land where you were reading. `[editor]` templates gain a `{line}` token (next to
  `{path}`); without it, common editors are handled automatically (vim family `+N`, VS Code
  `-g path:N`, Sublime/Helix `path:N`). Rendered Markdown/Mermaid reflow the source, so they still open
  at the top — press `R` first for an exact-line open.

### Changed
- **Markdown code-block copy moved into the `y` menu.** Focusing a code block (`Tab`) and pressing `y`
  no longer copies it immediately (which shadowed the normal path-copy `y`); instead the copy which-key
  menu now shows a `c:code block` entry alongside the path options, so `y c` copies the block and the
  path-copy commands stay reachable. The entry appears only while a code block is focused.

### Fixed
- **Switching to a tab now reloads it from disk.** The file watcher only watches the active tab's
  root, so a tab left in the background could show a stale tree (files created/deleted/renamed while
  away were missing), stale git status, and stale change gutters/diffs. Activating a tab now runs the
  same refresh used for filesystem events — rebuilding the tree, refreshing git status and the
  diff/gutter caches, and reloading the preview — so the tab reflects the current filesystem. The
  heavy ignore-set is not recomputed on switch (it is refreshed lazily per repository), and preview
  scroll/zoom/table-cursor positions are preserved.

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

[Unreleased]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.14.2...HEAD
[0.14.2]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.14.1...v0.14.2
[0.14.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.14.0...v0.14.1
[0.14.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.11.3...v0.12.0
[0.11.3]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.11.2...v0.11.3
[0.11.2]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.11.1...v0.11.2
[0.11.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.10.0...v0.11.0
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
