//! mermaid's `xychart-beta` language.
//!
//! Read from `xychart.jison` and `xychartDb.ts` at tag `mermaid@11.17.2`. Four things in there are
//! not guessable, and each of them changes what a chart says:
//!
//! * **Inside an axis clause a digit is always a number, never part of the title.** The lexer
//!   pushes an `axis_data` state where `[+-]?(?:\d+(?:\.\d+)?|\.\d+)` is matched *before* the
//!   `[0-9]+` that feeds the title, and `NUMBER_WITH_DECIMAL` is not one of `alphaNumToken`. So
//!   `x-axis Year2024` is a parse error upstream, and `x-axis Month 0 --> 12` reads the range.
//! * **`-->` on an axis only ever joins two numbers.** `xAxisData` is `bandData | NUMBER ARROW
//!   NUMBER` — there is no textual form, unlike `quadrantChart`, whose arrow separates two labels.
//! * **The x-axis and the plot data are resolved against each other in source order.** A `bar`
//!   read before the `x-axis` line sets the x range from its own length (`hasSetXAxis` is still
//!   false); one read after a band axis is **truncated to the number of categories**. Two sources
//!   with the same lines in a different order are two different charts, and
//!   [`super::tests`] pins that.
//! * **A chart with no `bar` and no `line` is an error**, not an empty frame:
//!   `getDrawableElem` throws "No Plot to render". That matches stage 1's rule for `graph TD`.
//!
//! # Two deliberate departures, both stated and pinned
//!
//! 1. **Unquoted text keeps its spaces.** `alphaNum` in the jison grammar concatenates its tokens
//!    with `$1 + '' + $2` while `\s+` is skipped, so upstream turns `title Sales Revenue` into
//!    `SalesRevenue`. konoma takes the source span instead. §0-1 makes the acceptance criterion
//!    "the same source read correctly", and a title with the spaces removed is not that.
//! 2. **`horizontal` is parsed and not applied.** See [`Orientation`].

use super::{
    find_header, first_word, parse_number, read_quoted, Envelope, ParseError, Preamble, Source,
    TitleSyntax,
};

/// The keywords, longest first — `xychart-beta` has to be tried before `xychart` or the `-beta`
/// would be left behind as the header's trailing text.
pub const KEYWORDS: &[&str] = &["xychart-beta", "xychart"];

/// The name used in messages.
pub const KEYWORD: &str = "xychart";

/// Which way the value axis runs.
///
/// **`horizontal` is read and deliberately not drawn.** Reading it is not optional: a word the
/// grammar has a rule for that konoma had none for would fall through to "unexpected", and a chart
/// that refuses to draw at all is worse than one drawn on the other axis (§2-3 — 描画不要 ≠
/// パース不要). Drawing it would mean a second, transposed copy of the axis, tick, bar and label
/// geometry, and every invariant stated twice; the same trade as `direction` inside a `subgraph`,
/// which `render::lay_out_spec` also reads and does not apply.
///
/// `super::tests::a_horizontal_xychart_is_parsed_and_drawn_vertically` is the test that has to
/// change if a later stage wants it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Orientation {
    /// The default: categories across, values up.
    #[default]
    Vertical,
    /// Read from the source, drawn as [`Orientation::Vertical`].
    Horizontal,
}

/// A `bar` or a `line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotKind {
    /// `bar [ … ]`.
    Bar,
    /// `line [ … ]`.
    Line,
}

/// One plotted series, with its data already resolved against the x-axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    /// Bar or line.
    pub kind: PlotKind,
    /// The series name, empty when the source gave none.
    pub title: String,
    /// `(x label, value)` in order — mermaid's `retData`, so a band chart's labels are the
    /// categories and a linear chart's are the computed steps.
    pub data: Vec<(String, f64)>,
    /// Per-point labels, when any point carried one (`bar [5 "five", 6]`).
    pub point_labels: Vec<String>,
}

