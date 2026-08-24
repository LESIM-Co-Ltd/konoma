//! Diff harness for the block-model renderer (`crate::preview::markdown::render`), over the exact
//! same parity corpus the golden render snapshot (`md_snapshot_tests`) already covers. This is
//! **not** a pass/fail equivalence check between the two renderers in general — `render::render_doc`
//! only covers `Heading`/`Paragraph`/`ThematicBreak`/`List`/`ListItem`/`Quote{alert: None}`/
//! `CodeBlock` so far (see that module's own doc comment on scope), so most cases are expected to
//! fall outside its coverage entirely. What this module checks is narrower and more honest: for every
//! case whose *entire* top-level block set is one this stage claims to support, does the new
//! renderer's fully post-processed output match the production pipeline's, line for line, span for
//! span, style for style — **and placement for placement**? A case only counts as a match
//! ([`Outcome::Match`]) when both halves agree: `RenderOut::lines` against `CaseRender::lines`
//! (`ratatui::text::Line`/`Span`/`Style` all derive `PartialEq`, so this is exact — content, span
//! boundaries, every style field) *and* `RenderOut::images` against `CaseRender::images` (`url`/`alt`/
//! `line`/`cols`/`rows`/`fence_ord`, likewise exact via `ImagePlacement`'s own derived `PartialEq`).
//! Comparing only the rendered lines would miss an entire class of real regression: a wrong
//! `ImagePlacement.url` (the synthetic cache key `md_image_cache`/`ensure_mermaid_fence_render` look
//! an inline image, mermaid diagram, or math expression up by) leaves every rendered placeholder row
//! byte-for-byte unchanged — the bug is invisible to `lines` entirely, and only shows up once a real
//! preview path tries to *resolve* that placement against the cache and finds nothing there. See
//! `run_case`'s own doc comment for exactly how the two comparisons combine into one `Outcome`.
//!
//! ## Three kinds of "expected mismatch" — and the one kind that is not
//!
//! A mismatch this module observes among the cases this stage claims to support always falls into
//! exactly one of three tracked lists, or it is a **real bug in `render_doc`** to be fixed, not
//! catalogued. The three lists are not interchangeable — mixing them together would hide the actual
//! reason production doesn't match, which is the whole point of separating them:
//!
//! * [`KNOWN_MISMATCHES`] — **this stage genuinely has not implemented the shape yet.** The gap is in
//!   `render_doc` itself; a future stage closes it, and the day it does, this list shrinks. **Never**
//!   a place to list something `render_doc` gets *worse* than production — a mismatch where `render_doc`
//!   produces something visibly broken (an empty line where content belongs, dropped structure, a
//!   panic) compared to what production already shows is not "a gap in this stage's coverage," it is a
//!   bug, and belongs fixed, not catalogued (see this section's own math history below for exactly this
//!   mistake, caught and corrected).
//! * [`INTENDED_IMPROVEMENTS`] — **`render_doc` is *better* than production on purpose**, and is not
//!   expected to ever start matching it: production has a real, confirmed bug (an upstream crash, a
//!   glued-together row that drops a code block's own header, ...) that `render_doc`'s own architecture
//!   structurally cannot reproduce even if it tried. See that list's own doc comment for the mechanism
//!   behind each entry.
//! * [`BEHAVIOR_DECISIONS`] — **the two sides disagree about what CommonMark itself says**, and neither
//!   is a bug: `render_doc` follows the real grammar (`Doc::parse`, straight off pulldown-cmark), while
//!   production's own hand-written pre-pass is deliberately *more permissive* than the spec, for real,
//!   still-useful reasons (see that list's own doc comment). Closing this gap is a **product decision**
//!   for whoever wires `render_doc` into a real preview path — not something to resolve inside this
//!   harness by picking a side.
//!
//! ## Tables, HTML blocks, standalone block images, and mermaid fences (closed — history, not a
//! ## live gap)
//!
//! `render_doc` used to leave four `BlockKind`s entirely unimplemented — a `Table`/`Html` block
//! reported the whole enclosing top-level construct `Unsupported`, and a standalone `![alt](url)`
//! paragraph or a `mermaid`-tagged fence rendered as something visibly different from production's
//! own extracted placement (inline alt-text; an ordinary highlighted code block) rather than being
//! screened out at all. All four now render straight from the model
//! (`render::render_table_from_model`/`render_html_block_from_model`/`try_render_paragraph_as_image`/
//! `render_code_block_dispatch`'s own mermaid branch — see each one's own doc comment), closing
//! `unsupported` to `0` across the whole corpus. Recorded here as history (why the gap existed, what
//! closed it, and what closing it *revealed*, both new gaps this stage's own reuse strategy could not
//! avoid and cases this stage's own architecture structurally improves on) rather than removed
//! outright — a future regression back to reporting any of the four `Unsupported` should read as
//! "this got worse," not as a mystery.
//!
//! A narrower, later gap closed the identical way: `contains_unsupported` went on reporting an `Html`
//! block **nested inside a `Quote`** unsupported specifically (not the unconditional case above —
//! `render_html_block_from_model` itself, once it existed, could draw a top-level one fine) until
//! `model::BlockKind::Html.body_spans` closed that too — see that field's own doc comment, and
//! `render.rs`'s own `contains_unsupported` doc comment, for the mechanism. This corpus's own
//! `"an HTML block nested inside a blockquote is drawn instead of dropped entirely"` case (`code_corpus`)
//! exercises the fixed shape, but is **not** listed in any of the three lists below: this harness's
//! own "production" side (`render_case`) calls the real dispatcher, which now routes this exact shape
//! to `render::render_doc` too, so both sides of the comparison are the same function call — an exact
//! match by construction, not a mismatch this stage's own architecture merely happens to improve on.
//! See that corpus case's own comment for the full reasoning. A quote-nested `Table` is a **still-open**
//! instance of the identical shape of gap — see `contains_unsupported`'s own doc comment for why the
//! two are not symmetric (a `Table` cell has no `Html.body_spans`-equivalent, already-marker-free range
//! pulldown-cmark reports on its own).
//!
//! Closing these four also **exposed** several corpus cases that were previously invisible to this
//! harness entirely — a document combining, say, a fenced fence-glue shape (already `KNOWN_MISMATCHES`/
//! `INTENDED_IMPROVEMENTS`-eligible on its own) with a GFM table elsewhere in the same document was
//! reported `Unsupported` as a whole *before* table support existed, so the fence-glue mismatch itself
//! was never actually measured. Table/HTML support does not introduce these — it merely lets this
//! harness see mismatches that were always latent. Four of the `INTENDED_IMPROVEMENTS` entries below
//! (`"closing line with trailing text..."` ×4) are exactly this: the identical Category 1/2 shapes the
//! rest of this list's own doc comment already documents in full, newly reachable now that a trailing
//! GFM table in the same source no longer excludes the whole case.
//!
//! ### HTML `<img>` extraction (closed — history, not a live gap)
//!
//! `render::paragraph_as_block_image` only ever recognized **Markdown** image syntax (`![alt](url)`,
//! optionally link-wrapped) — the one shape its own doc comment names explicitly. Production's own
//! `extract_block_image` additionally recognizes a standalone HTML `<img src=...>` tag (optionally
//! wrapped in layout tags like `<p>`/`<a>` — `extract_html_img`), which this stage used not to attempt
//! at all: such a line was, structurally, an ordinary `Html` leaf here, and `render_html_block_from_\
//! model` simply stripped its tags — an `<img>` tag has no text content of its own to survive that
//! stripping, so the line disappeared from the output entirely, where production shows a real "🖼 alt —
//! url" fallback (`samples: images.md`, an `<img>` wrapped in a centered `<p>` with nothing else on its
//! own line — the *simplest* shape `extract_html_img` recognizes). Closed by having
//! `render_html_block_from_model` call the identical `super::extract_block_image` production's own
//! `split_block_parts_masked` calls, checked first, before ever falling through to tag-stripping (see
//! that function's own doc comment) — reusing the extraction, not re-deriving a second one.
//!
//! ### Image extraction splitting an HTML block apart (`INTENDED_IMPROVEMENTS`)
//!
//! The "centered banner" family (`code_corpus`'s and `preprocess_corpus`'s several `"centered
//! banner..."`/`"...br..."` cases, all drawn from real registry READMEs) is a **different** shape from
//! the previous section, and the fix above does not close it — a `<div align="center">…</div>` (or
//! `<p align="center">…</p>`) wrapping *several* `<a>`/`<img>`/`<br>` elements and literal text (a `|`
//! separator, say) is one, single `Html` block in this file's own model, and `extract_block_image`
//! requires the **whole** block's own raw text to be nothing but tags (`html_is_only_tags`) — there
//! being real, visible text (or more than one link) alongside the image is exactly what keeps a
//! multi-element banner like this from qualifying, matching production's own identical, line-scoped
//! "nothing else on this line" requirement, just applied to the whole block instead of one line.
//!
//! Production's own line-based `extract_block_image`, by contrast, has no concept of "which HTML block
//! is this `<img>` line even inside of" at all — it runs over the raw source *before*
//! `split_html_blocks` ever determines block boundaries (`split_block_parts` executes first — see that
//! function's own doc comment), so pulling an `<img>`-only line out from the *middle* of what was one
//! HTML construct genuinely **splits it into fragments**, each independently re-examined by
//! `split_html_blocks` afterward. The result is a real, confirmed production defect (documented at the
//! `code_corpus` cases' own definition sites: "pulling an `<img>` line out of a centered banner splits
//! the surrounding HTML block in two, and the `    |` separator left between the halves becomes a real
//! indented code block on screen") — a stray `|` character rendered as its own bogus indented code
//! block, a `<br>`-turned-whitespace-only line ending the HTML block early and turning *everything*
//! after it into more indented code, or (in the mildest case) the block's own surviving fragments
//! rendered as scattered, disconnected pieces rather than the one coherent (if still tag-stripped,
//! `<img>`-content-blind) unit this file's own model-based approach always draws. `render_doc` reads
//! the block's own real, whole extent straight from `Doc::parse` (`model::BlockKind::Html`) and so has
//! no comparable two-pass, line-based splitting step to have this defect *in* — the identical
//! "structurally cannot reproduce a real upstream bug" reasoning `INTENDED_IMPROVEMENTS`'s own doc
//! comment already establishes for Category 1/2 below, now extended to this third mechanism.
//!
//! One family member (`"code_corpus: crlf centered banner with no <br> (registry: tinytemplate\
//! README.md)"`) is **not** in this category any more, for a reason distinct from either of the two
//! above: its own `<div>` wraps exactly one `<a>` wrapping exactly one `<img>` and nothing else —
//! `html_is_only_tags` accepts that shape (an embedded `'\n'` between tags counts as ordinary
//! whitespace, identically to a space — see `render_html_block_from_model`'s own doc comment), so the
//! *whole*-block extraction the previous section closed also happens to close this one, not by
//! reproducing production's own fragmenting-then-reassembling (which, for this particular banner,
//! nets out to the identical single image line — no other content there for the fragmenting to
//! garble), but by recognizing the same "nothing but this one image" shape a different way. Removed
//! from `INTENDED_IMPROVEMENTS` below accordingly (confirmed directly: both sides now emit the
//! identical eight lines).
//!
//! ### A GFM table swallowing a subsequent indented line (`BEHAVIOR_DECISIONS`)
//!
//! `"code_corpus: an indented line right after a table block, with no blank line between"`
//! (`"| a | b |\n|---|---|\n    code here\n"`): production's own `split_tables` requires every row
//! after the header/delimiter to still `looks_like_table_row` (contain a literal `|`) — `"    code
//! here"` does not, so the table ends at the delimiter row and the indented line becomes its own,
//! separate indented code block. `Doc::parse` (real pulldown-cmark, `ENABLE_TABLES` on) disagrees:
//! it keeps the table open and reads the indented line as one more body row (one cell, `"code here"`,
//! the second column empty) — confirmed directly, not merely inferred (`Doc::parse` on this exact
//! source reports one `Table` block whose own `rows` include a `"code here"` cell, not a separate
//! `CodeBlock` at all). Neither is "the bug": GFM's own interaction between an already-open table and a
//! subsequent indented line is not the kind of thing a hand-written, `|`-requiring line scanner and a
//! real conforming parser are guaranteed to agree on, and whether konoma should keep matching its own
//! established (arguably more intuitive — an indented line *looks* like code) table-ending heuristic or
//! switch to the real grammar's own reading is a product decision, not a defect this harness can settle
//! by picking a side — the identical standard every other `BEHAVIOR_DECISIONS` entry is held to.
//!
//! ### An HTML comment's own real end condition (`BEHAVIOR_DECISIONS`)
//!
//! `"code_corpus: an indented line swallowed by an HTML comment block is not code"`
//! (`"<!-- note -->\n    code here\n"`): the identical shape the module doc comment's own "Inline HTML
//! that interrupts a paragraph" section already documents for a different trigger (`split_html_blocks`
//! running an HTML block to the *next blank line*, wider than CommonMark's own, construct-specific end
//! conditions) — here the construct is a comment, whose own real end condition is `-->`, not a blank
//! line. Production swallows `"    code here"` into the same HTML block as the comment (no blank line
//! separates them) and renders it as rescued, tag-stripped text; `Doc::parse` ends the comment at its
//! own `-->` and reads the indented line as its own, separate, real indented code block — the same
//! spec-vs-permissive-pre-pass disagreement, over a comment instead of an inline tag, filed here rather
//! than duplicating a second, near-identical entry's worth of reasoning.
//!
//! ### A `<details>`/`<summary>` with no blank line before its own body (closed — history, not a
//! ### live gap)
//!
//! `"code_span_corpus: details/summary immediately followed by a fenced code block, no blank line"`,
//! and the two `samples` cases it also affected (`markdown.md`/`markdown.ja.md`'s own `<details open>`
//! sections, each written with no blank line between `</summary>` and the paragraph right after it):
//! CommonMark's own HTML-block continuation rule (type 6/7 — `<details>` is not one of the six special
//! tags, so it is type 6, which continues to the next blank line) does not require a blank line to
//! *start* a `<details>`'s own body — it requires one to *end the whole block*, tags included.
//! Confirmed directly: `Doc::parse` on `"<details open>\n<summary>D</summary>\n\`x\` starts
//! expanded.\n</details>\n"` (no blank line anywhere) reports one, single `Html` leaf — no *separate*,
//! later sibling `Html` leaf for `</details>` for `fold_details`'s own well-formed branch to have found
//! a close among, since pulldown-cmark's own grammar reads the whole thing as one block to begin with.
//!
//! **This is closed, not a live gap**: `fold_details` no longer requires a separate sibling to fold —
//! `glued_details_fold` (`model.rs`) recognizes the *unambiguous* instance of this same glued shape (no
//! nested `<details>` glued in, no unrelated trailing content after the found close — see that
//! function's own doc comment for exactly which cases still don't qualify, and why) and folds it into a
//! real `BlockKind::Details`, storing the raw body's own byte range (`glued_body`) rather than a
//! `children` list, since there is no finer block structure in `Doc.events` for one to point at (an
//! HTML block's own interior is copied literally, per spec — no inline events were ever emitted for it
//! at all). `render_details_from_model` (`render.rs`) renders that body via a fresh, isolated
//! `Doc::parse` — the one narrow, documented exception to this file's "never re-parses anything"
//! promise (see the module doc comment's own "How inline content is rendered" section) — immediately,
//! into plain `Line`s, never spliced back into any `Doc`'s own tree. Confirmed directly: all three cases
//! now render identically to production.
//!
//! Closing this **also exposed** a second, wholly unrelated bug in the two `samples` cases specifically
//! (not `code_span_corpus`'s own case, which has no math in it at all): once the details fold stopped
//! masking the *first* line each of those two files actually diverged on, the *next* one turned up much
//! further down — a **multi-line** `$$…$$` display-math block (`"$$\n\\int_{-\\infty}^{\\infty} \
//! e^{-x^2}\\,dx = \\sqrt{\\pi}\n$$\n"`) rendering as a genuinely empty line, not merely worse-looking
//! text — `render_dollar_math_tail`'s own growing-raw-slice scan gives up the instant it sees a soft/
//! hard break at `depth == 0` (by design — see that function's own doc comment on why a break is a real
//! stopping point for the *generic*, single-physical-line `$…$`/`$$…$$` shape it handles), so a `$$`
//! that opens a *display* block spanning several physical lines never reached a closer at all: the
//! opening `$$` replayed as ordinary literal text, and the actual expression (on the line right after
//! it) rendered as plain, unlifted prose with its own escapes silently stripped by pulldown-cmark's
//! normal escape-processed dispatch (`\,` losing its own backslash) — a real bug (this stage's own
//! `KNOWN_MISMATCHES` doc comment's own warning against cataloguing rather than fixing, precisely), not
//! a gap in coverage. `markdown.rs`'s own `split_math` has a **dedicated** branch for exactly this shape
//! — a line that is exactly `$$`/`\[` alone collects forward to a matching closer line, never touching
//! the generic per-line scan at all (see that function's own doc comment: `"Multi-line display opener"`)
//! — mirrored here by `multiline_math_opener`/`render_multiline_display_math` (`render.rs`), checked
//! *first* in `walk_inline_math`'s own dispatch, before the generic dollar-math and backslash-math
//! openers ever get a chance to give up on the first soft break. Confirmed directly: both `samples`
//! cases now match production exactly, in full.
//!
//! ## Math (closed — history, not a live gap)
//!
//! Lifted LaTeX math (`$…$`/`$$…$$`/`\(…\)`/`\[…\]`) used to be [`KNOWN_MISMATCHES`] for exactly the
//! three `code_span_corpus` cases that combine it with an indented code block — **not** because
//! `render_doc`'s own math-lifting architecture was incomplete, but because of two real, pre-existing
//! bugs in `render::render_paragraph_math`/`walk_inline_math`, present since that function was added
//! (`c39dfe3`) and confirmed with **no `CodeBlock` involved at all** (minimal repro: `"\[y\]\n"` alone,
//! and `"$$y$$\n"` alone) — simply never exercised by this harness before those three cases existed,
//! because every corpus case combining math with an indented code block was previously `Unsupported`
//! outright:
//!
//! 1. `backslash_math_opener`'s own detection read the *gap* between the previous event's end and this
//!    one's start — but `walk_inline_math` started each paragraph's own walk with `prev_end: None`, so a
//!    backslash-math expression (`\(…\)`/`\[…\]`) that is the very *first* event of its own paragraph had
//!    no previous event, in that paragraph's own local walk, to compute a gap from — it could never be
//!    detected as an opener at all and fell through as literal text (`"[y]"`, not lifted).
//! 2. `render_paragraph_math`'s own leading blank-line convention (conditional-if-`needs_newline` +
//!    unconditional, mirroring `start_paragraph`) assumed the paragraph's own first content would reach
//!    `w.lines` via `push_span` (which fills the row that convention just opened) — true for plain text,
//!    but a lift goes straight to `self.lines.extend(...)` instead (`render_math_slot`, deliberately
//!    bypassing `push_line`/`push_span` — see `Writer`'s own doc comment on `after_math`), so that opened
//!    row was left behind, genuinely empty, forever. Confirmed even for `$$…$$` math (not affected by
//!    bug 1 at all, since `scan_inline_math` finds it by substring, not by gap) — `"$$y$$\n"` alone
//!    rendered with one *extra*, orphaned blank row ahead of the placeholder that production never shows.
//!
//! **Both are real bugs**, not gaps in this stage's own scope — `render_doc`'s math-lifting output was
//! genuinely *worse* than production's (an empty line where a rendered expression belongs; a lifted
//! backslash expression left as unrendered literal text), which is precisely the shape
//! [`KNOWN_MISMATCHES`]'s own doc comment now warns against cataloguing rather than fixing. Both are
//! fixed, constructively, in `render.rs` itself:
//!
//! 1. `walk_inline_math` now takes a `block_start: usize` parameter — the byte position *conceptually*
//!    just before the walk's own first event (the owning `Block`'s own `src.start`) — and seeds
//!    `prev_end` with it instead of `None`, so a leading backslash escape computes the same non-empty
//!    gap for the walk's own first event that it already did for every later one (see that parameter's
//!    own doc comment on `walk_inline_math` for the exact mechanism, and why this closes the bug for a
//!    top-level or loose-list paragraph but not a tight list item's own synthetic first child, safely).
//! 2. The leading-blank-line convention is now **deferred**, not eager: `render_paragraph_math` records
//!    that a paragraph-start is *owed* (`Writer::pending_para_start`) rather than opening it up front,
//!    and whichever of the paragraph's own first rendered content resolves it — `open_pending_para_start`
//!    for real text (opens the same two-part convention, now lazily), `drop_pending_para_start` for a
//!    lifted expression (drops it, unopened) — matching production's own confirmed behavior exactly: a
//!    lifted expression that is a paragraph's own entire, only content gets **no** leading blank line at
//!    all, regardless of what preceded it (see `pending_para_start`'s own doc comment for the full
//!    mechanism and the production traces that pin this).
//!
//! All three `code_span_corpus` math cases match production exactly now (regression tests: `render.rs`'s
//! own `mod tests`, `math_lift_that_is_a_paragraphs_entire_content_gets_no_leading_blank_line`/
//! `backslash_math_opener_detects_a_paragraphs_own_first_event`), so this category no longer appears in
//! [`KNOWN_MISMATCHES`] at all — recorded here as history (why these bugs existed, how they were closed)
//! rather than removed outright, since a *future* regression back to the old, buggy shape should read as
//! "this got worse," not as a mystery.
//!
//! ### A third round: a backslash-escaped ASCII punctuation character *inside* the math content itself
//!
//! A later, separate pair of bugs (found once the corpus grew a case that combines math with an escape
//! *inside* the expression — `` $$a\,b$$ ``/`` \(a\,b\) ``, ordinary LaTeX, `\` before *any* ASCII
//! punctuation being CommonMark's own generic escape syntax, not a notation this file's own math
//! extension owns): the escape's own backslash is never part of *any* individual `Event::Text`'s own
//! range (pulldown-cmark's escape handling strips it out, splitting what was one unbroken run of raw
//! `src` into several separate events — the identical mechanism `backslash_math_opener`'s own doc
//! comment already documents for a leading `\(`/`\[`, now recurring *inside* an already-opened span).
//! `$…$`/`$$…$$` lost the still-open span entirely across such a split (`walk_inline_math`'s own
//! per-event `scan_inline_math` call could never see the closer, which landed in a later, separate
//! event — the expression fell through as ordinary, unlifted literal text, `out.images` empty,
//! `out.unsupported` still empty too — silently wrong, not flagged); `\(…\)`/`\[…\]` — already
//! event-spanning, unlike `$…$`/`$$…$$` — still lifted, but its own content, accumulated by
//! `push_str`-ing each intervening event's own **already escape-processed** payload, silently dropped
//! the escape's own backslash (`\(a\,b\)` became `"a,b"`, not `"a\,b"` — a real, confirmed mismatch
//! against production's own `collect_math_exprs`, which works from raw `src` and never had this
//! problem). Both fixed constructively, in `render.rs`: `render_dollar_math_tail` (new — re-runs
//! `scan_inline_math` itself over a growing **raw** `src` slice, one more event at a time, until either
//! a closer resolves it or the attempt is given up on and every buffered event replays through its own
//! normal, escape-processed dispatch) and `render_backslash_math` (its own content is now a direct raw
//! `src` slice too, bounded by two already-known byte positions, rather than an accumulation) — see each
//! function's own doc comment for the full mechanism. Pinned by `code_span_corpus`'s own escaped-math
//! cases (gated here, in this same report) and by `render.rs`'s own `mod tests`
//! (`escaped_dollar_math_lifts_instead_of_rendering_as_two_literal_fragments`/
//! `escaped_backslash_math_content_keeps_the_escapes_own_backslash`/
//! `a_dollar_that_never_closes_still_renders_literally_even_with_unrelated_escapes_nearby`) — recorded
//! here as history for the identical reason the first two bugs are, immediately above.
//!
//! `render_dollar_math_tail`'s own first version gated growth on `gap == "\\"` specifically — narrower
//! than the shape it shipped with — which hid a **real, confirmed panic**: `dollar_math_still_open`'s
//! own trigger condition fires for *any* never-closing `$` (ordinary currency prose, no escape needed at
//! all — not just the escape-splitting shape this section is otherwise about), and giving up the instant
//! a non-continuing event turned up crashed the moment that event was a `Start(tag)` (bold/italic/link)
//! whose own `range` spans past the individual nested events still left in the shared iterator —
//! `` "It costs $50 **bold** text.\n" `` panicked with `"byte range starts at X but ends at Y"`. Closed
//! by widening growth to consume *any* event (mirroring how `render_backslash_math`'s own loop already
//! avoids this, simply by never special-casing `Start`/`End` at all) and tracking nesting depth so
//! neither exit fires mid-construct — see `render_dollar_math_tail`'s own "Never stop mid-construct" doc
//! comment for the exact mechanism. Pinned by `code_span_corpus`'s own three "does not crash" cases
//! (gated here too) and `render.rs`'s own
//! `dollar_that_never_closes_does_not_panic_when_unrelated_markup_follows`/
//! `dollar_math_resolves_across_an_intervening_code_span_or_nested_markup` — the latter also records the
//! resulting improvement (a dollar-math span may now resolve across an intervening code span or nested
//! markup, matching production's own markup-blind raw-source scan).
//!
//! ## Inline HTML that interrupts a paragraph (`BEHAVIOR_DECISIONS`)
//!
//! **Inline HTML that interrupts a paragraph in a way production's own hand-written `split_html_blocks`
//! pre-pass recognizes, but CommonMark's real HTML-block grammar (and so `Doc::parse`) does not.**
//! `<span>x</span>` on its own line, directly under a paragraph with no blank line, is *not* one of
//! CommonMark's HTML-block-starting tags (not in the type-6 list, and a type-7 block explicitly cannot
//! interrupt a paragraph) — pulldown-cmark reports it as ordinary `Event::InlineHtml` *inside* that same
//! paragraph (confirmed directly against the raw event stream), which is exactly what `Doc::parse` sees
//! too — `render_doc` is CommonMark-compliant here. Production's *own*, more permissive, hand-written
//! `split_html_blocks` pre-pass intercepts it anyway, *before* the text ever reaches tui-markdown,
//! rendering it as its own separate (tag-stripped) line via `render_html_block` — a whole extra
//! pre-processing pass `render.rs`'s `Doc::parse`/`render_doc` has no equivalent of at all (nor of
//! `split_tables`/`split_alerts`/`split_details`/`split_math`, the same family — see `markdown.rs`'s own
//! `split_segments`).
//!
//! This is **not** "a gap in this stage" the way the images/mermaid section above is: `render_doc` is
//! not missing an implementation of CommonMark's own HTML-block grammar here, it is *already correct*
//! against that grammar — production is the one that deviates, on purpose, and for a real reason:
//! `split_html_blocks`'s own permissiveness is exactly what lets konoma rescue a `<details>` block or a
//! centered `<div>` banner that a strict CommonMark reading would otherwise leave as raw, tag-cluttered
//! text (see `markdown.rs`'s own doc comment on that pre-pass for the motivating cases). Picking either
//! side inside this harness would be wrong: matching production exactly would mean deliberately
//! re-implementing its *deviation* from the spec, not closing a gap; matching the spec exactly would
//! change how real `<details>`/centered-banner Markdown (README files included) actually renders once
//! `render_doc` is wired into a real preview path. That choice — and whether `render_doc` needs its own
//! `split_html_blocks`-equivalent pass before it can be, or ships as a deliberate, documented behavior
//! change — belongs to whoever makes that wiring decision, not to this diff harness; tracked in
//! [`BEHAVIOR_DECISIONS`] rather than [`KNOWN_MISMATCHES`] for exactly that reason.
//!
//! ## A GitHub alert's own flat, pre-parse extraction can change a surrounding list's tight/loose
//! ## shape (`BEHAVIOR_DECISIONS`)
//!
//! **A GitHub alert (`> [!TYPE]`) sitting inside a list item, alongside other sibling content in
//! that same item, can make production render the item *tight* while `render_doc` — following real
//! CommonMark structure — renders it *loose*, and neither side is a bug.** `markdown.rs`'s own
//! `split_alerts` runs over the **whole** top-level segment's raw text, line by line, *before*
//! tui-markdown (or `Doc::parse`) ever sees any of it — and it has no concept of list-item nesting at
//! all: it simply deletes the `>`-prefixed alert lines from the middle of the text, wherever they
//! happen to sit. For `"- item\n\n  > [!NOTE]\n  > ```sh\n  > echo hi\n  > ```\n"`, that leaves
//! tui-markdown with effectively `"- item\n\n"` to parse — a **single-item list whose only block is
//! one paragraph**, which CommonMark's own tight/loose rule makes tight (no second, blank-line-separated
//! block remains inside the item once the alert is gone) — so `"item"`'s own text lands on the very
//! same row as the `"- "` marker (confirmed directly: `code_corpus`'s own "an alert inside a list
//! item" case, `old[0] = ["- ", "item"]`, one line). `Doc::parse` never performs this kind of
//! string-level deletion at all (see the module doc comment's own "no re-parsing" invariant) — it
//! reads the **real** source structure, where the item genuinely *does* contain two block-level
//! children (the paragraph and the alert quote) separated by a blank line, which CommonMark's own
//! rule makes **loose** — so `render_item`'s own loose-first convention puts `"item"` on its own row,
//! one below the marker.
//!
//! This is not "a gap in this stage" (see [`KNOWN_MISMATCHES`]: `render_doc` already draws the alert
//! itself correctly, via `render::render_alert_from_model` — the *list item surrounding it* is what
//! differs) and not an improvement either (see [`INTENDED_IMPROVEMENTS`]: production's own shape here
//! is not a bug, it is simply a side effect of extracting the alert before any list parsing happens
//! at all) — it is the identical kind of disagreement the "Inline HTML that interrupts a paragraph"
//! section above already tracks here, just triggered by `split_alerts`'s own flat extraction instead
//! of `split_html_blocks`'s own permissive one. Resolving it — teaching `render_doc` to mimic
//! production's own pre-parse deletion (which would mean building a *second*, string-level pass that
//! violates this file's whole "read structure once, from the real parser" premise) or leaving
//! production to catch up to real CommonMark structure instead — is a product decision, not something
//! this harness can settle by picking a side.
//!
//! **The identical mechanism recurs for mermaid-fence extraction, not just alerts.**
//! `"code_corpus: a mermaid fence at an ordered item's content column is diverted to a diagram"`
//! (`"1. Diagram:\n\n   ```mermaid\n   flowchart TD\n   A-->B\n   ```\n"`): production's own
//! `split_block_parts` deletes the fence's own lines from the raw text *before* tui-markdown ever
//! parses anything, leaving `"1. Diagram:\n\n"` — a single-paragraph, **tight** item (no second block
//! remains once the fence is gone), so `"Diagram:"` lands on the marker's own row
//! (`old[0] = ["1. ", "Diagram:"]`, one line). `Doc::parse` reads the real source, where the item
//! genuinely contains two block-level children (the paragraph and the `CodeBlock`) separated by a
//! blank line — CommonMark's own rule makes that **loose**, so `render_item`'s loose-first convention
//! puts `"Diagram:"` on its own row below the marker, exactly like the alert case above. Filed
//! alongside it rather than as its own, separate entry in the list below, for the identical reason.
//!
//! ## Two categories closed structurally, not merely observed
//!
//! Two categories used to apply here — both artifacts of `render::render_doc` re-parsing each
//! `Heading`/`Paragraph`'s own byte range in isolation rather than reading from a single whole-document
//! parse: a **cross-block reference-style link** (`[text][ref]` whose `[ref]: url` definition lives in a
//! different top-level block) could never resolve that way, and *any* CommonMark construct whose
//! classification depends on context outside its own block was, in principle, at risk of coming out
//! differently when re-parsed alone. `render.rs` no longer re-parses anything at all (see its own module
//! doc comment) — every block's own inline events are read straight out of the one event stream
//! `Doc::parse` recorded for the *whole* document (`model::Doc.events`) — so both categories are gone
//! structurally, not merely unobserved; see `crate::preview::markdown::inline_corpus`'s own doc comment
//! for how the reference-link case specifically was proven to resolve, and
//! [`markdown_render_diff_report`]'s own numbers below for the (lack of) fallout from the second,
//! broader category.
//!
//! A *third* former category, **inline LaTeX math**, is gone the same structural way as of
//! `render::render_paragraph_math` (see that function's own doc comment): `render_doc` now takes the
//! same `math_slot`/`math_on` arguments `render_markdown_with_images` does, reusing that function's own
//! `scan_inline_math`/`math_url` (no second math detector) to lift `$…$`/`$$…$$`/`\(…\)`/`\[…\]` out of a
//! top-level paragraph's own text onto its own placeholder line(s), matching production line for line,
//! span for span — see that function's own doc comment for the "before text / math placeholder / after
//! text" mechanism and why it needs to reproduce a *fresh reparse boundary* at every lift point (not
//! merely "insert a line"); the two bugs the "Math" section above documents were narrower gaps *within*
//! that otherwise-working mechanism, now closed, not a regression of this category as a whole.
//!
//! Every mismatch this module actually observes is recorded in [`KNOWN_MISMATCHES`]/
//! [`INTENDED_IMPROVEMENTS`]/[`BEHAVIOR_DECISIONS`], by case name, filled in from a real run of this
//! harness (not asserted to be empty from the start — see the task this module was built for).
//! [`markdown_render_diff_report`] fails only when a *new*, previously unlisted mismatch appears among
//! the cases this stage claims to support — a regression in `render::render_doc` itself, not a gap this
//! stage already knows about and accepts, and not one of the other two lists either (see each one's own
//! doc comment — three genuinely different kinds of expected difference: a gap, an improvement, and an
//! unresolved spec-vs-production disagreement).
//!
//! ## History: this harness was a self-comparison from v0.26.0 until 2026-08-24
//!
//! From the commit that wired `render::render_doc` into `render_markdown_with_images` (the dispatcher;
//! v0.26.0) until 2026-08-24, `run_case`'s "old" side called plain `render_case`
//! (`Renderer::Dispatcher`), which itself tries `render_doc` first and only falls back to the legacy
//! renderer when `render_doc` reports part of a document `unsupported`. Because `render_doc` already
//! covered every shape this corpus exercises, that fallback never triggered, so both sides of every
//! comparison resolved to `render_doc` — this harness was silently comparing `render_doc` against
//! itself and reporting a match no matter what it actually produced (see `run_case`'s own doc comment
//! for the sentinel-poisoning proof). Fixed by anchoring the "old" side to `Renderer::Legacy` — see
//! that function's own doc comment — with
//! [`run_case_old_side_is_really_the_legacy_renderer_not_render_doc`] added as a permanent, mechanical
//! tripwire against the identical regression recurring unnoticed a second time.
//!
//! Fixing it surfaced **eleven** previously invisible mismatches, all among cases this stage claims to
//! support. Every one of the eleven was found, read, and classified by hand: ten are
//! [`INTENDED_IMPROVEMENTS`]'s own Category 4 (a plain block quote's `"> "`-every-line prefix defeating
//! production's row-based decorators — the identical root cause Category 2 already named for a
//! quote-nested fence, now confirmed for six more constructs a quote can wrap), and one is
//! [`BEHAVIOR_DECISIONS`]'s two-level-nested-quote spacing entry (both sides draw the nesting correctly;
//! they disagree only on how many spaces to put after each level's own `>`). **None of the eleven is a
//! new regression** — production's own output on all of them is unchanged by this fix; only this
//! harness's own ability to see the difference is. See each entry's own comment, at its list's own
//! definition site, for the full mechanism.

