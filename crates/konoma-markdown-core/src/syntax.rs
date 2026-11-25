//! Markdown source analysis that needs no rendering: the pre-passes over the document text (front
//! matter, footnotes, inline HTML), the code/structure masks, `<details>` and alert splitting, and
//! the parser options every scanner shares.

// The parser's own option type keeps the name (`ParseOptions`) it was given while konoma's
// renderer sat on top of `tui-markdown` — see `markdown_parse_options`, which is now konoma's own
// option set (no upstream crate to stay in lockstep with).
use pulldown_cmark::Options as ParseOptions;

/// A run of Markdown source text plus, for each of its lines, whether the **document** placed that
/// line inside a code block.
///
/// The mask travels with the text instead of each splitter re-deriving it, because by the time most
/// of these splitters run, the text they hold is no longer a document that parses the way its lines
/// did in the file. [`split_block_parts`] (block-level images, ```mermaid fences), [`split_segments`]
/// and [`split_math`] all remove lines from *within* a block and pass the survivors on as one run —
/// and a fragment cut out of the middle of a block reads differently from the same lines in place.
///
/// The shape that forced this is the centered `<div align="center">` badge banner at the top of a
/// great many crate READMEs. Pulling its `<img>` lines out leaves fragments like
/// `    </a>\n    <a href="…">\n`, which **open** with 4-column-indented content only because their
/// `<div>` went into the previous fragment. Asked about that fragment alone, the parser correctly
/// calls it an indented code block; asked about the original document, it correctly calls it the
/// interior of an HTML block — and the second reading is the one konoma draws. Both answers are
/// right for what the parser was shown, and the two inputs are byte-for-byte the same shape, so no
/// amount of care *inside* a splitter can recover the document's answer once the surrounding block
/// has been cut away. The only fix is not to destroy the context: compute the mask once, over the
/// whole document, before any line is removed, and carry each run's slice of it.
///
/// The three splitters that peel a *container* — [`split_alerts`] (strips the `>` markers),
/// [`split_details`] (strips the `<details>`/`<summary>` tags) and [`split_tables`] — are the
/// deliberate exception, and the reason this type has two constructors. They remove the markers
/// *along with* the block, so what they hand on genuinely is a standalone document; re-deriving the
/// mask there is not merely allowed but required (see [`literal_code_mask`]'s doc comment for the
/// fence that only parses as code once the `<summary>` line above it is gone). So a peeled body is
/// built with [`SourceRun::parse`], while a verbatim text run gets a slice of the mask its splitter
/// was handed, via [`SourceRun::new`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceRun {
    text: String,
    /// One entry per line of `text` (`text.lines().count()`).
    code: Vec<bool>,
}

impl SourceRun {
    /// Compute the code mask over `text` itself. The correct constructor for a **whole document**
    /// and for a **peeled container body** — the two cases where the text really is a standalone
    /// document (see the type's doc comment). Never for a fragment a splitter cut lines out of.
    pub fn parse(text: String) -> Self {
        let code = splitter_code_mask(&text.lines().collect::<Vec<_>>());
        Self { text, code }
    }

    /// Pair `text` with an already-computed mask — how a splitter emits a verbatim run: it copies
    /// the lines it did not consume and, alongside them, the entries those lines had in the mask it
    /// was handed.
    ///
    /// The assertion is the reason every such emit is routed through here rather than building the
    /// struct literally. A splitter that forgets to push an entry for a line it kept (or pushes one
    /// for a line it dropped) shifts every later line's answer by one, and a mask that is merely
    /// *offset* is far worse than one that is absent: it reports plausible answers for the wrong
    /// lines, which is precisely the drift this type exists to end. As a debug assertion the slip
    /// becomes a loud failure in the test suite instead of a quietly wrong render.
    pub fn new(text: String, code: Vec<bool>) -> Self {
        debug_assert_eq!(
            code.len(),
            text.lines().count(),
            "SourceRun mask/line-count drift for {text:?}"
        );
        Self { text, code }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The per-line code mask, positionally paired with [`lines`](Self::lines).
    pub fn code(&self) -> &[bool] {
        &self.code
    }

    /// The run's lines, positionally paired with [`code`](Self::code).
    pub fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

/// Read an HTML attribute value (double- or single-quoted) from a tag string.
pub fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find(name) {
        let i = search + rel;
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        if before_ok {
            let after = tag[i + name.len()..].trim_start();
            if let Some(rest) = after.strip_prefix('=') {
                let rest = rest.trim_start();
                if let Some(q) = rest.chars().next() {
                    if (q == '"' || q == '\'') && rest.len() > 1 {
                        if let Some(end) = rest[1..].find(q) {
                            return Some(rest[1..1 + end].to_string());
                        }
                    }
                }
            }
        }
        search = i + name.len();
    }
    None
}

/// Column width of `line`'s leading whitespace run, expanding a tab to the next multiple of 4
/// (CommonMark's tab-stop rule, applied from column 0 — the only case that matters here, since this
/// is only ever called on a line's own leading run). Verified against the actual renderer: a single
/// leading tab opens an indented code block exactly like 4 spaces do.
pub fn leading_ws_width(line: &str) -> usize {
    let mut col = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col = (col / 4 + 1) * 4,
            _ => break,
        }
    }
    col
}

/// The pulldown-cmark parse options konoma's own renderer walks with — used by both the renderer
/// and the scanners below, so the scanners ask the very parser the renderer renders through where
/// the code blocks are. These six flags used to have to be kept identical to `tui-markdown`'s own
/// set (written down in `tui-markdown-0.3.7/src/lib.rs`, `from_str_with_options`), because a second,
/// independently drifting option set reading the same document would re-create exactly the
/// scanner-vs-renderer disagreement asking the parser exists to eliminate. Since the single-renderer
/// migration dropped the `tui-markdown` dependency entirely, that upstream crate no longer exists to
/// drift against — this option set is now konoma's own spec, not a mirror of anything external — but
/// the reasoning still holds *within* konoma: every caller below asks this one function rather than
/// re-deriving its own flags, so the renderer and its scanners can never independently disagree.
///
/// Note what is deliberately **absent**: `ENABLE_TABLES`. konoma intercepts table blocks with its own
/// renderer (`split_tables`) before the text ever reaches this parser — see `render_md_text_inner` —
/// so this parser is never asked to understand a table at all; turning `ENABLE_TABLES` on here would
/// make the scanners below see tables the renderer never shows them.
pub fn markdown_parse_options() -> ParseOptions {
    let mut o = ParseOptions::empty();
    o.insert(ParseOptions::ENABLE_STRIKETHROUGH);
    o.insert(ParseOptions::ENABLE_TASKLISTS);
    o.insert(ParseOptions::ENABLE_HEADING_ATTRIBUTES);
    o.insert(ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    o.insert(ParseOptions::ENABLE_SUPERSCRIPT);
    o.insert(ParseOptions::ENABLE_SUBSCRIPT);
    o
}

/// Collect the content of every code block in one Markdown text run, **asking pulldown-cmark** —
/// the same parser tui-markdown renders through — rather than re-deriving CommonMark's block rules.
///
/// Root cause this replaced (2026-08, reported by a user on the released build): the hand-written
/// scanner measured indentation as an absolute column, while CommonMark measures it relative to the
/// enclosing container. Inside a list item every column shifts right by the marker's width, so a fence
/// indented 4 columns under `1. ` is a *real fence* (the parser and the screen agree) that the scanner
/// called an indented code block, and a continuation paragraph indented 6 columns under `   4. ` is
/// *not* code that the scanner counted as one. Either way the count guard broke and `y c` was refused
/// for the whole document — measured on 2,182 real `.md` files from the crate registry, 26 of them
/// (Apache license texts, several CHANGELOGs, a README) tripped it. Getting the absolute-column math
/// right is not possible without reproducing container tracking, i.e. re-implementing the parser; so
/// the second implementation is gone instead. This was the seventh renderer-vs-scanner drift of the
/// same shape, and tests can only ever *detect* a drift — they cannot stop one from being written.
///
/// The one thing the parser alone does not know (everything else the caller has already narrowed
/// down by mirroring the render pipeline's own splitting — see `scan_code_run`): code nested in a
/// plain block quote draws no header. tui-markdown prefixes every line of it with `"> "`, so
/// `flush_code_run` cannot find the block's own markers and hands the run back undecorated (see its
/// doc comment). Blockquote nesting is read off the event stream, so this needs no `>`-parsing of
/// its own — and an *alert* blockquote never reaches here, having been peeled off upstream.
#[cfg(any(test, feature = "test-hooks"))]
pub fn parser_code_blocks(text: &str, out: &mut Vec<String>) {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let mut quote_depth = 0usize;
    // `Some` while inside a code block (they never nest), accumulating its content.
    let mut body: Option<String> = None;
    let mut drawn = false;
    for ev in Parser::new_ext(text, markdown_parse_options()) {
        match ev {
            Event::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            Event::End(TagEnd::BlockQuote(_)) => quote_depth = quote_depth.saturating_sub(1),
            Event::Start(Tag::CodeBlock(_)) => {
                drawn = quote_depth == 0;
                body = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(b) = body.take() {
                    if drawn {
                        // The parser always terminates a code block's content with a newline; the
                        // rendered block has no trailing blank row, so neither does the copy.
                        out.push(b.strip_suffix('\n').unwrap_or(&b).to_string());
                    }
                }
            }
            Event::Text(t) => {
                if let Some(b) = body.as_mut() {
                    b.push_str(&t);
                }
            }
            _ => {}
        }
    }
}

/// UTF-8 byte length of the char whose leading byte is `b` (for advancing a byte cursor over text).
pub fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Length of the run of backticks starting at `i` (0 when `bytes[i]` is not a backtick).
pub fn backtick_run_len(bytes: &[u8], i: usize) -> usize {
    let mut n = 0;
    while i + n < bytes.len() && bytes[i + n] == b'`' {
        n += 1;
    }
    n
}

/// Given that a backtick run starts at `start`, the byte index just past the run that closes it.
/// `None` when nothing closes it — CommonMark's rule is that a code span is delimited by runs of
/// **equal length**, and an opener with no equal-length closer is literal text, not a span.
pub fn inline_code_span_end(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let n = line.len();
    let run = backtick_run_len(bytes, start);
    if run == 0 {
        return None;
    }
    let mut j = start + run;
    while j < n {
        if bytes[j] == b'`' {
            let r = backtick_run_len(bytes, j);
            j += r;
            if r == run {
                return Some(j);
            }
        } else {
            // Byte-wise is safe here: only ASCII backticks are matched, and every index handed out
            // is a run boundary, so slices never land inside a multibyte char.
            j += 1;
        }
    }
    None
}

/// The first inline code span at or after byte `from`, as `(start, end)` with `end` exclusive —
/// `line[start..end]` includes both delimiter runs. Unmatched backtick runs are literal text and are
/// skipped over, and a backslash-escaped `` \` `` never opens a span (CommonMark: escapes apply
/// everywhere *except* inside code). Line-scoped; see the section comment above.
pub fn next_inline_code_span(line: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let n = line.len();
    let mut i = from;
    while i < n {
        match bytes[i] {
            b'`' => match inline_code_span_end(line, i) {
                Some(end) => return Some((i, end)),
                None => i += backtick_run_len(bytes, i), // literal run: keep looking after it
            },
            // Escaped char: skip the backslash + the WHOLE next char. A fixed +2 would split a
            // multibyte char (`\あ`) and leave `i` off a char boundary.
            b'\\' if i + 1 < n => i = (i + 1 + utf8_len(bytes[i + 1])).min(n),
            b => i += utf8_len(b),
        }
    }
    None
}

/// Stand-in for a backslash-escaped `<` while the inline-HTML pass runs (NUL cannot occur in a
/// document, and its letter keeps it apart from a code span's `NUL digits NUL`).
pub const ESCAPED_LT: &str = "\u{0}e\u{0}";

/// `line` with every backslash-escaped `<` (`\<`, the backslash itself not escaped) replaced by
/// [`ESCAPED_LT`], so no tag rewrite can see it.
pub fn mask_escaped_lt(line: &str) -> String {
    if !line.contains("\\<") {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut it = line.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('<') => out.push_str(ESCAPED_LT),
            Some(n) => {
                out.push(c);
                out.push(n);
            }
            None => out.push(c),
        }
    }
    out
}

