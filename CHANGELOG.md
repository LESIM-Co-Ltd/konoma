# Changelog

All notable changes to konoma are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **External command delegation (`[[preview.rules]] command = "..."`) is now implemented.** The
  config docs have documented this since the beginning (`{path}`/`{out}` templates, `render_as =
  "image"`, `detached = true`) but the renderer itself (`src/preview/command.rs`) was a stub —
  matching a rule with `command` set showed a raw `{:?}` dump of the resolved kind
  (`[command] mpv {path} :: /note.mp4 (render_as=None, detached=true)`) instead of running
  anything. Three modes, matching the documented contract: `detached = true` spawns the process
  without waiting (a video player, etc. — never blocks the TUI); `render_as = "image"` runs on the
  existing media worker thread and shows the produced artifact full-screen like any other image;
  anything else (unset, `"text"`, or an unrecognized value) runs on the same worker and shows the
  captured output through the ordinary windowed (less-style) text reader, with the title still
  naming the original file rather than the generated temp path. A missing/failing command
  (binary not found, non-zero exit, `{out}` never produced, an undecodable image) degrades safely
  to `[can not preview: <ext>]` with the failure reason attached — never a crash, never a raw Debug
  dump. `[external] preview_commands = false` continues to disable the whole delegation path.

### Fixed
- **Finishing a background delete/trash could silently wipe the selection you were still building
  in a *different* tab.** `apply_file_op` (the async file-operation completion handler) has two
  places that must only act on the tab that actually dispatched the operation, because the user
  can switch tabs while a delete is in flight: the tree-cursor reveal already carried a `res.root
  == self.tab.root` guard for exactly this reason, but the selection-clearing branch a few lines
  below it in the same function did not — so a delete finishing while a different tab was active
  cleared *that* tab's selection instead of leaving it alone. Added the same root guard to the
  selection-clearing branch. The originating tab's now-stale selection (paths that no longer
  exist) is left as-is when this guard skips it; it self-heals the moment that tab becomes active
  again, since every tab switch already prunes vanished paths from the selection
  (`refresh_fs_inner`'s `self.tab.selection.retain(...)`, run via `refresh_fs_after_tab_switch`).
- **Switching to a different repository's tab left the branch name and the `WT <origin>` chip
  showing the *previous* tab's repository**, even though the changed-file list updated correctly
  (a "list is right, branch is wrong" split). Root cause: `git_branch`/`worktree_origin` are only
  re-verified by `App::refresh_git_if_needed`, which used to be called solely from inside
  `tree::render` — but the Git full-screen views (changes hub, log, graph, branches, worktrees,
  commit detail) and Preview mode all bypass `tree::render` entirely, so re-verification never ran
  while any of those were on screen. The changed-file list itself is separate per-tab state
  (rebuilt by `open_git_view`), which is why it always looked right while the chrome around it
  didn't. Moved the re-verification call to the top of `ui::render`, once per frame, ahead of
  every view (still a cheap no-op when the root hasn't changed and nothing is dirty).
- **`[ui] show_hidden = true` had no effect at startup.** The config field parsed correctly and
  the reference docs described it as "show dotfiles at startup", but `App::new` never read it —
  the fresh tab's hidden-file flag was unconditionally initialized to `false`, only ever changing
  via the `.` keypress or session restore. Dotfiles now start visible when the config says so, a
  new tab (`t`/`Ctrl-t`) resets to the same config default instead of silently inheriting whatever
  the source tab's `.` toggle last left it at, and a restored tab's saved visibility still wins
  over the config default (unchanged).
- **Text/code preview: the first PageUp/HalfPageUp right after paging down was silently
  ignored — happens with the default keymap.** `Ctrl-f Ctrl-f Ctrl-f Ctrl-b` (vim scheme) or
  `f f f b` (less scheme) left the window exactly where it was on that first `Ctrl-b`/`b`;
  `Ctrl-d`/`Ctrl-u` and `PageDown`/`PageUp` had the same problem. Root cause: paging reused the
  single-step "always-visible caret" model (`preview_cursor_move`/`follow_cursor`), which moves
  the caret first and only scrolls the window once the caret would leave the screen. Paging down
  leaves the caret sitting exactly on the last visible row (the closest `follow_cursor` gets while
  keeping it on screen); the next page/half-page up then moves the caret back up by one page,
  landing it exactly on the window's top row — still "on screen" — so `follow_cursor` had nothing
  to do and the window never moved. Text/code paging now moves the window directly and carries the
  caret along, preserving its on-screen row; `j`/`k` (single-step) are unaffected.

## [0.23.2] - 2026-08-03

### Fixed
- **Inline math (`$…$`) inside a block structure tore that structure apart.** Worst case: a math
  expression inside a **closed** `<details>` block made its hidden body render anyway — the `▸`
  collapsed marker stopped hiding anything, which is an information disclosure. The same lifting
  spilled a GitHub alert's remaining lines out of its callout box as raw `>`-prefixed text, emptied
  the cell of a table row and dumped the rest of the table as raw `| a | b |` text, and split a
  plain blockquote's continuation into a separate unquoted paragraph. Root cause: math extraction
  ran over the whole document up front and lifted every `$…$` onto its own line, but an alert, a
  `<details>`/HTML block and a table are each recognized by their own parser only while their lines
  stay consecutive — lifting math out mid-block tore that continuity. Math inside one of those
  blocks is now left as literal `$…$` text: the structure survives and only that one expression
  stays unrendered instead of becoming an image (principle #3). Lists and headings are unaffected —
  splitting a line there does not change how they parse. Visible only in terminals with an image
  backend, where math renders as images at all.

## [0.23.1] - 2026-08-01

### Added
- **Create a linked worktree from the list**: `n` in the worktree list (`w` from the changes hub)
  opens a one-line input for a branch name. New-vs-existing is auto-detected
  (`git branch_tip`) — an existing branch is checked out without `-b`, a new name creates the
  branch (`-b`, from HEAD) — so there's no separate prompt for it. Placed next to the **main**
  worktree under the new `[git] worktree_dir` setting (default `"../"`; a branch name's `/` is
  replaced with `-` for the directory name, since a slash would otherwise make git create a nested
  directory). On success, closes the list and switches this tab's root — and `open_dir` — into the
  new worktree (same treatment as `worktree_goto`'s `Enter`). On failure, git's own `fatal: ...`
  message is flashed and the list stays open — the flash now leads with that line specifically
  (not the command that was run, and not `git`'s own progress chatter ahead of it on stderr), since
  the flash footer is a single line clipped at the terminal width and anything ahead of the reason
  just pushes it off-screen.

### Fixed
- **File-operation errors always showed Japanese, regardless of `ui.lang`**: `fileops` (which
  doesn't — and shouldn't — know the display language) reported conditions like "a file/directory
  with that name already exists" or "failed to move to Trash" as hardcoded Japanese strings that
  went straight into the flash message, so an English-configured UI would suddenly show Japanese
  text on the most common mistakes (creating a file with a name that already exists, a rename
  collision, a batch-rename collision, a Trash failure). These are now typed
  (`fileops::FileOpError`) and translated on the `App` side (`App::describe_error`, via
  `anyhow::Error::downcast_ref`) into whichever language the UI is currently showing.

## [0.23.0] - 2026-07-31

### Added
- **Linked-worktree list**: `w` in the changes hub opens a list of the repository's linked worktrees
  (`git worktree add`) — not to be confused with the rest of the app's "worktree" (always the
  uncommitted working tree). `Enter` switches this tab's root (and `open_dir`, so `@`-reference
  copies stay anchored to the new location) to the selected worktree; `Ctrl-t` opens it in a new
  tab, leaving the current tab untouched. A bare main worktree, a locked/prunable one, or the
  currently-active one all refuse to switch with an explanatory flash instead. `/` filters by
  branch name or path. `d` shows the selected worktree's diff since the base branch —
  **committed and uncommitted changes together** (equivalent to
  `git diff $(git merge-base <base> HEAD)`), so it still shows something even when an AI agent has
  been committing along the way inside that worktree (an uncommitted-only diff would otherwise go
  blank the moment it commits). The base is picked from `graph_base_branches` (reusing the graph's
  existing setting) plus the main worktree's branch, choosing whichever candidate's merge-base with
  the diffed worktree is newest — the branch it actually diverged from, not necessarily the first
  one listed — so a stale first-listed branch doesn't pull in an unrelated sibling branch's
  accumulated history; falls back further to an uncommitted-only diff when there's no resolvable
  base or nothing has piled up yet, with the title making clear which one is being shown. Opens as
  a detail overlay on top of the list
  (`q`/Esc returns to it); unlike switching, this also works on the currently-active worktree.
- **Linked-worktree indicator**: a persistent `WT <origin>` chip in the top context bar, shown
  whenever the current root is inside a linked worktree (`git worktree add`) — never for the main
  working tree. Without it there was nowhere on screen that said "you're in a worktree" or named
  the repository it belongs to: the tree/preview title shows only the root directory's own name,
  which for a linked worktree is an unrelated branch/feature name, not the project's. `<origin>` is
  the main repository's directory name (or, for a bare-repo layout, the bare repo's directory name
  with the trailing `.git` stripped), truncated to 20 columns. Computed alongside the branch name in
  the existing background status scan — never recomputed on every render.

### Fixed
- **External git operations were never picked up when the tree root was a linked worktree**
  (`git worktree add`), so `M`/`U` markers stayed stale forever. A linked worktree's own `HEAD` and
  `index` live under the main repository's `.git/worktrees/<name>/`, not under the worktree's own
  checkout at all, so no filesystem event under root — however deep — ever reached the recursive
  watch, and the watch-gap detector (`git_dir_watch`) additionally short-circuited to "already
  watched" because it compared against the checked-out tree instead of the actual git directory.
  konoma now derives the git directory directly (`git::git_dir`) and watches it non-recursively
  whenever it isn't already covered by root, which also naturally covers the pre-existing
  subdirectory-root case in one check instead of two.

## [0.22.2] - 2026-07-30

### Fixed
- **git integration now switches itself off when no `git` executable is present, instead of
  half-working.** Repository discovery goes through the bundled libgit2 and succeeded regardless, so
  the git views would open while every CLI-backed operation (status, staging, committing) silently
  produced nothing. konoma now probes for a usable `git` once, on first use, and — whatever
  `[external] git` says — degrades exactly as it does when built without the `git` feature: reads
  return empty/`None`, writes return an error. The Git view (`o`) now says git is not installed
  rather than claiming the directory is not a repository, which it may well be. The probe is lazy on
  purpose: on macOS without the Xcode Command Line Tools, invoking `/usr/bin/git` pops a system
  "install the developer tools" dialog, and merely opening a non-repository directory must not
  trigger that.

## [0.22.1] - 2026-07-29

### Changed
- **Copy / move / duplicate / delete now run on a background thread**, with a live progress
  readout (`N/M` targets, plus a running file count for large directories) in the top-right
  busy indicator. The `N/M` count advances per target for copy, move, duplicate and permanent
  delete; moving to the trash is a single batch call to the OS, so it reports only on
  completion. The indicator is shown for a file operation even when `ui.busy_indicator` is
  off — it is what you are waiting on, not background bookkeeping. Pasting or deleting a large
  directory no longer freezes keyboard input and rendering (design principle #4). Only one
  filesystem operation runs at a time; starting another while one is in flight is rejected with
  a flash rather than queued. While the operation runs, the filesystem-watcher events it
  generates are held back and applied as a single refresh once it finishes, so a large copy no
  longer starves the input loop either. Quitting while an operation is still running always asks
  for confirmation (even with `ui.confirm_quit = false`), since leaving interrupts it.

### Fixed
- **A large burst of filesystem changes — a build, an install, an AI agent generating thousands of
  files — could pin konoma at 100% CPU with a completely frozen UI for minutes.** The filesystem
  watcher de-duplicated a burst's changed paths with a linear scan (`Vec::contains`), which costs
  O(n) per path and O(n²) for the whole burst; a real-world burst of 72,000 created files (three
  `cp -R` of a 24,000-file tree) took over 3 minutes at 99% CPU with no keystroke processed. Bursts
  are now de-duplicated in constant time and capped at 1024 individually-tracked paths; a larger
  burst falls back to a single conservative full refresh instead of trying to track every path.

## [0.22.0] - 2026-07-28

### Changed
- **PDF pages now render natively in Rust — no poppler, no Quick Look, no external process at all
  for the common case.** `hayro` (pure Rust) is the new primary PDF renderer; `pdftocairo`/
  `pdftoppm`/`qlmanage`/`sips` are kept purely as a fallback for the rare cases `hayro` can't handle
  (an encrypted PDF, a corrupt file, or anything else it fails to parse/render). Because `hayro`
  renders **any** page directly (not just page 1 like `qlmanage`/`sips`), page navigation (`J`/`K`)
  is no longer gated on poppler being installed — only on the page count being known, which (as
  before) is parsed natively too (`hayro-syntax`, no `pdfinfo` process). `[external] pdf = false`
  now means "never launch the external fallback tools"; it no longer disables PDF preview outright,
  since the primary renderer was never external to begin with. PDF pages also pick up two behaviors
  already used for mermaid/LaTeX-math images: a **transparent background** (only the page's own
  painted content is opaque, so the terminal's background/theme shows through unpainted margins
  instead of forcing white), and a **CJK font rescue** for PDFs that reference a non-embedded CJK
  font by name (e.g. the predefined `HeiseiMin-W3`/Adobe-Japan1 font some tools emit) — a real
  system CJK font is substituted instead of the previous silent all-glyphs-missing blank page.
  Fetching `http(s)://` images referenced from Markdown (`[external] remote_images`) no longer
  spawns `curl`; it's done in-process with `ureq` (rustls + webpki-roots — pure Rust, no
  OpenSSL/native-tls, no system TLS dependency), with the same timeout/size-limit/redirect-following
  behavior. Detecting the OS's preferred language (`[ui] lang = "auto"`) no longer spawns `defaults`
  on macOS; `sys-locale` reads it via a direct CoreFoundation call instead (Linux/BSD locale-env-var
  detection is also handled by the same crate now).

### Added
- **Read a table cell that does not fit.** Wide cells are cut off with an ellipsis in the CSV/TSV
  grid, and there was no way to see the rest — `y c` would copy the value but you still could not
  read it. `Enter` on the cursor cell now opens it in a scrollable popup, wrapped, showing the raw
  value (newlines embedded in a CSV field stay newlines); `Enter`, `q` or `Esc` closes it. Archive
  listings share the table renderer, so a long path inside a `.zip` reads the same way.
- **The `/` tree filter matches fuzzily.** Typing `app` found `app_resolver.rs` before, but `aprs`
  found nothing: the match was a plain substring test and results came back in the tree's own order.
  Matching now goes through nucleo (the matcher Helix uses) and results are ranked best-first;
  space-separated words are AND-ed. Case is still always ignored, as it has always been in konoma.
  Set **`[ui] filter_mode = "substring"`** for the previous behaviour.
- **Archives (`.zip` / `.tar` / `.tar.gz` / `.tgz`) preview as a table of their entries** — Name /
  Size / Modified, in the archive's own order, through the exact same grid as CSV/TSV (`hjkl` cell
  navigation, `/` in-preview search, `y →` cell/row/column copy all come for free). Metadata only:
  entry contents are never extracted or decompressed (`ZipArchive::by_index_raw` and
  `tar::Archive::entries` never read content, only headers/central-directory metadata), and an
  in-archive entry name is never joined onto a filesystem path — this stays entirely outside the
  historical `tar` unpack-time symlink/path-traversal advisories (RUSTSEC-2018-0002 / 2021-0080 /
  2026-0067 / 2026-0068), which are all about `Archive::unpack`. Capped at 100k entries (mirrors
  the CSV row cap); corrupt/empty/wrong-format files degrade to `[can not preview: <ext>]` instead
  of panicking. New default rule: `*.{zip,tar,tgz}` / `*.tar.gz` → `builtin = "archive"`.
- **`[external]` — one on/off switch per external process konoma can launch**, all defaulting to
  `true` (no behavior change unless you opt in). `git` gates status colors/gutter/the Git
  views/stage-commit-checkout (`false` behaves exactly like a `--no-default-features` build: every
  read returns empty/`None`, every write returns an error — `o` flashes a message distinct from
  "not a repo"); `git_tool` gates the external tool launched with `!`; `pdf`/`video` gate PDF page
  rasterization and video thumbnail extraction; `remote_images` gates fetching `http(s)://` images
  referenced from Markdown (`curl` — the only outbound network call konoma makes); `open_links`
  gates opening URLs/files with the OS handler; `preview_commands` gates
  `[[preview.rules]] command = "..."` delegation (falls through to `[can not preview]` when off,
  builtin renderers unaffected). Disabled mechanisms degrade exactly like an absent optional tool
  already does — nothing crashes. Also fixes opening a Markdown link/URL on Linux: it unconditionally
  ran macOS's `open`, so links never opened there; now `open` on macOS, `xdg-open` elsewhere.

### Fixed
- **The `?` help overlay no longer advertises keys that do nothing.** Help renders as a centred popup
  that leaves the top and bottom chrome visible, and those still described the view *behind* it — the
  chip said `TREE` and the footer offered `l:enter`, `/:filter`, `Space:file ops`, `e:edit`, `o:git`,
  none of which are bound while help is open (it only takes `j`/`k`/`g`/`G`/`q`). The cause was that
  "which surface am I on" was derived in two places and only one of them knew about help; the chip and
  footer now come from the same answer and show the overlay's own keys.
- **`P` (jump to the path in the clipboard) while a tree selection is in progress no longer leaves the
  keyboard behind in the tree.** It was the one action that did not close the selection first, so the
  preview opened full-screen while keys kept going to the tree's selection map — `j` moved the hidden
  cursor instead of scrolling what you were looking at. Entering a full-screen preview now always ends
  an in-progress tree selection (discarding it, like `Esc`, rather than committing it).
- **The graph's branch panel can be rebound after all.** `config.example.toml` documented its five
  actions as configurable, but no `[keys.<surface>]` section resolved to that panel, so any binding
  written for it was silently ignored. The surface now has a name (`[keys.git_graph_picker]`), and the
  name table is exhaustive over the surfaces, so a new one cannot be added without deciding whether it
  is configurable.
- **Markdown rendering: code fences no longer break when their contents look like a table, an HTML
  tag, or a `> [!NOTE]` alert.** Three of the renderer's block-splitters (`split_tables`,
  `split_html_blocks`, `split_alerts`) had no idea they might be inside a fenced code block, unlike
  the fence-only mermaid splitter. A fence like:
  ````
  ```text
  | a | b |
  |---|---|
  | 1 | 2 |
  ```
  ````
  had its table-looking lines carved out and rendered as an actual table, splitting the fence in two
  (each half growing its own, broken code header) — and everything after the break could be swallowed
  into that broken code block instead of rendering normally. An HTML tag inside a fence (` ```html `)
  similarly triggered the HTML-block rescue, which also leaked the fence's own closing marker into the
  rescued text. A `> [!NOTE]` line inside a fence opened a (mostly empty) GitHub-alert callout box. All
  three now share one fence-tracking pass and leave fenced content alone, so it renders as plain code —
  and, since the on-screen code-block count now matches what the write-back scanner finds, `y` →
  `c` (copy code block) is no longer refused for documents containing this pattern.
- **Markdown checkbox toggling (and `y c` code-block copy) refused every checkbox in documents larger
  than the preview's display caps, including ones fully on screen.** The preview renderer only shows
  the first 5,000 lines / 1 MiB of a file (`preview::text::load`'s truncation), but the write-back
  scanners (`md_toggle_focused_task`, `focused_code_source`) re-scanned the **whole** file from disk.
  In a document with real checkboxes or fences both inside and beyond that cutoff, the scanner counted
  more items than the renderer drew, and the safety check that guards against writing to the wrong
  line then cancelled *every* toggle in the file ("file changed on disk — reloaded") — even the very
  first checkbox, which was fully visible. A 6,530-line real-world file showed 136 checkboxes on
  screen but the naive scanner found 185. Both scanners now mirror the exact prefix the renderer used
  (`preview::text::cap_lines`, factored out of `load()` so the limit is defined once) before counting;
  the checkbox-toggle write itself still targets the full on-disk file, so nothing beyond the visible
  prefix is ever touched or truncated. The same mismatch also existed for a pseudo-checkbox-looking
  line inside a document's YAML front matter (which the renderer strips before rendering but the old
  scanner still counted); the scanners now strip front matter the same way, with the removed line count
  folded back into the checkbox's real line offset for the write.
- Known, deliberately out-of-scope limitation (documented in code): a `<br>` inside inline HTML can
  force a line break that turns the text right after it into what looks like a new checkbox line; the
  write-back scanner does not special-case this, so it errs on the side of refusing rather than
  guessing wrong.

## [0.20.0] - 2026-07-25

### Added
- **Inline Markdown GIFs now animate.** Full-screen GIF preview has always cycled frames, but a GIF
  embedded in a Markdown document (`![...](x.gif)`) only ever showed its first frame. The inline
  decode path now expands all frames the same way the full-screen path does, budgeted smaller
  (32 MiB vs. the full-screen 128 MiB) since a single document can embed several GIFs at once. A
  static or single-frame GIF still decodes through the normal still-image path, unchanged.

### Fixed
- **Linux: reading a file is no longer mistaken for changing it.** The filesystem watcher never looked
  at the event *kind*, and notify's inotify backend subscribes to `IN_OPEN` — so on Linux merely
  opening or reading a file was reported to konoma as a change. Two things followed from that, both
  Linux-only (macOS's FSEvents does not report reads at all, which is why this went unnoticed):
  - **Follow mode (`F`) jumped to files that were only read.** Anything your agent merely grepped, or
    that a build opened, pulled the view away from what it was actually writing.
  - **konoma spun `git status` forever.** Its own status scan opens tracked files, those reads came
    back as "changes", and that refresh ran another scan — a self-feeding loop. Measured in a Linux
    container on an idle 40-file repository with no input at all: **~18.5 `git status` invocations per
    second, indefinitely** (416 + 416 calls in 24 s), holding CPU at ~5.6% and leaving the "git scan"
    busy indicator permanently lit. After the fix the same idle 24 s window runs **zero** further
    invocations and CPU sits at ~0.5%.

  Events are now filtered to actual content changes before anything else looks at them: every
  `Access(..)` kind is dropped except `Close(Write)` (inotify's `IN_CLOSE_WRITE`, which does mean a
  write happened). macOS behaviour is unchanged — FSEvents only produces kinds that still pass.

## [0.19.0] - 2026-07-25

### Changed
- **Filesystem-change refreshes are skipped when a change burst touches only gitignored paths.** Build
  churn — writes under `target/`, `node_modules/`, `dist/` and other ignored directories — no longer
  triggers a tree rebuild + git-status refresh. Real (non-ignored) file changes and `.gitignore` /
  `.git/info/exclude` changes still refresh as before. (It skips writes under ignored directories that
  exist in the ignore set; a newly-created glob-matched file at the root is not covered.)
- **`samples/` is no longer bundled in the published crate.** The binary never reads them — they are
  demo/tutorial material and test fixtures, and the docs point at the repository copy ("open
  `samples/tutorial.md` from the repository in konoma"). Bundling them made every `cargo install`
  download media it cannot use. The published crate drops from 2.2 MB to 0.7 MB. Nothing changes for
  a git checkout: the samples, the tutorial and the test fixtures are all still there.
- **New key visual.** `samples/sample.png` (the image the previews demo) and the README banner are now
  a daylight forest looking through the gap between two trees into open sky — the meaning of the name
  (木の間, "between the trees"). The site landing page uses a text-free variant, and social previews
  (Open Graph / Twitter cards) now have a dedicated image.
- **All screenshots and the tour GIF were re-shot.** They were captured in a window narrow enough that
  the terminal text stays legible at the width the images are actually displayed, and they no longer
  show the previous key visual. The tour GIF also lost the black border that the macOS window shadow
  left around one of its frames.
- **The README and the agent-watch guide now show follow mode as an animation.** The GIF is built
  around what makes follow mode different from a plain `git diff`: the file already carries one of
  your own uncommitted lines, and after `F` only the lines the agent adds are highlighted, until `f`
  switches to the full git diff and both show up.

### Fixed
- **Clipboard copy now works on Linux.** On X11/Wayland the clipboard contents are owned by the
  process holding the selection, so the previous implementation (set the text, then immediately drop
  the `Clipboard`) released the selection and the copied text was lost right away — copy appeared to
  succeed but nothing could be pasted. `set_clipboard` now holds the selection in a detached thread
  via arboard's `SetExtLinux::wait()` on Linux (macOS/Windows are unchanged; their system clipboards
  persist on their own). Verified in a Linux VM: after a copy, `xclip` reads the copied path and keeps
  reading it. Not unit-tested (requires an X11/Wayland display).
- **The `/` text filter is kept after a tab switch or filesystem change.** Switching away from and
  back to a tab (or any fs event) while a `/` filter was active re-read the tree unfiltered, so the
  title still showed the query (e.g. `/txt`) but the list showed every file. The filter is now
  re-applied after the tree rebuild — refreshing the pool from the current tree so it follows external
  add/remove — mirroring how the `C` changed-files filter is already re-applied.

## [0.18.8] - 2026-07-24

### Changed
- **Follow mode (`F`) now shows the diff *since follow-start*, not the full uncommitted diff.** Pressing
  `F` captures a baseline snapshot of the current working tree, so files that already had uncommitted
  changes appear "unchanged" at follow-start and only edits made *after* `F` are highlighted. The
  baseline is held in memory (bounded to the files dirty at follow-start; clean files use the pinned
  HEAD blob), never writes to your `.git`, and is discarded/recaptured on the next `F`. Design notes:
  `docs/FEATURE-FOLLOW-BASELINE-2026-07.md`.
- **Follow mode is now sticky.** Only `q` (leaving the follow diff), entering a text-input/confirm
  surface, or pressing `F` again stops following — scrolling, `n`/`N` (cycling changed files), and `f`
  (scope toggle) keep it on, so you can read and navigate the follow diff without losing the auto-jump.
  Previously any key other than `F` stopped following (Zed-style hands-off).

### Added
- In a follow-opened diff, **`f` toggles between the diff since follow-start (default) and the full git
  diff** for the same file (configurable via `[keys.preview_git_diff] toggle_follow_diff_scope`). The
  diff title shows the current scope (`· since follow start` / `· full diff`).
- `[ui] restore_single_tab` (default true): set false to not persist single-tab sessions (multi-tab
  sessions still restore).

### Security
- Bumped the transitive dependencies `crossbeam-epoch` (→ 0.9.20) and `plist` (→ 1.10.0, which pulls
  `quick-xml` → 0.41.0) to clear RUSTSEC-2026-0204 and RUSTSEC-2026-0194 / -0195. `cargo audit` now
  reports no vulnerabilities. `syntect` and konoma's own code are unchanged.

## [0.18.6] - 2026-07-23

### Fixed
- **Checkboxes inside a `> [!NOTE]` alert can be toggled, and no longer break the whole document.**
  The renderer strips the `>` and draws an alert's body as normal Markdown, so its task list appears as
  real checkboxes — but the scanner that writes a toggle back only matched lines beginning with
  `-`/`*`/`+` and never found them. The counts disagreed, so the safety check cancelled **every** toggle
  in the file with "file changed on disk — reloaded", including the plain checkboxes outside the alert.
  A task inside a **collapsed** `<details>` was the mirror image: not drawn, but counted. The scanner now
  follows the same block rules as the renderer (alert bodies with the `>` stripped; a `<details>` body
  only while it is open).

  This is the third time the same class of bug has surfaced (the first was `*` and `+` bullets in
  0.18.1), so the tests now pin the **class**: a 40-document corpus covering every construct the
  renderer treats specially asserts that the scanner finds exactly the checkboxes the renderer draws;
  a second pass toggles **every** checkbox in **every** corpus document through the real preview
  pipeline and asserts the write is byte-exact — one state character flips, the target is the Nth box
  in the source, no other box moves, and CRLF and a missing trailing newline survive; and end-to-end
  tests drive the real keys (Tab/Space) over documents mixing plain, alert and collapsed-`<details>`
  checkboxes. Reverting any of the three fixes (bullets, alerts, details) turns several of them red.
- **Copying a code block (`y c`) works in documents containing an alert or a collapsed `<details>`.**
  The copy resolves the source by ordinal with the same count guard as the checkbox toggle, and had the
  same defect: a fence inside a `> [!NOTE]` was drawn but never scanned, and a fence inside a collapsed
  `<details>` was scanned but never drawn — so **every** copy in such a document was refused with
  "code block copy unavailable". A fence inside an alert now also copies the code itself, without the
  `>` quote prefix that would have been pasted before. (A ```mermaid fence inside an alert is not
  turned into a diagram — the alert body goes through the plain text path — so it is treated as an
  ordinary code block, and the scanner agrees.)