use std::fmt::Write as _;

// Brings in the same `app`-and-descendants scope `md_snapshot_tests` itself relies on (`Config`,
// `Line`/`Span`, and the `md_text` free functions `collapse_links`/`autolink_bare_urls`/
// `substitute_emoji` glob-imported into `app`'s own scope by `app.rs`'s `use md_text::*;`) — see that
// module's own doc comment for the precedent.
use super::md_snapshot_tests::{
    all_cases, pre_src_for, render_case_with, Renderer, SNAPSHOT_WIDTH,
};
use super::*;
use crate::preview::markdown::ImagePlacement;

/// **This stage genuinely has not implemented the shape yet.** Cases (by the same `"corpus: name"`
/// label `all_cases()` reports) whose entire top-level block set is one `render::render_doc` claims to
/// support (`unsupported` is empty), but whose fully post-processed output still does not match the
/// production pipeline's, and where that mismatch is a real, still-open **gap in this stage's own
/// implementation** — never a place to list `render_doc` producing something visibly *worse* than
/// production (that is a bug to fix, not a mismatch to catalogue; see the module doc comment's own
/// "Math (closed — history, not a live gap)" section for a concrete case that used to be miscategorized
/// here exactly that way, before the underlying bugs were fixed constructively). Not the same kind of
/// expected difference as [`INTENDED_IMPROVEMENTS`] (`render_doc` deliberately, permanently *better*
/// than production) or [`BEHAVIOR_DECISIONS`] (the two sides disagree about what CommonMark itself
/// says, and neither is wrong) — see each list's own doc comment. Filled in from an actual run of
/// [`markdown_render_diff_report`] (see its own `--nocapture` output), not asserted empty from the
/// start: most of this stage's job *is* measuring how many of these there are, honestly, not hiding
/// them behind a passing test.
///
/// Empty today — the one entry this list ever held (the HTML `<img>`-extraction gap the module doc
/// comment's own "HTML `<img>` extraction" section documents in full) is closed: `render_html_block_\
/// from_model` (`render.rs`) now checks `super::extract_block_image` — the identical function
/// production's own `split_block_parts_masked` calls — before falling back to plain tag-stripping, so
/// an `Html` leaf that is nothing but a standalone (optionally layout-wrapped) `<img>` tag renders as
/// a real block-image placement, matching production, not an empty line. Left declared, empty, rather
/// than removed outright — the module doc comment's own "Two categories closed structurally" and
/// "Math (closed — history, not a live gap)" sections already establish that precedent: a *future*
/// regression back to reporting an `<img>`-only `Html` leaf as a bare empty line should read as "this
/// got worse," not as a mystery this list has no memory of.
const KNOWN_MISMATCHES: &[&str] = &[];