/// Stand-in for the `n`-th code span of a line while a pass rewrites around it. NUL is used because
/// CommonMark forbids it in a document at all ("the character U+0000 must be replaced with the
/// REPLACEMENT CHARACTER"), so it cannot collide with real content, and because it carries no
/// meaning to any of the passes — no `<`, `[`, `$` or backtick to react to. The trailing NUL keeps
/// index 1 from matching inside index 11 when the spans are put back.
pub fn code_span_placeholder(n: usize) -> String {
    format!("\u{0}{n}\u{0}")
}

/// Replace every inline code span of `line` with `code_span_placeholder`, returning the masked line
/// and the spans in order. Empty span list means the line had none.
pub fn mask_code_spans(line: &str) -> (String, Vec<String>) {
    let mut masked = String::new();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some((start, end)) = next_inline_code_span(line, cursor) {
        masked.push_str(&line[cursor..start]);
        masked.push_str(&code_span_placeholder(spans.len()));
        spans.push(line[start..end].to_string());
        cursor = end;
    }
    if spans.is_empty() {
        return (line.to_string(), spans);
    }
    masked.push_str(&line[cursor..]);
    (masked, spans)
}

/// Run `edit` over `line` with its inline code spans masked out, then put them back verbatim.
///
/// Masking rather than *splitting at* the spans is deliberate: the passes rewrite constructs that
/// may legitimately wrap a span — `<strike>Enables `named::from_str`, …</strike>` occurs verbatim in
/// palette's README — and splitting the line there would hide the closing tag from the opening one
/// and leave both raw. Masking keeps the line's structure whole while making the span's *contents*
/// unreachable, which is the actual requirement.
pub fn rewrite_masking_code_spans(line: &str, edit: impl FnOnce(&str) -> String) -> String {
    let (masked, spans) = mask_code_spans(line);
    let mut out = edit(&masked);
    for (i, span) in spans.iter().enumerate() {
        out = out.replace(&code_span_placeholder(i), span);
    }
    out
}

/// Information about an open code fence (the fence character and its length).
#[derive(Clone, Copy)]
pub struct Fence {
    pub ch: u8,
    pub len: usize,
}

/// If the line is a code fence (three or more of ``` or ~~~, indented no more than three columns),
/// return (fence, info string). CommonMark §4.5: a fence indented 4+ columns is not a fence at all —
/// it is (part of) an indented code block instead, and its content — including anything that merely
/// *looks* like fence syntax — is literal text (verified against the renderer: `"para\n\n    ```rust\n
/// fn a(){}\n    ```\n"` shows as one indented code block whose literal first line is `` ```rust ``,
/// not a code-block header). This threshold applies uniformly to both the opening line and any
/// candidate closing line — CommonMark requires the closing fence to independently satisfy the same
/// ≤3-column rule, regardless of the opening fence's own indentation.
pub fn parse_fence(line: &str) -> Option<(Fence, String)> {
    if leading_ws_width(line) >= 4 {
        return None;
    }
    let trimmed = line.trim_start();
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|&b| b == ch).count();
    if len < 3 {
        return None;
    }
    let info = trimmed[len..].trim().to_string();
    Some((Fence { ch, len }, info))
}

/// For each of `lines`, whether it is part of a fenced code block — the opening/closing marker
/// lines and everything between (using the same fence-matching rules as `split_segments`/
/// `parse_fence`: matching char, len ≥ opening len, empty info to close).
///
/// **One remaining production caller**: [`literal_code_mask`], which unions this with
/// [`code_block_mask`] — this half is what recovers a fence konoma renders in isolation but the
/// parser, reading the whole raw document, swallows into an HTML block. (That union is consulted
/// only by the source pre-passes; see `literal_code_mask`'s own doc comment for why they need it
/// and a block splitter must not.)
///
/// Every block splitter — all four, including [`split_html_blocks`], which was the last holdout
/// until the document mask started travelling on a [`SourceRun`] — plus
/// `structure_mask`/`details_mask` and the two source scanners now go through
/// [`splitter_code_mask`] instead. **This function knows nothing about containers**: CommonMark
/// also closes a fence at the end of the list item or block quote it was opened in, so a fence
/// whose closing line carries trailing text (not a close) is read here as running to the end of the
/// document, when in fact its list item ends it a few lines later. That is the runaway
/// `splitter_code_mask` exists to avoid; see its doc comment.
pub fn fence_mask(lines: &[&str]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let mut open: Option<Fence> = None;
    for (i, line) in lines.iter().enumerate() {
        match &open {
            None => {
                if let Some((fence, _info)) = parse_fence(line) {
                    mask[i] = true;
                    open = Some(fence);
                }
            }
            Some(fence) => {
                mask[i] = true;
                let closing = parse_fence(line)
                    .map(|(f, info)| f.ch == fence.ch && f.len >= fence.len && info.is_empty())
                    .unwrap_or(false);
                if closing {
                    open = None;
                }
            }
        }
    }
    mask
}

/// For each of `lines`, whether it is part of a code block — fenced (```` ``` ```` / `~~~`) **or
/// indented** (CommonMark's other code block kind: 4+ columns, measured relative to the enclosing
/// container) — asking pulldown-cmark, the same parser tui-markdown renders through, rather than
/// re-deriving the block rules by hand.
///
/// Reached two ways: directly, as [`splitter_code_mask`] — the gate every block splitter and source
/// scanner uses (see that function for why the union below is *not* the right answer there) — and as
/// the parser half of [`literal_code_mask`], the union the source pre-passes consult.
///
/// `fence_mask` above only ever recognizes fenced blocks, and only by their delimiters. But
/// `process_footnotes`, `process_inline_html` and `split_math` used
/// `fence_mask` (or, until this change, their own hand-rolled copy of the same fence-only state
/// machine) to decide which lines to leave untouched — and an indented code block is invisible to
/// it. So `<kbd>` written inside one to *document* the tag became a real keycap, `<br>` injected a
/// newline mid-block and split it in two, `[^1]` became a footnote reference, and `$x$` was lifted
/// out onto its own line as a math image — inside a block that is supposed to be a literal,
/// verbatim transcript on screen (2026-08). Confirmed by temporarily swapping this function's own
/// callers back to `fence_mask`: `process_footnotes_leaves_indented_code_untouched`,
/// `process_inline_html_leaves_indented_code_untouched` and the "inside an indented code block" case
/// in `math_extraction_covers_delimiters_and_rejects_lookalikes` all fail exactly this way against
/// `fence_mask` alone and pass once the mask covers indented blocks too.
///
/// This can't be patched onto `fence_mask` by hand the way the fenced half was: CommonMark measures
/// the 4-column indent *relative to the enclosing container*, so a list item's marker shifts its
/// content column right — the identical container-tracking problem `2c41192` hit for the
/// copy-scanner's code-block count (`parser_code_blocks`'s doc comment has the worked example).
/// Rather than grow a second copy of that tracking here, ask the parser directly, exactly as
/// `parser_code_blocks` does.
///
/// Implementation: parse `lines` (rejoined with `\n`) via `Parser::new_ext` using
/// `markdown_parse_options()` — **the same option set the renderer itself parses with, reused
/// rather than redeclared**, so this mask can never drift from what *that parser* would call a code
/// block when it reads `lines` as one raw run — through `into_offset_iter()`, so each
/// `Event::Start(Tag::CodeBlock(_))` carries the block's byte range in that text. A line is marked
/// when its own byte range *overlaps* a code
/// block's range, not when it's fully contained: pulldown-cmark's block range starts after a
/// block's leading structural indentation/quote-marker bytes (measured directly — the range for
/// `    indented` begins at the `i`, not the first space), so containment would silently miss the
/// leading run of an indented block's first line. Each line's range is `[starts[i], starts[i] + 1 +
/// lines[i].len())` — the `+1` for the `\n` the caller's line splitting removed — so two adjacent
/// lines' ranges touch with no gap, and a blank line pulldown-cmark folds into an indented block's
/// range (two chunks separated by one blank line, which CommonMark keeps as a single block) is
/// still marked, matching what a fenced block's blank interior lines already are.
///
/// Both bounds are found by binary search rather than a linear scan per block: `starts` and `ends`
/// are monotonically increasing (each line is at least 1 byte including its `\n`), so
/// `partition_point` finds the first/last overlapping line directly, keeping this at
/// O(lines + blocks·log(lines)) rather than O(lines·blocks) for a document with many blocks.
///
/// Unlike `parser_code_blocks`/`scan_code_run`, this does not special-case a code block nested
/// inside a plain blockquote (which the renderer draws with no header) or exclude ```mermaid fences
/// (diverted to diagram rendering): none of that distinction matters here — every line covered by a
/// `CodeBlock` event, in this one-shot raw-document parse, is code as far as this function is
/// concerned, regardless of what decoration (if any) the renderer eventually draws around it.
///
/// Two ways this can still disagree with what actually reaches the screen (neither reaches this
/// function's own callers directly — see `literal_code_mask`, which is what `process_footnotes`,
/// `process_inline_html` and `split_math` actually consult, and folds both of these in):
///
/// * Not a strict superset of `fence_mask`: a fenced block that pulldown-cmark parses as part of an
///   *HTML block* instead (e.g. `~~~`/code/`~~~` sandwiched inside a raw `<div>…</div>`) is not a
///   `CodeBlock` event at all — and konoma's own renderer agrees here, having already carved that
///   span out as HTML before the parser ever sees it (`split_html_blocks`). `fence_mask` would have
///   called those lines code; this function, correctly, does not.
/// * Not a strict superset in the other direction either: a fence with no blank line between it and
///   a preceding `<summary>…</summary>` line, inside a `<details>` block, is read by *this* function
///   (which parses the whole raw document as one HTML-block-type-7 run — verified directly against
///   pulldown-cmark) as HTML, not code — but konoma's own renderer draws it as a real code block,
///   because `split_details` extracts the `<details>` body without requiring that blank line and
///   hands it to `render_md_body_nested` in isolation, where the same fence, with no competing
///   `<summary>` text around it, does produce a `CodeBlock` event. `fence_mask` gets this one right
///   (it is blind to the surrounding `<details>`/`<summary>` context entirely), so union with it —
///   `literal_code_mask` — is what recovers the renderer's actual answer. Note that
///   [`splitter_code_mask`] does **not** need the union for this: a splitter runs on the text it was
///   handed, and by then `split_details` has already peeled those tags off.
pub fn code_block_mask(lines: &[&str]) -> Vec<bool> {
    use pulldown_cmark::{Event, Parser, Tag};
    if lines.is_empty() {
        return Vec::new();
    }
    let text = lines.join("\n");
    // `starts[i]` = byte offset line `i` starts at in `text`; `starts[lines.len()]` is a sentinel
    // one past where a line after the last one would start. Since every line contributes `len + 1`
    // (the `+1` uniformly standing in for the `\n` rejoined between lines, including — harmlessly —
    // after the very last line, which `join` does not actually add one after), `ends[i] ==
    // starts[i + 1]` always holds, so a single array serves both.
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut pos = 0usize;
    for line in lines {
        starts.push(pos);
        pos += line.len() + 1;
    }
    starts.push(pos);

    let mut mask = vec![false; lines.len()];
    for (ev, range) in Parser::new_ext(&text, markdown_parse_options()).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(_)) = ev else {
            continue;
        };
        // Smallest `i` with `starts[i] < range.end` (condition `starts[i] < range.end`, upper bound
        // on the overlapping range): `starts` restricted to real lines is what `hi` — the exclusive
        // upper bound — is searched over.
        let hi = starts[..lines.len()].partition_point(|&s| s < range.end);
        // Smallest `j` with `starts[j] > range.start`; `ends[i] > range.start` is `starts[i + 1] >
        // range.start`, so the smallest satisfying `i` is `j - 1` (searched over the full array,
        // sentinel included, since `ends[lines.len() - 1]` is `starts[lines.len()]`).
        let j = starts.partition_point(|&s| s <= range.start);
        let lo = j.saturating_sub(1);
        if lo < hi {
            mask[lo..hi].fill(true);
        }
    }
    mask
}