- **Leaving a git repository for an ordinary directory now drops that repository's ignore set.** The
  "already computing" guard compared the pending workdir with the target workdir, and for a non-repo
  root **both are `None`**, so the guard always fired and the clearing branch was never reached — the
  previous repository's ignored paths stayed applied to an unrelated tree.

### Tests
- Pinned the extraction contracts that are indexed **by ordinal**, since a drift there is silent and
  total: the mermaid fence list (both the render pass and "open this diagram full screen" index into
  it), the math expression list (including the look-alikes that must *not* become pictures — currency,
  escaped dollars, code), and the alignment between link spans and their targets across inline,
  reference, autolink, table-cell, alert, anchor and CJK forms (a drift there would make every link in
  a document open the wrong place while looking correct).

- **Tab now scrolls the whole focused block into view.** Focus lands on a single line — a code block's
  header, a `<details>` summary — but what you want to read continues below it, so tabbing to a block
  near the bottom of the screen parked it against the edge with its contents cut off and you had to
  scroll by hand. Every multi-line item now reports its real extent and is brought fully into view
  (a block taller than the window is aligned to its top), the behaviour diagrams already had.

### Changed
- **Switching tabs in a git repository no longer waits for `git status`.** The whole-worktree scan is
  now run on a worker thread and applied when it arrives, instead of blocking the UI. Every tab switch
  used to force a fresh synchronous scan — it deliberately invalidated the per-workdir cache that makes
  `h`/`l` instant, because a backgrounded tab can miss file-system events and must be re-validated.
  That re-validation still happens; it just no longer holds up the keypress. Depending on which tab you
  land on, the scan ran either inside the key handler (freezing the keystroke itself) or during the
  tree draw. Measured on a real 4-tab session: switching to a tree tab in a git repo went from
  **~11.3ms to ~1.5ms**, and the cost no longer grows with the size of the repository — a scan costs
  ~5ms even on a six-file repo and hundreds of milliseconds on a large working tree.
  While a scan is in flight the previously known statuses stay on screen (no blinking markers) and the
  top-right busy indicator shows the git scan. Commands that *answer* from the status — `d` (open diff),
  `C` (changed-only filter), `n`/`N` (jump to change), and the branch label right after a checkout —
  still resolve synchronously, so they never report "no changes" merely because a scan is still running.
  Re-validation requests that arrive while a scan is running are coalesced into a single follow-up scan
  rather than starting one per file-system event.
