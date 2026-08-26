//! Reading mermaid's **data-chart** languages — stage 5.
//!
//! Seven languages live here, and they are together because of what they are not: none of them is
//! a graph. A pie has no edges, a bar chart has no ranking, a packet diagram's order is the bit
//! order the author wrote. `docs/FEATURE-MERMAID-RENDERER.md` §7 groups them as the "simple chart
//! bundle" for exactly that reason, and [`render::chart`] draws them without going anywhere near
//! `lay_out_spec` — the same decision stage 4 took for sequence diagrams, for the same reason.
//!
//! [`render::chart`]: crate::preview::mermaid::render::chart
//!
//! | module | mermaid keyword | grammar konoma read |
//! |---|---|---|
//! | [`pie`] | `pie` | `pie.langium` (langium) |
//! | [`xychart`] | `xychart-beta` / `xychart` | `xychart.jison` |
//! | [`quadrant`] | `quadrantChart` | `quadrant.jison` |
//! | [`radar`] | `radar-beta` | `radar.langium` |
//! | [`treemap`] | `treemap-beta` / `treemap` | `treemap.langium` |
//! | [`packet`] | `packet-beta` / `packet` | `packet.langium` |
//! | [`sankey`] | `sankey-beta` / `sankey` | `sankey.jison` |
//!
//! Every one of them was read at tag `mermaid@11.17.2` rather than recalled, which is the rule
//! stages 2 and 3 arrived at after recall produced seven real parser bugs between them.
//!
//! # What is shared, and what is not
//!
//! The seven grammars have almost nothing in common as *grammars* — one is CSV, one is
//! indentation-sensitive, one is a jison lexer with eight start states. What they do share is the
//! envelope every mermaid diagram has: a keyword line, `title`, `accTitle:`, `accDescr:` and
//! `accDescr { … }`, on top of the front matter and comments [`preprocess`] has already removed.
//! That envelope is [`Preamble`] and is written once.
//!
//! [`preprocess`]: crate::preview::mermaid::flowchart::preprocess
//!
//! # Accessibility statements are parsed and not drawn — on purpose
//!
//! `accTitle` and `accDescr` exist for screen readers, and konoma's output is a raster image in a
//! terminal. Reading them and dropping them is deliberate; **not** reading them would be a bug,
//! because a line the grammar has a rule for that the parser has no rule for becomes a phantom
//! element (§2-3: 描画不要 ≠ パース不要). [`super::chart::tests`] pins the drop.

// konoma is a binary crate, so `pub` marks nothing as used: the parts of this module's surface
// that exist for the tests look unreachable to rustc. Same reasoning, and the same lint, as the
// sibling parsers.
#![allow(dead_code)]

pub mod packet;
pub mod pie;
pub mod quadrant;
pub mod radar;
pub mod sankey;
pub mod treemap;
pub mod xychart;

#[cfg(test)]
pub mod tests;

use std::fmt;

use crate::preview::mermaid::flowchart::preprocess::preprocess;