/// For each of `lines`, whether the text is literal code **on screen** — the mask `process_footnotes`,
/// `process_inline_html` and `split_math` actually consult. It is the union (logical OR) of
/// `code_block_mask` (what pulldown-cmark, parsing the raw document in one pass, calls a code block)
/// and `fence_mask` (what a bare `` ``` ``/`~~~` delimiter line says, ignoring any surrounding
/// construct entirely).
///
/// Neither mask alone matches what konoma draws, because konoma decides "is this a code block" two
/// different ways depending on where the text sits, and this function's callers can't tell which way
/// applies to a given line without redoing that routing:
///
/// * Text that reaches tui-markdown's own parser unmodified is exactly what `code_block_mask`
///   describes — that *is* the parser's decision.
/// * Text konoma peels off **before** the parser ever sees it — a `<details>` body (`split_details`,
///   spanning blank lines) or an alert body (`split_alerts`, `>` stripped) — is re-parsed **in
///   isolation** by `render_md_body_nested`, without the surrounding tag/quote-marker that could have
///   changed how the parser reads it. Concretely: parsing the whole raw document, a fence with no
///   blank line between it and a preceding `<summary>…</summary>` line is swallowed into one
///   `Html_block` — no `CodeBlock` event at all — because CommonMark's HTML-block-type-7 rule keeps
///   consuming lines until a blank one, whichever construct wrote them (verified directly against
///   pulldown-cmark). But `split_details` doesn't require that blank line (`details_block_close`
///   just looks for the literal `</details>` line), so it hands the fence to
///   `render_md_body_nested` as an **isolated** two-line body with no `<details>`/`<summary>` text
///   around it to compete for the parse — and there, with nothing else in the running, the exact
///   same three lines *do* produce a `CodeBlock` event. `code_block_mask` alone, parsing the raw
///   document once, cannot see this: it only ever gets the version *with* the swallowing context.
///   `fence_mask`, being blind to any surrounding construct, gets this one right by accident — it
///   would call those same three lines fenced regardless of what wraps them.
///
/// So a line either mask calls code really is drawn as code somewhere in konoma's own rendering, and
/// the union is what "leave code alone" has to mean. The asymmetry this exists to protect is
/// principle #3's: **over-protecting a line only costs one construct staying unrendered as literal
/// text** (safe), while **under-protecting one corrupts a block that's genuinely on screen** — `<br>`
/// injects a newline that splits a code block's header in two, `$x$` gets lifted out as a stray math
/// image, `[^1]` gets renumbered as a real reference. A union can only ever mark *more* lines than
/// either mask alone, so it can never fall on the unsafe side of that asymmetry, and it strictly
/// includes `fence_mask` — which is what every one of these three passes used, unconditionally,
/// before this mask existed — so no line that used to be protected can lose that protection here.
///
/// The cost carried over unchanged: `fence_mask`'s own pre-existing over-protection (an opening fence
/// delimiter whose info string itself contains a backtick — CommonMark forbids that, so it isn't
/// really a fence at all, but `fence_mask`'s hand-rolled matcher doesn't enforce the rule and reads
/// everything after it as fenced through to the next line shaped like a closing delimiter, or to the
/// end of the document if none comes) is inherited by the union exactly as before. That is pre-existing
/// behavior, not something this mask introduces or could remove without dropping `fence_mask` from the
/// union — which would reintroduce the `<details>`-without-a-blank-line gap above.
pub fn literal_code_mask(lines: &[&str]) -> Vec<bool> {
    literal_code_mask_from(&code_block_mask(lines), lines)
}

/// The one named home of the rule "don't start a construct on this line, it is code" — so that the
/// rule cannot grow a second hand-written copy, which is the drift this whole area keeps regressing
/// on.
///
/// It is asked **once per text run, by whoever owns that run's text**, and the answer then travels
/// on a [`SourceRun`]. The block splitters ([`split_tables`], [`split_html_blocks`],
/// [`split_alerts`], [`split_details`]) and the `structure_mask`/`details_mask` they feed no longer
/// call this themselves: they read the slice they were handed, because the text in their hands may
/// be a fragment that parses differently from the same lines in the file (see [`SourceRun`], and
/// the last paragraph here). The callers that *do* ask are the ones holding a whole document or a
/// peeled container body — [`SourceRun::parse`], [`split_block_parts`], and the two source scanners
/// that must count exactly what those splitters draw ([`code_block_source_locs`],
/// [`task_source_locs`]).
///
/// It is [`code_block_mask`] — the parser's answer — and deliberately **not** [`literal_code_mask`],
/// even though that union is what the *source pre-passes* (`process_footnotes`,
/// `process_inline_html`, `split_math`) consult. The two have opposite safety asymmetries:
///
/// * A pre-pass's mask means "leave this text alone". Over-marking costs one construct staying
///   literal (safe); under-marking corrupts a code block that is genuinely on screen. So the union
///   — mark if *either* mask says code — is right there.
/// * A splitter's mask means "do not carve a block out here". Over-marking is **not** safe: the
///   construct falls through to tui-markdown, which collapses a GFM table into one line of raw
///   `| a | b |` text, drops an HTML block's contents entirely, and draws an alert as a plain
///   quote. Under-marking splits a code block in two. Both directions cost something real, so the
///   only defensible answer is the one the renderer itself will act on — and everything a splitter
///   does *not* carve out goes to that same parser.
///
/// Concretely, the union cannot be used here because `fence_mask`'s half runs away: its hand-rolled
/// state machine has no notion of a container, so a fence opened inside a list item whose closing
/// line carries trailing text (`` ``` ([#2642](…)) `` — not a close per CommonMark, but the list
/// item ends the block anyway) swallows every remaining line of the document. nix 0.31.3's
/// CHANGELOG.md does exactly this at line 110, and the 617-line GFM table below it rendered as raw
/// pipes (2026-08). pulldown-cmark closes the block at the list item, so `code_block_mask` does too.
///
/// The reason `literal_code_mask` needs `fence_mask` in its union — a fence with no blank line
/// between it and a preceding `<summary>` line, which the parser reads as one HTML block when it
/// sees the whole raw document — does not apply to a **peeled container body**: by the time a
/// splitter runs on a `<details>` body, `split_details` has already taken the surrounding tags
/// away, so the parser sees the fence in isolation and calls it code. Evaluating a peeled body on
/// its own terms is load-bearing, not incidental, and that is what [`SourceRun::parse`] is for.
///
/// The distinction matters because the opposite is true for the other kind of text run. A splitter
/// that removes lines from *within* a block — `split_block_parts`, `split_segments`, `split_math` —
/// hands on a fragment that is not a document at all, and asking this function about one gets a
/// confidently wrong answer: the interior of a `<div>` badge banner reads as an indented code block
/// the moment the `<img>` lines between its `<a>` tags are gone. Those runs therefore carry a slice
/// of the mask computed over the intact document instead of re-deriving one here; see [`SourceRun`],
/// which exists entirely to keep the two cases apart, and [`split_html_blocks`], the gate whose
/// misbehavior on exactly that shape forced the distinction into the open.
pub fn splitter_code_mask(lines: &[&str]) -> Vec<bool> {
    code_block_mask(lines)
}

/// [`literal_code_mask`] with the [`code_block_mask`] half already computed — for a caller that
/// needs the parser's answer on its own as well (`split_math`, which also gates `structure_mask` on
/// it), so the pulldown-cmark parse happens once per text run instead of twice.
pub fn literal_code_mask_from(parsed: &[bool], lines: &[&str]) -> Vec<bool> {
    let fenced = fence_mask(lines);
    parsed.iter().zip(fenced).map(|(&a, b)| a || b).collect()
}

/// Which line-continuity-dependent construct a line belongs to, as reported by [`structure_mask`].
///
/// "Line-continuity-dependent" means the construct's own splitter recognizes it by an unbroken run
/// of lines each matching a shape, so a pass that inserts, removes or reshapes a line in the middle
/// of the run destroys the construct rather than editing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Structure {
    /// In none of the constructs below.
    None,
    /// A blockquote line — a plain quote or a GitHub alert body; both are `>`-prefixed, and
    /// `split_alerts` captures a body by consuming consecutive `>`-prefixed lines.
    Quote,
    /// Inside a `<details>` … `</details>` block, its tag lines included.
    Details,
    /// A GFM table row: the header, the delimiter row, or one of the consecutive data rows.
    Table,
    /// The line that *opens* any other HTML block (`<p …>`, `<div …>`, `<!-- … -->`, …).
    ///
    /// Split from [`Structure::HtmlBody`] because the two answer differently for a caller that
    /// rewrites the tags on the line. On this line no block is open yet — it is about to be — so a
    /// rewrite here cannot destroy one; on a body line the block already exists and a rewrite can.
    /// The distinction is load-bearing rather than pedantic because [`is_html_block_start`] accepts
    /// *any* tag name, `br` included, where CommonMark's HTML-block rules do not (type 6 matches a
    /// fixed list of block-level names that has no `br` in it, and type 7 needs the tag alone on the
    /// line). So `<br>` and `<br> tail` open a konoma HTML block that exists **only** because of the
    /// very tag a caller is there to rewrite — which is why the two must not be collapsed back into
    /// one variant now that the body rule *removes* lines: applied to an opener it would delete the
    /// break in the everyday `text` / `<br>` / `more` prose idiom, whose middle line is a block
    /// opener by this over-broad rule alone. See [`process_inline_html_traced`].
    HtmlOpen,
    /// A line inside an HTML block that an earlier line opened.
    ///
    /// `quoted` records whether that block is itself nested inside a blockquote, which a caller
    /// deciding "would rewriting this line leave it **blank**" has to know: inside a quote the
    /// leading `>` marker is container, not content, so `">   "` is exactly as blank to the HTML
    /// block within the quote as `"   "` is to one at top level, and ends it just the same. The flag
    /// cannot be recovered from the line itself — a top-level `<div>` whose body text happens to
    /// start with `>` is byte-for-byte the same shape and there the marker *is* content, so treating
    /// it as a prefix would inject a spurious `>` into the rewritten line. See
    /// [`process_inline_html_traced`].
    HtmlBody { quoted: bool },
}

/// Label the HTML block opening at `view[start]` into `out` (positionally paired with `view`) and
/// return the index one past its last line.
///
/// **The one place that decides an HTML block's extent for this mask** — the top-level pass in
/// [`structure_mask`] and its nested rescan both call it — so the two cannot drift, which is the
/// failure mode this module keeps regressing on (see [`splitter_code_mask`]). The extent is "up to
/// the line before the next blank one", the same walk [`split_html_blocks`] performs.
pub fn mark_html_block(view: &[&str], start: usize, quoted: bool, out: &mut [Structure]) -> usize {
    let mut j = start + 1;
    while j < view.len() && !view[j].trim().is_empty() {
        j += 1;
    }
    // The opener is reported separately from the body it opens — see `Structure::HtmlOpen`.
    out[start..j].fill(Structure::HtmlBody { quoted });
    out[start] = Structure::HtmlOpen;
    j
}