- **Switching tabs no longer rebuilds the preview twice.** `load_active` reopens the file itself and then
  handed off to the generic filesystem-refresh path, whose `reload_preview()` repeated the same work — a
  table tab re-parsed the whole CSV on every switch, a text tab reopened its window reader. External
  edits still reload the preview exactly as before; only the redundant second pass on tab switch is gone.
- **Returning to an image / PDF / SVG / video tab no longer re-decodes it.** Every switch used to redo
  the work that produced the picture: an image decode, an SVG or mermaid rasterization, or a
  `pdftocairo` / `ffmpeg` run — the last two costing hundreds of milliseconds. The most recently left
  tab's decoded image is now kept in a single slot and reused when you switch back, keyed on path, size,
  modification time, PDF page and mermaid-fence index. A tool that rewrites a file while preserving both
  its mtime and its size (rare, but `cp -p` of an identically sized file would) can still show the
  previous picture until you reopen it. Images over 128 MiB are not kept, and closing a tab releases its
  cached image. The kitty image itself is still rebuilt and re-transmitted
  for the new display geometry (measured ~26-33 ms to build plus ~62 ms to encode a 417 KB transfer);
  reusing that as well would need the terminal-side image to be tracked across tabs and is not done.

## [0.18.1] - 2026-07-22

