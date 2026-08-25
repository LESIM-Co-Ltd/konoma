# Changelog

All notable changes to konoma are documented in this file. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- **Text an author wrote outside the cells of an HTML `<table>` vanished from the screen.** A
  `<caption>`, a stray line sitting directly inside the `<table>` or a `<tr>`, prose between two
  cells — all of it was silently deleted, because a folded table carries its cells and nothing else.
  A `<table>` holding any non-whitespace character outside its `<td>`/`<th>` cells is now left as a
  plain HTML block instead of being folded into a grid, so every character the author wrote is
  still shown (tag-stripped, the way HTML that konoma does not model has always been drawn). This
  is the same choice konoma already made for content glued on after a `</table>` and for a
  `<details>` with content after its close: when the structured form cannot carry the text, the
  unstructured form that can carry it wins. Whitespace, markup, and commented-out text are not
  characters for this purpose, so an ordinary indented table — and one with a commented-out row —
  still draws as a real table.
- **A tab, an escape, or any other control character in a table cell knocked the grid a column out
  of line.** Cell widths were measured with a function that scores a control character as one
  column, while the terminal paints none of it at all, so every row holding one came out narrower
  than its own borders and the box stopped being a rectangle. Control characters in a cell are now
  turned into a space, on both the GFM `|` and the HTML `<table>` path — so a tab inside a cell
  reads as the space it was meant to be, and the grid lines up.
- **Text commented out with `<!-- … -->` inside an HTML `<table>` was drawn on screen.** Commenting
  a whole `<tr>` out is the ordinary way to disable a row, but the table parser added with HTML
  table support scanned tags without any comment rule at all, so a `<tr>`/`<td>` inside a comment
  opened a real row — and the folded cell then named a byte range that no longer contained the
  `<!--` that would have hidden it, so the author's removed text was rendered as an ordinary cell.
  The same source was correctly hidden whenever the table happened *not* to fold (anything glued
  after `</table>` keeps the block on the plain-HTML path, which has always dropped comments),
  which is what made the disclosure easy to miss. The table parser now skips comments through the
  one shared rule the two HTML text scanners already used, so `<tr>`, `<td>`, `<table>` and
  `</table>` written inside a comment are content, never structure — including comments spanning
  several lines, comments containing the table's own `</table>`, and comments that are never
  closed. A comment written inside a cell's own text is unchanged: it stays that cell's content and
  is dropped when the cell is rendered, exactly as before.