/// Re-label every HTML block found inside a container's body, overwriting the container's own label
/// on just those lines. `view` is the body **as the renderer sees it** (a blockquote's lines with
/// their `>` markers stripped, a `<details>` body as-is), positionally paired with `in_code` and
/// with `out`.
///
/// Only ever *re-labels* lines that already carry the container's non-[`Structure::None`] label — it
/// neither widens nor narrows the container's span — which is what keeps [`split_math`], whose only
/// question is "is this line inside something at all", byte-for-byte unaffected.
pub fn mark_nested_html_blocks(
    view: &[&str],
    in_code: &[bool],
    quoted: bool,
    out: &mut [Structure],
) {
    let mut k = 0;
    while k < view.len() {
        if !in_code[k] && is_html_block_start(view[k]) {
            k = mark_html_block(view, k, quoted, out);
            continue;
        }
        k += 1;
    }
}

/// For each of `lines`, which construct whose own parser requires line continuity it sits inside:
/// a blockquote (a plain quote or a GitHub alert — both are `>`-prefixed), a `<details>` block,
/// another HTML block, or a GFM table row. [`Structure::None`] means none of them.
///
/// **Two callers ask this mask two different questions**, which is why it reports *which* construct
/// rather than a bare yes/no:
///
/// * [`split_math`] only needs "is this line inside one of them at all" — i.e. anything but
///   [`Structure::None`], which is exactly what this function meant when it returned `Vec<bool>`.
///   It consults the mask so it never lifts an inline math expression out onto its own line from
///   inside one of these; doing so breaks the very line sequence those blocks' own splitters
///   (`split_alerts`, `split_details`, `split_html_blocks`, `split_tables`) require, tearing the
///   block apart (see `split_math`'s doc comment for the concrete failure modes this prevents).
/// * [`process_inline_html_traced`] needs the kind, because the three ways it can rewrite a `<br>`
///   are not interchangeable. Inside the body of an HTML block a line the rewrite would leave blank
///   has to be dropped outright — every *replacement* still leaves the line blank (and a
///   whitespace-only line *is* blank to CommonMark), which terminates the block, and keeping the tag
///   instead leaves a block opener for a later pass to relocate. Inside a table row it has to
///   collapse to a space — a table
///   row has no continuation form, so a second line can only ever be a second row. Everywhere else,
///   the opening line of an HTML block included (see [`Structure::HtmlOpen`]), it may become a hard
///   break, provided the injected line repeats the enclosing blockquote prefix. See that function's
///   doc comment for the failures each case prevents.
///
/// Reuses the same predicates the block splitters themselves use to decide where a block starts
/// and ends, so this mask can never drift out of sync with what those splitters actually carve out
/// — and, now that there are two callers, so the second one does not become another hand-written
/// copy of those rules. That drift is what this module keeps regressing on (see
/// [`splitter_code_mask`]). Gated by the caller's code mask ([`code_block_mask`]), exactly like
/// `split_tables`/`split_html_blocks`/`split_alerts`: a `>`/`|`/`<div>`-looking line inside a code
/// block is ordinary code, not structure. Taken as a parameter rather than recomputed here so a
/// caller that also needs `literal_code_mask` (both of them do, and it is built on
/// `code_block_mask`) parses each text run exactly once.
///
/// # Nesting
///
/// A container's body is rescanned for **HTML blocks**, whose lines are relabelled
/// [`Structure::HtmlOpen`]/[`Structure::HtmlBody`] over the container's own label: for a
/// `<details>` block the body between its tag lines, for a blockquote the run of `>` lines with
/// their markers stripped — in each case the text the renderer goes on to treat as its own
/// document, and therefore the text whose blank lines decide where a block inside it ends.
///
/// Reporting only the outermost construct was not a neutral simplification, which is why this
/// exists (2026-08). A badge banner nested one level deep came back as all-`Details` (or all-`Quote`),
/// so its lone `    <br>` line took the default hard-break rewrite instead of the HTML-block one,
/// `"    " + "  "` is a blank line to CommonMark, the inner `<p align="center">` ended there, the
/// 4-column-indented lines below became a genuine indented code block — and the block splitter then
/// faithfully honoured that, drawing the markup as `▎ …` and dropping one of the badges, because an
/// `<img>` line the document calls code is no longer extracted. Both containers are affected and by
/// different routes, so both are rescanned.
///
/// **The rescan only ever re-labels lines the container already claimed** — never widening nor
/// narrowing a span — which is what keeps [`split_math`] byte-for-byte unaffected: its only question
/// is whether a line is non-[`Structure::None`], and every line involved was, and stays, non-`None`.
///
/// **Still reported at the outermost construct only:** a **table** nested in either container, and a
/// container nested in a container. Neither has been observed to cause damage — the `<br>` rules for
/// `Quote`, `Details` and `None` all end up at the same hard break — and the rescan is deliberately
/// limited to the one construct whose "runs to the next blank line" rule a rewritten line can
/// actually destroy. The container tag lines themselves (`<details>` / `</details>`) are excluded
/// too; see the call site for why relabelling those would delete breaks the document really has.
pub fn structure_mask(lines: &[&str], in_code: &[bool]) -> Vec<Structure> {
    let mut mask = vec![Structure::None; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if in_code[i] {
            i += 1;
            continue;
        }
        // Blockquote / GitHub alert: `split_alerts` captures a run of `>`-prefixed lines (its body
        // loop is exactly `while is_blockquote_line(...)`), and a plain (non-alert) blockquote is
        // parsed the same way by tui-markdown itself — one masked line at a time is enough since
        // every continuation line re-tests true.
        // Deliberately NOT extended to CommonMark "lazy continuation" (a quote line followed by an
        // unprefixed paragraph line): `split_alerts` stops at the first non-`>` line too, and staying
        // on its exact rule is what keeps this mask from drifting. The residue is mild and matches
        // what already happens to a list item — the lazy line's tail moves below the lifted math —
        // with no marker leak, no broken callout box and no hidden content revealed.
        if is_blockquote_line(lines[i]) {
            let start = i;
            let mut j = start + 1;
            while j < lines.len() && !in_code[j] && is_blockquote_line(lines[j]) {
                j += 1;
            }
            mask[start..j].fill(Structure::Quote);
            // The run is gathered rather than marked one line at a time only so its interior can be
            // rescanned; the set of lines it marks is identical either way (every continuation line
            // re-tested true under the old per-line form, and a code line ends the run here exactly
            // as it fell through to the code gate there).
            //
            // Rescan that interior for HTML blocks, on the `>`-stripped text — what
            // `split_alerts`/tui-markdown hand down as the quote's body, and therefore the text
            // whose blank lines decide where a block inside the quote ends.
            let body: Vec<String> = lines[start..j]
                .iter()
                .map(|l| strip_blockquote(l))
                .collect();
            let body: Vec<&str> = body.iter().map(String::as_str).collect();
            mark_nested_html_blocks(&body, &in_code[start..j], true, &mut mask[start..j]);
            i = j;
            continue;
        }
        // `<details>` … `</details>`: mirrors `split_details`'s span (opening tag line through the
        // closing tag line inclusive, spanning blank lines; runs to the end of input if unclosed).
        // `details_block_close` is `split_details`'s own scan, shared (see its doc comment) so
        // this can never drift out of sync with what `split_details` actually carves out.
        if details_open_tag(lines[i]).is_some() {
            let start = i;
            let close = details_block_close(lines, start);
            let end = close.unwrap_or(lines.len() - 1);
            mask[start..=end].fill(Structure::Details);
            // Rescan the body — the lines between the container's own tag lines — for HTML blocks.
            // The `<details>`/`</details>` lines themselves are deliberately left out: they are
            // markers `split_details` consumes structurally, and an HTML block opened *on* the
            // `<details>` line would swallow the body behind a label meant for lines that sit inside
            // an already-open block, turning the everyday `text` / `<br>` / `more` idiom in a body
            // into a deletion (the hazard `Structure::HtmlOpen` exists for).
            let body = start + 1..close.unwrap_or(lines.len());
            mark_nested_html_blocks(
                &lines[body.clone()],
                &in_code[body.clone()],
                false,
                &mut mask[body],
            );
            i = close.map_or(lines.len(), |c| c + 1);
            continue;
        }
        // GFM table: header row + delimiter row + consecutive data rows, mirroring `split_tables`.
        if i + 1 < lines.len()
            && !in_code[i + 1]
            && looks_like_table_row(lines[i])
            && is_table_delimiter(lines[i + 1])
        {
            let start = i;
            let mut j = start + 2;
            while j < lines.len() && !in_code[j] && looks_like_table_row(lines[j]) {
                j += 1;
            }
            mask[start..j].fill(Structure::Table);
            i = j;
            continue;
        }
        // Any other HTML block: opening tag line through the line before the next blank line,
        // mirroring `split_html_blocks`.
        if is_html_block_start(lines[i]) {
            i = mark_html_block(lines, i, false, &mut mask);
            continue;
        }
        i += 1;
    }
    mask
}

// ---- HTML block rescue --------------------------------------------------------
// tui-markdown (pulldown-cmark) silently drops Html events, so a block like `<details>` vanished
// **along with its inner text content**. Intercept the block and display its text with the tags
// stripped (konoma doesn't render HTML = a safe degradation, principle #3). A `<!-- -->` comment is hidden entirely.

/// Whether `line` starts an HTML block: `<tag ...>` / `</tag>` / `<!--`. The tag name must be
/// followed by space / `>` / `/` / end — so an autolink like `<https://…>` does NOT match
/// (`:` ends the name with an invalid terminator).
pub fn is_html_block_start(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("<!--") {
        return true;
    }
    let Some(rest) = t.strip_prefix('<') else {
        return false;
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let name_len = rest
        .char_indices()
        .take_while(|(i, c)| {
            if *i == 0 {
                c.is_ascii_alphabetic()
            } else {
                c.is_ascii_alphanumeric() || *c == '-'
            }
        })
        .count();
    if name_len == 0 {
        return false;
    }
    matches!(
        rest[name_len..].chars().next(),
        None | Some(' ') | Some('\t') | Some('>') | Some('/')
    )
}

/// Where an HTML comment ends: the byte offset in `s` just past the `-->` closing the comment that
/// starts at `at`, or `None` when `s[at..]` does not start a comment at all.
///
/// **The one place this pipeline decides that.** Three scanners walk raw HTML tag by tag and each
/// has to know where a comment ends, for three different reasons — [`render_html_block`] to drop it,
/// [`html_cell_to_markdown`] to hand it through untranslated, and `model::parse_html_table` to
/// refuse to read table structure out of it — and a comment may contain any of `<`, `>` and a whole
/// tag, so a scanner without this rule reads the author's *removed* text as markup. That is exactly
/// how a commented-out `<tr>` came to be drawn as a real row (see `html_table_corpus`'s comment
/// cases). Sharing one function, rather than repeating the two-line skip a third time, is what makes
/// "commented out" mean the same thing in all three.
///
/// The rule: from `<!--` to the first following `-->`; comments do not nest, so an inner `<!--` is
/// ordinary comment text. An unterminated comment runs to the end of `s` — and `s` is always one
/// HTML *block* (CommonMark ends one at a blank line), never the rest of the document. Deliberately
/// **not** CommonMark 0.31's two special cases, where `<!-->` and `<!--->` are complete empty
/// comments: this must agree with `render_html_block`, the pass that decides what text is visible,
/// because a scanner that ended a comment *earlier* than that pass does would fold rows out of bytes
/// the renderer would have hidden — the disclosure this rule exists to prevent, arrived at from the
/// other side. Ending one *later* only ever hides more.
pub fn html_comment_end(s: &str, at: usize) -> Option<usize> {
    let body = s.get(at..)?.strip_prefix("<!--")?;
    Some(match body.find("-->") {
        Some(e) => at + "<!--".len() + e + "-->".len(),
        None => s.len(),
    })
}

/// One of the five GitHub alert types (`> [!NOTE]` etc.). A handful of common Obsidian aliases map
/// onto the nearest GitHub type so those documents render too.
///
/// `pub(crate)`, not private: `preview::markdown::model` (a child module) re-exports this so
/// `BlockKind::Quote.alert` can use the exact same type the renderer does, rather than a second
/// enum with the same variants — see that module's doc comment on `Quote`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    pub fn parse(s: &str) -> Option<AlertKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "note" | "info" => Some(AlertKind::Note),
            "tip" | "hint" => Some(AlertKind::Tip),
            "important" => Some(AlertKind::Important),
            "warning" | "attention" => Some(AlertKind::Warning),
            "caution" | "danger" | "error" => Some(AlertKind::Caution),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AlertKind::Note => "Note",
            AlertKind::Tip => "Tip",
            AlertKind::Important => "Important",
            AlertKind::Warning => "Warning",
            AlertKind::Caution => "Caution",
        }
    }
    /// A Nerd Font glyph (classic FontAwesome block, same source as `ui/icons.rs`, so it is present
    /// in Symbols Nerd Font). Only used when `ui.icons` is on.
    pub fn icon(self) -> char {
        match self {
            AlertKind::Note => '\u{f05a}',      // nf-fa-info_circle
            AlertKind::Tip => '\u{f0eb}',       // nf-fa-lightbulb_o
            AlertKind::Important => '\u{f0a1}', // nf-fa-bullhorn
            AlertKind::Warning => '\u{f071}',   // nf-fa-exclamation_triangle
            AlertKind::Caution => '\u{f06a}',   // nf-fa-exclamation_circle
        }
    }
}