### Fixed
- **Checkboxes on `*` and `+` bullets can be toggled again.** GFM task lists accept any bullet
  (`-`, `*`, `+`) and konoma rendered all three as checkboxes, but the source scanner used to write
  the toggle back only recognized `-`, so the toggle's safety check mismatched and cancelled every
  toggle with "file changed on disk — reloaded". `*` and `+` task lists now toggle like `-` ones.
  (A known remaining edge case: a task placed *inside* a `> [!NOTE]` alert or a `<details>` block can
  still trip the same safe guard; the toggle is cancelled rather than writing to the wrong place.)

### Tests
- Expanded the test suite substantially (no runtime behavior change): coverage for the math/image
  inline-cell sizing, table paging, Markdown task-toggle guards, the file-info popup render, and the
  paste-jump parser edge cases + navigation; hot-path **speed** regression guards for Markdown
  decoration, CSV tables, windowed paging, the kitty transmit path, git diff/views, and tab switching;
  and a test-only counting allocator with **memory** guards (MdCache reuse, windowed previews not
  scaling with file size, per-tab struct size, table row cap).

## [0.18.0] - 2026-07-22

### Fixed
- **`h`/`l` navigation is no longer slow in large git repos.** Moving the root with `h`/`l` re-ran a
  full `git status` (a whole-worktree scan) every time — 0.3 s+ on a large repo, on every keypress.
  But `git status` is run from the repo's working directory, so its result is identical anywhere in
  the same repo; it is now cached per working directory and reused across `h`/`l` (the same treatment
  the heavy ignore-set already had). Status stays correct: a working-file change, a commit or
  checkout (the `.git` directory is now also watched when the tree root is a subdirectory, so
  external git operations are seen), switching tabs, or returning to the tree all re-verify it.