/// Why a chart source could not be read.
///
/// One enum for all seven languages. They are different grammars but they fail in the same six
/// ways, and a shared enum keeps `render::RenderError` from growing a variant per chart kind —
/// which would make the *renderer's* error type grow every time a language is added, for no gain
/// at the call site. Each variant carries the chart's own keyword so the message still says which
/// language refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The source does not begin with this chart's keyword.
    NotThisChart {
        /// The keyword that was expected.
        expected: &'static str,
        /// The first non-blank word that was there instead.
        header: String,
    },
    /// Nothing but blank lines and comments.
    Empty {
        /// The chart's keyword.
        kind: &'static str,
    },
    /// A header with nothing under it.
    ///
    /// Refused rather than drawn, which is stage 1's rule and the reason `graph TD` on its own is
    /// an error: an empty diagram rasterises to a transparent rectangle that the pane then
    /// stretches to fill itself, which is precisely the failure §6 records the crate having.
    NoData {
        /// The chart's keyword.
        kind: &'static str,
        /// What was missing, in words: "slice", "plot", "field".
        wanted: &'static str,
    },
    /// A line the grammar has no rule for.
    Unexpected {
        /// The chart's keyword.
        kind: &'static str,
        /// 1-based source line.
        line: usize,
        /// The line itself, trimmed.
        text: String,
    },
    /// A number that is not one, or is one the chart cannot use.
    BadNumber {
        /// 1-based source line.
        line: usize,
        /// What was written.
        text: String,
        /// Why it is not usable — "not a number", "must not be negative", …
        why: &'static str,
    },
    /// Something the grammar accepts but the chart cannot hold.
    Invalid {
        /// 1-based source line, or **0 for a fact about the whole chart** — a cycle, a total of
        /// zero — which no single line is responsible for and which is therefore reported without
        /// a location rather than with a wrong one.
        line: usize,
        /// The whole message.
        message: String,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::NotThisChart { expected, header } => {
                if header.is_empty() {
                    write!(f, "not a {expected}: the source is empty")
                } else {
                    write!(f, "not a {expected}: the source starts with `{header}`")
                }
            }
            ParseError::Empty { kind } => write!(f, "{kind} has no content"),
            ParseError::NoData { kind, wanted } => {
                write!(f, "{kind} declares no {wanted}")
            }
            ParseError::Unexpected { kind, line, text } => {
                write!(f, "{kind}: unexpected `{text}` at line {line}")
            }
            ParseError::BadNumber { line, text, why } => {
                write!(f, "`{text}` at line {line}: {why}")
            }
            ParseError::Invalid { line: 0, message } => write!(f, "{message}"),
            ParseError::Invalid { line, message } => write!(f, "{message} at line {line}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// The envelope every mermaid diagram shares, once [`preprocess`] has removed front matter,
/// directives and comments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preamble {
    /// The chart's title: `title …`, or the front matter's `title:` when the body has none.
    pub title: Option<String>,
    /// `accTitle:` — read so it is not mistaken for data, never drawn. See the module docs.
    pub acc_title: Option<String>,
    /// `accDescr:` / `accDescr { … }` — likewise.
    pub acc_descr: Option<String>,
}

/// A source split into its keyword line and the lines under it.
pub struct Source {
    /// Lines of the preprocessed text, so a 1-based index into this is a line in the user's file.
    pub lines: Vec<String>,
    /// Index of the keyword line.
    pub header_index: usize,
    /// Whatever followed the keyword on its own line, trimmed.
    pub header_rest: String,
    /// The front matter's `title:`, which the body's own `title` overrides.
    pub front_matter_title: Option<String>,
}

/// Finds a chart's keyword line.
///
/// `keywords` is tried **longest first by the caller**, because `packet-beta` and `packet` share a
/// prefix and matching the short one first would leave `-beta` as the header's trailing text.
///
/// The match is case-insensitive, following the jison lexers (`%options case-insensitive`) rather
/// than the detector regexes, which are not. Being more permissive than the detector cannot lose a
/// diagram — `PIE` is not any other kind's keyword either — and it keeps these seven consistent
/// with the four languages konoma already reads, whose headers are matched the same way.
pub fn find_header(src: &str, keywords: &[&str]) -> Option<Source> {
    let pre = preprocess(src);
    let lines: Vec<String> = pre.text.split('\n').map(str::to_string).collect();
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let t = line.trim_start();
        for kw in keywords {
            if starts_ci(t, kw) {
                let rest = &t[kw.len()..];
                // The keyword must end here: `pieces` is not `pie`.
                if rest.chars().next().is_some_and(is_word_char) {
                    continue;
                }
                return Some(Source {
                    header_index: i,
                    header_rest: rest.trim().to_string(),
                    lines,
                    front_matter_title: pre.title,
                });
            }
        }
        // The first non-blank line decides. A chart keyword further down is not a chart.
        return None;
    }
    None
}