/// Parse a GitHub-alert header line (`> [!NOTE]`, optionally with a trailing Obsidian-style title).
/// Returns the type and the (possibly empty) title. None if the line is not an alert header.
pub fn parse_alert_header(line: &str) -> Option<(AlertKind, String)> {
    let rest = line.trim_start().strip_prefix('>')?.trim_start();
    let rest = rest.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = AlertKind::parse(&rest[..close])?;
    Some((kind, rest[close + 1..].trim().to_string()))
}

/// A blockquote continuation line (`> …` or a bare `>`), whitespace-tolerant.
pub fn is_blockquote_line(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// Strip one level of blockquote marker (`>` and one optional following space) from a line.
pub fn strip_blockquote(line: &str) -> String {
    let l = line.trim_start();
    let l = l.strip_prefix('>').unwrap_or(l);
    l.strip_prefix(' ').unwrap_or(l).to_string()
}

pub enum DetailsPart {
    // Only read back by test code (`split_details`'s own tests): production
    // (`collect_details_open`) discards a `Text` part's body entirely and reads only
    // `Details::open_attr` from the other variant — see that function's own `match`.
    Text(#[allow(dead_code)] SourceRun),
    Details {
        open_attr: bool,
        #[allow(dead_code)]
        summary: String,
        #[allow(dead_code)]
        body: String,
    },
}

/// If the line opens a `<details>` block, return its `open` attribute. Case-insensitive; matches
/// `<details>` and `<details open …>` (a well-formed opening tag on its own line).
pub fn details_open_tag(line: &str) -> Option<bool> {
    let t = line.trim();
    let lower = t.to_ascii_lowercase();
    let rest = lower.strip_prefix("<details")?;
    if !(rest.is_empty() || rest.starts_with('>') || rest.starts_with(' ')) {
        return None; // e.g. "<detailsx" is a different tag
    }
    Some(rest.contains("open"))
}

pub fn is_details_close(line: &str) -> bool {
    line.trim().eq_ignore_ascii_case("</details>")
}

/// The line index of the `</details>` line that closes the `<details>` … `</details>` block opening
/// at `lines[start]` (the caller has already confirmed this via `details_open_tag`) — `None` if
/// unclosed (the block runs to the end of input). **The one place that scans for a `<details>`
/// block's extent** — `split_details`, `structure_mask`, and `details_mask` all call it, so all
/// three can never drift out of sync with each other about where a block starts and ends.
pub fn details_block_close(lines: &[&str], start: usize) -> Option<usize> {
    ((start + 1)..lines.len()).find(|&j| is_details_close(lines[j]))
}

/// Split off `<details>` … `</details>` blocks (spanning blank lines) from a text run, extracting the
/// `<summary>` and the body. Non-details text is returned verbatim for the normal pipeline.
/// Code-aware: it reads the mask off the [`SourceRun`] it is handed rather than deriving one, so a
/// `<details>` inside a code block is left as text.
///
/// That gate is what makes this function's block count and order agree with
/// [`collect_details_open`], which seeds the per-ordinal open state — and the agreement rests on the
/// two asking about the *same text*, not on them running the same rule. `collect_details_open` is
/// handed the whole preprocessed document and derives the document's mask from it; this runs on a
/// run carved out of that document and is handed the matching slice of that same mask. The
/// [`SourceRun`] is what carries it here, and it is load-bearing rather than an optimisation: a
/// splitter that re-derived a mask from the fragment in its hands would answer for text the
/// document never contained, and every `<details>` after the first disagreement would be seeded with
/// another block's open state.
pub fn split_details(run: &SourceRun) -> Vec<DetailsPart> {
    let lines: Vec<&str> = run.lines();
    let in_code = run.code();
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut mask: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if in_code[i] {
            text.push_str(lines[i]);
            text.push('\n');
            mask.push(in_code[i]);
            i += 1;
            continue;
        }
        if let Some(open_attr) = details_open_tag(lines[i]) {
            if !text.is_empty() {
                parts.push(DetailsPart::Text(SourceRun::new(
                    std::mem::take(&mut text),
                    std::mem::take(&mut mask),
                )));
            }
            let start = i;
            let close = details_block_close(&lines, start);
            let body_end = close.unwrap_or(lines.len());
            let block = &lines[start + 1..body_end];
            let (summary, body) = extract_summary_body(&block.join("\n"));
            parts.push(DetailsPart::Details {
                open_attr,
                summary,
                body,
            });
            i = close.map_or(lines.len(), |c| c + 1);
            continue;
        }
        text.push_str(lines[i]);
        text.push('\n');
        mask.push(in_code[i]);
        i += 1;
    }
    if !text.is_empty() {
        parts.push(DetailsPart::Text(SourceRun::new(text, mask)));
    }
    parts
}

/// The `(s, e)` byte-offset pair — into `inner` — of a well-formed `<summary>...</summary>` tag's own
/// `<summary` and `</summary>` starts: present only when the former actually closes (a literal `>`
/// somewhere in `[s, e)`) before the latter begins. The one place this search happens —
/// `extract_summary_body`/`summary_tag_end` both call this rather than re-deriving the identical
/// `<summary`/`</summary>` scan a second time (the same "one search, several thin callers" shape
/// `details_block_close` already is for `split_details`/`structure_mask`/`details_mask`).
pub fn summary_tag_bounds(inner: &str) -> Option<(usize, usize)> {
    let lower = inner.to_ascii_lowercase();
    let (s, e) = (lower.find("<summary")?, lower.find("</summary>")?);
    // The `>` that closes the opening `<summary …>` tag must sit before `</summary>`. Bound the
    // search to `[s, e)`: a malformed `<summary` with no `>` of its own would otherwise match the `>`
    // inside `</summary>` (past `e`), giving an inverted slice range that panics further down. When
    // the opening tag never closes, this reports "no summary" (`None`) — the caller falls back to
    // treating the whole `inner` as the body.
    (s < e && inner[s..e].contains('>')).then_some((s, e))
}

/// Pull the `<summary>` text and the remaining body out of a details block's inner content. Tags in
/// the summary are stripped; the body keeps its Markdown. A missing summary yields an empty string
/// (the renderer supplies a default label).
pub fn extract_summary_body(inner: &str) -> (String, String) {
    if let Some((s, e)) = summary_tag_bounds(inner) {
        // `summary_tag_bounds` already confirmed a `>` sits somewhere in `[s, e)`.
        let gt = inner[s..e]
            .find('>')
            .expect("summary_tag_bounds confirmed a '>' in this range");
        let sum = strip_inline_html_tags(inner[s + gt + 1..e].trim());
        // Trim the blank lines around the body, **not** its indentation. `str::trim` also eats
        // leading spaces, and when the body's first content is an indented code block those spaces
        // *are* the block: the four columns disappeared and the code rendered as an ordinary
        // paragraph, so the screen showed one code block fewer than the source had and `y c` was
        // refused for the whole document (found while auditing this bug class, 2026-08; a
        // `<details>` whose body starts with prose was unaffected, which is why it went unnoticed). A
        // fenced block survives `trim` because its delimiter is not indentation-sensitive — only the
        // indented kind is.
        let body = trim_blank_lines(&inner[e + "</summary>".len()..]);
        return (sum, body);
    }
    (String::new(), inner.trim().to_string())
}

/// The byte offset, into `inner`, right after a well-formed `<summary>...</summary>` tag pair —
/// `None` when `inner` has no such pair (matching `extract_summary_body`'s own "no summary"
/// fallback: the caller should treat `inner` as the whole body, from byte `0`, in that case). Used by
/// `model::glued_details_body_range` to compute the raw byte range of a **glued** `<details>` block's
/// own body (see that function's own doc comment) — the identical boundary `extract_summary_body`'s
/// own second return value is sliced from, before `trim_blank_lines` runs, but as an offset rather
/// than an owned, trimmed `String`: a byte-range caller needs to know *where* the body starts in
/// `inner`'s own coordinate space to slice a **different**, larger string (`model.rs`'s own whole-
/// document `src`) by the identical amount, not read the trimmed text itself (the model's own
/// `Doc::parse`, handed that byte range, does its own, real block-level parsing — see
/// `BlockKind::Details.glued_body`'s own doc comment for why leading/trailing blank lines need no
/// special handling there the way `trim_blank_lines` gives this function's own sibling).
pub fn summary_tag_end(inner: &str) -> Option<usize> {
    summary_tag_bounds(inner).map(|(_, e)| e + "</summary>".len())
}

/// Drop wholly blank lines from both ends of `s`, leaving the surviving lines' own indentation
/// intact — `str::trim` for a block of Markdown, where leading spaces can be structural.
pub fn trim_blank_lines(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let Some(start) = start else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .unwrap_or(start);
    lines[start..=end].join("\n")
}

/// Remove any inline HTML tags from `s`, keeping the text (for a `<summary>` label).
pub fn strip_inline_html_tags(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        match rest[lt..].find('>') {
            Some(gt) => rest = &rest[lt + gt + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The `open` attribute of every top-level `<details>` block in `src`, in document order. Derived
/// from `split_details` itself so the collect order is identical to the split/render order and to the
/// `MdItemKind::Details` ordinals — a `<details>` nested inside another **or reachable only through
/// a GitHub alert's body** is swallowed and not counted separately (counting it here, as a flat tag
/// scan once did, drifted the per-ordinal open state so a later block read the wrong slot) — see
/// `render_md_body_nested`'s doc comment, which renders exactly those cases statically for the same
/// reason. A `<details>`-inside-an-alert is invisible to this function specifically because
/// `split_details` sees the raw, not-yet-alert-stripped source, where such a block's opening tag is
/// still `>`-prefixed and so does not match `details_open_tag`. Fence-aware via `split_details`.
/// Used by the app to seed the default open state.
pub fn collect_details_open(src: &str) -> Vec<bool> {
    split_details(&SourceRun::parse(src.to_string()))
        .into_iter()
        .filter_map(|p| match p {
            DetailsPart::Details { open_attr, .. } => Some(open_attr),
            DetailsPart::Text(_) => None,
        })
        .collect()
}

// ---- Inline HTML (`<kbd>`, `<del>`, `<sup>`, `<sub>`, `<br>`) ----

/// Superscript form of a char, or None if it has no clean superscript.
pub fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'][c as usize - '0' as usize],
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => return None,
    })
}

/// Subscript form of a char, or None if it has no clean subscript.
pub fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0'..='9' => ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'][c as usize - '0' as usize],
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        _ => return None,
    })
}