### Changed
- **Zooming and panning a full-screen image no longer hitches.** The resize + compression for the
  kitty transfer (~50 ms) now runs on a worker thread instead of the render thread, so rapid `+`/`-`
  and `hjkl` stay responsive — the current image keeps showing until the sharper one for the new
  zoom/pan arrives (latest-wins). The very first frame when a file opens still builds synchronously
  so the image appears at once. Idle CPU stays at zero.

## [0.17.0] - 2026-07-21

### Added
- **Search inside rendered Markdown.** `/` now works in a decorated Markdown preview instead of
  refusing with "code/text preview only". It searches the rendered lines — what you actually see —
  scrolls each hit into view with a few lines of context, highlights every match on screen
  (current one in orange, the rest in yellow), and `n` / `N` walk through them. You no longer have
  to switch to raw source (`R`) to find something.
- **Search inside CSV/TSV tables.** `/` searches the cells (case-insensitive) and moves the cell
  cursor to the first match; `n` / `N` step through the matches in reading order and wrap at the
  ends; matching cells are underlined so you can see the rest at a glance. Same keys as the
  code/text preview search.
- **`0` also clears the pinned base branch in the git graph** (alongside the existing `x`), and the
  graph's `s` / `x` / `0` / `b` keys now appear in the `?` help.

