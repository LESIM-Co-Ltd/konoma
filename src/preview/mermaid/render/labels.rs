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
//! There is **no automatic wrapping**. §2-5 puts `wrappingWidth` in the "not implemented" list
//! (it is a px-space idea), so a line breaks only where the author wrote `<br>` or `\n` — which
//! the parser has already turned into a real newline by the time the text arrives here.

use crate::preview::mermaid::flowchart;
use crate::preview::mermaid::text_metrics::{self, FONT_SIZE};

/// Line spacing as a multiple of the font size. mermaid's SVG-label spacing (§4-4).
pub const LINE_HEIGHT_RATIO: f64 = 1.1;

/// Where a line's baseline sits below the middle of its line box, as a multiple of the font size.
///
/// Roughly half a capital's height: sans-serif cap heights cluster around `0.7em`, so centring
/// capitals puts the baseline `0.35em` below centre. CJK, whose ink runs about `-0.88em..0.12em`,
/// wants `0.38em`; one constant covers both within half a pixel at 14px, and picking it by script
/// would make a mixed-script line jump.
pub const BASELINE_RATIO: f64 = 0.35;

/// Height of one line of text, in px.
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
    /// `lines.len()` line heights.
    pub height: f64,
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
        // `common.getRows` — mermaid splits a label into rows **at render time**, for every
        // diagram kind, on the four `<br>` spellings *and* on a literal `\n`. Doing it here rather
        // than in each parser is what makes it true for all of them at once: the flowchart parser
        // has already done it (`flowchart::text::decode_label`), so this is idempotent there and
        // the flowchart goldens do not move.
        let text = flowchart::text::replace_breaks(text);
        let text = text.replace("\\n", "\n");
        let lines: Vec<String> = text
            .split('\n')
            .map(|l| text_metrics::collapse_spaces(l).into_owned())
            .collect();
        let width = lines
            .iter()
            .map(|l| text_metrics::measure(l, FONT_SIZE) as f64)
            .fold(0.0_f64, f64::max);
        Label {
            height: lines.len() as f64 * line_height(),
            lines,
            width,
        }
    }

    /// Whether the label would draw nothing at all (so a shape need not reserve room for it, and
    /// no `<text>` need be emitted).
    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    /// Baseline y of line `i` when the block is centred on `center_y`.
    pub fn baseline(&self, center_y: f64, i: usize) -> f64 {
        let top = center_y - self.height / 2.0;
        // Line `i` occupies `[top + i*lh, top + (i+1)*lh]`; its baseline sits `BASELINE_RATIO`
        // of an em below the middle of that box.
        top + (i as f64 + 0.5) * line_height() + FONT_SIZE as f64 * BASELINE_RATIO
    }
}