/// Map every char through `f`; return the mapped string only if ALL chars mapped, else the original
/// (so `<sup>2</sup>` becomes `²` but `<sup>note</sup>` keeps its text rather than mangling it).
pub fn map_all_or_keep(inner: &str, f: impl Fn(char) -> Option<char>) -> String {
    match inner.chars().map(f).collect::<Option<String>>() {
        Some(s) => s,
        None => inner.to_string(),
    }
}

/// Replace `<tag>inner</tag>` pairs (no attributes) in `s` with `f(inner)`. Case-sensitive, literal.
pub fn replace_tag_pair(s: &str, tag: &str, f: impl Fn(&str) -> String) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::new();
    let mut rest = s;
    while let Some(o) = rest.find(&open) {
        let after = o + open.len();
        let Some(c) = rest[after..].find(&close) else {
            break;
        };
        out.push_str(&rest[..o]);
        out.push_str(&f(&rest[after..after + c]));
        rest = &rest[after + c + close.len()..];
    }
    out.push_str(rest);
    out
}

/// The spellings of a line break tag that [`rewrite_br`] recognizes. None of the six is a prefix of
/// another (they already differ at index 3), so at most one can match at a given position.
pub const BR_TAGS: [&str; 6] = ["<br>", "<br/>", "<br />", "<BR>", "<BR/>", "<BR />"];

/// The first line break tag in `s`, as `(byte offset, the spelling that matched)`.
pub fn next_br(s: &str) -> Option<(usize, &'static str)> {
    BR_TAGS
        .iter()
        .filter_map(|t| s.find(t).map(|i| (i, *t)))
        .min_by_key(|(i, _)| *i)
}

/// How a `<br>` on one particular line may be rewritten. Which one applies is decided per line from
/// [`structure_mask`]; see [`process_inline_html_traced`] for the failure each variant prevents.
pub enum BrMode<'a> {
    /// Inside the body of an HTML block: a hard break, with **every line it would leave blank
    /// dropped instead of emitted** — including all of them, in which case the source line produces
    /// no output line at all.
    ///
    /// Stated as the invariant rather than as a rule about where the tag sits, because the blank
    /// line *is* the whole hazard: an HTML block runs to the next blank line, and a whitespace-only
    /// line is blank to CommonMark. Splitting `x<br>y` inside a block yields two non-blank lines and
    /// leaves the block intact, so there is nothing to protect against there — whereas a line that
    /// is nothing but the tag (`    <br>`, the centered-banner idiom) becomes whitespace and kills
    /// the block, and no substitution can save it, a single space least of all. Removing the line
    /// does, and it costs no output: see [`process_inline_html_traced`] for why dropping is the only
    /// safe way to keep the block whole, and what keeping the tag instead did to `shlex`.
    ///
    /// `prefix` is the enclosing blockquote marker, and empty for a block at top level or in a
    /// `<details>` body. It makes "blank" mean *blank to the block* rather than blank to the file:
    /// when the banner is quoted, the same lone-tag line reads `">     <br>"` and rewriting it
    /// yields `">       "`, which `str::trim` calls non-empty while the quote's own body sees
    /// nothing but whitespace — so without the prefix the block died exactly as it did unquoted.
    /// It is carried here rather than read off the line because only [`structure_mask`] can tell a
    /// quote marker from a `>` that is body text of a top-level block (see
    /// [`Structure::HtmlBody`]).
    HtmlBody { prefix: &'a str },
    /// Replace it with a single space, keeping the text on one line.
    Space,
    /// Turn it into a Markdown hard break, starting the injected continuation line with `prefix`
    /// (the enclosing blockquote marker, empty outside a quote).
    Hard { prefix: &'a str },
}

/// The blockquote prefix of `line`: the leading run matching `^ {0,3}(>[ ]?)+`, returned **verbatim**
/// so that nesting (`> > `) and the author's own spacing survive. Empty when the line is not a
/// blockquote line, which is the common case and reproduces the pre-2026-08 behavior exactly.
///
/// Taken from the line rather than reconstructed from a nesting depth because a reconstruction would
/// have to guess: `>text`, `> text` and `> > text` all need different prefixes, and normalizing them
/// would rewrite lines that have nothing to do with the `<br>` being processed.
pub fn blockquote_prefix(line: &str) -> &str {
    let b = line.as_bytes();
    // CommonMark allows a block marker up to 3 columns of indentation; at 4 it is code, and
    // `structure_mask`'s own code gate has already excluded those lines from reaching here.
    let mut i = 0;
    while i < 3 && i < b.len() && b[i] == b' ' {
        i += 1;
    }
    if b.get(i) != Some(&b'>') {
        return "";
    }
    let mut end = i;
    while b.get(end) == Some(&b'>') {
        end += 1;
        // One optional space after each marker — that space belongs to the marker, not the content.
        if b.get(end) == Some(&b' ') {
            end += 1;
        }
    }
    &line[..end]
}

/// Rewrite every line break tag in `s` as a Markdown hard break, starting each line the break
/// injects with `prefix`.
pub fn br_hard(s: &str, prefix: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((at, tag)) = next_br(rest) {
        out.push_str(&rest[..at]);
        rest = &rest[at + tag.len()..];
        // Nothing but whitespace after the tag: the source line's own newline already supplies the
        // break, so emit the two-space hard-break marker and stop there. Adding a newline as well
        // would produce a *blank* line, which ends the paragraph — and, inside a container, the
        // container with it. (An inline code span left behind by `mask_code_spans` is a NUL
        // placeholder, not whitespace, so a span following the tag correctly counts as "not at the
        // end".)
        if rest.trim().is_empty() {
            out.push_str("  ");
        } else {
            out.push_str("  \n");
            out.push_str(prefix);
        }
    }
    out.push_str(rest);
    out
}

/// Whether `line` is blank **to the block that encloses it**: whitespace-only once `prefix` — the
/// enclosing blockquote marker, empty outside a quote — is taken off the front. A quote marker is
/// container, not content, so `">   "` ends an HTML block nested in a quote exactly as `"   "` ends
/// one at top level. With an empty prefix this is `str::trim().is_empty()`, byte for byte.
pub fn blank_within(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix).unwrap_or(line).trim().is_empty()
}

/// Replace every line break tag in `s` with a single space.
pub fn br_space(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((at, tag)) = next_br(rest) {
        out.push_str(&rest[..at]);
        rest = &rest[at + tag.len()..];
        out.push(' ');
    }
    out.push_str(rest);
    out
}

/// Rewrite every line break tag in `s` according to `mode`.
///
/// Replaces a `s.replace("<br>", "  \n")` chain, which could not express the three distinctions
/// that turned out to matter: whether the tag is the last thing on the line, what the line the
/// break injects has to start with, and whether injecting it would leave a blank line where the
/// enclosing block cannot survive one. See [`process_inline_html_traced`] for why each matters.
///
/// Returns the empty string when [`BrMode::HtmlBody`] drops every line it produced; the caller must
/// then emit no line at all (and no `origin` entry for it).
pub fn rewrite_br(s: &str, mode: &BrMode<'_>) -> String {
    match mode {
        BrMode::Hard { prefix } => br_hard(s, prefix),
        BrMode::Space => br_space(s),
        BrMode::HtmlBody { prefix } => {
            let hard = br_hard(s, prefix);
            // A blank line terminates the block, so emit none: drop each produced line that is
            // whitespace-only rather than substituting something into it. Note this is per produced
            // line, not all-or-nothing — `a<br><br>b` leaves a blank line *between* two non-blank
            // ones, and dropping just that one keeps both halves and the block. Lines with no blank
            // among them come back byte-identical to `hard`, so the common `x<br>y` is untouched.
            // A masked code span is a NUL placeholder, not whitespace, so a line holding only a
            // code span correctly counts as having content.
            let kept: Vec<&str> = hard
                .split('\n')
                .filter(|l| !blank_within(l, prefix))
                .collect();
            kept.join("\n")
        }
    }
}

/// Convert the common inline HTML that GitHub renders (but tui-markdown strips) into Markdown/Unicode
/// it renders faithfully: `<del>/<s>/<strike>` → strikethrough, `<kbd>` → an inline-code keycap,
/// `<sup>/<sub>` → Unicode (when the content maps), `<br>` → a line break (in one of three forms —
/// see [`process_inline_html_traced`]). Code-aware (fenced **or indented**); a no-op per line without
/// any of these tags. `<mark>`/`<ins>` have no faithful terminal form and are left to tui-markdown
/// (their tags are stripped, text kept).
/// Convenience form without origin tracking. Production always goes through
/// [`process_inline_html_traced`] (it needs the origins); this is the shorthand the tests use.
#[cfg(any(test, feature = "test-hooks"))]
pub fn process_inline_html(src: &str) -> String {
    process_inline_html_traced(src, &identity_origin(src)).0
}

/// For each line of a pre-pass's output, which line of the **body** (the text before any pre-pass
/// ran) it came from. `None` marks a line no source line can be held responsible for: one a pre-pass
/// invented outright (the appended footnotes section) or the tail of a line it split in two (the text
/// after a `<br>`).
///
/// This exists so a write-back can prove *correspondence* rather than infer it. The checkbox toggle
/// edits bytes of the file on disk, but the checkbox it must edit is identified by its position on
/// screen — i.e. in the preprocessed text. Counting checkboxes on both sides and comparing the totals
/// looks like a check but is not one: the released build's guards compared count and state character,
/// and a document that simultaneously hid one checkbox from the screen and revealed a different one
/// to the scanner satisfied both while the ordinals pointed at different lines, so `Space` silently
/// wrote `x` into a line the reader was not looking at (verified end to end against v0.23.5). A
/// per-line origin turns "the Nth of each happens to tally" into "this exact screen line came from
/// that exact source line", and where no origin exists the toggle refuses instead of guessing.
pub type LineOrigin = Vec<Option<usize>>;

/// The identity mapping for a body that has not been through any pre-pass yet.
pub fn identity_origin(src: &str) -> LineOrigin {
    (0..src.lines().count()).map(Some).collect()
}