### Fixed
- **A table search's highlights no longer leak to another tab.** The set of matching cells was not
  carried in per-tab state, so switching from a table where a search was active to another table
  could highlight cells at the old match coordinates. It is now rebuilt from that tab's own search
  on switch.
- **`Esc` did nothing in a CSV/TSV table preview.** It now clears an active search, and otherwise
  returns to the tree — the same two-step behaviour as the text and image previews.
- **The git graph's keys could not be rebound.** All eight graph actions (`git_graph_set_base`,
  `git_graph_clear_base`, `git_graph_open_picker` and the five picker actions) were missing from the
  config parser, so `[keys.git_graph]` entries for them were rejected as unknown — contradicting the
  documented "every command is rebindable". A new test walks every default binding and asserts it
  round-trips through the config parser, so this class of gap cannot reappear silently.

### Changed
- **Full-screen images transfer far less data on kitty terminals.** konoma now transmits still
  images (PNG/JPG/SVG/PDF/video-thumbnail) with its own kitty-graphics path that **zlib-compresses
  the pixels** (`o=z`) instead of sending raw RGBA. konoma's own work to show an image was already
  ~37 ms; the multi-second wait was the terminal receiving several MB of escape data. Compression
  cuts that transfer 2× for photos and ~50× for the screenshots and diagrams typical of AI
  pair-programming — the whole point of the tool. Zoom/pan and the fit geometry are unchanged, and
  non-kitty terminals (sixel/iTerm2/halfblocks) are unaffected. See
  `docs/PERF-IMAGE-TRANSFER-2026-07.md` for the measurements.
