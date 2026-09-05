//! Turning a label's text into a measured block of lines.
//!
//! Every box in the diagram is sized from this, and everything downstream — rank assignment,
//! ordering, coordinates — is derived from the box sizes, so this is the top of the causal chain
//! (`text_metrics`'s module docs say the same thing from the other end).
//!
//! Two decisions worth stating because they are not obvious:
//!
//! * **Width comes from [`text_metrics`], never from a per-character estimate.** That module
//!   measures with the same font database and the same font-resolution rules resvg will use, and
//!   the test that pins the two together is this renderer's main instrument
//!   (`docs/FEATURE-MERMAID-RENDERER.md` §6).
//! * **Height is a declared constant, not a measurement.** mermaid gets it from `getBBox()` on a
//!   real `<text>`, which has no counterpart here; §4-4 records that konoma is effectively
//!   `htmlLabels: false`, where mermaid's own line spacing is `1.1em`. Using that ratio keeps the
//!   number honest about being a convention rather than a measurement.
//!
//! Under `Routing::Splines` there is **no automatic wrapping**. §2-5 puts `wrappingWidth` in the
//! "not implemented" list (it is a px-space idea), so a line breaks only where the author wrote
//! `<br>` or `\n` — which the parser has already turned into a real newline by the time the text
//! arrives here. `Routing::Orthogonal` is the one exception, and it is a rule of its own rather
//! than mermaid's: §10-8 N2 caps a node box at 240px and wraps past it — see
//! [`Label::measure_wrapped`].

use crate::preview::mermaid::flowchart;
use crate::preview::mermaid::text_metrics::{self, FONT_SIZE};

/// Line spacing as a multiple of the font size. mermaid's SVG-label spacing (§4-4).
pub const LINE_HEIGHT_RATIO: f64 = 1.1;

/// §10-8 N1 ("行間 6"): the clear gap konoma-orthogonal leaves between two lines of a node label,
/// in px. Its line pitch is therefore `FONT_SIZE + ORTHO_LINE_GAP` — see
/// [`Label::measure_wrapped`], which is the only thing that ever builds a label at that pitch.
pub const ORTHO_LINE_GAP: f64 = 6.0;

/// The distance between two successive baselines of a konoma-orthogonal node label (§10-8 N1).
pub fn ortho_line_pitch() -> f64 {
    FONT_SIZE as f64 + ORTHO_LINE_GAP
}

/// Where a line's baseline sits below the middle of its line box, as a multiple of the font size.
///
/// Roughly half a capital's height: sans-serif cap heights cluster around `0.7em`, so centring
/// capitals puts the baseline `0.35em` below centre. CJK, whose ink runs about `-0.88em..0.12em`,
/// wants `0.38em`; one constant covers both within half a pixel at 14px, and picking it by script
/// would make a mixed-script line jump.
pub const BASELINE_RATIO: f64 = 0.35;

/// Height of one line of body text, in px.
pub fn line_height() -> f64 {
    FONT_SIZE as f64 * LINE_HEIGHT_RATIO
}

/// A label, split into lines and measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    /// The lines, in order. Always at least one entry; an empty label is one empty line.
    pub lines: Vec<String>,
    /// Width of the widest line, in px, as [`text_metrics`] measures it.
    pub width: f64,
    /// `lines.len()` line pitches.
    pub height: f64,
    /// The distance between two successive baselines, in px.
    ///
    /// Carried on the label for the same reason [`Label::font_size`] is: it is the number
    /// [`Label::height`] was computed from *and* the number [`super::svg`] steps each `<tspan>`
    /// down by, so keeping one copy is what stops "how tall the box was made" and "how far apart
    /// the words were drawn" from drifting. mermaid's own `1.1em`
    /// ([`LINE_HEIGHT_RATIO`]) everywhere except a konoma-orthogonal node label, whose §10-8 N1
    /// pitch is a declared `14 + 6`.
    pub line_pitch: f64,
    /// The px size this label was **measured at**, and therefore the size [`super::svg`] draws it
    /// at.
    ///
    /// Almost always [`FONT_SIZE`] — the body size every diagram konoma has ever drawn uses, and
    /// what [`Label::measure`] fills in. The exception is an edge label under
    /// `Routing::Orthogonal`, which the design reference draws smaller than the body text
    /// (`docs/FEATURE-MERMAID-RENDERER.md` §10-1 item 5): carrying the size **on the label** is
    /// what keeps "what was measured" and "what is drawn" the same number, so a smaller label
    /// shrinks the plate behind it and the rank the router widens for it, instead of leaving a
    /// box sized for text that is no longer there.
    pub font_size: f64,
}