/// [`process_inline_html`] that also composes the line origins (see [`LineOrigin`]). A `<br>` is the
/// only substitution that can add a line: the fragment before it keeps the origin, everything after
/// it gets `None`, because that text was never a line of its own in the file and so has no line
/// whose bytes a write-back could target.
///
/// # How a `<br>` is rewritten, and why it is not one rule
///
/// Root cause (2026-08, found by rendering every README in the local crate registry): rewriting
/// `<br>` unconditionally into `"  \n"` **injects a line that carries none of the prefix its
/// enclosing block requires**, so the block it lives in is torn apart. `split_alerts` captures a
/// callout body by consuming consecutive `>`-prefixed lines, so an injected unprefixed line ended
/// the box mid-way; the rest of the alert — including a fenced code block inside it — then leaked
/// out as literal `> ```rust` text. The same injection ended an HTML block, because the tag often
/// sits alone on its own indented line (`    <br>` between two badge `<a>` blocks is the standard
/// centered-banner idiom) and `"    " + "  "` is a whitespace-only line, which *is* a blank line to
/// CommonMark: the block terminated there and every following indented line became a genuine
/// indented code block. `raw-window-metal` and `static_assertions` both drew their badges as a
/// `▎ …` code block for exactly that reason.
///
/// So the governing rule is: **a `<br>` may only become a hard break when the line it injects still
/// belongs to the same block.** Three cases, decided per line from [`structure_mask`]:
///
/// * [`Structure::HtmlBody`] → a hard break, but **any line that would come out blank is dropped
///   instead of emitted** ([`BrMode::HtmlBody`]) — so a line that is nothing but the tag produces no
///   output line at all. No *substitution* is safe here, not even a single space, because the tag is
///   frequently the whole line and any whitespace-only result still reads as blank and still ends
///   the block; removing the line is the only rewrite that keeps the block whole. It costs no
///   output either: konoma renders no HTML inside an HTML block anyway — `split_html_blocks` hands
///   the block to `render_html_block`, which strips every tag and drops the lines that become empty
///   — so the screen content is identical to what keeping the tag would have drawn.
///
///   **Keeping the tag was tried first and is wrong**, which is worth recording because the failure
///   is not visible from here (found 2026-08 by rendering every `.md` in the local crate registry;
///   `shlex`'s `quoting_warning.md` was the one file it damaged). A preserved tag is still a token
///   *this* module treats as a block opener — [`is_html_block_start`] accepts any tag name — and a
///   **later** pass may move the text somewhere the surrounding blank lines no longer exist.
///   `process_footnotes` does exactly that: it relocates definition bodies into the section it
///   appends at the end of the document. A footnote definition whose body held a lone `  <br>` line
///   arrived there with the tag intact, at the head of an unbroken run of lines, where it opened an
///   HTML block that ran to the end of the document — handing the *following* footnotes to
///   `render_html_block`, which emits their text without parsing Markdown at all, so their inline
///   code stopped rendering and drew as literal backticks. Dropping the line leaves nothing behind
///   that a later pass can plant, which is why it is preferred over any rule that keeps the tag
///   "only when it looks safe here".
///
///   The block's *opening* line is deliberately excluded and takes the default below; see
///   [`Structure::HtmlOpen`] for why dropping it there would delete a break the document really has.
/// * [`Structure::Table`] → **a single space** ([`BrMode::Space`]). There is no prefix that makes a
///   second line a valid continuation of a table row, and `render_table` draws one screen row per
///   source row, so any line injected into a row becomes a second, malformed row (`| x<br>y | z |`
///   drew as `│ x │   │` + `│ y │ z │`). Keeping the text on one line is the only rendering that
///   does not corrupt the table — an honest degradation (principle #3): the text is preserved and
///   the markup does not leak. A table row always contains pipes, so it can never collapse to a
///   blank line this way.
/// * anything else → **a hard break** ([`BrMode::Hard`]), with the two corrections that make it
///   correct rather than merely usual: the injected line repeats the current line's blockquote
///   prefix ([`blockquote_prefix`], taken verbatim so `> > ` nesting survives), keeping a quote or
///   alert body one contiguous run of `>` lines; and a `<br>` at the end of a line emits only the
///   `"  "` marker and no newline, because the source already supplies the break and adding another
///   makes a blank line that ends the paragraph — and with it the container. Outside a quote the
///   prefix is empty and the output is byte-identical to the old behavior.
///
/// A `<br>` inside a list item, a `<details>` body or a plain paragraph needs no correction and gets
/// none: CommonMark lazy continuation keeps an unprefixed line attached to the list/quote paragraph
/// above it, and `split_details` spans blank lines, so those three were never broken.
pub fn process_inline_html_traced(src: &str, origin_in: &[Option<usize>]) -> (String, LineOrigin) {
    let mut out = String::new();
    let mut origin = LineOrigin::new();
    let lines: Vec<&str> = src.lines().collect();
    // One pulldown-cmark parse serving both masks (see `literal_code_mask_from`): the union decides
    // "leave this line alone entirely", while `structure_mask` — like every block splitter it
    // mirrors — gates on the parser's answer on its own.
    let parsed_code = code_block_mask(&lines);
    let in_code = literal_code_mask_from(&parsed_code, &lines);
    // Which block each line belongs to, from the one implementation of those rules. Deliberately not
    // re-derived here: a fourth hand-written copy of "is this an HTML block / a table row" is exactly
    // the drift this module keeps regressing on (see `splitter_code_mask`).
    let structure = structure_mask(&lines, &parsed_code);
    for (i, line) in lines.iter().enumerate() {
        let line = *line;
        let src_line = origin_in.get(i).copied().flatten();
        if in_code[i] {
            out.push_str(line);
            out.push('\n');
            origin.push(src_line);
            continue;
        }
        if !line.contains('<') {
            out.push_str(line);
            out.push('\n');
            origin.push(src_line);
            continue;
        }
        // Inline code spans are literal: `` `<kbd>x</kbd>` `` is documentation *of* the tag, not a
        // tag to convert. Converting it also wrapped the content in a second pair of backticks
        // (`` ``x`` ``) and a `<br>` inside a span injected a newline, breaking the span outright.
        // The spans are masked rather than cut out, so a tag pair may still wrap one.
        // Which of the three `<br>` rewrites this line gets (see the doc comment). The blockquote
        // prefix is read off the raw line, not the code-span-masked one: a code span begins with a
        // backtick, so it can never be part of a leading `>` run, and the two agree by construction.
        let mode = match structure[i] {
            // The prefix is empty unless the block is nested in a blockquote, and only
            // `structure_mask` can say: a top-level `<div>` whose body text starts with `>` looks
            // identical from here, and there the marker is content — repeating it would inject a
            // `>` the document never had. See `Structure::HtmlBody`.
            Structure::HtmlBody { quoted } => BrMode::HtmlBody {
                prefix: if quoted { blockquote_prefix(line) } else { "" },
            },
            Structure::Table => BrMode::Space,
            // `HtmlOpen` belongs with the default, not with `HtmlBody`: there is no already-open
            // block to protect on an opening line, and konoma calls a line starting with `<br>` an
            // opener at all only because `is_html_block_start` accepts any tag name (see
            // `Structure::HtmlOpen`). Dropping the line there would delete a break the document
            // really has: `text` / `<br>` / `more`, the ordinary way to force a gap in prose, is a
            // *lone* `<br>` line and konoma classifies it as an opener, so `HtmlBody` treatment
            // would rewrite it to nothing and silently join the two paragraphs. The default leaves
            // the whitespace-only line it has always produced, which reads as the blank line the
            // author asked for. Measured rather than assumed: routing `HtmlOpen` here too changes 5
            // more of the 2182 registry `.md` files against the released rendering, and in each the
            // damage is that deletion — `bit-set`/`bit-vec`'s README asks for a gap with two `<br />`
            // lines and got its two badge rows run together into one.
            //
            // Includes `Structure::Details` and a nested quote inside one too: reading the prefix
            // off the line itself means a `> quoted<br>tail` line in a `<details>` body still gets
            // its marker repeated, without this needing to know it is nested.
            Structure::None | Structure::Quote | Structure::Details | Structure::HtmlOpen => {
                BrMode::Hard {
                    prefix: blockquote_prefix(line),
                }
            }
        };
        let s = rewrite_masking_code_spans(line, |masked| {
            // A backslash-escaped `\<` is a literal `<` (CommonMark), never the start of a tag:
            // `x\<br># h` must not become a line break that opens a heading.
            let masked = mask_escaped_lt(masked);
            let mut s = replace_tag_pair(&masked, "kbd", |i| format!("`{i}`"));
            s = replace_tag_pair(&s, "del", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "s", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "strike", |i| format!("~~{i}~~"));
            s = replace_tag_pair(&s, "sup", |i| map_all_or_keep(i, sup_char));
            s = replace_tag_pair(&s, "sub", |i| map_all_or_keep(i, sub_char));
            rewrite_br(&s, &mode).replace(ESCAPED_LT, "\\<")
        });
        // `BrMode::HtmlBody` drops the blank lines it would have produced, so a line that was
        // nothing but the tag comes back empty: emit nothing for it. Every line reaching here
        // contains a `<`, so it was not empty to begin with, and an empty result can only be that
        // drop. Gated on the mode so the other two keep emitting whatever they produce — outside an
        // HTML block a blank line is meaningful (it ends a paragraph), and removing it would be a
        // silent rendering change rather than a rescue.
        if matches!(mode, BrMode::HtmlBody { .. }) && s.is_empty() {
            continue;
        }
        out.push_str(&s);
        out.push('\n');
        // Only the first fragment still starts where the source line started; a `<br>` tail is new
        // text at a position no source line owns. The count is derived from the produced string, so
        // it stays in step with whichever `BrMode` ran — including the cases that emit *no* newline
        // (a trailing `<br>`, or `Space`) and the one that emits no line at all (a bare `<br>` line
        // inside an HTML block, which `continue`d above without pushing an entry). A drift here is a
        // correctness bug rather than a cosmetic one: `LineOrigin` is what proves the checkbox
        // write-back edits the checkbox the reader is looking at.
        origin.push(src_line);
        origin.extend(std::iter::repeat_n(None, s.matches('\n').count()));
    }
    (out, origin)
}

// ---- Footnotes (`text[^1]` … `[^1]: definition`) ----

/// Superscript form of a number (`12` → `¹²`), for rendering a footnote reference marker.
pub fn to_superscript(n: usize) -> String {
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    n.to_string()
        .chars()
        .map(|c| SUP[c.to_digit(10).unwrap_or(0) as usize])
        .collect()
}

/// One footnote definition as the parser delimits it: the id, the definition's full text (label
/// stripped, continuation lines kept and dedented to their own left margin), and the half-open range
/// of source lines it occupies.
pub struct FootnoteDef {
    id: String,
    text: String,
    lines: std::ops::Range<usize>,
}

/// Parse options for locating footnote definitions. Deliberately a *separate* value from
/// [`markdown_parse_options`] — enabling `ENABLE_FOOTNOTES` there would change what the renderer
/// itself parses, and the whole point of `process_footnotes` is that footnotes are rewritten into
/// plain Markdown *before* the renderer ever sees them.
pub fn footnote_parse_options() -> ParseOptions {
    let mut o = markdown_parse_options();
    o.insert(ParseOptions::ENABLE_FOOTNOTES);
    o
}

/// Every footnote definition in `lines`, **asking pulldown-cmark** rather than re-deriving GFM's
/// continuation rules.
///
/// Root cause this replaced (2026-08, reported by a user on the released build): definitions were
/// matched one line at a time, so only the *first* line of a multi-line definition was moved into
/// the footnotes section. Everything after it stayed in the body — and since a continuation is
/// indented, it landed there as a **phantom indented code block**, while the footnote itself was left
/// truncated mid-syntax (rand_chacha's README ends up showing an unclosed `[label](` and a stray
/// code block containing the URL). The count of code blocks on screen then disagreed with the source
/// scanner's, so `y c` was refused for the whole document.
///
/// Getting this right by hand means implementing container-relative indentation, lazy continuation
/// and blank-line-separated paragraphs inside a definition — i.e. re-implementing the block parser,
/// which is exactly the drift this module keeps regressing on (see [`splitter_code_mask`]). So the
/// definition's extent is the parser's answer, and only the parts the parser cannot tell us — which
/// lines konoma itself will draw as literal code — stay local, via [`literal_code_mask`]: a
/// definition the union mask calls code is dropped, so a `[^1]:` inside a fence that only konoma's
/// own splitting makes literal (see that mask's doc comment) is still left verbatim.
pub fn footnote_defs(lines: &[&str], in_code: &[bool]) -> Vec<FootnoteDef> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    if lines.is_empty() {
        return Vec::new();
    }
    let text = lines.join("\n");
    // `starts[i]` = byte offset of line `i` in `text`; the trailing sentinel is one past where a
    // line after the last would start. Mirrors `code_block_mask`'s mapping exactly.
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut pos = 0usize;
    for line in lines {
        starts.push(pos);
        pos += line.len() + 1;
    }
    starts.push(pos);

    let mut out: Vec<FootnoteDef> = Vec::new();
    let mut depth = 0usize;
    let mut open: Option<(String, usize)> = None;
    for (ev, range) in Parser::new_ext(&text, footnote_parse_options()).into_offset_iter() {
        match ev {
            Event::Start(Tag::FootnoteDefinition(name)) => {
                // Only the outermost definition matters: a definition nested inside another is part
                // of that one's body and moves with it.
                if depth == 0 {
                    open = Some((name.to_string(), range.start));
                }
                depth += 1;
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                depth = depth.saturating_sub(1);
                if depth > 0 {
                    continue;
                }
                let Some((id, start)) = open.take() else {
                    continue;
                };
                let lo = starts.partition_point(|&s| s <= start).saturating_sub(1);
                let hi = starts[..lines.len()].partition_point(|&s| s < range.end);
                if lo >= hi || hi > lines.len() {
                    continue;
                }
                // The parser's range runs to the end of the blank lines that terminate the
                // definition; those are not part of it.
                let mut end = hi;
                while end > lo + 1 && lines[end - 1].trim().is_empty() {
                    end -= 1;
                }
                if in_code[lo..end].iter().any(|&c| c) {
                    continue; // literal code as konoma draws it — leave the whole run alone
                }
                let Some(text) = footnote_def_text(&lines[lo..end]) else {
                    continue;
                };
                out.push(FootnoteDef {
                    id,
                    text,
                    lines: lo..end,
                });
            }
            _ => {}
        }
    }
    out
}