- **Previews are no longer rebuilt on unrelated file changes.** A filesystem event used to re-read
  and re-render whatever was on screen no matter which file changed, so an agent writing to `src/`
  repeatedly re-rendered an unrelated Markdown document (or re-parsed a large CSV). The preview now
  reloads only when the previewed file itself changed, when the change cannot be attributed (a
  commit, a checkout, a `.git`-only event), or when a git diff is on screen — external edits to what
  you are looking at still appear immediately.
- **The published crate is much smaller** — 6.1 MiB / 127 files down to 2.7 MiB / 99 files. The
  documentation site (`site/`, published to GitHub Pages) and a 1.8 MB auto-generated 52k-line
  scrolling-performance sample are no longer packaged. Nothing that konoma reads at build or run
  time was removed.

## [0.16.0] - 2026-07-20

### Added
- **Heading outline / jump panel.** Press `o` in a Markdown preview to open an outline of the
  document's headings (indented by level); `j`/`k`/`g`/`G` move, `Enter` scrolls the preview to the
  selected heading, `o`/`q`/`Esc` close.
- **Bare URL & email autolinking in Markdown previews** (GFM autolink). A plain `https://…`,
  `www.…`, or `foo@bar.com` now becomes a focusable link (`Tab` to it, `Enter` opens it), like
  GitHub. Never applied inside code spans or code fences. Toggle with `[ui] md_autolink`.
- **GitHub-style alerts.** `> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]`
  (case-insensitive, plus common aliases, with an optional inline title) render as colored
  callout boxes with an icon and label instead of a plain blockquote. Toggle with `[ui] md_alerts`.
- **Emoji shortcodes.** `:rocket:`, `:sparkles:`, `:+1:`, … are converted to real Unicode emoji
  in Markdown previews, like GitHub. Shortcodes with no Unicode equivalent (GitHub-custom like
  `:shipit:`) and shortcodes inside code stay literal. Toggle with `[ui] md_emoji`.
- **In-page anchor jumps.** A Markdown link to a heading (`[x](#slug)`) now scrolls the preview to
  that heading instead of flashing "not supported" (GitHub-style slugs, with duplicate
  disambiguation). Works via `Tab` + `Enter` like any other link.
- **YAML front matter** is recognized (`---` … `---` at the very start) and shown as a compact dim
  metadata block instead of a rule + raw YAML. Toggle with `[ui] md_frontmatter`.
- **GFM footnotes.** `text[^1]` references render as superscript numbers and the `[^1]: …`
  definitions are collected into a numbered footnotes section at the end. Toggle with
  `[ui] md_footnotes`.
- **Inline HTML** that GitHub renders but the Markdown engine strips: `<del>`/`<s>`/`<strike>` as
  strikethrough, `<kbd>` as an inline-code keycap, `<sup>`/`<sub>` as Unicode, `<br>` as a hard line
  break. Toggle with `[ui] md_inline_html`.
- **Collapsible `<details>`.** `<details>`/`<summary>` blocks render as a collapsible section with
  a `▸`/`▾` disclosure marker; `Tab` focuses the summary and `Space`/`Enter` toggles it open/closed.
  By default the `open` attribute is honored like GitHub (`<details>` collapsed, `<details open>`
  expanded); force a start state with `[ui] md_details` (`"auto"` / `"open"` / `"closed"`).