- **Markdown nested inside a plain `>` blockquote is now decorated the same way it is anywhere
  else.** A quote used to be a dead zone: a table in one collapsed to a single wrapped line of raw
  `|` and `-` characters, an HTML block inside one **disappeared from the screen entirely**, and a
  heading, thematic break, code fence, GitHub alert, or task checkbox nested in one kept its raw
  `#`, `---`, ```` ``` ````, `[!NOTE]`, or `[ ]` text instead of being drawn. All of them render
  properly now. GitHub alerts (`> [!NOTE]`) were already decorated and are unchanged.
- **A list item whose own content is nothing but an image (`- ![alt](x.png)`, the common "one
  image per bullet" idiom) rendered as bare alt text instead of a real image.** A list item's own
  *first* child paragraph took a special "continue on the marker's own row" code path that never
  checked for a standalone image at all — every *later* paragraph in the same item already did, so
  a two-paragraph item (`- a\n\n  ![img](x.png)`) drew the second paragraph as a real image while
  the same shape as an item's sole/first content stayed text-only. Now checked before that
  splice's decision is made, so a first-child-only image is drawn as a real image block, same as
  any other position.

### Added
- **Tables and block images can now be placed left, centered, or right in a Markdown preview.**
  Two new `[ui]` options: `md_table_align` (default `"left"`) and `md_image_align` (default
  `"center"`), each accepting `"left"` / `"center"` / `"right"`. Until now a table was nailed to
  the left edge and every block image was nailed to the center, with no way to say otherwise — a
  narrow two-column table in a wide pane looked stranded, and a document whose pictures should read
  as a left-hand column had no way to ask. `md_table_align` moves the whole grid, GFM pipe tables
  and HTML `<table>` alike, and any picture inside a cell travels with the box (so a right-aligned
  table still draws its images between its own borders); it deliberately does **not** change
  alignment *within* a cell, which a column's `:---:` and an HTML cell's `align=` keep deciding.
  `md_image_align` covers a standalone `![alt](url)`, a packed row of badges, and a Mermaid
  diagram — whose caption line and focus frame move with it, all three derived from one
  computation so they cannot drift apart by a column. An image inside a table cell (placed by its
  cell) and display math (centered by typesetting convention, not by image layout) are unaffected,
  as is anything at least as wide as the pane, which stays flush left whatever the setting says.
  **The defaults are exactly what konoma has always drawn**, so an existing config sees no change.
- **A table cell can now hold a real image, drawn in pixels, instead of a `🖼 alt` label.** Both
  kinds of table: the GFM pipe table (`| png | ![alt](a.png) |`) and the HTML `<table>` whose cell
  holds an `<img>` — the side-by-side screenshot grid a great many READMEs are built from, konoma's
  own included. The cell reserves a rectangle sized to its own column (the same fit a full-width
  image gets, measured against the column instead of the page), the row grows to whatever the
  tallest image in it needs, and the picture is drawn over that rectangle exactly the way a
  standalone image already was. Works inside a blockquote and inside `<details>`, with
  `align="center"` and friends (the rectangle is centered, not just the text), and remote images in
  a cell are now downloaded like any other — they never were before, so a README pointing at
  `raw.githubusercontent.com` could not have shown them at all. A cell holding an image *and* text
  stacks them, image first. Anything that cannot be drawn — a missing file, a fetch still in
  flight, a terminal with no image support — keeps the `🖼 alt` label exactly as before, and a
  badge written as a link wrapping an image (`[![alt](img)](href)`) stays an openable link rather
  than becoming a picture. Known limitation: the same image file shown at two *different* sizes on
  one screen (e.g. the same logo in two columns the width budget shaved differently) draws only one
  of them — the inline-image cache holds a single encoded size per file. See `docs/STATUS.md`.
- **An HTML `<table>` in a Markdown document is now drawn as a real table.** The side-by-side
  screenshot grid a great many READMEs write as `<table><tr><td>…` (konoma's own included) used to
  lose its structure completely -- the images rendered one after another and the captions followed
  as plain left-aligned lines. It now goes through the same box-drawing renderer a GFM `|` table
  does, including per-cell `align="left|center|right"`, `<th>` header cells (a leading run of
  all-`<th>` rows gets a header rule; a `<th>` anywhere else is styled on its own), `<thead>`/
  `<tbody>`/`<tfoot>` wrappers, and `<b>`/`<i>`/`<code>`/`<a href>`/`<img>` inside a cell.
  Works inside a blockquote and inside `<details>` too. Not modeled in this pass: `colspan`/
  `rowspan` (drawn as one plain cell), a nested `<table>` (flattened into its enclosing cell),
  `<br>` in a cell (a space, not a line break), an attribute written without
  quotes (`align=center`), and a `<table>` wrapped in a `<div>` (left as plain text, as before). Three shapes deliberately keep their old rendering rather than becoming a
  table, so that no text can go missing: a `<table>` interrupted by a blank line (not a single HTML
  block at all, per CommonMark), a `<table>` with further content glued onto the same block
  right after its `</table>`, and a `<table>` carrying text outside its cells — a `<caption>`, most
  of all.

### Changed
- Markdown preview rendering is now handled end to end by a single, built-in renderer, and the
  `tui-markdown` crate it used to sit on top of has been removed as a dependency entirely. This
  frees konoma from the upstream crate's own input-handling panics (previously caught and
  degraded to raw text rather than crashing konoma, but still a rendering loss) and from a few
  small mismatches between how it and konoma's own extraction logic read GFM syntax. No change in
  how any supported Markdown document is drawn.

## [0.26.5] - 2026-08-24

### Fixed
- **CJK class diagrams (`classDiagram`) in Mermaid previews could panic instead of rendering.**
  A relation or directive line with a non-ASCII (e.g. CJK) class name, appearing outside a
  `class Name { ... }` body, could crash the parser (`byte index N is not a char boundary`) --
  caught by an existing safety net so konoma itself never crashed, but the diagram degraded to
  raw source text. Bumped the bundled `mermaid-text` fork to 0.57.0, which upstream-fixed this
  for every diagram kind except `classDiagram`; patched that one remaining case the same way
  upstream fixed the others. Sequence/flowchart/gantt/timeline/journey diagrams were unaffected
  by this bug and are unchanged.

### Changed
- Refreshed dependencies (`base64`, `infer`, `resvg`, `toml`, and a general `cargo update`),
  including a fix for an unsound `lru` use-after-free advisory (RUSTSEC-2026-0253) pulled in
  transitively through the TUI framework. No user-visible behaviour change beyond the fix above.

## [0.26.4] - 2026-08-21

### Changed
- **Going up a directory (`h`) now puts the cursor on the directory you just left**, instead of the
  first row. Stepping into a folder, looking at something and coming back out used to lose your
  place in the listing every time. `[ui] tree_cursor = "top"` restores the previous behaviour.

## [0.26.3] - 2026-08-21

### Fixed
- **jj: one unreadable repository could silently downgrade every other one.** Whether this jj
  understands the diff template was probed once and remembered for the whole process, and the probe
  cannot tell "this jj is too old" apart from "this repository could not be read at all". Opening an
  unreadable repository first therefore decided the answer for every workspace opened afterwards --
  without launching jj again to check -- and pushed them onto the fallback reader, which is exactly
  the one that drops a rename whose filename contains `{`, `}` or `" => "`. The verdict is kept per
  workspace now, so a repository-specific failure stays with that repository.
- **The license notice shipped with the binaries was missing `ignore`.** The jj backend links it for
  its working-copy walk and it was never added to `THIRD-PARTY-LICENSES.md`, so v0.26.1 and v0.26.2
  both shipped without attributing it. Regenerated, and the release preflight now refuses to publish
  when a direct dependency has no entry at all.

## [0.26.2] - 2026-08-19

### Fixed
- **jj: a path beginning with `-` was read as a flag, not a path.** `jj file show` rejected it
  outright, and the failure was silent: an unchanged file showed a change marker, and a real diff
  rendered as though the whole file were new. Paths now go to jj as fileset literals, which also
  carry spaces and quotes safely.
- **jj: renamed files disappeared from a commit's diff, and their destination never got a marker.**
  `jj diff --summary` prints a rename compressed (`R {orig => renamed}`, with the common prefix and
  suffix hoisted out, and one side empty when a directory component is dropped). konoma cut the
  line at its first space, which produced a path that existed on neither side.
- **jj: every symlink in the repository showed as deleted.** jj tracks symlinks; konoma's walk
  skipped everything that was not a regular file, so a tracked link never entered the comparison
  and fell out the other end as a deletion. Retargeting a link is now noticed too — that changes
  the link's own timestamp, which is the one konoma reads.
- **jj: a tab inside a commit message broke the graph row it belonged to.** Every field after the
  description shifted by one: the author read as the date, the timestamp failed to parse and fell
  back to the sentinel meaning "root commit", and the working copy stopped being drawn as `@`.
- **jj: moving between repositories kept the previous one's backend until the scan finished.**
  Stepping from a git repository into a jj workspace and pressing `!` in that window launched
  lazygit against a repository lazygit cannot see.
- **jj: merging broke the whole view while the merge was in progress.** A merged working copy has
  two parents, and asking jj for `@-` then names both, which it refuses to resolve. konoma read
  that refusal as "no answer": every file on disk showed as newly added, every deletion became
  invisible, and the diff showed each file as though it were new. Reads now name the first
  parent's commit id, which the same query already had to hand.
- **jj: renames whose filename contains `{`, `}` or `" => "` were dropped or resolved to a path
  that does not exist.** `jj diff --summary` compresses a rename for a human reader, and the
  compression is ambiguous for those names; konoma asks jj for the source and target as separate
  fields now, and keeps the old reader only for a jj too old to answer that way.
- **jj: a symlink created since the last snapshot showed as modified rather than added**, and
  asking for a symlink's diff followed it through to its target, rendering the target's entire
  contents as new.
- **jj: `o` was refused on a machine with jj but no git.** Opening the hub asked whether git was
  installed and whether the git integration was switched on before asking which backend answers,
  so a jj workspace drew its chip and change markers in the tree and then answered "git is not
  installed". Both questions now apply only when git is the backend.

### Changed
- The `[external] vcs` doc comment in the source said `auto` prefers jj; it has kept git wherever
  git can answer since v0.26.1, as every other description of the setting already said.
- CI installs jj on the macOS job as well, so the jj tests stop skipping silently on the platform
  konoma is developed on.
- The crate's own description names jj, so the crates.io page says what the tree already does. It
  says "in preview" there too, for the same reason every other surface does.

## [0.26.1] - 2026-08-18

### Added
- **jj (Jujutsu) repositories now work — preview.** The surface is complete and was checked against
  jj's own output in every layout jj can produce, but it has not lived through long everyday use,
  `jj workspace` has no list of its own yet, and jj is pre-1.0 (breaking changes land monthly, so
  konoma probes the `jj` on the machine rather than pinning a version).

  **Nothing changes for a repository that already worked.** git keeps answering wherever it can, so
  a colocated repository looks exactly as it did; jj answers where there is no git repository to
  ask. A repository created with `jj git init --no-colocate`, and
  every `jj workspace`, has no `.git` — so konoma showed nothing at all for them: no change markers,
  no chip, `d` answering "no changes" and `o` answering "not a git repo". They now get the same
  views a git repository does: the tree's markers and ignore dimming, the full-screen diff, the
  changed-file list, follow mode, the change gutter, and the hub (`o`) with its log, graph,
  bookmarks and per-revision detail.

  What jj gets is jj's, not git's wearing jj's data. The chip names the working-copy commit the way
  jj does (`@ owyqxpku side B`) rather than claiming a branch, because jj tracks none and leaves git
  detached — "HEAD" would be a lie. A new file reads as added rather than modified, since jj has no
  untracked state. The graph uses jj's own range and symbols: `@` for the working copy, `○` for an
  ordinary commit, `◆` for immutable, `×` for a conflict, and only *this* workspace's checkout gets
  `@` — another workspace's is an ordinary commit carrying a `ws-second@` label. Rows are named by
  change ID, the name a commit keeps across rewrites. The pointer list is titled bookmarks and marks
  none of them current, because a bookmark does not follow the working copy the way a branch follows
  HEAD.

  **konoma only reads a jj repository.** Every call carries `--ignore-working-copy`, attached in one
  place and held there by a test: without it *any* jj command snapshots the working copy into a new
  commit, and konoma re-reads status on every file-watch event — which is how a read turns into a
  write loop. Keys that would write are hidden rather than offered, and explain themselves if
  pressed anyway.

  That flag answers from the last snapshot somebody else took, so `jj diff` alone goes stale. konoma
  closes the gap itself: it walks the tree anyway, so files newer than the snapshot are checked
  against the parent's content. A file written by an agent with no jj command in between — invisible
  to `jj diff --summary --ignore-working-copy` — still shows up, and follow mode still jumps to it.

- `R` in the hub asks jj for a snapshot — the one thing konoma writes to a jj repository, and only
  when asked. It exists for what konoma cannot see on its own: a file whose contents changed while
  its timestamp did not. `[ui] confirm_jj_sync` (default on) asks first. That nothing else writes is
  checked rather than stated: a test reads the backend's own source and fails if any jj call skips
  `--ignore-working-copy` outside the two places named for it.
- `a` in the graph widens it from jj's own range to every revision, and `[jj] tool` (default
  `lazyjj`) is what `!` launches in a jj repository.
- `[external] vcs` (`"auto"` | `"git"` | `"jj"`) decides which system answers. `auto` (default)
  keeps git wherever git can answer and lets jj fill the gap; `"git"` is always git; `"jj"` uses jj
  wherever a `.jj` exists, colocated or not. With no `jj` binary on the machine, konoma falls back
  to git.

### Changed
- The commit graph's node symbols are now chosen from a table keyed by what the node *means*, and
  the node cell is located by position rather than by matching on the character. Rendering is
  unchanged for git; the previous arrangement would have silently lost the branch legend's colours
  and the selected row's emphasis the moment a second system drew a node differently.

## [0.26.0] - 2026-08-17

### Fixed
- **A code block written inside a numbered or bulleted list could not be copied, and its lines ran
  together on screen.** Pressing `y c` anywhere in such a document answered "couldn't copy code
  block" and left the clipboard untouched — not just for that block, for every block in the file.
  The same documents drew the block's opening fence as literal ``` text, and glued all of its lines
  into one unreadable row.

  Both came from the same place. konoma reconstructed where each code block began and ended by
  reading back the styles of the rows a third-party renderer had already drawn, and that renderer
  glues rows together in two situations: it joins a list-nested block's body into a single row, and
  it attaches the first inline-code span of a following paragraph onto the closing fence's row. The
  second one cost the block its header, so the count of blocks on screen no longer matched the count
  in the file, and a guard that compares the two refused the whole document.

  konoma now derives Markdown structure once, from the document itself, and renders from that. A
  code block's text comes from the file, so copying is a slice of the source rather than a
  reconstruction that has to be sanity-checked, and the guard — along with the refusal message — is
  gone. Checkbox toggling changed the same way: it replaces one byte at a known offset, after
  confirming that byte still reads as expected on disk.

  Documents in Japanese hit this constantly, because a step-by-step list whose items are commands is
  exactly the shape that triggers it; English prose rarely writes a paragraph that starts with inline
  code right under a fence, which is why no crate README in a 2,082-file audit ever showed it.