/// The definition's own text: the `[^id]:` label stripped off the first line, and the continuation
/// lines dedented by their common left margin so the text can be re-indented under a list marker
/// when the footnotes section is emitted. Returns `None` if the first line is not a definition
/// (defensive — the parser said it was).
pub fn footnote_def_text(block: &[&str]) -> Option<String> {
    let (_, first) = parse_footnote_def(block.first()?)?;
    let rest = &block[1..];
    // Common indent over the non-blank continuation lines, measured in *whitespace characters*
    // (not bytes). `trim_start()`/`trim()` strip any Unicode-whitespace character (U+3000
    // ideographic space, U+00A0 no-break space, ...), so a byte length measured on one line is not
    // a valid offset on another: a byte count from a line that starts with an ASCII space can point
    // inside a multi-byte whitespace character on a different line. A character count doesn't have
    // that problem — stripping it below walks characters (`strip_leading_ws_chars`), so it can only
    // ever land on a char boundary, on any line. A lazy continuation (indent 0) makes this 0 and the
    // lines are used as-is.
    let indent = rest
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().count() - l.trim_start().chars().count())
        .min()
        .unwrap_or(0);
    let mut text = first;
    for l in rest {
        text.push('\n');
        if l.trim().is_empty() {
            continue; // a blank line separating paragraphs inside the definition carries no indent
        }
        let line_indent = l.chars().count() - l.trim_start().chars().count();
        text.push_str(strip_leading_ws_chars(l, indent.min(line_indent)));
    }
    Some(text)
}

/// Strip the first `n` *characters* (not bytes) of whitespace from the front of `line`, returning
/// the rest verbatim. Used by `footnote_def_text` to dedent by a character count derived from
/// `trim_start()`: walking characters (via `char_indices`) keeps the resulting slice on a char
/// boundary even when `n` is less than the line's own leading-whitespace run and that run mixes
/// ASCII and multi-byte Unicode whitespace. Callers only ever pass `n <= line_indent` for a line
/// whose `trim()` is non-empty, so `n` always lands inside (or right at the end of) the leading
/// whitespace run, never past the end of the line.
pub fn strip_leading_ws_chars(line: &str, n: usize) -> &str {
    match line.char_indices().nth(n) {
        Some((idx, _)) => &line[idx..],
        None => "",
    }
}

/// If the line is a footnote definition `[^id]: text`, return `(id, text)`.
pub fn parse_footnote_def(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let rest = t.strip_prefix("[^")?;
    let close = rest.find(']')?;
    let id = &rest[..close];
    if id.is_empty() || id.contains('[') {
        return None;
    }
    let after = rest[close + 1..].strip_prefix(':')?;
    Some((id.to_string(), after.trim().to_string()))
}

/// Scan `line` for footnote references `[^id]`, returning the ids in order. References inside an
/// inline code span are literal examples of the syntax, not references, and are skipped — the same
/// rule `replace_footnote_refs` applies, so a `` `[^1]` `` is neither numbered nor rewritten.
pub fn find_footnote_refs(line: &str) -> Vec<String> {
    let mut ids = Vec::new();
    collect_footnote_refs(&mask_code_spans(line).0, &mut ids);
    ids
}

/// `find_footnote_refs` for one run of text already known to be outside any code span.
pub fn collect_footnote_refs(line: &str, ids: &mut Vec<String>) {
    let mut i = 0;
    while i < line.len() {
        if line[i..].starts_with("[^") {
            if let Some(close) = line[i + 2..].find(']') {
                let id = &line[i + 2..i + 2 + close];
                if !id.is_empty() && !id.contains('[') {
                    ids.push(id.to_string());
                    i += 2 + close + 1;
                    continue;
                }
            }
        }
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }
}

/// Replace footnote references `[^id]` in `line` with a superscript number (only ids present in
/// `num`; an undefined reference is left literal, matching GitHub). Inline code spans are copied
/// verbatim — see `find_footnote_refs`.
pub fn replace_footnote_refs(line: &str, num: &std::collections::HashMap<String, usize>) -> String {
    rewrite_masking_code_spans(line, |masked| {
        replace_footnote_refs_outside_code(masked, num)
    })
}

/// `replace_footnote_refs` for one run of text already known to be outside any code span.
pub fn replace_footnote_refs_outside_code(
    line: &str,
    num: &std::collections::HashMap<String, usize>,
) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < line.len() {
        if line[i..].starts_with("[^") {
            if let Some(close) = line[i + 2..].find(']') {
                let id = &line[i + 2..i + 2 + close];
                if !id.is_empty() && !id.contains('[') {
                    if let Some(&n) = num.get(id) {
                        out.push_str(&to_superscript(n));
                        i += 2 + close + 1;
                        continue;
                    }
                }
            }
        }
        let ch_len = line[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Rewrite `src` so GFM footnotes render: references `[^id]` become superscript numbers (ordered by
/// first appearance), the `[^id]: …` definitions are pulled out of the body, and a numbered
/// footnotes section is appended after a rule. Code-aware (fenced **or indented**) — refs/defs
/// inside a code block are left verbatim. A no-op (returns `src` unchanged) when there are no
/// definitions.
///
/// A definition may span several lines: its continuation lines (indented, lazy, or further
/// paragraphs) are part of it and move into the section with it, so a reference-style link split
/// across two lines stays a single working link. The extent of each definition comes from
/// [`footnote_defs`] — the parser's answer — rather than a line-shaped guess here; see that
/// function's doc comment for the bug that motivated it.
/// Convenience form without origin tracking. Production always goes through
/// [`process_footnotes_traced`] (it needs the origins); this is the shorthand the tests use.
#[cfg(any(test, feature = "test-hooks"))]
pub fn process_footnotes(src: &str) -> String {
    process_footnotes_traced(src, &identity_origin(src)).0
}

/// [`process_footnotes`] that also composes the line origins (see [`LineOrigin`]). Definition lines
/// disappear from the body, so nothing maps to them; the appended footnotes section is new text and
/// maps to nothing. Every other line keeps its origin, since reference rewriting is in place.
pub fn process_footnotes_traced(src: &str, origin_in: &[Option<usize>]) -> (String, LineOrigin) {
    use std::collections::HashMap;
    let lines: Vec<&str> = src.lines().collect();
    // Per-line "is this literal code on screen" mask (`literal_code_mask`) — all three passes below
    // just need to know, per line, whether to skip/echo it verbatim.
    let in_code = literal_code_mask(&lines);
    // Pass 1: collect definitions (code-aware) and mark **every** line each one occupies.
    let found = footnote_defs(&lines, &in_code);
    let mut defs: Vec<(String, String)> = Vec::new();
    let mut is_def = vec![false; lines.len()];
    for d in &found {
        defs.push((d.id.clone(), d.text.clone()));
        is_def[d.lines.clone()].fill(true);
    }
    if defs.is_empty() {
        return (src.to_string(), origin_in.to_vec());
    }
    let def_ids: std::collections::HashSet<&str> = defs.iter().map(|(id, _)| id.as_str()).collect();
    // Pass 2: number by first reference appearance (code-aware, defs excluded).
    let mut num: HashMap<String, usize> = HashMap::new();
    let mut next = 1usize;
    for (i, line) in lines.iter().enumerate() {
        if in_code[i] || is_def[i] {
            continue;
        }
        for id in find_footnote_refs(line) {
            if def_ids.contains(id.as_str()) && !num.contains_key(&id) {
                num.insert(id, next);
                next += 1;
            }
        }
    }
    if num.is_empty() {
        // definitions present but never referenced → leave as-is
        return (src.to_string(), origin_in.to_vec());
    }
    // Pass 3: rebuild the body (drop def lines, replace refs; code blocks verbatim).
    let mut out = String::new();
    let mut origin = LineOrigin::new();
    for (i, line) in lines.iter().enumerate() {
        if in_code[i] {
            out.push_str(line);
            out.push('\n');
            origin.push(origin_in.get(i).copied().flatten());
            continue;
        }
        if is_def[i] {
            continue; // moved into the section below — no line of the output maps here
        }
        out.push_str(&replace_footnote_refs(line, &num));
        out.push('\n');
        origin.push(origin_in.get(i).copied().flatten());
    }
    // Append the footnotes section, numbered by reference order.
    let mut items: Vec<(usize, &str)> = num
        .iter()
        .map(|(id, &n)| {
            let text = defs
                .iter()
                .find(|(did, _)| did == id)
                .map(|(_, t)| t.as_str())
                .unwrap_or("");
            (n, text)
        })
        .collect();
    items.sort_by_key(|(n, _)| *n);
    // Everything from here on is text this pass invented: no line of the file corresponds to it, so
    // a write-back must never be pointed at one (see `LineOrigin`).
    let body_end = out.len();
    out.push_str("\n---\n\n");
    for (n, text) in items {
        // A multi-line definition has to stay *inside* its numbered item. Leaving the continuation
        // flush-left would work for the first item by lazy continuation, but the next item's marker
        // (`2.`) cannot interrupt a paragraph in CommonMark — only a `1.` can — so item 2 onwards
        // would be swallowed into item 1's text. Indenting to the marker's content column makes each
        // continuation an unambiguous part of its own item, whatever the item's number is.
        let marker = format!("{n}. ");
        let pad = " ".repeat(marker.len());
        out.push_str(&marker);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
                if !line.is_empty() {
                    out.push_str(&pad);
                }
            }
            out.push_str(line);
        }
        out.push('\n');
    }
    origin.extend(std::iter::repeat_n(None, out[body_end..].lines().count()));
    (out, origin)
}

// ---- YAML front matter (leading `---` … `---`) ----

/// Split off leading YAML front matter (a `---` fence on the very first line, closed by `---` or
/// `...` on its own line). Returns `(Some(inner), body)` when present, else `(None, src)`. Only the
/// document start is considered, matching GitHub — a later `---` is an ordinary thematic break.
pub fn strip_front_matter(src: &str) -> (Option<String>, String) {
    let lines: Vec<&str> = src.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return (None, src.to_string());
    }
    let Some(rel) = lines[1..]
        .iter()
        .position(|l| matches!(l.trim_end(), "---" | "..."))
    else {
        return (None, src.to_string()); // no closing fence → not front matter
    };
    let close = rel + 1;
    let inner = lines[1..close].join("\n");
    let body = lines[close + 1..].join("\n");
    (Some(inner), body)
}

/// Whether the line is a GFM table delimiter row (e.g. `|---|:--:|`). Contains `-` and `|`, with only spaces/`-`/`:`/`|` as characters.
pub fn is_table_delimiter(line: &str) -> bool {
    let t = line.trim();
    if !t.contains('-') || !t.contains('|') {
        return false;
    }
    t.chars().all(|c| matches!(c, ' ' | '\t' | '-' | ':' | '|'))
}

/// Whether this is a table row candidate (a non-empty line containing `|`).
pub fn looks_like_table_row(line: &str) -> bool {
    line.contains('|') && !line.trim().is_empty()
}