impl Label {
    /// Measures `text`, splitting it on the newlines the parser already normalised.
    ///
    /// Each line is put through [`text_metrics::collapse_spaces`] **and kept in that form**, so
    /// the string this label carries is the string [`super::svg`] writes and the string usvg
    /// shapes. Collapsing only for the measurement and emitting the author's bytes would still be
    /// correct about the width — usvg folds them either way — but it leaves two spellings of the
    /// same line in the program, and the one a golden pins would not be the one a box was sized
    /// from. Folding once, here, means "measured" and "drawn" cannot drift apart.
    ///
    /// It also removes a second-order trap: usvg folds whitespace across the *whole* `<text>`
    /// element, so a line ending in a space and the next line beginning with one collapse into a
    /// single space that belongs to the second line — a multi-line label would be measured
    /// per-line and drawn as something else. With every line already trimmed there is nothing at
    /// a line boundary to fold.
    pub fn measure(text: &str) -> Label {
        Label::measure_at(text, FONT_SIZE as f64)
    }

    /// [`Label::measure`] at a font size other than the body's — see [`Label::font_size`] for the
    /// one caller that needs it and why the size travels with the label.
    pub fn measure_at(text: &str, font_size: f64) -> Label {
        Label::from_lines(author_lines(text), font_size, font_size * LINE_HEIGHT_RATIO)
    }

    /// [`Label::measure`] at konoma-orthogonal's own line pitch (§10-8 N1), re-wrapping any
    /// author line whose measured width exceeds `budget` px.
    ///
    /// `budget` is the **text** width a box of the §10-8 N2 cap can hold — the caller subtracts
    /// its own padding — so a line that fits is left exactly as the author wrote it and this is a
    /// no-op for every label short enough to need no wrapping, which is nearly all of them.
    ///
    /// Two things this deliberately does not do, both from N2: it never truncates and never adds
    /// an ellipsis, so a single unbreakable run wider than `budget` simply comes back on its own
    /// line and the caller's cap is what gives way; and it wraps a run of CJK per **character**
    /// rather than only at spaces, because CJK is written without them and a space-only rule
    /// would leave a whole Japanese label unwrappable.
    pub fn measure_wrapped(text: &str, budget: f64, font_size: f64) -> Label {
        let lines = author_lines(text)
            .into_iter()
            .flat_map(|line| wrap_line(&line, budget, font_size))
            .collect();
        Label::from_lines(lines, font_size, ortho_line_pitch())
    }

    fn from_lines(lines: Vec<String>, font_size: f64, line_pitch: f64) -> Label {
        let width = lines
            .iter()
            .map(|l| text_metrics::measure(l, font_size as f32) as f64)
            .fold(0.0_f64, f64::max);
        Label {
            height: lines.len() as f64 * line_pitch,
            lines,
            width,
            line_pitch,
            font_size,
        }
    }

    /// Whether the label would draw nothing at all (so a shape need not reserve room for it, and
    /// no `<text>` need be emitted).
    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    /// The same words at a different size, for a palette that draws an edge label smaller than
    /// the body text it labels ([`super::theme::Tokens::edge_label_font_size`]).
    ///
    /// Scaled rather than re-measured, and that is the design of it: a glyph's advance is
    /// proportional to the font size, so `width * (new / old)` **is** the measurement, to within
    /// the hinting a rasteriser applies at the end. Re-measuring would be no more accurate and
    /// would put a second `text_metrics` call on the drawing path.
    ///
    /// Why the size changes here and not at measuring time: the box the layout reserved for an
    /// edge label is an *input to the routing* (`orthogonal::label_min_length` widens a rank to
    /// hold it), so shrinking the label before the layout would move every diagram in the mode —
    /// measured on the state corpus, it collapses `concurrent`'s own perimeter ring into a
    /// sub-pixel doubling-back. What this keeps true is the thing that matters at drawing time —
    /// **the patch behind the words is the size of the words** — while the rank the router opened
    /// for the label stays as roomy as it always was.
    pub fn resized(&self, font_size: f64) -> Label {
        let scale = font_size / self.font_size;
        Label {
            lines: self.lines.clone(),
            width: self.width * scale,
            height: self.height * scale,
            line_pitch: self.line_pitch * scale,
            font_size,
        }
    }

    /// Height of one of this label's own lines, in px.
    pub fn line_height(&self) -> f64 {
        self.line_pitch
    }

    /// Baseline y of line `i` when the block is centred on `center_y`.
    pub fn baseline(&self, center_y: f64, i: usize) -> f64 {
        let top = center_y - self.height / 2.0;
        // Line `i` occupies `[top + i*lh, top + (i+1)*lh]`; its baseline sits `BASELINE_RATIO`
        // of an em below the middle of that box.
        top + (i as f64 + 0.5) * self.line_height() + self.font_size * BASELINE_RATIO
    }
}