- **A code block inside a block quote is now drawn and can be copied.** It was previously skipped by
  both the renderer and the copy path, so the feature simply did nothing there.

- **A diagram could sit at "loading" forever.** The key a rendered diagram is stored under was
  computed from the fence body with its trailing newline, while the placeholder asked for it without,
  so the two never met.

- **Several ways a document could quietly lose content**, each found by comparing every rendered row
  against the previous renderer across the whole corpus and 2,080 real crate documents: a centred
  banner showed only its first badge, and badges were also dropped when several shared a line, when a
  bare backslash line break separated them, when a comment trailed the row, when they followed a
  heading's text, and when a paragraph opened with them; a `<details>` opened without a blank line
  lost the first sentence of its folded body; display maths opened by `$$` on its own line, or
  containing an escaped character such as `\,`, vanished; and with `md_frontmatter = false` the `---`
  header disappeared instead of being shown as text.

- **`\(` followed by a Markdown link crashed the preview** — the shape a changelog uses to cite a
  pull request, `\([#519](https://…))`.

- **Toggling a checkbox could change a different line.** In a document that mixes a numbered list
  written with `[ ]` — which konoma leaves as literal text — with an ordinary checkbox further down,
  pressing `Space` on the checkbox rewrote the first numbered line instead, with nothing on screen to
  say so. Found before release; the number of checkboxes recorded and the number drawn are now
  compared across every test document, so the two cannot drift apart again.

- **A checkbox in a list whose items are separated by blank lines** was drawn as literal `[ ]` on a
  row of its own, could not be focused with `Tab`, and did nothing on `Space`. It now behaves like
  any other checkbox.

- **A checkbox inside a block quote** (`> - [ ] …`) was never recognised as one — drawn as text, and
  skipped by `Tab`. It is now focusable and toggleable. Headings inside a block quote are styled too,
  which they were not before.

### Changed
- **A section commented out with `<!-- … -->` is no longer shown.** The comment used to be treated as
  ending at the first blank line, so a long commented-out block — an internal checklist, a draft
  section — was printed in full from that blank line onward, even though no Markdown viewer shows it.

- **Markdown previews are drawn from the document's own structure rather than reconstructed from a
  rendered result.** Beyond the fixes above this is invisible, and it measures faster and lighter on
  large files, but it is a large change to the most-used path in konoma. If a document renders
  differently than you expect, that is worth reporting.

## [0.25.0] - 2026-08-13

### Added
- **Previews that scroll now show where you are and how much is left.** A scrollbar thumb rides the
  frame's right border — its length is the share of the document currently on screen, so a long file
  gets a short thumb and a short one gets a long thumb — and the top border carries a position
  marker: `All` when nothing is off-screen, `Top` and `Bot` at the two ends, and a percentage of the
  scrollable range in between (`先頭` / `末尾` / `全体` in Japanese).

  It covers all three scrolling previews: text, code and raw Markdown (`R`), the decorated Markdown
  view, and the full-screen git diff — the last two previously showed no position at all. Image, PDF,
  video-thumbnail, SVG and full-screen mermaid views are deliberately left alone: they don't scroll
  (zoom `x1.6` and PDF paging `2/3` already report position for them), and their cells carry kitty
  graphics placeholders that must not be drawn over.

  The thumb replaces part of the border rather than sitting next to it, so **no text column is
  given up** — the body wraps exactly where it did before. The marker is drawn as its own
  right-aligned title, so a long path can never push it off the border.