- **LaTeX math rendering.** `$…$` / `\(…\)` (inline) and `$$…$$` / `\[…\]` (display) render as
  rasterized images via RaTeX (pure Rust, KaTeX quality — no browser/Node), rasterized in-process by
  the same resvg path as SVG/mermaid. A terminal cannot place an image mid-text, so inline math is
  lifted onto its own line; display math is centered. Currency (`$5 and $10`), code spans/fences, and
  escaped `\$` are never mistaken for math. Equations are painted in `[ui] math_color` (default a light
  gray) over a transparent background so they read on a dark terminal (RaTeX paints pure black); set a
  dark color on a light terminal. Toggle rendering with `[ui] math` (`"image"` / `"text"`); non-graphics
  terminals and render failures degrade to the raw LaTeX automatically.

### Changed
- **The bundled `samples/` are now English by default.** The walkthrough demos (`markdown.md`,
  `links.md`, `long-lines.txt`) are English; the Japanese versions live alongside them with a `.ja`
  suffix (`markdown.ja.md`, `links.ja.md`, `long-lines.ja.txt`), matching the existing
  `tutorial.md` / `tutorial.ja.md` convention. `japanese.txt` and the full-width cells in
  `sample.csv` stay as deliberate CJK-width demos.

## [0.15.1] - 2026-07-18

### Fixed
- **Mermaid previews survive tab switches.** Returning to a tab that was showing a `.mmd`
  image (or a full-screen fence diagram) re-rasterizes it instead of silently degrading to
  the Unicode text diagram / a false "cannot render" notice (the tab-restore path skipped
  mermaid kinds, and a stale media mtime blocked the reload fallback).
- **`Enter` on an inline diagram opens the right fence.** With an unsupported/broken fence
  earlier in the document (rendered as text), the focused diagram's ordinal drifted and
  `Enter` re-extracted a different fence's source — usually the broken one, showing a false
  error. Fence ordinals are now carried per placement in source order.
- **Diagram cache no longer grows unboundedly while an agent edits fences.** Fence rasters
  are keyed by content hash; entries whose fence no longer exists in the document are pruned
  on every re-decoration (previously they accumulated until you switched files).
- **Tab focus and in-place diagram zoom/pan are per-tab.** They no longer leak into another
  tab's document on switch (which could hijack `hjkl` for an invisible diagram), and are
  restored when you come back — same treatment as the full-screen image zoom.
- **`q` from a full-screen diagram returns instantly.** The inline-image cache is kept on
  same-file re-entry, so all fences no longer re-render (and the restored scroll/focus can
  no longer be clamped away by the transient loading layout).
- **A failed image encode no longer freezes that image and pins the busy spinner.** The
  encode worker now always reports back; a failure keeps the last good frame (or degrades
  to text if nothing was ever shown) instead of latching the in-flight flag forever and
  keeping the run loop polling at 16ms.
- **Scrolling a Markdown document with off-screen images no longer full-clears the terminal
  on every keypress.** The placeholder-orphan sweep now fires only when an actually drawn
  image moves, appears with prior residue, or leaves the screen.
- **Key-repeat `+` on a full-screen SVG/mermaid no longer spawns duplicate re-raster jobs**
  (one in flight at a time, converging to the latest zoom on arrival).
- **Diagrams larger than the 4096px raster cap are scaled down to fit** instead of being
  silently cropped at the right/bottom edge.
- **An empty ```` ```mermaid ```` fence no longer shows a stuck "loading" line** (it falls
  through to the text path like any other non-renderable fence).
- Zoomed inline diagrams scrolled off screen no longer consume `hjkl`/arrow keys (panning
  applies only while the diagram is visible; scroll back and panning resumes).
- Stale background render results (arriving after a file switch) are dropped instead of
  resurrecting cache entries and invalidating the current document's layout.
- The panic-message hook is now installed once with thread-local suppression, so concurrent
  diagram renders can no longer race the global hook swap and permanently silence panic
  messages.
- **A crashing background worker no longer freezes a preview on "Loading…" with the run loop
  polling at 16ms.** Media load, image decode, fence render, and remote fetch now run inside a
  panic-catching net (like the encode worker), so a pathological/corrupt file that makes
  resvg/`image` panic always reports back — the busy flag clears and the render degrades to
  text (idle CPU stays 0%).
- **git graph decoration (base pin, legend, visible branches, priority order) is now
  per-tab**, matching the graph rows themselves — switching between two tabs both in graph
  view no longer shows one tab's graph with the other tab's legend/`base:` title.
- **The remote inline-image disk cache is now bounded** (`~/.cache/konoma/remote-images`,
  256 newest files) instead of growing for the machine's lifetime.

### Changed
- The inline-diagram caption and focus frame are now localized (Japanese "Enter: 全画面" etc.).
- `?` help, `docs/KEYMAP.md`, and the configuration reference now document the inline-diagram
  keys (`Tab` focus, `Enter` full screen, `+`/`-`/`0` zoom, `hjkl` pan) and that the `mermaid`
  builtin renders as an image by default.
- `config.example.toml` now has English comments (the canonical, packaged copy). A
  Japanese-annotated copy is kept alongside it as `config.example.ja.toml`.

## [0.15.0] - 2026-07-17

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

[Unreleased]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.2...HEAD
[0.23.2]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.1...v0.23.2
[0.23.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.0...v0.23.1
[0.23.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.22.2...v0.23.0
[0.22.2]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.22.1...v0.22.2
[0.22.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.22.0...v0.22.1
[0.22.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.21.0...v0.22.0
[0.21.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.20.0...v0.21.0
[0.20.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.18.8...v0.19.0
[0.18.8]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.18.6...v0.18.8
[0.18.6]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.18.1...v0.18.6
[0.18.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.18.0...v0.18.1
[0.18.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.15.1...v0.16.0
[0.15.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.15.0...v0.15.1
[0.15.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.14.2...v0.15.0
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
