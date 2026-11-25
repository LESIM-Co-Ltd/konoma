//! mermaid's `pie` language.
//!
//! Read from `pie.langium` and `pieDb.ts` at tag `mermaid@11.17.2`. The whole grammar is four
//! lines, and every one of them holds a rule that is not guessable:
//!
//! ```text
//! entry Pie: NEWLINE* "pie" showData?="showData"? (TitleAndAccessibilities | sections+=PieSection | NEWLINE)*;
//! PieSection: label=STRING ":" value=NUMBER_PIE EOL;
//! terminal FLOAT_PIE returns number: /-?[0-9]+\.[0-9]+(?!\.)/;
//! terminal INT_PIE   returns number: /-?(0|[1-9][0-9]*)(?!\.)/;
//! ```
//!
//! * **A slice's label must be quoted.** `Dogs : 386` is not a `PieSection`; only `"Dogs" : 386`
//!   is. A parser that accepted the bare form would draw a diagram mermaid refuses.
//! * **`NUMBER_PIE` carries a sign** where the shared `NUMBER` does not — so `-5` *parses*, and
//!   then `pieDb.addSection` throws on it ("Negative values are not allowed in pie charts"). Two
//!   different rules; konoma reproduces both, because "parses but is rejected" is a different
//!   error message from "is not a number", and §1 says the message is the point.
//! * **A repeated label keeps the first value and is not an error**: `if (!sections.has(label))`.
//!   Dropping the second silently is upstream's behaviour and it is what
//!   [`super::tests`] pins.
//! * `showData` is a flag on the keyword line, not a statement, and it puts the raw value beside
//!   the label in the legend.

use super::{
    find_header, first_word, parse_number, read_quoted, Envelope, ParseError, Preamble, Source,
    TitleSyntax,
};

/// The keyword, for messages and for [`is_pie`].
pub const KEYWORD: &str = "pie";

/// One slice.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    /// The quoted label, unescaped.
    pub label: String,
    /// The value. Never negative — [`parse`] refuses those, as mermaid does.
    pub value: f64,
}

/// A parsed pie chart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pie {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// Slices in source order. mermaid draws them in this order too — `pieRenderer` feeds the
    /// section map straight to d3's `pie()` with `.sort(null)`.
    pub slices: Vec<Slice>,
    /// Whether `showData` was written on the keyword line.
    pub show_data: bool,
}

impl Pie {
    /// The sum of every slice, which is what a percentage is a fraction of.
    pub fn total(&self) -> f64 {
        self.slices.iter().map(|s| s.value).sum()
    }
}

/// Whether this source is a pie chart.
///
/// The routing predicate asks this **before either renderer runs** (§1): a chart konoma owns and
/// then refuses degrades to the text diagram rather than being redrawn by the crate. Classifying
/// therefore reads only the header.
pub fn is_pie(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid pie chart.
pub fn parse(src: &str) -> Result<Pie, ParseError> {
    let Some(Source {
        lines,
        header_index,
        header_rest,
        front_matter_title,
    }) = find_header(src, &[KEYWORD])
    else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };

    let mut pie = Pie::default();
    let mut env = Envelope::new(front_matter_title);

    // `showData?="showData"?` sits between the keyword and the first statement, on the keyword's
    // own line. Anything else there is not part of the grammar.
    let mut rest = header_rest.as_str();
    if let Some(after) = super::strip_ci(rest, "showData") {
        if after.trim().is_empty() || after.starts_with([' ', '\t']) {
            pie.show_data = true;
            rest = after.trim();
        }
    }
    if !rest.is_empty() {
        read_line(&mut pie, &mut env, rest, header_index + 1)?;
    }

    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        read_line(&mut pie, &mut env, line, i + 1)?;
    }

    pie.preamble = env.preamble;
    if pie.slices.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "slice",
        });
    }
    Ok(pie)
}

fn read_line(
    pie: &mut Pie,
    env: &mut Envelope,
    line: &str,
    number: usize,
) -> Result<(), ParseError> {
    if env.read(line, TitleSyntax::Terminal) {
        return Ok(());
    }
    let t = line.trim();
    if t.is_empty() {
        return Ok(());
    }
    let chars: Vec<char> = t.chars().collect();
    let Some((label, after)) = read_quoted(&chars, 0) else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            // The most common way to get here is writing the label unquoted, and the message has
            // to say so or the author has nothing to go on.
            text: t.to_string(),
        });
    };
    let tail: String = chars[after..].iter().collect();
    let Some(value_text) = tail.trim_start().strip_prefix(':') else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    };
    let value_text = value_text.trim();
    // `NUMBER_PIE` takes a sign; the shared `NUMBER` does not. Getting this wrong would report
    // "not a number" for a case mermaid reports as "negative values are not allowed".
    let Some(value) = parse_number(value_text, true).filter(|_| !value_text.starts_with('+'))
    else {
        return Err(ParseError::BadNumber {
            line: number,
            text: value_text.to_string(),
            why: "a slice's value must be a number",
        });
    };
    if value < 0.0 {
        return Err(ParseError::BadNumber {
            line: number,
            text: value_text.to_string(),
            why: "a slice's value must not be negative",
        });
    }
    // Upstream keeps the first value for a repeated label and does **not** raise: `pieDb.ts`'s
    // `addSection` is guarded by `if (!sections.has(label))`.
    if !pie.slices.iter().any(|s| s.label == label) {
        pie.slices.push(Slice { label, value });
    }
    Ok(())
}