### Fixed
- **A file deleted inside a gitignored directory stayed in the tree as a ghost row, sized `0 B`.**
  Browsing a directory that a `.gitignore` covers — a build output directory, `node_modules`, or in
  the reported case an `out/` full of generated images — and deleting one of its files from outside
  konoma left the row on screen. Opening a preview and returning to the tree then replaced its size
  with a confident `0 B`, so the row didn't look stale, it looked like a real empty file. Nothing
  short of navigating out of the directory and back would clear it.

  Two things had to line up. First, the optimization that absorbs build churn — when every path in a
  filesystem burst is gitignored, skip rebuilding the tree — never looked at *what* the events were.
  It was written for writes to `target/`, where skipping only means a size column is briefly stale
  and the next event fixes it, but it swallowed creations, deletions and renames just the same, and
  those change which rows exist at all. The guard now skips a burst only when every event in it
  merely rewrote the contents of something that already existed; anything that adds, removes or
  renames an entry always refreshes, gitignored or not. Writes to ignored paths are still skipped,
  so the churn the optimization exists for is still absorbed.

  Second, returning from a preview to the tree dropped the cached detail columns without re-deriving
  the rows, which is worse than doing neither: it recomputed half of each row against the disk while
  the other half still came from a listing that could be minutes old. The listing itself is now
  re-derived at that same moment (measured at ~0.02 ms for a typical tree and ~5.3 ms with 10,000
  rows expanded, once per return to the tree), and any active `/` or `C` filter is preserved across
  it. Finally, a row whose metadata cannot be read now shows **blank** detail columns instead of
  inventing `size 0`, so no future gap can turn a stale row back into a confident lie.
- **A still image in a Markdown preview disappeared after about five minutes, if the same document
  also had an animated GIF in it.** An animated inline GIF re-encodes its picture on every frame,
  and each re-encode asked ratatui-image for a fresh kitty protocol — which picks its image id with
  `rand::random()`. To the terminal a new id is a new *image*, so nothing was ever replaced: konoma
  handed a kitty terminal a full, uncompressed copy of the frame, forever, at a measured ~66 MB per
  minute. Ghostty budgets 320 MB for image storage and, when it runs out, evicts images that have no
  placement on screen — which is exactly what a still that has scrolled out of view looks like.
  About five minutes in, that still was thrown away, and konoma had no reason to send it again, so
  it came back blank.

  Inline images are now drawn on a kitty terminal with konoma's own graphics path, the one the
  full-screen viewer already used. Each cached image keeps a **fixed** id per protocol slot and
  reuses it, so every later transmit *replaces* the picture in the terminal instead of adding one:
  a GIF can loop for hours and still cost the terminal a single image. The pixels also go out
  zlib-compressed (`o=z`), which for a diagram- or screenshot-like frame is around 90× less data
  per frame than before (2.0 MB → 22 KB at 86×22 cells).

  Nothing changes for sixel, iTerm2 or halfblocks terminals: those write the image as cell content,
  so the terminal keeps nothing between frames and there was never anything to accumulate.
- **The position indicator in the text/code preview title reached the end of a file without ever
  saying so.** The old `[N%]` was the window's start byte over the *whole file length*, so the final
  screenful — which is never above the window — could not be counted: scrolling to the very bottom
  of a file twice as tall as the viewport stopped at roughly `[50%]`, and the taller the terminal,
  the further short it fell. It also had no way to express "there is nothing to scroll" or "you are
  at the end". The percentage is now taken against the reachable scroll range, and the two ends and
  the fits-on-one-screen case have their own labels.
- **The text, code and raw-Markdown preview footer no longer hides most of its key hints behind
  `…`.** The `v`/`V` range-selection hint was wired to the explanatory sentence written for the `?`
  help screen — `v: select a character range   V: select whole lines (copy with y)` — so a single
  hint spent more than 60 columns and everything after it (`Y:@ref` for copying an `@path#L`
  reference, `F` to resume follow mode, `q`, `?`, `e`, `g/G`, `hl`, `0/$`, `[/]`, `p`, and the page
  keys) was pushed past the footer's trailing `…` and could not be read at all. It also printed the
  key twice, as `v/V:v: … V: …`. The footer now shows the short label `v/V:select` (`v/V:選択`), in
  keeping with its sibling hints, and the full explanation stays in the `?` help screen where there
  is room for it.

## [0.24.1] - 2026-08-12

### Fixed
- **A repository created with `git init --ref-format=reftable` (git 2.45+) is no longer treated as
  "not a git repository at all".** The branch chip, the status markers in the tree, the ignored
  (dimmed) entries, the Git hub (`o`) with its changed-file list, the changed-files filter (`C`),
  the worktree list (`w`), the `WT` chip inside a linked worktree, and every write (stage, unstage,
  discard, commit, branch and worktree operations) all work there now.

  konoma runs those over the `git` CLI, which handles reftable perfectly, but it asked *libgit2*
  where the repository was — and libgit2 rejects the `extensions.refstorage = reftable` key
  outright, so a failed lookup took the CLI-backed features down with it. Discovery now falls back
  to `git rev-parse` when, and only when, a `.git` marker says a repository really is there. An
  ordinary directory still answers "not a repository" from a pure filesystem check without
  launching anything, and the fallback's result is remembered per repository, so neither case pays
  a process per filesystem event.

  The branch name comes from `git` itself, never from `.git/HEAD` — a reftable repository keeps the
  backwards-compatibility placeholder `ref: refs/heads/.invalid` in that file, and a tool that reads
  it displays a branch that does not exist.

  The reads that go through libgit2's *object database* rather than a path — the diff views (`d`,
  `Enter` on a change), the log and the graph, the branch list, the commit detail heading, the
  worktree list's `d` diff and the follow-mode baseline — now fall back to the CLI as well, so a
  reftable repository gets the entire git suite rather than a set of empty views.

  Those fallbacks compute a diff by asking git *which* paths differ and then diffing the two sides'
  contents with the same line differ konoma already renders for the follow-mode baseline, rather
  than parsing git's own unified-diff output. The same lines come out marked added and removed; the
  hunk boundaries and the grouping of nearby changes are that differ's rather than libgit2's, and
  can differ slightly. Binary files are left out of the line diff entirely, as they are on the
  libgit2 path. Every object a diff needs — both sides of every file — is read by a single
  `git cat-file --batch`, so the number of processes a diff launches stays the same whether it
  touches one file or hundreds: a large commit does not get slower in proportion to its size.

  **A repository libgit2 can open is untouched by all of this** — not one extra child process runs
  there, which matters because the change gutter asks for a file's diff every time you open one.

## [0.24.0] - 2026-08-11