/// **The two sides disagree about what CommonMark itself says, and neither is a bug.** Cases whose
/// fully post-processed output does not match production because `render_doc` follows the real
/// CommonMark grammar (straight off `Doc::parse`/pulldown-cmark) while production's own hand-written
/// pre-pass is *deliberately* more permissive than the spec, for a real, still-useful reason of its
/// own — not a gap in this stage (see [`KNOWN_MISMATCHES`] for that: something `render_doc` has not
/// implemented yet) and not an improvement either (see [`INTENDED_IMPROVEMENTS`]: production having a
/// confirmed bug `render_doc`'s own architecture cannot reproduce). Resolving an entry here is a
/// **product decision**, not something this harness can settle by picking a side — see each entry's
/// own doc comment for exactly what production would lose by matching the spec exactly, and vice
/// versa. Kept separate from both other lists for the identical reason they are kept separate from each
/// other: conflating "not implemented yet" with "the spec itself is ambiguous about which side is
/// right" would hide which of the two questions actually needs answering before `render_doc` could ever
/// replace production.
const BEHAVIOR_DECISIONS: &[&str] = &[
    // `split_html_blocks` interception (production's own hand-written pre-pass recognizes
    // `<span>x</span>` as its own segment, rescuing constructs like `<details>` or a centered `<div>`
    // banner that a strict CommonMark reading would otherwise leave as raw, tag-cluttered text;
    // CommonMark's real grammar — and so `Doc::parse` — does not, since a type-7 HTML block can never
    // interrupt a paragraph). See the module doc comment's own "Inline HTML that interrupts a
    // paragraph" section for the full trade-off.
    "code_corpus: an indented line swallowed by an inline-tag block that interrupts a paragraph",
    // `split_alerts`'s own flat, pre-parse deletion of a `> [!TYPE]` alert's lines changes the
    // surrounding list item's own tight/loose shape before tui-markdown ever sees it (production:
    // tight, one block left in the item; `render_doc`: loose, the real two-block structure). See the
    // module doc comment's own "A GitHub alert's own flat, pre-parse extraction..." section.
    "code_corpus: an alert inside a list item",
    // The identical mechanism, for a mermaid fence's own flat, pre-parse extraction instead of an
    // alert's — see the module doc comment's own "The identical mechanism recurs for mermaid-fence
    // extraction" paragraph, right after the alert entry's own explanation.
    "code_corpus: a mermaid fence at an ordered item's content column is diverted to a diagram",
    // A GFM table's own interaction with a subsequent indented line — `split_tables` requires a
    // literal `|` to keep the table open; `Doc::parse` reads it as one more table row instead. See
    // the module doc comment's own "A GFM table swallowing a subsequent indented line" section.
    "code_corpus: an indented line right after a table block, with no blank line between",
    // An HTML comment's own real end condition (`-->`) vs. `split_html_blocks`'s own "runs to the
    // next blank line" rule — the identical `split_html_blocks`-permissiveness disagreement as the
    // inline-tag entry above, over a comment instead. See the module doc comment's own "An HTML
    // comment's own real end condition" section.
    "code_corpus: an indented line swallowed by an HTML comment block is not code",
    // The `<details>`/`<summary>`-with-no-blank-line entries that used to be here (the
    // `code_span_corpus` case and the two `samples` files) are closed — see the module doc comment's
    // own "A `<details>`/`<summary>` with no blank line before its own body (closed — history, not a
    // live gap)" section for how, and for the separate, unrelated multi-line-math bug closing them
    // exposed and fixed along the way.
    //
    // Both sides draw a two-level-deep nested quote (`"> a\n>> b\n"`) without losing anything, but
    // disagree on how to *space* the second level's own marker: production's tui-markdown packs the
    // two `>` characters together and puts exactly one space after the pair (confirmed directly:
    // `old = [">", ">", " ", "b"]`), while `render_doc` puts a space after *each* level's own `>`
    // (`new = [">", " ", ">", " ", "b"]`) — the identical per-level spacing a single-level quote
    // already uses on both sides. Neither drops content and neither shifts a column relative to its
    // own single-level baseline; this is a formatting choice, not a bug on either side, so resolving
    // it (if ever) is a product decision for whoever wires `render_doc` into a real preview path, not
    // something this harness can settle by picking a side.
    "list_corpus: nested quotes two levels",
];