/// What the x-axis turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum XAxis {
    /// One slot per category. mermaid's default, with no categories until something sets them.
    Band(Vec<String>),
    /// A numeric range.
    Linear {
        /// Low end.
        min: f64,
        /// High end.
        max: f64,
    },
}

impl Default for XAxis {
    fn default() -> XAxis {
        XAxis::Band(Vec::new())
    }
}

/// A parsed xy chart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct XyChart {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// Read from the keyword line. See [`Orientation`].
    pub orientation: Orientation,
    /// The x-axis title.
    pub x_title: String,
    /// The y-axis title.
    pub y_title: String,
    /// The x-axis, after every statement has had its say.
    pub x: XAxis,
    /// The y-axis range, likewise.
    pub y: (f64, f64),
    /// The series, in source order.
    pub plots: Vec<Plot>,
}

/// Whether this source is an xy chart.
pub fn is_xychart(src: &str) -> bool {
    find_header(src, KEYWORDS).is_some()
}

/// Parses a mermaid xy chart.
pub fn parse(src: &str) -> Result<XyChart, ParseError> {
    let Some(Source {
        lines,
        header_index,
        header_rest,
        front_matter_title,
    }) = find_header(src, KEYWORDS)
    else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };

    let mut b = Builder {
        chart: XyChart::default(),
        env: Envelope::new(front_matter_title),
        x_set: false,
        y_set: false,
        y_min: f64::INFINITY,
        y_max: f64::NEG_INFINITY,
    };

    // `start: XYCHART chartConfig start` — the orientation, if there is one, sits on the keyword's
    // own line and is not a statement.
    let mut rest = header_rest.as_str();
    for (word, orientation) in [
        ("horizontal", Orientation::Horizontal),
        ("vertical", Orientation::Vertical),
    ] {
        if let Some(after) = super::strip_ci(rest, word) {
            if after.trim().is_empty() || after.starts_with([' ', '\t', ';']) {
                b.chart.orientation = orientation;
                rest = after.trim();
                break;
            }
        }
    }
    if !rest.is_empty() {
        b.line(rest, header_index + 1)?;
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        b.line(line, i + 1)?;
    }
    b.finish()
}

struct Builder {
    chart: XyChart,
    env: Envelope,
    /// mermaid's `hasSetXAxis` / `hasSetYAxis`: whether an explicit axis statement has been seen
    /// *yet*, which is what makes the resolution order-sensitive.
    x_set: bool,
    y_set: bool,
    y_min: f64,
    y_max: f64,
}

impl Builder {
    fn line(&mut self, line: &str, number: usize) -> Result<(), ParseError> {
        // jison's `eol` is `NEWLINE | SEMI | EOF`, so a `;` ends a statement too.
        for stmt in line.split(';') {
            self.statement(stmt, number)?;
        }
        Ok(())
    }