### Added
- **Video thumbnails for H.264 and HEVC now need nothing installed, in mp4/m4v/mov *and*
  mkv/webm.** konoma reads the container itself and decodes one keyframe, in pure Rust — so a screen
  recording, a `git`-tracked demo clip, anything a browser can play, **what an iPhone records by
  default** (HEVC, Main and Main 10), and the Matroska container most ripped or re-encoded files
  arrive in all show a thumbnail on a machine with no `ffmpeg` at all. The frame is chosen by the
  same rule `ffmpegthumbnailer` uses by default — the keyframe at the 10% mark, walking forward if it
  lands on a blank fade-in — so the frame you see stays comparable.
  **`ffmpegthumbnailer`/`ffmpeg` are now needed only for** VP9, AV1 and the older codecs (Xvid,
  MPEG-2, WMV, …), the `.avi` container, and the profiles the built-in decoders deliberately refuse
  (H.264 in 10-bit / 4:2:2 / 4:4:4 / monochrome; HEVC outside Main and Main 10 4:2:0 — 4:2:2, 4:4:4,
  12-bit, monochrome and the Range Extensions family).
  Those degrade to the usual hint when no tool is installed, exactly as before. Still thumbnails
  only — konoma does not play video in the terminal.

  Each codec is refused by reading **its own bitstream's** sequence parameter set rather than the
  container's summary of it, before any decoding happens, because the two need not agree — an `hvcC`
  record even carries the chroma format and bit depth it is being checked on, and nothing verifies
  those against the stream they describe. That guard, the decoders and everything downstream of them
  are shared by both containers, so "supported" means the same thing in each. What passes matched
  ffmpeg **byte for byte** on every fixture measured: `samples/sample.mp4`, `samples/sample-hevc.mp4`
  and `samples/sample.mkv` (115,200 Y/U/V samples each) and a 640x480 Main 10 clip compared at full
  16-bit precision (460,800 samples).

  Matroska has no sample table, so unlike mp4 there is no index saying which frames are keyframes:
  konoma seeks to the 10% mark and then reads packets forward, identifying keyframes from the
  bitstream itself. That search is bounded — 32 MiB and 2,000 packets per attempt — so a huge or
  cue-less file gives up and degrades to the external chain instead of reading on. A `.webm` is
  usually VP9 or AV1, and those are still refused by codec ID before anything is decoded.

### Removed
- **poppler is no longer part of the PDF fallback chain, so PDF now needs nothing installed on any
  platform.** `pdftocairo` and `pdftoppm` used to be tried whenever the built-in pure-Rust renderer
  declined a document. Across the 1,628 real PDFs on a development machine run through the same
  dispatch, the built-in renderer handled 1,604 (98.53%) with no crashes, 24 fell through — and
  poppler rescued **none** of those 24: 18 were genuinely blank (poppler produced a uniform image for
  them too) and 6 were not PDFs at all. Its only remaining effect was to make the answer to "why is
  this PDF blank?" be "install poppler", for zero recovered documents. The fallback is now macOS's
  bundled `qlmanage`/`sips`, which are already on the machine, and on every other platform there is
  no external PDF chain at all — no child process is even compiled in.

### Changed
- `[external] video` now gates **only** the fallback extractors, matching what `[external] pdf`
  means for PDF: the built-in decoders never launch a process, so with the flag off H.264 and HEVC
  thumbnails from mp4/m4v/mov and mkv/webm now appear where previously nothing did.
- **A PDF the built-in renderer cannot draw has no fallback past page 1.** `qlmanage`/`sips` can only
  produce the first page, which poppler was not limited to. In practice the built-in renderer draws
  every page of everything it can open, and Linux has behaved exactly this way all along, but where
  poppler would previously have rasterized page 2 of an unsupported document you now get
  `[can not preview]`.
- `[external] pdf` now means "may macOS's `qlmanage`/`sips` be launched for page 1". On other
  platforms it is effectively a no-op. PDF preview and page counting are unaffected by it either way,
  as before — both are pure Rust.
- The `[can not preview]` hint for a PDF no longer suggests installing poppler; it points at the
  terminal's graphics support, or an encrypted/corrupt document.
- **Documentation corrected on what a terminal needs.** Every entry point said images, PDF, SVG,
  Mermaid, math and video thumbnails *require* a kitty-graphics terminal. They do not: konoma's own
  compressed transfer is kitty-only, but iTerm2 and sixel terminals are drawn as real pixels through
  ratatui-image, and only the remaining ones fall back to half-blocks. Text previews always worked
  everywhere. The issue templates also claimed konoma is macOS-only and that Linux is out of scope,
  while CI runs a Linux job and every release ships a Linux binary. Drag & drop, `--help`/`--version`,
  `Space -> D`, `y c`, `Ctrl-t` and global `P` were documented nowhere.

## [0.23.9] - 2026-08-09

### Changed
- **A large diff no longer costs 743ms a keystroke.** Every frame highlighted every line of the diff
  and then threw away everything off screen — twelve times the drawing budget on a 6,500-line diff,
  and a newly created file is a diff where every line is added, which follow mode opens by itself.
  Only the visible rows are highlighted now: 4.5ms unified, 5.2ms side-by-side.
- **Listing a `.tar` or `.tar.gz` no longer reads the whole archive.** A plain `.tar` is seekable and
  was not being told so; a `.tar.gz` cannot seek and now stops after 64MiB and says the listing is
  truncated. A 3MB `.tar.gz` holding a gigabyte took 289ms, on every tab switch.

### Fixed
- **A blank PDF page is no longer shown as blank.** The native renderer returns a pixmap rather than a
  result, so a page it could not draw came back empty and the fallback to the external renderer —
  which draws those pages fine — was never reached.
- **`⎇` and `⌖` follow `ui.icons` like every other symbol**, and no longer sit flush against the branch
  dot in the graph legend. They come from a fallback font whose glyphs are wider than the cell, so
  they could overlap what was next to them.
- **Images are laid out with the terminal's real cell size** when it answers the capability query but
  not the size query. The fallback was 10x20, described by the library as arbitrary; against the font
  this is used with, every image, diagram and formula was about 6% off.
- **A file whose name is stored decomposed showed no git marker and could not be diffed** (macOS).
  `git status` recomposes names before reporting them while the tree reads them from the directory, so
  the two spellings never matched.
- **A remote image that failed to download is retried on an explicit reload.** Remembering the failure
  is deliberate — an agent writing files fires an event per write — but `r` is the user asking for it
  to be fetched again, and until now only restarting konoma would do that.
- **Full-screen images work in tmux when `$TMUX` is not set but `TERM` says tmux** — after an ssh, or
  under `sudo`. konoma and the image backend disagreed about how to detect tmux, so konoma wrote raw
  escapes that tmux then swallowed, while inline images kept working.
- **The changed-files view (`C`) survived neither a file operation nor a tab switch.** Creating,
  renaming, pasting, duplicating or deleting left the ordinary listing on screen under a header still
  claiming `± CHANGED (n)`, with the count taken from the wrong list; `/` behaved the same way. And
  switching away while a `git status` scan was still running snapshotted the unfiltered tree, so
  coming back showed an unmodified file under the CHANGED header and left the cursor on a different
  file than it had been on.

### Fixed
- **Two crashes.** A footnote whose continuation lines mixed ASCII-space indentation with a
  full-width space (U+3000) or a no-break space took the app down the moment the file was previewed:
  the common indent was measured in bytes on one line and applied to another, landing inside a
  multi-byte character. Indenting with a full-width space is ordinary in Japanese Markdown and
  footnotes are on by default. Separately, opening a table cell's full-text popup in a terminal 21
  columns wide or 7 rows tall aborted — including in release builds — because the popup sized itself
  with `clamp`, whose upper bound comes from the terminal and can fall below its fixed lower bound.
- **Toggling a tab-indented checkbox inside an alert or an open `<details>` edited the wrong
  character**, silently: `- [ ] task` became `- [ ] xask`, the checkbox stayed unchecked, and nothing
  was reported. The nested scan located the state character in display columns, which round a tab up
  to four, while the position it wrote to is a byte offset.