/// The first non-blank word of a source, for a `NotThisChart` message.
pub fn first_word(src: &str) -> String {
    let pre = preprocess(src);
    pre.text
        .split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

/// ASCII-case-insensitive `starts_with`.
///
/// **Compared as bytes, not as a string slice.** `&haystack[..needle.len()]` panics when that
/// index lands inside a multi-byte character, which `pie title 円グラフ` does for every keyword
/// twelve bytes long — the CJK case in [`tests`] found it. Every keyword here is ASCII, so a
/// byte-wise comparison is not an approximation: it is the same answer, without the slice.
pub fn starts_ci(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    h.len() >= n.len() && h[..n.len()].eq_ignore_ascii_case(n)
}

/// Whether `c` can continue an identifier — used to stop `pie` matching inside `pieces`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// How a chart spells `title`.
///
/// Three of the seven grammars answer differently, and the answer is visible on the page.
/// Transcribed rather than recalled; see each variant.
///
/// ---
///
/// Reads one line of the shared envelope, and says whether it consumed it.
///
/// The three statements are transcribed from `common.langium`'s terminals rather than recalled:
///
/// * `ACC_TITLE`: `accTitle` `[\t ]*` `:` then the rest of the line.
/// * `ACC_DESCR`: `accDescr` either `[\t ]*:` and the rest of the line, or `\s*{` … `}`.
/// * `TITLE`: `title` followed by **a tab or a space** and the rest of the line, *or* `title`
///   alone. Note what that excludes: `title:` is not a title, because the terminal requires
///   whitespace between the keyword and the text. jison's version of the same statement
///   (`quadrant.jison`, `xychart.jison`) is looser, and konoma follows the looser rule for the
///   jison-based charts by trimming a leading `:` — recorded in [`Envelope::read`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSyntax {
    /// `common.langium`'s `TITLE` terminal — `pie`, `radar`, `treemap`, `packet`, `sankey`.
    ///
    /// `title` then **a tab or a space** then the raw rest of the line. Two things follow that a
    /// looser rule would get wrong: `title:` is not a title at all (there is no `:` in the
    /// terminal), and `title "x"` keeps its quotation marks, because the terminal takes the
    /// characters rather than a `STRING`.
    Terminal,
    /// `quadrant.jison`'s `title` start state: `<title>(?!\n|;|#)*[^\n]*` — the raw rest of the
    /// line again, but the keyword may be followed by anything, `:` included.
    RawRestOfLine,
    /// `xychart.jison`: the grammar is `title text`, and `text` may be a `STR`. **This is the one
    /// chart of the seven where `title "Sales Revenue"` loses its quotes**, because the lexer's
    /// `["]` rule takes them and `$text.text` is what is left.
    Text,
}

pub struct Envelope {
    /// What has been collected so far.
    pub preamble: Preamble,
    /// True while an `accDescr { … }` block is open, so its body is not read as data.
    in_acc_descr_block: bool,
    /// Whether a `title` seen in the body has already overridden the front matter's.
    body_title: bool,
}

impl Envelope {
    /// A fresh envelope, seeded with the front matter's `title:`.
    pub fn new(front_matter_title: Option<String>) -> Envelope {
        Envelope {
            preamble: Preamble {
                title: front_matter_title,
                ..Preamble::default()
            },
            in_acc_descr_block: false,
            body_title: false,
        }
    }