/// **`render_doc` is better than production on purpose, and is not expected to ever start matching
/// it.** Cases whose fully post-processed output does not match production **on purpose** — not a gap
/// in this stage (see [`KNOWN_MISMATCHES`] for that: something `render_doc` has not implemented yet)
/// and not an unresolved spec-vs-production disagreement either (see [`BEHAVIOR_DECISIONS`]: neither
/// side is a bug) — but this stage's own *improvement* over a real upstream crash the production
/// pipeline only partially recovers from. Kept in a list separate from `KNOWN_MISMATCHES`, deliberately,
/// because the two are not the same kind of "expected mismatch": a `KNOWN_MISMATCHES` entry is something
/// to eventually close (`render_doc` still has a gap); an `INTENDED_IMPROVEMENTS` entry is not expected
/// to *ever* start matching — the day it does, something regressed `render_doc` back down to
/// reproducing production's own bug, not fixed a gap.
///
/// ## The mechanism (verified directly, not merely quoted from the corpus comment)
///
/// Real, upstream `tui-markdown` (`tui-markdown-0.3.7`/`0.3.8`, the pinned dependency) panics parsing
/// a **loose** list item that has a task marker — the doc comment on `render.rs`'s own former
/// `contains_unsupported`'s `"LooseTask"` arm (now removed — see that function's own doc comment)
/// names the panic exactly. `render_text_block_safe` catches that panic and, rather than degrading the
/// *whole* segment to plain text, bisects it at the nearest blank line outside a fence and *retries
/// each half independently* (see that function's own doc comment) — for both cases below, the split
/// lands exactly on the blank line CommonMark itself used to decide the list is loose in the first
/// place, so each half is now a **single-item list with no blank line inside it at all**, which
/// CommonMark's own tight/loose rule makes *tight* — a shape that does not panic. So the *specific*
/// claim "production degrades to plain text" is not quite what was directly observed here: each half
/// still decorates normally (task-marker icon, styling, all present) — what actually differs is that
/// bisection has silently turned one **loose** list (its own items' markers landing on their own line,
/// separate from their bullet — see `render_item`'s own doc comment for exactly why) into *two
/// independent tight one-item lists* glued back together, each rendered as if its marker and text
/// shared one line. `render::render_doc` does not re-parse at all (see its own module doc comment) and
/// so never desyncs the list this way — it draws the one, real, loose list tui-markdown itself would
/// draw if asked to (crash aside), matching `Writer`'s own reimplementation of `TextWriter`'s real
/// (if visually surprising) loose-item layout faithfully. Confirmed directly: calling the production
/// pipeline's own `render_md_segment`/`split_block_for_retry` on each of these two sources shows the
/// whole-text parse panicking (caught) and *both* bisected halves parsing (and decorating) cleanly on
/// their own.
///
/// ## `render_code_block`'s own two improvements (added for this task's own `CodeBlock` support)
///
/// Every remaining entry below falls into one of two further categories, both real, upstream defects
/// in tui-markdown's own per-`Event::Text` rendering that `render_code_block` structurally cannot
/// reproduce (nor should it — see each one's own explanation) because it never asks tui-markdown to
/// render a code block at all, drawing it straight from the model instead
/// (`model::code_body_text(body_spans, src)`; see that function's own doc comment).
///
/// **Category 1 — a code block's own multi-line content, correctly kept one row per source line.**
/// Real tui-markdown's `text()` only inserts a fresh row *within* the physical lines of a single
/// `Event::Text` payload (`text.lines().with_position()`) — it does **not** insert one *between* two
/// separate `Event::Text` calls unless `needs_newline` happens to be `true` going in, which it never is
/// mid-code-block (`end_codeblock` alone sets it, and only once, at the very end). Whenever
/// pulldown-cmark reports a code block's content as more than one `Event::Text` — a continuation line's
/// own share of a list item's or block quote's indentation sits *between* two spans it never reports as
/// text at all (confirmed via `model::BlockKind::CodeBlock.body_spans`'s own doc comment, the same
/// reasoning `body_spans` itself is a `Vec<Range<usize>>` rather than one `Range` for), a CRLF line
/// ending, two indented chunks CommonMark glues across a blank line, or (the shape that actually
/// motivated this whole refactor) a fence nested inside a list item at all — every event after the
/// first lands glued onto the *previous* event's own row (`"line oneline two"`, `"aaabbb"`, even
/// `` "```rustfn a(){}```" `` — a fence-lookalike's own literal backtick text glued the identical way).
/// `render_code_block` reconstructs the block's own content directly from `body_spans` and splits it
/// on `'\n'` (see that function's own doc comment on "Content: reconstructed from `body_spans`, never
/// from a rendered run") — one row per source line, always, matching what `y c` (which copies the
/// file's own bytes, not the rendered screen) already gave a reader, and what a human reading the
/// *source* would expect.
///
/// One entry in this category is not, on its own, a multi-line body — `"KNOWN FAILURE: list item fence
/// glued to a following inline-code paragraph"` and its two siblings are the *header-dropping*
/// consequence `code_corpus`'s own doc comment (next to those three cases) describes in full: gluing a
/// **following** paragraph's own leading inline code span onto the fence's closing row breaks
/// `flush_code_run`'s "last row is fence characters only" check, so production draws *no header at all*
/// for that block. `render_code_block` draws the header unconditionally, straight from the model, with
/// no such check to break — and `render_bare_paragraph`'s own fix (see that function's own doc comment)
/// keeps the following paragraph itself from gluing onto the block's own last row either. Two defects,
/// one shared root cause, one shared fix.
///
/// **Category 2 — a code block nested inside a `Quote`, now drawn instead of left raw.** Production's
/// own `flush_code_run` looks for a *literal* `` "```" `` (optionally indented) as a run's own first
/// rendered row (`opening_trimmed.starts_with("```")`) — but tui-markdown's real blockquote handling
/// prepends `"> "` to *every* line while a quote is open, fence markers included, so a quote-nested
/// fence's own opening row reads `` "> ```" ``, never matches, and the whole run — opener, body, and
/// closer — is handed back completely undecorated (see `flush_code_run`'s own doc comment, and the
/// `code_corpus` comment directly above its "a plain (non-alert) block quote wrapping a fence" case).
/// `render_code_block` has no rendered-text boundary to mis-detect in the first place — it draws
/// directly from a `model::BlockKind::CodeBlock` node regardless of how many `Quote`s it sits inside,
/// which is exactly the "intentional superset" that field's own doc comment describes, made real now
/// that `contains_unsupported` no longer excludes `CodeBlock` from a `Quote`'s own subtree (see that
/// function's own doc comment). The quote's own `"> "` prefix still applies, correctly, to every row of
/// the resulting header/body/padding band (`Writer::push_line`, unchanged) — production simply never
/// gets far enough to draw a band there at all.
///
/// **Category 3 — a "centered banner" HTML block, drawn as one coherent (if `<img>`-blind) unit
/// instead of being split apart.** See the module doc comment's own "Image extraction splitting an
/// HTML block apart" section for the full mechanism: production's own `extract_block_image` runs
/// *before* `split_html_blocks` ever determines block boundaries, so pulling an `<img>`-only line out
/// of the *middle* of a multi-element HTML construct (a `<div align="center">` wrapping several
/// `<a>`/`<img>`/`<br>` elements, the idiom real registry READMEs use for a row of badges) genuinely
/// splits that construct into fragments — each re-examined independently, with real, confirmed
/// fallout (a stray `|` character misread as its own indented code block; a `<br>`-turned-blank line
/// ending the HTML block early and turning everything after it into more indented code). `render_doc`
/// reads the block's own real extent straight from `Doc::parse` and has no comparable two-pass,
/// line-based splitting step to have this defect in.
///
/// **Category 1/2 entries newly reachable via table support.** The four `"closing line with trailing
/// text..."` entries below are the identical Category 1 (multi-line glue) and Category 2
/// (quote-nested fence, undecorated in production) shapes documented above — each corpus case ends
/// with a trailing GFM table specifically so that, before this pass added `Table` support, the whole
/// case reported `Unsupported` and never reached this harness's comparison at all (see the module doc
/// comment's own "Tables, HTML blocks, ..." section).
///
/// **Category 4 — the identical `"> "`-every-line defect Category 2 already names, for six more
/// constructs a plain (non-alert) block quote can wrap.** The Category 2 entry `"code_corpus: fence
/// inside a plain block quote draws no header (tui-markdown prefixes every line)"` already names the
/// root cause for one construct nested in a quote (a fence); the same tui-markdown behavior — prepending
/// `"> "` to **every** line of an open blockquote, not just its own first line — defeats every one of
/// production's own row-based decorators the identical way, whichever construct happens to sit inside
/// the quote. Confirmed directly for each shape below:
///
/// * an `Html` block nested inside a plain quote is dropped **entirely** — zero rows — where
///   `render_doc` draws its own two lines correctly (`"code_corpus: an HTML block nested inside a
///   blockquote is drawn instead of dropped entirely"`);
/// * a task checkbox's own `[ ] ` marker is emitted as raw text **ahead of** its own list bullet
///   (confirmed: `old = [">", "[ ] ", " ", "- ", "quoted"]`), not as the styled sentinel span
///   `render_doc` draws in its place (`new = [">", " ", "- ", "\u{f096} "` (cyan, bold), `"quoted"]`) —
///   `"code_corpus: checkbox inside a plain block quote"`;
/// * a fence nested two plain-quote levels deep, or a fence inside an alert that is itself nested
///   inside a plain quote, is left as raw ` ```sh ` text — no code gutter, no language badge —
///   `"code_corpus: fence inside a nested plain block quote (two levels)"` and `"code_corpus: fence
///   inside an alert nested inside a plain block quote"`;
/// * a heading inside a plain quote keeps its own raw `# ` marker and draws no underline rule —
///   `"list_corpus: quote containing a heading"`;
/// * a thematic break inside a plain quote keeps its own raw `---` text instead of becoming a
///   full-width rule — `"list_corpus: quote containing a thematic break"`;
/// * a task checkbox inside an alert nested inside a plain quote, inside a plain quote nested two
///   levels deep, inside a bare plain quote, or inside a plain quote nested inside an alert, all break
///   the identical marker-before-bullet way the second bullet above already describes —
///   `"task_corpus: alert nested inside a plain blockquote"`, `"task_corpus: nested plain blockquote
///   (two levels)"`, `"task_corpus: plain blockquote"`, and `"task_corpus: plain blockquote nested
///   inside an alert"`.
///
/// `render_doc` walks the quote as real block structure (`Doc::parse`'s own `Quote` node, recursed into
/// exactly like the `CodeBlock`-in-`Quote` case Category 2 already covers), so every one of these draws
/// the identical way it would at the top level, prefixed correctly by the quote's own `"> "` gutter
/// (`Writer::push_line`) — it has no row-based "does this line start with a literal marker" check for
/// tui-markdown's `"> "` prefix to ever defeat. **None of these six shapes is a new regression**: they
/// were simply invisible to this harness until `run_case`'s "old" side was fixed, on 2026-08-24, to call
/// the real legacy renderer instead of comparing `render_doc` against itself — see the module doc
/// comment's own history section. Production's own behavior on all ten cases above is unchanged by that
/// fix; only this harness's ability to see it is.
const INTENDED_IMPROVEMENTS: &[&str] = &[
    "list_corpus: task item inside a loose list",
    "task_corpus: a real task at a list item's own indentation is still a real task",
    // ---- Category 1: multi-line code-block content, one row per source line (see above) ----
    "code_corpus: KNOWN FAILURE: list item fence glued to a following inline-code paragraph (no blank line) drops the header",
    "code_corpus: KNOWN FAILURE: same glue in a bullet list item",
    "code_corpus: KNOWN FAILURE: same glue, with an unrelated real fence following later in the document",
    "code_corpus: a checkbox after a fence whose closing line has trailing text (list ended by a paragraph)",
    "code_corpus: a checkbox after a fence whose closing line has trailing text (tight list)",
    "code_corpus: a details block after a fence whose closing line has trailing text",
    "code_corpus: a fence-lookalike indented 4 columns is the literal content of an indented code block",
    "code_corpus: a heading and a real fence after a fence whose closing line has trailing text",
    "code_corpus: an alert after a fence whose closing line has trailing text",
    "code_corpus: a mermaid fence indented 4 columns is left in the text and drawn as ordinary code",
    "code_corpus: a shorter closing line does not close a longer fence",
    "code_corpus: a tilde closing line does not close a backtick fence",
    "code_corpus: crlf indented block",
    "code_corpus: fence-lookalike 4 columns past a bullet item's content column is indented code",
    "code_corpus: indented block at doc start",
    "code_corpus: indented block whose content is a GFM table stays literal code",
    "code_corpus: indented block whose content is an alert header stays literal code",
    "code_corpus: indented block whose first line is plain and a later line is tag-shaped",
    "code_corpus: indented code inside a bullet item (content column + 4)",
    "code_corpus: list item fence with a two-line body (count parity only — content is checked elsewhere)",
    "code_corpus: standalone indented block",
    "code_corpus: two chunks glued by a blank separator into one code block",
    "code_span_corpus: indented code block opening with a closing tag",
    "code_span_corpus: indented code block opening with a rewritable tag",
    "code_span_corpus: indented code block opening with a tag with attributes",
    "code_span_corpus: indented code block opening with an HTML comment",
    "code_span_corpus: indented code block opening with an autolink",
    "code_span_corpus: indented code block opening with cjk in angle brackets",
    "code_span_corpus: two indented chunks separated by a blank line stay one literal block",
    "preprocess_corpus: def carrying a fence",
    "preprocess_corpus: def carrying a list",
    "task_corpus: task-lookalike inside a fence-lookalike top-level indented code block",
    // ---- Category 2: a code block nested inside a Quote, now decorated (see above) ----
    "code_corpus: a plain (non-alert) block quote wrapping a fence is not detected by either side",
    "code_corpus: block quote nested inside a list item, wrapping a fence",
    "code_corpus: block quote nested inside a list item, wrapping an indented block",
    "code_corpus: fence inside a plain block quote draws no header (tui-markdown prefixes every line)",
    "code_corpus: list item fence nested inside a plain block quote, with an inline-code continuation — masked by the pre-existing block-quote non-detection, not the glue defect",
    "code_span_corpus: indented code block inside a blockquote",
    // ---- Category 1/2, newly reachable now that a trailing GFM table no longer excludes the case ----
    "code_corpus: closing line with trailing text does not close, but the list item ends the block",
    "code_corpus: closing line with trailing text inside a block quote",
    "code_corpus: closing line with trailing text inside a list item inside a block quote",
    "code_corpus: closing line with trailing text inside a nested list item",
    // ---- Category 3: a "centered banner" HTML block split apart by image extraction (see above) ----
    "code_corpus: a centered banner whose links wrap images (registry: criterion README.md)",
    "code_corpus: centered banner preceded by a markdown block image (registry: static_assertions README.md)",
    "code_corpus: centered banner whose <img> lines are cut out of it stays HTML",
    "code_corpus: centered banner with a bare <br> line in it (registry: raw-window-metal README.md)",
    // `"code_corpus: crlf centered banner with no <br> (registry: tinytemplate README.md)"` used to be
    // here — see the module doc comment's own "Image extraction splitting an HTML block apart" section
    // for why closing HTML `<img>` extraction happened to close this one family member too, even though
    // the rest of this family stays open.
    "preprocess_corpus: bare br line inside an html block",
    "preprocess_corpus: centered banner with a bare br line (registry: raw-window-metal README.md)",
    "preprocess_corpus: crlf centered banner with no br (registry: tinytemplate README.md)",
    "preprocess_corpus: markdown block image above a centered banner (registry: static_assertions README.md)",
    // ---- Category 4: the identical "> "-every-line defect, for six more constructs a plain
    // (non-alert) block quote can wrap (see above) — surfaced only once run_case's "old" side was
    // fixed on 2026-08-24 to call the real legacy renderer instead of render_doc against itself ----
    "code_corpus: an HTML block nested inside a blockquote is drawn instead of dropped entirely",
    "code_corpus: checkbox inside a plain block quote",
    "code_corpus: fence inside a nested plain block quote (two levels)",
    "code_corpus: fence inside an alert nested inside a plain block quote",
    "list_corpus: quote containing a heading",
    "list_corpus: quote containing a thematic break",
    "task_corpus: alert nested inside a plain blockquote",
    "task_corpus: nested plain blockquote (two levels)",
    "task_corpus: plain blockquote",
    "task_corpus: plain blockquote nested inside an alert",
];