- **An empty `command` in a preview rule ran the file being previewed.** A typo, or an empty string
  meant to disable the rule, produced an argument vector consisting of the previewed file and nothing
  else — which is the program konoma launches. It now fails the rule instead, degrading to
  `[can not preview]`.
- **Opening a named pipe or a device froze konoma completely** — not just the drawing but the key
  loop, so `q` and Ctrl-C did nothing. Reading from a FIFO blocks until a writer appears, and only
  directories were being distinguished from files. `konoma /dev` did it too.
- **Passing a path that does not exist left the terminal in raw mode**, so in `sh`, in a script, or
  anywhere the shell does not reset the terminal at each prompt, the user had to run `reset`. The
  path is now checked before the terminal is taken over, and the error names the path instead of
  reporting a bare errno.
- **The changed-files view (`C`) could move the cursor onto a different file, silently drop itself,
  or show an ordinary listing under its own header.** Rebuilding the list resorts it, so the cursor
  has to carry the file's path across the rebuild — three places that rebuild the tree never learned
  to. It takes two filesystem events landing inside one status scan, which is what an agent writing
  files in a row produces. `.` during a scan concluded there were no changes and turned the filter
  off with a message saying so; `s` rebuilt without re-applying the filter; and `/` did not turn `C`
  off, though `C` turns `/` off.
- **Saving a bookmark could lose every other bookmark.** The file was truncated and then written, and
  an unreadable file reads back as no bookmarks at all, so one `m` after an interrupted write left a
  single entry where the whole set had been. It is now written through a temp file and renamed, the
  way sessions already were.
- **Helper processes stole keystrokes, left world-readable files, and could hang forever.** ffmpeg
  and the PDF renderers inherited the terminal's stdin, and ffmpeg reads commands from it. Their
  output went into the shared temp directory, which on Linux is world-readable, so a rendered PDF
  page or a video thumbnail could be read by anyone else on the machine. And none of them had a
  timeout: one that hung kept its worker thread and child process for the rest of the session, and
  every further preview added another.
- **The config and cache directories are resolved consistently.** Only the cache honoured
  `XDG_CACHE_HOME`, so a desktop that sets `XDG_CONFIG_HOME` found konoma alone still using
  `~/.config`. With `HOME` unset the bookmarks path fell back to a *relative* one, so konoma wrote
  its bookmarks and sessions into whatever directory it had been opened on. A config error also
  reported only its location; it now says what was wrong.
- **A remote image in Markdown stayed at "loading" forever** when there was no cache directory to
  download it to: the fetch returned before starting, so it never reached the failure path that
  degrades the placeholder.
- **On Linux: the clipboard now works under Wayland** — the backend was behind a feature that had
  never been enabled, so every copy failed in a session without XWayland. **A failed filesystem watch
  is now reported once** instead of silently stopping follow mode, the changed-files view and the git
  markers — and it is no longer retried ten times a second. **Opening a link no longer prints
  `xdg-open`'s warnings onto the screen** or leaves a zombie process behind.
- **`tab_width` is bounded.** It was used directly as an allocation size and a loop count, so a
  mistyped `tab_width = 1000000000` allocated about a gigabyte for a single tab character, and a
  value near the maximum aborted the process outright.

### Changed
- **An inline GIF no longer animates after you leave the preview.** The frame cache was only cleared
  when a *different* file was previewed, so returning to the tree left konoma waking every 10ms to
  redraw a tree where nothing was moving, holding the frames the whole time.
- **A document full of `$…$` no longer starts one thread per expression.** Six hundred of them took
  911ms to reach the first frame against a 60ms budget, and enough of them make thread creation fail,
  which panics. At most sixteen render at a time now; the rest follow as those land.

## [0.23.8] - 2026-08-08

### Added
- **A `STALE` chip while the listing is out of date.** When a rebuild of the tree fails, konoma keeps
  the last good listing, so nothing breaks on screen — but the tree goes on showing files that may
  already be gone. The message saying so disappeared on the next keypress, like every other message,
  while the outage did not. The chip sits in the context bar next to the follow and worktree chips
  and stays until a rebuild succeeds.

### Fixed
- **A failed refresh of the tree is now reported.** Two paths swallowed it outright — the refresh
  driven by the filesystem watcher, and the one at the end of a tab switch — so the same failure was
  announced or not depending on how you got there, and a stale listing looked exactly like a current
  one. Only the start of an outage is announced, so a watcher firing per burst cannot take over the
  message line.
- **A batch rename whose rollback also fails now says which files it left behind.** Files stranded
  under `.konoma-rename-tmp-N` or under a new name had to be cleaned up by hand, but the message
  showed only why the rename failed: the error's outer context describing the incomplete rollback was
  dropped while looking for the inner cause. The reason and the leftovers are now reported together,
  and which files were left is determined by looking at the disk rather than by inferring it.
- **The cursor could come to rest on a different file than the one it was on.** Both the `/` filter
  and the changed-files view (`C`) rebuild their list and re-sort it — by fuzzy score and by path
  respectively — but only clamped the cursor's index into range, and an index that stays in range
  simply means something else afterwards. A file appearing above the cursor was enough: the next
  `Enter`, `y`, `d` or `Space→d` then acted on a file you had not selected. `C` is the view for
  watching an agent work, and an agent writing a file is what triggers the re-sort. The cursor now
  carries the file's path across a rebuild and finds it again.
- **`.` while filtering showed an unfiltered list under a header that said otherwise**, and hidden
  files could not be found by the filter afterwards. The same gap existed in `C`.

### Changed
- **Pressing `/` on a very large tree no longer blocks.** The walk starts on the keypress and hands
  off whatever is left after 20ms, so a directory that finishes inside that budget behaves exactly as
  before — the pool is complete on the first frame — while a repository whose `target/` alone reaches
  the 50,000-entry cap now returns in 29ms instead of 101ms and fills in while you type. The rescan
  that happens on every filesystem event while a filter is open, which is every time an agent touches
  a file, no longer has a synchronous part at all.


## [0.23.6] - 2026-08-07