/// The lines the *author* wrote, normalised — everything [`Label::measure_at`] and
/// [`Label::measure_wrapped`] both do before either one measures anything.
///
/// `common.getRows` — mermaid splits a label into rows **at render time**, for every diagram kind,
/// on the four `<br>` spellings *and* on a literal `\n`. Doing it here rather than in each parser
/// is what makes it true for all of them at once: the flowchart parser has already done it
/// (`flowchart::text::decode_label`), so this is idempotent there and the flowchart goldens do not
/// move.
fn author_lines(text: &str) -> Vec<String> {
    let text = flowchart::text::replace_breaks(text);
    let text = text.replace("\\n", "\n");
    text.split('\n')
        .map(|l| text_metrics::collapse_spaces(l).into_owned())
        .collect()
}

/// Whether `c` may be broken **before and after** without a space, the way CJK is set (§10-8 N2's
/// "日本語は文字単位で").
///
/// The ranges are the scripts written without word spaces — Han and its two Japanese kana, Hangul,
/// CJK punctuation, and the fullwidth/halfwidth compatibility forms — not "anything non-ASCII":
/// Cyrillic or Greek are spaced languages and wrapping one mid-word would be a bug, not a feature.
fn breaks_anywhere(c: char) -> bool {
    matches!(c,
        '\u{1100}'..='\u{11ff}'   // Hangul Jamo
        | '\u{2e80}'..='\u{303f}' // CJK radicals, Kangxi, CJK symbols and punctuation
        | '\u{3040}'..='\u{30ff}' // Hiragana, Katakana
        | '\u{3400}'..='\u{4dbf}' // CJK Unified Ideographs Extension A
        | '\u{4e00}'..='\u{9fff}' // CJK Unified Ideographs
        | '\u{a960}'..='\u{a97f}' // Hangul Jamo Extended-A
        | '\u{ac00}'..='\u{d7ff}' // Hangul syllables, Jamo Extended-B
        | '\u{f900}'..='\u{faff}' // CJK Compatibility Ideographs
        | '\u{fe30}'..='\u{fe4f}' // CJK Compatibility Forms
        | '\u{ff00}'..='\u{ffef}' // Halfwidth and fullwidth forms
        | '\u{20000}'..='\u{3ffff}' // CJK Unified Ideographs Extensions B..
    )
}

/// One author line broken into the smallest pieces a line break is allowed to fall between: a
/// space, one CJK character, or one run of everything else (a "word").
///
/// A space is kept as its own atom rather than glued to a neighbour so that [`wrap_line`] can drop
/// it when it falls at a break — a wrapped line must not start or end with the space that was only
/// ever there to separate two words.
fn break_atoms(line: &str) -> Vec<&str> {
    let mut atoms = Vec::new();
    let mut word_start: Option<usize> = None;
    for (i, c) in line.char_indices() {
        if c == ' ' || breaks_anywhere(c) {
            if let Some(s) = word_start.take() {
                atoms.push(&line[s..i]);
            }
            atoms.push(&line[i..i + c.len_utf8()]);
        } else if word_start.is_none() {
            word_start = Some(i);
        }
    }
    if let Some(s) = word_start {
        atoms.push(&line[s..]);
    }
    atoms
}

/// Greedily fills lines of at most `budget` px from [`break_atoms`].
///
/// Greedy rather than balanced (Knuth-Plass) on purpose: a node label is a handful of words, where
/// the two agree, and a greedy pass has one property a balanced one does not — adding a character
/// to the text never *removes* a line, so the box a label asks for is monotonic in the label.
///
/// An atom wider than `budget` all by itself (a long unbreakable token) still gets its own line
/// and comes back over budget; N2's cap is what gives way, never the text.
fn wrap_line(line: &str, budget: f64, font_size: f64) -> Vec<String> {
    if text_metrics::measure(line, font_size as f32) as f64 <= budget {
        return vec![line.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for atom in break_atoms(line) {
        if cur.is_empty() && atom == " " {
            continue;
        }
        let candidate = format!("{cur}{atom}");
        if !cur.is_empty()
            && text_metrics::measure(candidate.trim_end(), font_size as f32) as f64 > budget
        {
            out.push(std::mem::take(&mut cur).trim_end().to_string());
            if atom == " " {
                continue;
            }
            cur.push_str(atom);
        } else {
            cur = candidate;
        }
    }
    let cur = cur.trim_end();
    if !cur.is_empty() || out.is_empty() {
        out.push(cur.to_string());
    }
    out
}