    fn statement(&mut self, stmt: &str, number: usize) -> Result<(), ParseError> {
        if self.env.read(stmt, TitleSyntax::Text) {
            return Ok(());
        }
        let t = stmt.trim();
        if t.is_empty() {
            return Ok(());
        }
        if let Some(rest) = keyword(t, "x-axis") {
            return self.axis(rest, number, true);
        }
        if let Some(rest) = keyword(t, "y-axis") {
            return self.axis(rest, number, false);
        }
        if let Some(rest) = keyword(t, "bar") {
            return self.plot(PlotKind::Bar, rest, number);
        }
        if let Some(rest) = keyword(t, "line") {
            return self.plot(PlotKind::Line, rest, number);
        }
        Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        })
    }

    /// `x-axis <title>? ( [a, b, …] | min --> max )?`
    fn axis(&mut self, rest: &str, number: usize, is_x: bool) -> Result<(), ParseError> {
        let (title, tail) = split_title(rest);
        if is_x {
            self.chart.x_title = title;
        } else {
            self.chart.y_title = title;
        }
        let tail = tail.trim();
        if tail.is_empty() {
            return Ok(());
        }
        if let Some(inner) = tail.strip_prefix('[') {
            if !is_x {
                // `yAxisData` has no `bandData` alternative.
                return Err(ParseError::Invalid {
                    line: number,
                    message: "a y-axis takes a numeric range, not a list of categories".to_string(),
                });
            }
            let Some(end) = inner.find(']') else {
                return Err(ParseError::Invalid {
                    line: number,
                    message: "the category list is never closed with `]`".to_string(),
                });
            };
            if !inner[end + 1..].trim().is_empty() {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: inner[end + 1..].trim().to_string(),
                });
            }
            let categories = split_texts(&inner[..end]);
            self.chart.x = XAxis::Band(categories);
            self.x_set = true;
            return Ok(());
        }
        let Some((lo, hi)) = tail.split_once("-->") else {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: tail.to_string(),
            });
        };
        let lo = number_at(lo.trim(), number)?;
        let hi = number_at(hi.trim(), number)?;
        if is_x {
            self.chart.x = XAxis::Linear { min: lo, max: hi };
            self.x_set = true;
        } else {
            self.chart.y = (lo, hi);
            self.y_set = true;
        }
        Ok(())
    }

    /// `bar <title>? [ v, v "label", … ]`
    fn plot(&mut self, kind: PlotKind, rest: &str, number: usize) -> Result<(), ParseError> {
        let (title, tail) = split_plot_title(rest);
        let tail = tail.trim();
        let Some(inner) = tail.strip_prefix('[') else {
            return Err(ParseError::Invalid {
                line: number,
                message: "a plot needs its data in `[ … ]`".to_string(),
            });
        };
        let Some(end) = inner.find(']') else {
            return Err(ParseError::Invalid {
                line: number,
                message: "the plot data is never closed with `]`".to_string(),
            });
        };
        if !inner[end + 1..].trim().is_empty() {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: inner[end + 1..].trim().to_string(),
            });
        }
        let mut values = Vec::new();
        let mut labels = Vec::new();
        for item in inner[..end].split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let chars: Vec<char> = item.chars().collect();
            let cut = chars
                .iter()
                .position(|c| *c == '"' || *c == '\'')
                .unwrap_or(chars.len());
            let value_text: String = chars[..cut].iter().collect();
            values.push(number_at(value_text.trim(), number)?);
            labels.push(match read_quoted(&chars, cut) {
                Some((text, _)) => text,
                None => String::new(),
            });
        }
        if values.is_empty() {
            return Err(ParseError::Invalid {
                line: number,
                message: "a plot needs at least one value".to_string(),
            });
        }

        // From here on this is `transformDataWithoutCategory`, in its own order.
        if !self.x_set {
            let len = values.len() as f64;
            let (prev_min, prev_max) = match self.chart.x {
                XAxis::Linear { min, max } => (min, max),
                XAxis::Band(_) => (f64::INFINITY, f64::NEG_INFINITY),
            };
            self.chart.x = XAxis::Linear {
                min: prev_min.min(1.0),
                max: prev_max.max(len),
            };
            // Note: upstream does *not* set `hasSetXAxis` here, so a later plot with more points
            // widens the range again.
        }
        if let XAxis::Band(categories) = &self.chart.x {
            // "prevent orphaned bars from rendering in unlabeled chart space" — upstream's words.
            if values.len() > categories.len() {
                values.truncate(categories.len());
                labels.truncate(categories.len());
            }
        }
        if !self.y_set {
            for v in &values {
                self.y_min = self.y_min.min(*v);
                self.y_max = self.y_max.max(*v);
            }
        }

        let data: Vec<(String, f64)> = match &self.chart.x {
            XAxis::Band(categories) => categories
                .iter()
                .zip(values.iter())
                .map(|(c, v)| (c.clone(), *v))
                .collect(),
            XAxis::Linear { min, max } => {
                if values.len() == 1 {
                    // Upstream's comment: "For backwards compatibility, avoid divide-by-zero".
                    vec![(format_step(*min), values[0])]
                } else {
                    let step = (max - min) / (values.len() as f64 - 1.0);
                    values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| (format_step(min + i as f64 * step), *v))
                        .collect()
                }
            }
        };
        let has_label = labels.iter().any(|l| !l.is_empty());
        self.chart.plots.push(Plot {
            kind,
            title,
            data,
            point_labels: if has_label { labels } else { Vec::new() },
        });
        Ok(())
    }

    fn finish(mut self) -> Result<XyChart, ParseError> {
        self.chart.preamble = self.env.preamble;
        if self.chart.plots.is_empty() {
            // `getDrawableElem`: "No Plot to render, please provide a plot with some data".
            return Err(ParseError::NoData {
                kind: KEYWORD,
                wanted: "plot",
            });
        }
        if !self.y_set {
            self.chart.y = (self.y_min, self.y_max);
        }
        Ok(self.chart)
    }
}