    /// Offers `line` to the envelope. `true` means it was one of the envelope's statements and the
    /// caller must not read it as data.
    pub fn read(&mut self, line: &str, title: TitleSyntax) -> bool {
        if self.in_acc_descr_block {
            if let Some((body, _)) = line.split_once('}') {
                self.push_descr(body);
                self.in_acc_descr_block = false;
            } else {
                self.push_descr(line);
            }
            return true;
        }
        let t = line.trim();
        if let Some(rest) = strip_ci(t, "accDescr") {
            let r = rest.trim_start();
            if let Some(body) = r.strip_prefix('{') {
                match body.split_once('}') {
                    Some((inner, _)) => self.push_descr(inner),
                    None => {
                        self.push_descr(body);
                        self.in_acc_descr_block = true;
                    }
                }
                return true;
            }
            if let Some(body) = r.strip_prefix(':') {
                self.push_descr(body);
                return true;
            }
        }
        if let Some(rest) = strip_ci(t, "accTitle") {
            if let Some(body) = rest.trim_start().strip_prefix(':') {
                self.preamble.acc_title = Some(body.trim().to_string());
                return true;
            }
        }
        if let Some(rest) = strip_ci(t, "title") {
            let is_title = rest.is_empty()
                || rest.starts_with([' ', '\t'])
                || (title != TitleSyntax::Terminal && rest.starts_with(':'));
            if is_title {
                let raw = rest.trim_start_matches([':', ' ', '\t']).trim_end();
                // The three grammars disagree about what follows the keyword, and the difference
                // is visible: `title "Sales Revenue"` keeps its quotes in five of the seven charts
                // and loses them in the sixth. See [`TitleSyntax`].
                let text = match title {
                    TitleSyntax::Text => {
                        let chars: Vec<char> = raw.chars().collect();
                        match read_quoted(&chars, 0) {
                            Some((inner, _)) => inner,
                            None => raw.to_string(),
                        }
                    }
                    _ => raw.to_string(),
                };
                // A body `title` always wins over the front matter's, including an empty one:
                // `title` on its own is how an author clears it.
                self.preamble.title = if text.is_empty() { None } else { Some(text) };
                self.body_title = true;
                return true;
            }
        }
        false
    }

    fn push_descr(&mut self, text: &str) {
        let text = text.trim();
        match &mut self.preamble.acc_descr {
            Some(existing) if !text.is_empty() => {
                existing.push('\n');
                existing.push_str(text);
            }
            Some(_) => {}
            None => self.preamble.acc_descr = Some(text.to_string()),
        }
    }
}

/// `strip_prefix`, case-insensitively.
pub fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    starts_ci(s, prefix).then(|| &s[prefix.len()..])
}

/// Reads a `"…"` or `'…'` string starting at `chars[i]`, honouring `\` escapes.
///
/// Returns the text and the index just past the closing quote. `None` when the quote never
/// closes, which every one of these grammars treats as a parse failure rather than as a string
/// that runs to end of line.
pub fn read_quoted(chars: &[char], i: usize) -> Option<(String, usize)> {
    let quote = *chars.get(i)?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            // `STRING` in `common.langium` is /"([^"\\]|\\.)*"/ — a backslash escapes the next
            // character, whatever it is, and is itself dropped.
            '\\' if j + 1 < chars.len() => {
                out.push(chars[j + 1]);
                j += 2;
            }
            c if c == quote => return Some((out, j + 1)),
            c => {
                out.push(c);
                j += 1;
            }
        }
    }
    None
}

/// Parses a decimal number the way these grammars' number terminals describe one.
///
/// Deliberately stricter than `f64::from_str`: that accepts `inf`, `NaN`, `1e5` and `+1`, none of
/// which any of these terminals match, and all of which would reach the geometry as a value no
/// chart can be drawn from. A chart whose axis maximum is `inf` has no scale at all.
pub fn parse_number(text: &str, allow_sign: bool) -> Option<f64> {
    let mut body = text;
    let mut negative = false;
    if allow_sign {
        if let Some(rest) = body.strip_prefix('-') {
            negative = true;
            body = rest;
        } else if let Some(rest) = body.strip_prefix('+') {
            body = rest;
        }
    }
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    if body.bytes().filter(|b| *b == b'.').count() > 1 {
        return None;
    }
    // `.5` and `5.` both appear in these terminals (`xychart.jison` allows `\.\d+`), but a lone
    // `.` is not a number.
    if body == "." {
        return None;
    }
    let v: f64 = body.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(if negative { -v } else { v })
}