/// Renders `src` through `render::render_doc` (the block-model renderer under test) and the exact
/// same app-side post-process `render_case` applies to the production pipeline's own output
/// (`app::md_text::collapse_links`/`autolink_bare_urls`/`substitute_emoji`, gated by the same `cfg`
/// fields) — so a mismatch this module reports can only come from `render_doc` itself (or the one
/// block kind it does not model at all, front matter — see below), never from the two sides running
/// different post-processing. Returns `(lines, images, unsupported)` — `images` is
/// `RenderOut::images` verbatim (aside from the same front-matter row shift `lines` itself gets, see
/// below): `collapse_links`/`autolink_bare_urls`/`substitute_emoji` only ever touch `Line`/`Span`
/// content, never `ImagePlacement`, matching `render_case`'s own identical "post-process runs on
/// `lines`, `images` passes through untouched by it" shape exactly (that function's own body never
/// threads `images` through its three post-process calls either).
///
/// Front matter is handled identically to `render_case`'s own (private, unexported) handling of it:
/// `Doc`/`render_doc` have no concept of a metadata block at all (`pre_src_for` already stripped it
/// before `Doc::parse` ever ran — see the model module's own doc comment on what text it expects), so
/// the rendered metadata block is computed the same way and prepended the same way here, by calling
/// the exact same two functions (`strip_front_matter`/`render_front_matter`) `render_case` calls, not
/// a second copy of that four-line prepend — **and, the identical way `render_case` itself does it
/// right next to that same prepend, every placement's own `line` is shifted down by `fm_lines.len()`
/// too**, not just the rendered rows: an `ImagePlacement.line` is an index into the *final* `lines`
/// list a real preview path overlays an image onto, so a case combining front matter with an image/
/// mermaid/math placement needs this exact +N to land on the right row post-prepend, the same reason
/// `render_case`'s own `for p in &mut images { p.line += fm_lines.len(); }` exists at all.
fn render_case_new(
    cfg: &Config,
    src: &str,
) -> (Vec<Line<'static>>, Vec<ImagePlacement>, Vec<&'static str>) {
    let icons = cfg.ui.icons;
    let code = crate::preview::markdown::CodeStyle {
        bg: cfg.ui.theme.code_bg(),
        label_bg: cfg.ui.theme.code_label_bg(),
        label_right: cfg.ui.theme.code_label_right(),
        tab_width: cfg.ui.tab_width,
        wrap: cfg.ui.wrap,
    };
    let theme = cfg.ui.theme.code_theme.as_str();
    let tasks = cfg.ui.md_task_state_chars();

    let fm_lines = if cfg.ui.md_frontmatter {
        match crate::preview::markdown::strip_front_matter(src) {
            (Some(fm), _) => crate::preview::markdown::render_front_matter(&fm, SNAPSHOT_WIDTH),
            (None, _) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Same fixed-answer image/mermaid/math slots `md_snapshot_tests::render_case` itself uses (see
    // that function's own doc comment): no live `Picker` in a unit test, so every expression/fence/
    // image "renders" to its own picker-independent placeholder answer — extraction and placement
    // logic is still exercised end to end, just not the picker-dependent raster step. `math_on: true`
    // matches the default `math = "image"` config; `mermaid_caption: "mermaid"` is the exact literal
    // `render_case` passes (not a translated string — this harness never varies `ui.lang`).
    let slot_of = |_: &str| crate::preview::markdown::ImageSlot::Unavailable;
    let mermaid_slot = |_: &str| crate::preview::markdown::MermaidSlot::Image { cols: 20, rows: 5 };
    let math_slot = |_: &str, _: bool| crate::preview::markdown::MathSlot::Raw;

    let pre_src = pre_src_for(cfg, src);
    let doc = crate::preview::markdown::model::Doc::parse(&pre_src);
    let out = crate::preview::markdown::render::render_doc(
        &doc,
        &pre_src,
        SNAPSHOT_WIDTH,
        code,
        theme,
        icons,
        &tasks,
        &slot_of,
        &mermaid_slot,
        "mermaid",
        // `cfg.ui.md_alerts` — matching `render_case`'s own identical argument to
        // `render_markdown_with_images` (the "old"/dispatcher side of this same comparison; see that
        // function's own call site) — not a hardcoded `true`: with `render_markdown_with_images` no
        // longer falling back to the legacy renderer just because `alerts` is off (see that
        // function's own doc comment for the narrowed fallback), a corpus case run with `md_alerts =
        // false` needs *both* sides of this comparison honoring the identical config, not one side
        // silently pinned to the default.
        cfg.ui.md_alerts,
        &math_slot,
        true,
    );

    let mut images = out.images;
    if !fm_lines.is_empty() {
        for p in &mut images {
            p.line += fm_lines.len();
        }
    }

    let mut lines = fm_lines;
    lines.extend(out.lines);

    let (lines, targets) = collapse_links(lines, icons);
    let (lines, _targets) = if cfg.ui.md_autolink {
        autolink_bare_urls(lines, targets)
    } else {
        (lines, targets)
    };
    let lines = if cfg.ui.md_emoji {
        substitute_emoji(lines)
    } else {
        lines
    };

    (lines, images, out.unsupported)
}

/// One case's outcome: unsupported (some top-level block kind `render_doc` does not render at all —
/// see the module doc comment), an exact match against the production pipeline on **both** `lines`
/// and `images` (see the module doc comment's own opening paragraph for why a match requires both),
/// or a mismatch, with enough of a diagnostic (`{:?}` of whichever of the two differed — possibly
/// both; `ratatui::text::Line` has a hand-written `Debug` impl, see `ratatui_core::text::line`, and
/// `ImagePlacement` derives one plainly) to see what differs without dumping the whole case.
enum Outcome {
    Unsupported,
    Match,
    Mismatch { first_diff: String },
}

/// The first index at which `a`/`b` differ (comparing by `PartialEq`), or `None` if one is a strict
/// prefix of the other (`min(a.len(), b.len())`, reported as the point of divergence too). Generic
/// over the element type so the one function serves both comparisons `run_case` makes — `Line`
/// (`Line`/`Span`/`Style` all derive `PartialEq`, so this is exact: content, span boundaries, and
/// every style field) and `ImagePlacement` (`url`/`alt`/`line`/`cols`/`rows`/`fence_ord`, likewise
/// exact via its own derived `PartialEq`) — rather than a second, hand-copied near-duplicate of the
/// identical index-walk for the second type.
fn first_diff_index<T: PartialEq>(a: &[T], b: &[T]) -> Option<usize> {
    let n = a.len().min(b.len());
    (0..n)
        .find(|&i| a[i] != b[i])
        .or(if a.len() != b.len() { Some(n) } else { None })
}

/// Formats one `{label}` diagnostic (`"line"`/`"image placement"` — see `run_case`'s own two call
/// sites) at divergence index `i`, generic over the element type for the identical reason
/// `first_diff_index` is (`Debug` is all this needs — every actual argument is `{:?}`-printed, never
/// compared directly here).
fn diff_message<T: std::fmt::Debug>(label: &str, old: &[T], new: &[T], i: usize) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "    first differing {label}: {i} (old has {} {label}(s), new has {} {label}(s))",
        old.len(),
        new.len()
    )
    .unwrap();
    writeln!(s, "    old {label}[{i}] = {:?}", old.get(i)).unwrap();
    writeln!(s, "    new {label}[{i}] = {:?}", new.get(i)).unwrap();
    s
}

/// Runs one case through both renderers and combines the two independent comparisons
/// `first_diff_index` makes (`lines` and `images` — see the module doc comment's own opening
/// paragraph for why both are required for `Outcome::Match`) into one `Outcome`. When only one of
/// the two actually diverges the diagnostic reports only that one (an unchanged `images` list next
/// to a genuine `lines` mismatch would just be noise); when both diverge, both diagnostics are
/// concatenated, so a case combining, say, a real `render_doc` regression in its rendered rows with
/// a stale `ImagePlacement.line` from the same underlying cause is not silently reported as only one
/// kind of mismatch.
///
/// The "old" side is `render_case_with(cfg, src, Renderer::Legacy)` — **never** `render_case`/
/// `Renderer::Dispatcher`. This was not always true, and the difference is not cosmetic: from the
/// commit that wired `render::render_doc` into `render_markdown_with_images` (the dispatcher; v0.26.0)
/// until this module switched to `Renderer::Legacy`, `run_case` called plain `render_case`, whose
/// `render_markdown_with_images` call tries `render_doc` first and only falls back to the legacy
/// renderer when `render_doc` itself reports `unsupported`. Because `render_doc` covers every shape
/// this corpus exercises (`unsupported` is empty on all 468 cases — see the module doc comment), that
/// fallback never triggered, so the dispatcher *always* resolved to `render_doc` on both sides: this
/// function was comparing `render_doc`'s own output against itself and reporting a match no matter
/// what `render_doc` actually produced. Confirmed by deliberately poisoning `render_paragraph`'s output
/// with a sentinel line and re-running `markdown_render_diff_report`: it still printed 468/468 exact
/// matches, 0 mismatches — while the golden snapshot tests (`markdown_render_snapshot_default`/
/// `_code_bg_none`, which pin `Renderer::Dispatcher`'s real output, not a self-comparison) correctly
/// failed on the same poisoned build. `Renderer::Legacy` calls
/// `render_markdown_with_images_legacy` directly, unconditionally, bypassing the dispatcher's
/// `render_doc`-first logic entirely — the one implementation that cannot itself start resolving to
/// `render_doc` out from under this comparison no matter how much of the corpus `render_doc` comes to
/// cover. See [`Renderer`]'s own doc comment for the full mechanism.
fn run_case(cfg: &Config, src: &str) -> (Outcome, Vec<Line<'static>>, Vec<Line<'static>>) {
    let old = render_case_with(cfg, src, Renderer::Legacy);
    let (new_lines, new_images, unsupported) = render_case_new(cfg, src);
    if !unsupported.is_empty() {
        return (Outcome::Unsupported, old.lines, new_lines);
    }
    let lines_diff = first_diff_index(&old.lines, &new_lines);
    let images_diff = first_diff_index(&old.images, &new_images);
    if lines_diff.is_none() && images_diff.is_none() {
        return (Outcome::Match, old.lines, new_lines);
    }
    let mut first_diff = String::new();
    if let Some(i) = lines_diff {
        first_diff.push_str(&diff_message("line", &old.lines, &new_lines, i));
    }
    if let Some(i) = images_diff {
        first_diff.push_str(&diff_message(
            "image placement",
            &old.images,
            &new_images,
            i,
        ));
    }
    (Outcome::Mismatch { first_diff }, old.lines, new_lines)
}

/// Runs every case in the parity corpus through both renderers and prints an honest tally — see the
/// module doc comment for the report's exact shape and what each mismatch category means. Fails only
/// when a case not already in [`KNOWN_MISMATCHES`], [`INTENDED_IMPROVEMENTS`], *or*
/// [`BEHAVIOR_DECISIONS`] mismatches; a case in any of the three that now matches is reported (via
/// `--nocapture`) as a stale entry, not a failure — this stage's job is to measure honestly, and a
/// shrinking mismatch list is progress, not something to guard against. All three lists are tracked
/// (and reported) separately throughout — see each one's own doc comment for why conflating them would
/// hide the distinction that is the whole point of having three: a gap to close, an improvement never
/// expected to close, and a spec-vs-production disagreement that needs a human decision, not code, to
/// close.
#[test]
fn markdown_render_diff_report() {
    let cfg = Config::default();
    let cases = all_cases();
    assert!(
        cases.len() > 100,
        "the parity corpus looks suspiciously small ({} cases) — a corpus-gathering call probably broke",
        cases.len()
    );

    let mut unsupported = 0usize;
    let mut matched = 0usize;
    let mut mismatched: Vec<(String, String)> = Vec::new(); // (case name, diagnostic)
    let mut seen_known: Vec<&'static str> = Vec::new();
    let mut seen_improvements: Vec<&'static str> = Vec::new();
    let mut seen_behavior: Vec<&'static str> = Vec::new();

    for (name, src) in &cases {
        let (outcome, _old, _new) = run_case(&cfg, src);
        match outcome {
            Outcome::Unsupported => unsupported += 1,
            Outcome::Match => {
                if let Some(k) = KNOWN_MISMATCHES.iter().find(|k| **k == name.as_str()) {
                    seen_known.push(k);
                }
                if let Some(k) = INTENDED_IMPROVEMENTS.iter().find(|k| **k == name.as_str()) {
                    seen_improvements.push(k);
                }
                if let Some(k) = BEHAVIOR_DECISIONS.iter().find(|k| **k == name.as_str()) {
                    seen_behavior.push(k);
                }
                matched += 1;
            }
            Outcome::Mismatch { first_diff } => mismatched.push((name.clone(), first_diff)),
        }
    }

    let supported = cases.len() - unsupported;
    let known_count = mismatched
        .iter()
        .filter(|(n, _)| KNOWN_MISMATCHES.contains(&n.as_str()))
        .count();
    let improvement_count = mismatched
        .iter()
        .filter(|(n, _)| INTENDED_IMPROVEMENTS.contains(&n.as_str()))
        .count();
    let behavior_count = mismatched
        .iter()
        .filter(|(n, _)| BEHAVIOR_DECISIONS.contains(&n.as_str()))
        .count();
    let new_count = mismatched.len() - known_count - improvement_count - behavior_count;

    eprintln!("=== new-renderer diff report ===");
    eprintln!("  total cases: {}", cases.len());
    eprintln!("  unsupported (outside this stage's block-kind coverage): {unsupported}");
    eprintln!("  supported (this stage claims to cover): {supported}");
    eprintln!("    exact match: {matched}");
    eprintln!(
        "    mismatch: {} (known gaps: {known_count}, intended improvements: {improvement_count}, \
         behavior decisions: {behavior_count}, NEW: {new_count})",
        mismatched.len()
    );
    if !mismatched.is_empty() {
        eprintln!("  mismatches (case, first differing line):");
        for (name, diag) in &mismatched {
            let label = if KNOWN_MISMATCHES.contains(&name.as_str()) {
                "known"
            } else if INTENDED_IMPROVEMENTS.contains(&name.as_str()) {
                "improvement"
            } else if BEHAVIOR_DECISIONS.contains(&name.as_str()) {
                "behavior"
            } else {
                "NEW"
            };
            eprintln!("  - [{label}] {name}");
            eprint!("{diag}");
        }
    }
    let stale_known: Vec<&&str> = KNOWN_MISMATCHES
        .iter()
        .filter(|k| !seen_known.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale_known.is_empty() {
        eprintln!("  stale KNOWN_MISMATCHES entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale_known {
            eprintln!("  - {k}");
        }
    }
    let stale_improvements: Vec<&&str> = INTENDED_IMPROVEMENTS
        .iter()
        .filter(|k| !seen_improvements.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale_improvements.is_empty() {
        eprintln!("  stale INTENDED_IMPROVEMENTS entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale_improvements {
            eprintln!("  - {k}");
        }
    }
    let stale_behavior: Vec<&&str> = BEHAVIOR_DECISIONS
        .iter()
        .filter(|k| !seen_behavior.contains(k))
        .filter(|k| !mismatched.iter().any(|(n, _)| n == **k))
        .collect();
    if !stale_behavior.is_empty() {
        eprintln!("  stale BEHAVIOR_DECISIONS entries (not found among this run's cases at all — corpus renamed/removed?):");
        for k in &stale_behavior {
            eprintln!("  - {k}");
        }
    }
    eprintln!("=== end report ===");

    let new_mismatches: Vec<&str> = mismatched
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| {
            !KNOWN_MISMATCHES.contains(n)
                && !INTENDED_IMPROVEMENTS.contains(n)
                && !BEHAVIOR_DECISIONS.contains(n)
        })
        .collect();
    assert!(
        new_mismatches.is_empty(),
        "{} new (unlisted) mismatch(es) between render::render_doc and the production renderer: {:?}\n\
         If this is an intentional, understood gap this stage has not implemented yet (see this \
         module's own doc comment for the accepted categories), add the case name(s) to \
         KNOWN_MISMATCHES; if it is a deliberate improvement over a real upstream bug (see \
         INTENDED_IMPROVEMENTS's own doc comment), add it there instead; if the two sides disagree \
         about what CommonMark itself says and neither is wrong (see BEHAVIOR_DECISIONS's own doc \
         comment), add it there instead. If render_doc is genuinely worse than production — an empty \
         line, dropped structure, a panic — that is a real bug: fix it, don't list it anywhere.",
        new_mismatches.len(),
        new_mismatches
    );
}

/// Sanity check on the report harness itself, mirroring `markdown_render_snapshot_corpus_is_not_empty`
/// in `md_snapshot_tests`: if `all_cases()` silently broke, `markdown_render_diff_report` would still
/// "pass" (zero cases in, zero mismatches out), defeating the whole point of this module.
#[test]
fn markdown_render_diff_corpus_is_not_empty() {
    assert!(all_cases().len() > 100);
}

/// **Guard against the exact regression this module suffered once already**: `run_case`'s "old" side
/// silently resolving to `render_doc` (the *new* renderer) instead of the legacy one, turning the
/// whole diff report into a self-comparison that always "passes" no matter what `render_doc` actually
/// produces — see `run_case`'s own doc comment for the full history (live, undetected, from the
/// v0.26.0 commit that wired `render_doc` into the dispatcher, until it was found and fixed on
/// 2026-08-24). That doc comment explains *why* `run_case` calls `Renderer::Legacy`; this test is the
/// mechanical tripwire that keeps it that way — if a future edit ever swaps `Renderer::Legacy` back to
/// `Renderer::Dispatcher` (or plain `render_case`) inside `run_case`, this test fails immediately. Same
/// role `code_block_scanner_matches_renderer_across_indented_code_corpus`'s own
/// `at_least_one_legacy_routed` assertion (`src/preview/markdown.rs`) plays for a sibling harness with
/// the identical shape of hollowing-out risk: a comparison whose "other side" quietly stops being
/// exercised once some threshold of corpus coverage closes.
///
/// The fixture is a real, still-open gap between the two renderers, not a hypothetical: a pipe table
/// nested inside a block quote. Confirmed directly (not assumed) against both renderers on this exact
/// input:
/// * `render::render_doc` reports the whole enclosing quote `unsupported` (its `Quote` support does not
///   recurse into a nested `Table` yet — see that module's own doc comment on scope), so
///   `render_markdown_with_images` (`Renderer::Dispatcher`) falls back to
///   `render_markdown_with_images_legacy` for this input too, same as `Renderer::Legacy` calls
///   directly — this fixture's assertions hold under either variant name, which is exactly why a
///   regression back to `Dispatcher` would *not* make this test fail via a different code path than the
///   one it exists to catch.
/// * `render_markdown_with_images_legacy` (`tui-markdown`'s block-quote handling) does not detect pipe-\
///   table syntax nested under `>` at all: it renders the quote as an ordinary paragraph, so the
///   source's own `|` characters and `---` separator survive verbatim in the flattened output, and none
///   of the box-drawing glyphs a real ruled table would use (`┌`/`┐`/`└`/`┘`/`│`/`─`) appear anywhere.
#[test]
fn run_case_old_side_is_really_the_legacy_renderer_not_render_doc() {
    let cfg = Config::default();
    let src = "> | a | b |\n> |---|---|\n> | 1 | 2 |\n";
    let old = render_case_with(&cfg, src, Renderer::Legacy);
    let flat = flatten_all(&old.lines);
    assert!(
        flat.contains('|'),
        "expected the legacy renderer's raw, un-tabled pipe text to survive in a quoted table; got: {flat:?}"
    );
    for glyph in ['┌', '┐', '└', '┘', '│', '─'] {
        assert!(
            !flat.contains(glyph),
            "found a ruled-table box-drawing glyph ({glyph:?}) in `Renderer::Legacy` output — that would \
             be `render_doc`'s own table rendering, not tui-markdown's, meaning `run_case`'s \"old\" side \
             has regressed back to comparing the new renderer against itself. Full output: {flat:?}"
        );
    }
}

/// **Second, complementary tripwire — this one actually calls `run_case`.** The test directly above
/// (`run_case_old_side_is_really_the_legacy_renderer_not_render_doc`) proves the *mechanism* exists:
/// `render_case_with(&cfg, src, Renderer::Legacy)`, called directly, really does return legacy, un-
/// tabled output. It does **not** prove `run_case` itself uses that mechanism — it never calls
/// `run_case` at all. Confirmed directly, not assumed: temporarily editing `run_case`'s own `let old =
/// render_case_with(cfg, src, Renderer::Legacy);` line to read `Renderer::Dispatcher` instead (the
/// exact regression this module's own doc comment history section describes, and the exact one the
/// test above exists to catch) and re-running `cargo test --bin konoma -- \
/// run_case_old_side_is_really_the_legacy_renderer` under that mutation still printed `test result:
/// ok. 1 passed; 0 failed` — the mutation sailed straight through, because the test above never routes
/// through `run_case`'s own code path in the first place. **This test closes that hole by calling
/// `run_case` itself.**
///
/// The fixture is `task_corpus`'s own `"plain blockquote"` case (`"> - [ ] quoted\n"`,
/// `src/preview/markdown.rs`), picked for two properties, both confirmed directly against the real
/// corpus and both renderers, not inferred from a list entry's prose alone:
/// * `render_doc` **supports** this shape — a bare `Quote{alert: None}` containing a task-list item is
///   squarely inside this stage's documented scope (see the module doc comment's own opening
///   paragraph) — so `run_case` does not short-circuit to `Outcome::Unsupported` before ever reaching
///   the comparison this test needs exercised. A case that returned `Unsupported` would prove nothing
///   about which renderer `old` resolves to.
/// * The two renderers are **known, permanently** to disagree on it — this exact case name is listed in
///   [`INTENDED_IMPROVEMENTS`] ("Category 4": a plain block quote's `"> "`-every-line prefix defeats
///   production's row-based decorator placement). Measured directly: `old` (legacy) emits `[ ] ` as
///   ordinary text *ahead of* the bullet marker (`old.lines[0]` flattens to `["> ", "[ ] ", " ", "- ",
///   "quoted"]`-shaped spans), while `render_doc` reads the real block structure and draws a proper
///   cyan/bold checkbox glyph *after* the marker instead. Because this divergence is structural (rooted
///   in `split_segments`'s own row-based, `"> "`-stripping pre-pass — not something either side is ever
///   expected to fix), a correctly-wired `run_case` (whose `old` side genuinely reaches
///   `render_markdown_with_images_legacy`) can **never** report `Outcome::Match` on this input.
///
/// So: if `run_case`'s own `old` binding is really `Renderer::Legacy`, this fixture must come back as
/// `Outcome::Mismatch` (the known, `INTENDED_IMPROVEMENTS`-tracked one) — never `Outcome::Match`. If
/// `old` has instead regressed back to `Renderer::Dispatcher` (or plain `render_case`), both sides
/// collapse onto `render_doc` again (see this module's own doc comment's "History" section) and this
/// fixture, like every other corpus case during that regression, reports a spurious `Outcome::Match`,
/// which is exactly what this test asserts against.
///
/// **If this test fails, the first thing to suspect is `run_case`'s own `old` binding having drifted
/// away from `Renderer::Legacy`** — not this fixture (it is pinned, real corpus data — see
/// `task_corpus` in `markdown.rs`) and not `render_doc`'s own quote/task-list rendering
/// ([`INTENDED_IMPROVEMENTS`] already tracks that divergence as permanent and deliberate, not a bug to
/// chase from this test).
#[test]
fn run_case_actually_routes_through_the_legacy_renderer_not_just_render_case_with_directly() {
    let cfg = Config::default();
    let src = "> - [ ] quoted\n";
    let (outcome, old_lines, new_lines) = run_case(&cfg, src);
    match outcome {
        Outcome::Match => panic!(
            "run_case reported Outcome::Match on \"task_corpus: plain blockquote\" \
             (\"> - [ ] quoted\\n\"), which is listed in INTENDED_IMPROVEMENTS as a permanent, \
             confirmed divergence between the legacy renderer and render_doc. A Match here means \
             run_case's \"old\" side is no longer really Renderer::Legacy (both sides have collapsed \
             onto render_doc) — see this test's own doc comment for the full mechanism. \
             old lines: {old_lines:?}\nnew lines: {new_lines:?}"
        ),
        Outcome::Unsupported => panic!(
            "run_case reported Outcome::Unsupported for \"> - [ ] quoted\\n\" — a bare blockquote \
             containing a task item is squarely inside render_doc's documented scope \
             (Quote{{alert: None}}), so this fixture is expected to reach the comparison, not bail out \
             early. If this genuinely regressed, that is a real render_doc bug to fix, not something \
             for this test to tolerate silently."
        ),
        Outcome::Mismatch { .. } => {
            // Expected, and the whole point: this is the known, permanent divergence
            // INTENDED_IMPROVEMENTS tracks under "task_corpus: plain blockquote". run_case must keep
            // observing it — a future edit that makes it disappear (Match) is exactly the regression
            // this test exists to catch, not something to relax the assertion around.
        }
    }
}

// -------------------------------------------------------------------------------------------
// Self-contained regressions for the block-image extraction fixes `md_real_file_sweep_tests`'s
// own real-registry sweep found and closed. That sweep needs a populated Cargo registry cache and
// is `#[ignore]`d for it (see its own module doc comment); these tests need neither — a handful of
// small, synthetic, hand-written documents reproducing the exact shape each real file that sweep
// found losing content used, run on every `cargo test`, using `render_case_new` directly (the same
// helper the diff-report cases above build on) rather than the full match/mismatch/content-loss
// machinery that module runs. `Config::default()` throughout: none of these shapes depend on any
// non-default `[ui]` setting. `render_case_new`'s own `slot_of` always answers `ImageSlot::\
// Unavailable` (no live `Picker` in a unit test — that function's own doc comment) — `render_image_\
// slot` never pushes an `ImagePlacement` for that slot (only for a real, picker-backed `Inline` one;
// see that function's own body), it draws `image_text_fallback`'s own `"🖼 <alt> — <url>"` text line
// instead — so these check the **rendered lines'** own flattened text for that `alt`/`url` pair,
// not the (always-empty, in this harness) `images` vector.

/// Every span's content concatenated, across every line, with no separator — enough to check
/// substring presence/absence without caring exactly which line or span something landed on.
fn flatten_all(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.as_ref())
        .collect()
}

/// `render.rs`'s `segment_as_block_images`/`split_top_level_image_units` — two badges sharing one
/// physical source line, separated only by a space, both extract as their own images (previously:
/// neither did, and the whole paragraph fell through to ordinary inline rendering, discarding both
/// URLs — confirmed real: `phf-0.11.3/README.md`'s own `[![CI](…badge.svg)](…) [![Latest Version]\
/// (…svg)](…)`, both badges on one line).
#[test]
fn two_badges_sharing_one_physical_line_both_extract_as_images() {
    let cfg = Config::default();
    let src = "[![CI](https://x.example/ci.svg)](https://x.example/ci) [![Docs](https://x.example/docs.svg)](https://x.example/docs)\n";
    let (lines, _images, unsupported) = render_case_new(&cfg, src);
    assert!(unsupported.is_empty(), "unsupported: {unsupported:?}");
    let text = flatten_all(&lines);
    assert!(
        text.contains("CI") && text.contains("https://x.example/ci.svg"),
        "{text:?}"
    );
    assert!(
        text.contains("Docs") && text.contains("https://x.example/docs.svg"),
        "{text:?}"
    );
    // Neither badge's own *wrapping link* `href` (as opposed to the image's own `src` above) is
    // ever shown as visible text — `render_image_slot`'s own doc comment — so this also confirms
    // the fix did not accidentally start showing it.
    assert!(!text.contains("https://x.example/ci)"), "{text:?}");
}

/// `render.rs`'s `segment_as_block_images`'s own empty-segment handling — a bare `\` hard-break line
/// between two rows of badges (`top_level_segment_bounds` reports the `\` line's own escape-newline
/// as one segment with *nothing* between two consecutive break events) does not disqualify either
/// row (previously: the whole paragraph — badges on both sides of the `\` line — fell through to
/// ordinary inline rendering, discarding every one of their URLs; confirmed real: `vello_common-\
/// 0.0.9/README.md`'s own six-badge banner, split into two rows of three by exactly this line).
#[test]
fn a_bare_backslash_hard_break_between_two_rows_of_badges_does_not_disqualify_either_row() {
    let cfg = Config::default();
    let src = "[![A](https://x.example/a.svg)](https://x.example/a)\n\\\n[![B](https://x.example/b.svg)](https://x.example/b)\n";
    let (lines, _images, unsupported) = render_case_new(&cfg, src);
    assert!(unsupported.is_empty(), "unsupported: {unsupported:?}");
    let text = flatten_all(&lines);
    assert!(text.contains("https://x.example/a.svg"), "{text:?}");
    assert!(text.contains("https://x.example/b.svg"), "{text:?}");
}

/// `render.rs`'s `split_top_level_image_units`'s own comment-skipping arm — an HTML comment trailing
/// a badge on the *same* physical line does not disqualify the badge (previously: the whole
/// paragraph fell through to ordinary inline rendering; confirmed real: `rangemap-1.7.1/README.md`'s
/// own `[![Rust](…)](…) <!-- Don't forget to update the GitHub actions config… -->`).
#[test]
fn a_trailing_html_comment_on_a_badge_line_does_not_disqualify_the_badge() {
    let cfg = Config::default();
    let src =
        "[![Rust](https://x.example/rust.svg)](https://x.example/rust) <!-- keep this in sync -->\n";
    let (lines, _images, unsupported) = render_case_new(&cfg, src);
    assert!(unsupported.is_empty(), "unsupported: {unsupported:?}");
    let text = flatten_all(&lines);
    assert!(
        text.contains("Rust") && text.contains("https://x.example/rust.svg"),
        "{text:?}"
    );
    assert!(!text.contains("keep this in sync"), "{text:?}");
}

/// `render.rs`'s `heading_trailing_images` — an ATX heading whose own title text is directly
/// followed, on the *same* physical line, by one or more reference-style badges, extracts them as
/// real images and keeps the title text (previously: `walk_inline`'s generic `Tag::Image` handling
/// discarded every badge's own URL unconditionally, heading or not; confirmed real: `rav1e-0.8.1/\
/// README.md`'s own `# rav1e [![Actions Status][actions badge]][actions] [![CodeCov][codecov \
/// badge]][codecov]`).
#[test]
fn a_heading_with_trailing_badges_on_the_same_line_extracts_them_as_images() {
    let cfg = Config::default();
    let src =
        "# rav1e [![Actions][actions badge]][actions] [![CodeCov][codecov badge]][codecov]\n\n\
        [actions badge]: https://x.example/actions.svg\n\
        [actions]: https://x.example/actions\n\
        [codecov badge]: https://x.example/codecov.svg\n\
        [codecov]: https://x.example/codecov\n";
    let (lines, _images, unsupported) = render_case_new(&cfg, src);
    assert!(unsupported.is_empty(), "unsupported: {unsupported:?}");
    let heading_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(heading_text.starts_with("rav1e"), "{heading_text:?}");
    let text = flatten_all(&lines);
    assert!(
        text.contains("Actions") && text.contains("https://x.example/actions.svg"),
        "{text:?}"
    );
    assert!(
        text.contains("CodeCov") && text.contains("https://x.example/codecov.svg"),
        "{text:?}"
    );
}

/// `render.rs`'s `heading_trailing_images` again — a **setext** heading whose title text is lazily
/// continued (no blank line anywhere) by standalone badge lines, then the `====`/`----` underline,
/// extracts the badges as real images and keeps the plain title text (previously: a real, narrower
/// gap this same test module's own history left deliberately open, closed once this sweep confirmed
/// it real rather than left open a second time; confirmed real: `ab_glyph-0.2.32/README.md`'s own
/// `ab_glyph\n[![crates.io](…)](…)\n[![Documentation](…)](…)\n========`).
#[test]
fn a_setext_heading_lazily_continued_by_badge_lines_extracts_them_as_images() {
    let cfg = Config::default();
    let src = "ab_glyph\n[![crates.io](https://x.example/crates.svg)](https://x.example/crates)\n[![Docs](https://x.example/docs.svg)](https://x.example/docs)\n========\n";
    let (lines, _images, unsupported) = render_case_new(&cfg, src);
    assert!(unsupported.is_empty(), "unsupported: {unsupported:?}");
    let heading_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(heading_text.trim(), "ab_glyph");
    let text = flatten_all(&lines);
    assert!(
        text.contains("crates.io") && text.contains("https://x.example/crates.svg"),
        "{text:?}"
    );
    assert!(
        text.contains("Docs") && text.contains("https://x.example/docs.svg"),
        "{text:?}"
    );
}

/// `render.rs`'s `paragraph_leading_images` — the mirror of `paragraph_trailing_images`: one or more
/// standalone image lines *first*, real text lazily continuing right after with no blank line —
/// extracts the leading badge as a real image and keeps the following sentence (previously: this
/// exact mirror shape was named, and deliberately left unaddressed, in `paragraph_trailing_images`'s
/// own doc comment; closed here once confirmed real rather than left open a second time; confirmed
/// real: `rusty-fork-0.3.1/README.md`'s own `[![](http://meritbadge.herokuapp.com/rusty-fork)]\
/// (https://crates.io/crates/rusty-fork)\nRusty-fork provides a way to "fork" unit tests…`).
#[test]
fn a_leading_badge_immediately_followed_by_real_text_extracts_the_badge_and_keeps_the_text() {
    let cfg = Config::default();
    let src = "[![](https://x.example/badge.svg)](https://x.example/badge)\nReal intro sentence follows immediately.\n";
    let (lines, _images, unsupported) = render_case_new(&cfg, src);
    assert!(unsupported.is_empty(), "unsupported: {unsupported:?}");
    let text = flatten_all(&lines);
    assert!(text.contains("https://x.example/badge.svg"), "{text:?}");
    assert!(
        text.contains("Real intro sentence follows immediately."),
        "{text:?}"
    );
}