### Fixed
- **`y c` still refused to copy on documents whose code blocks were perfectly ordinary.** The
  refusal covers the whole file, so one disagreement anywhere silences every code block in it. The
  renderer does not parse the file — it parses the file after front matter is stripped, footnote
  definitions are rewritten and inline HTML is mapped onto Markdown — while the scanners parsed the
  file itself. A footnote definition spanning more than one line was the common trigger: only its
  first line moved into the footnote section, and the continuation was left behind indented, where
  it became a code block that exists nowhere in the file (and left the definition's link unclosed).
  Four crates in a populated registry are affected, including rand_chacha.
- **Pressing space on a checkbox could tick a different one, silently.** The toggle's guard compared
  counts and state characters, and preprocessing can add one checkbox to each side at once — a
  `<br>` conjuring one on screen, a stray footnote continuation exposing one in the source. Counts
  balanced, states matched, and the write went to the wrong line — one displayed as code — with no
  warning. Preprocessing now reports where each line came from and the toggle follows that trail to
  the byte it is about to change, refusing when the trail breaks. No document that could toggle a
  checkbox before has lost the ability.
- **An indented code block starting with a tag was not drawn as a code block**, so `    <code>foo</code>`
  lost its tags and its gutter, and could not be focused or copied at all. Whether a line is code is
  now decided from the whole document rather than from a fragment that has already had block images
  lifted out of it.
- **`<br>` broke out of whatever contained it.** An alert's frame ended mid-box with the fenced block
  below it leaking out as raw text, and a table row split in two with an empty cell. Across a
  populated registry this corrects 29 files, including feature tables in `image` and
  `portable-atomic` that had collapsed to a single row.


## [0.23.5] - 2026-08-06

### Fixed
- **Copying a code block with `y c` was refused for the whole document whenever a fence sat inside a
  list item** — the shape almost every contributing guide uses (`1. Fork it:` followed by an indented
  fenced block). It also hit any document made of indented paragraphs under a numbered marker, which
  is what the Apache license is: opening a vendored `LICENSE-APACHE.md` and trying to copy anything
  from it failed. Measured against the 2182 Markdown files in a populated `~/.cargo/registry`, 26 of
  them were affected, including the changelogs of crossterm, nix and itertools. The copy only runs
  when the blocks found in the source match the blocks drawn on screen, and the scanner measured
  indentation from the start of the line while Markdown measures it from the start of the enclosing
  block — a list marker shifts everything inside its item to the right. Code blocks are now located
  with the same parser the renderer uses, so the two cannot disagree about what a code block is.
- **A fenced code block whose closing line carried trailing text swallowed the rest of the
  document.** Markdown ends such a block where its list item or quote ends, but konoma had no notion
  of a container, so in nix's changelog everything after line 115 — 1316 lines — stopped being
  recognized: a table rendered as raw pipe characters and eight list items lost their bullets. Two
  other published changelogs have the same shape. Code blocks are now identified by the same parser
  the renderer uses, which knows both about containers and about indented blocks.
- **Markdown syntax written inside a code block was rewritten as if it were syntax.** Explaining a
  keycap by writing `` `<kbd>Ctrl</kbd>` `` produced a keycap indistinguishable from a real one;
  `` `[^1]` `` became a superscript; `` `<br>` `` inserted a line break that split the code span in
  two. The same held for indented code blocks, where an equation was even lifted out of the block and
  drawn as an image below it. Inline code and both kinds of code block are now left alone. (An
  indented block containing `<kbd>` or `<details>` still loses its formatting on screen; its contents
  are no longer altered.)
- **Staging, committing, checking out a branch or creating a worktree froze the whole interface
  until git finished.** A slow pre-commit hook, a lock held by another process, or a repository on a
  stalled network mount left konoma unresponsive for the duration — with a 30 second hook, no key did
  anything at all. Git writes now run on a worker: the tree stays scrollable, a spinner shows the
  write is in flight, and quitting mid-write asks first. They are not killed on a timeout, since
  interrupting a commit would leave `.git/index.lock` behind and a hook that lints a large repository
  is legitimately slow.
- **A file with no newlines could take konoma to gigabytes of memory and freeze the interface.**
  Minified JavaScript and one-line JSON logs were read a line at a time with no limit, so the entire
  line was materialized at once, inside the draw call. Opening a 22 MB one-line JSON reached 2.1 GB
  resident; it now stays under 30 MB. Each line is read up to 8 KiB — thousands of columns, far wider
  than any screen — and the rest is skipped. Searching a line stops at the same point.
- **Pressing `/` on a large tree stalled the interface.** Collecting the pool asked the filesystem
  about every entry twice; the directory listing already carries what was needed. On a repository
  whose `target/` alone reaches the 50,000-entry cap this drops from 329ms to 101ms, with no
  filesystem calls at all for ordinary entries. It is still above the 60ms the interface aims for,
  which cannot be reached while the walk happens on the drawing thread — that remains open rather
  than being papered over by lowering the cap.
- **Checkbox toggling was refused for documents using two spellings GFM allows**: a bullet followed
  by two to four spaces before the checkbox (`*   [ ] task`), and a checkbox with nothing after its
  closing bracket. Both appear in published crates.
- **A panic while highlighting a source file could take down the whole app.** Every other renderer
  — mermaid, LaTeX, PDF, SVG, GIF, archive, remote images — was already wrapped in a panic safety
  net, but syntect, the code path used by nearly every preview, had none. `highlight()` runs on the
  main thread inside the draw call and `highlight_line_by_ext()` runs once per diff line, so a
  pathological input that made syntect panic would abort konoma outright rather than degrade. A
  panic in the background warm-up had a quieter but equally stuck failure: the completion was never
  reported, so that file type's loading spinner turned forever. A caught panic now degrades to
  plain text — the file stays readable, it just loses its coloring — and only the offending line
  loses color, which is how a syntect error was already handled.
- **Three background workers could leave a spinner turning forever.** If the ignore-set scan, the
  git status scan, or a diagram re-rasterization panicked, no result came back and the "in
  progress" flag was never cleared: the busy indicator kept spinning, git status froze at whatever
  it last saw, and the affected diagram could never sharpen again. All three now always deliver a
  result, falling back to a safe empty/failed one, so the flag is always cleared.

## [0.23.4] - 2026-08-05

### Fixed
- **A closed `<details>` block containing a GitHub alert (`> [!NOTE]` etc.) leaked the alert's
  body — an information disclosure — regardless of whether the block was collapsed.** The renderer
  used to run `split_alerts` over the whole document *before* `split_details`, so a `> [!NOTE]`
  nested inside a `<details>` block was pulled out and drawn at the top level, outside the fold,
  before `<details>`'s own open/closed check ever got a say. **The reverse nesting — a `<details>`
  block inside an alert's body — was worse: it always leaked, `open` or not**, because an alert
  body's rendering pipeline had no `<details>` handling at all; the block fell through to the
  generic HTML-block-rescue fallback, which strips tags and shows whatever text remains
  unconditionally. Fixed by gating `split_alerts`'s header detection on a new `<details>`-block mask
  (so an alert nested inside one stays literal text until that block's own fold decides whether to
  show it), and by making both an alert's and a `<details>` block's body render through a shared,
  recursive pipeline that understands both constructs — so a `<details>` found inside an alert now
  folds too. A `<details>` reached only through nesting (inside an alert, inside another
  `<details>`, or any depth of alternating the two) is rendered using its own `open` attribute
  directly rather than a document-wide Tab-toggle ordinal slot, so it is not individually
  toggleable but — critically — can never drift that ordinal sequence out of step with what's drawn
  on screen (the exact failure mode a previous, unrelated fix for plain nested `<details>` blocks
  already had to guard against). The write-back scanners (`task_source_locs`/
  `code_block_source_locs`, used by checkbox toggling and `y c`) were updated to recognize the same
  nesting so a checkbox or code fence reachable this way can still be toggled/copied when visible,
  not just correctly hidden when it isn't.
- **A Markdown fence nested inside a longer fence of the same character** (e.g. a `` ```` `` fence
  wrapping a `` ``` `` fence — the standard way to show "how to write a fence" in a document about
  Markdown) **made the renderer (`decorate_code_blocks`), not just the write-back scanners fixed
  below, mistake the inner marker for the block's real close.** Everything after it — headings,
  links, tasks — silently rendered as a second, bogus, empty code block, so the rest of the document
  went dark with no crash to notice. Like the write-back scanners, the renderer used to scan the
  rendered **text** of each line for `` ``` ``; it now groups a code block's lines by tui-markdown's
  own per-line **style** (`is_code_block_line`) instead — the same signal tui-markdown itself uses
  to color a code block, which stays correct no matter what `` ``` ``-looking text ends up inside
  the block's body.
- **Six Markdown write-back/panic-retry scanners tracked "am I inside a code fence" with their own
  naive per-function toggle** (`starts_with("```") || starts_with("~~~")`, closing on the first
  line that merely started with 3 of the same character) instead of the file's one CommonMark-correct
  `parse_fence` (matching fence character, requiring the closing fence's length to be at least the
  opening fence's, and requiring 0-3 columns of indentation for a line to count as a fence at all —
  `parse_fence` itself was missing that indentation check and has been fixed too). For a fence
  nested inside a longer fence of the same type (` ```` ` around ` ``` `), or a fence-lookalike
  indented 4+ columns (which is CommonMark for an *indented code block*, not a fence), the old
  per-function count of code blocks/checkboxes could still happen to match what's drawn on screen
  — so `y c` (copy focused code block) was never refused — while the *content* it silently copied to
  the clipboard was wrong (missing text, extra leading indentation, or an empty string). Unified
  `code_block_source_locs`, `task_source_locs`, `process_inline_html`, `process_footnotes`,
  `split_details`, and `split_block_for_retry` (the panic-isolation retry splitter) onto
  `parse_fence`/`fence_mask`, so all six now agree on fence boundaries with each other and with
  CommonMark. `code_block_source_locs` also now strips the fence's own indentation from its
  content (0-3 columns), matching what's actually shown on screen.
- **`y c` (copy focused code block) and checkbox toggling refused to work on any Markdown document
  that contained a CommonMark indented (4+ column) code block**, even for a real fence right next
  to it — the write-back scanners (`code_block_source_locs`/`task_source_locs`) only recognized
  fenced (```` ``` ```` / `~~~`) code blocks, so an indented one it did not know about threw off the
  on-screen-vs-source count and the safety check cancelled every code-block copy or checkbox toggle
  in the whole file. Both scanners now also recognize top-level indented code blocks, matching
  pulldown-cmark's real behavior (verified empirically): they require a preceding blank line except
  after a non-paragraph block, glue blank-line-separated chunks into one block, and conservatively
  treat any content inside an active list as non-code (avoiding false positives on ordinary
  nested-list paragraphs) at the cost of not detecting the rare deeply-nested-in-a-list case (a
  known, documented gap, same class as the pre-existing plain-block-quote limitation).
- **The refusal message named "the file changed" as the cause** ("cannot copy code block (file
  changed)" / "file changed on disk — reloaded (toggle cancelled)"), which was almost always wrong:
  in practice this refusal has repeatedly turned out to be konoma's own scanner disagreeing with
  the renderer (indented code blocks, `*`/`+` bullet lists, content inside a GitHub alert or
  `<details>`, a document over the size cap), not a concurrent external edit. The messages no
  longer blame the file.
- **An indented code block right after a heading, with no blank line between them, was invisible to
  the write-back scanners** — CommonMark only gates a following indented code block behind a
  *paragraph*; a heading isn't one, so the renderer draws the block fine, but
  `code_block_source_locs`/`task_source_locs` conservatively still required a blank line. As with
  the other indented-code-block mismatches above, this refused `y c`/checkbox toggling for the
  *whole document*, not just the block after the heading — so an unrelated real fence later in the
  same file (e.g. an actual shell example right after a `## Usage` heading whose next line is an
  indented example) was collaterally refused too. The scanners now also recognize a thematic break
  (`---`/`***`/`___`) and a setext heading underline (`Title\n=====`) as non-paragraph content, the
  same way they already treat a heading — verified against the renderer for each construct.

## [0.23.3] - 2026-08-04

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
- **Follow mode's session and "since follow-start" baseline could silently leak across repositories.**
  `follow_mode`/`follow_session`/`follow_baseline` are App-level (follow is a global mode, not
  per-tab), but they describe one specific repository — and the active root can change without ever
  calling `toggle_follow` (tab switch, `l`/`h`, worktree switch via `o`→`w`→`Enter`, paste-jump, a
  bookmark jump, ...). Two symptoms: (1) switching to a tab rooted in a *different* repo kept the old
  repo's file in the follow session, so `n`/`N`'s population and the title's `(i/n)` denominator
  mixed paths from two repositories; (2) switching this *same* tab's root to a linked worktree (which
  shares one object database with the repo the baseline was captured against) let `follow_baseline_diff`'s
  `blob_at` call *successfully* resolve against the wrong worktree's pinned HEAD, producing a diff
  that looked plausible but was wrong — worse than symptom 1, because it failed silently instead of
  visibly. Root cause: nothing recorded which root a captured session/baseline belonged to. Fixed by
  adding `follow_root` (the root pinned at capture time) and a cheap `follow_scope_valid` check
  (compares it against the tab's current root, no I/O); `follow_session_paths`/`follow_baseline_diff`
  degrade to empty/`None` the moment the scope goes stale, and only `follow_note_change` — the
  event-drain side, never the render path — recaptures a stale scope, the same recovery a fresh
  `F`-on gives.
- **A batch permanent-delete or send-to-trash that failed partway through said "Failed" as if
  *nothing* had happened, when in fact some of the targets were already gone — for permanent
  delete, unrecoverably.** `delete_permanently_with_progress` loops per path and bumps its progress
  counter on each success before returning early on the first failure, but `App::run_file_op`'s
  `DeletePermanent` arm never read that counter back on the error path, leaving the reported
  success count at 0 regardless of how far the batch actually got. Trash has the opposite problem —
  `trash::delete_all` is a single call with no partial-progress signal at all — so a failure there
  always looked like a flat "nothing succeeded" even when part of the batch had genuinely been
  trashed. Both now report what actually happened: `DeletePermanent` reads back the real count of
  targets removed before the failure; Trash observes the filesystem afterward (`fileops::
  trash_partial_outcome`) to see which targets are actually gone, since the library doesn't say. The
  flash now reads "<Moved to Trash|Deleted permanently> N / Failed: <reason>" whenever at least one
  target actually succeeded, and additionally names one target that's still present for Trash
  failures — giving the user something concrete instead of a bare "Failed". A failure where nothing
  succeeded at all still shows the plain "Failed: <reason>" with no misleading zero count.

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

[Unreleased]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.26.5...HEAD
[0.26.5]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.26.4...v0.26.5
[0.26.4]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.26.3...v0.26.4
[0.26.3]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.26.2...v0.26.3
[0.26.2]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.26.1...v0.26.2
[0.26.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.26.0...v0.26.1
[0.26.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.25.0...v0.26.0
[0.25.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.24.1...v0.25.0
[0.24.1]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.24.0...v0.24.1
[0.24.0]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.9...v0.24.0
[0.23.9]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.8...v0.23.9
[0.23.8]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.6...v0.23.8
[0.23.6]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.5...v0.23.6
[0.23.5]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.4...v0.23.5
[0.23.4]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.3...v0.23.4
[0.23.3]: https://github.com/LESIM-Co-Ltd/konoma/compare/v0.23.2...v0.23.3
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