/// `keyword` at the start of a statement, followed by whitespace, `[`, `"` or end of line.
fn keyword<'a>(stmt: &'a str, word: &str) -> Option<&'a str> {
    let rest = super::strip_ci(stmt, word)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() || c == '[' || c == '"' || c == '\'' => Some(rest),
        Some(_) => None,
    }
}

/// Splits an axis clause into its title and the rest.
///
/// The title ends where the lexer's `axis_data` state stops feeding `alphaNumToken`: at a `[`, or
/// at the first character that can begin `NUMBER_WITH_DECIMAL`. A quoted title ends at its quote.
fn split_title(rest: &str) -> (String, &str) {
    let rest = rest.trim_start();
    let chars: Vec<char> = rest.chars().collect();
    if let Some((text, after)) = read_quoted(&chars, 0) {
        let byte = chars[..after].iter().map(|c| c.len_utf8()).sum::<usize>();
        return (text, &rest[byte..]);
    }
    for (i, c) in rest.char_indices() {
        if c == '[' || starts_number(&rest[i..]) {
            return (rest[..i].trim_end().to_string(), &rest[i..]);
        }
    }
    (rest.trim_end().to_string(), "")
}

/// The same for a plot clause, where the title ends at the `[` alone — a digit inside a `data`
/// state is an ordinary `NUM` and belongs to the title.
fn split_plot_title(rest: &str) -> (String, &str) {
    let rest = rest.trim_start();
    let chars: Vec<char> = rest.chars().collect();
    if let Some((text, after)) = read_quoted(&chars, 0) {
        let byte = chars[..after].iter().map(|c| c.len_utf8()).sum::<usize>();
        return (text, &rest[byte..]);
    }
    match rest.find('[') {
        Some(i) => (rest[..i].trim_end().to_string(), &rest[i..]),
        None => (rest.trim_end().to_string(), ""),
    }
}

/// Whether `s` begins a `NUMBER_WITH_DECIMAL`: `[+-]?(?:\d+(?:\.\d+)?|\.\d+)`.
fn starts_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
    }
    i < b.len() && b[i].is_ascii_digit()
}

fn number_at(text: &str, line: usize) -> Result<f64, ParseError> {
    parse_number(text, true).ok_or_else(|| ParseError::BadNumber {
        line,
        text: text.to_string(),
        why: "expected a number",
    })
}

/// `[a, "b c", d]` — a comma-separated list of `text`s.
fn split_texts(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in split_top_level(inner) {
        let part = part.trim();
        let chars: Vec<char> = part.chars().collect();
        match read_quoted(&chars, 0) {
            Some((text, _)) => out.push(text),
            None if part.is_empty() => {}
            None => out.push(part.to_string()),
        }
    }
    out
}

/// Splits on commas that are not inside a quoted string.
fn split_top_level(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            None if c == ',' => {
                out.push(std::mem::take(&mut cur));
            }
            None => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// A computed x step, spelled the way `${min + index * step}` spells it in JavaScript: an integral
/// value has no `.0`.
fn format_step(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
