//! mermaid's `quadrantChart` language.
//!
//! Read from `quadrant.jison` and `quadrantDb.ts` at tag `mermaid@11.17.2`. What the grammar says
//! that recall would not:
//!
//! * **A point's coordinates are `(1)|(0(.\d+)?)` — and nothing else.** Not `1.0`, not `2`, not
//!   `-0.1`: the lexer's `point_x` state has no rule for the character after the `1`, so `[1.0,
//!   0.5]` is a *lexer* error upstream. konoma refuses it too, with a message that says what the
//!   range is, because silently clamping would put a point somewhere the author did not write it.
//! * **`text` here includes spaces**, unlike `xychart.jison`'s: `textNoTagsToken` is
//!   `alphaNumToken | SPACE | MINUS`. So `x-axis Low Reach --> High Reach` reads two labels, and
//!   the arrow is picked out because `" "*\-\-+\>" "*` is a longer match at that position than the
//!   single space `\s` would be.
//! * **An arrow with nothing after it appends `" ⟶ "` to the left label.** That is a real branch
//!   in the grammar (`X-AXIS text AXIS-TEXT-DELIMITER { $2.text += " ⟶ "; }`), and it is visible
//!   in the drawing, so konoma does the same rather than dropping the arrow.
//! * **`classDef` and `:::` and per-point styles are parsed here so a statement the grammar has a
//!   rule for and the parser has none for cannot become a phantom element** — a `classDef` line
//!   would otherwise be read as a *point* label. [`super::tests`] pins that they do not become
//!   one. `render::chart::quadrant::apply_quadrant_decl` is what reads their declarations for
//!   real (`docs/STATUS.md`'s flowchart entry, 2026-08-28).

use super::{
    find_header, first_word, read_quoted, Envelope, ParseError, Preamble, Source, TitleSyntax,
};

/// The keyword.
pub const KEYWORD: &str = "quadrantChart";

/// One plotted point.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadrantPoint {
    /// The label written before the `:`.
    pub label: String,
    /// Position across, in `0.0..=1.0`.
    pub x: f64,
    /// Position up, in `0.0..=1.0`.
    pub y: f64,
    /// The `:::name` class. Resolved, along with [`QuadrantPoint::styles`], by
    /// `render::chart::quadrant::quadrant_style`.
    pub class: Option<String>,
    /// The `radius: 5, color: #ff0000` styles.
    pub styles: Vec<String>,
}

/// A parsed quadrant chart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadrantChart {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// The label at the left end of the x-axis.
    pub x_left: String,
    /// The label at its right end.
    pub x_right: String,
    /// The label at the bottom of the y-axis.
    pub y_bottom: String,
    /// The label at its top.
    pub y_top: String,
    /// `quadrant-1` — top right.
    pub quadrant1: String,
    /// `quadrant-2` — top left.
    pub quadrant2: String,
    /// `quadrant-3` — bottom left.
    pub quadrant3: String,
    /// `quadrant-4` — bottom right.
    pub quadrant4: String,
    /// The points, in source order.
    pub points: Vec<QuadrantPoint>,
    /// `classDef name styles`.
    pub class_defs: Vec<(String, Vec<String>)>,
}

/// Whether this source is a quadrant chart.
pub fn is_quadrant_chart(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid quadrant chart.
pub fn parse(src: &str) -> Result<QuadrantChart, ParseError> {
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
    let mut chart = QuadrantChart::default();
    let mut env = Envelope::new(front_matter_title);
    if !header_rest.is_empty() {
        read_line(&mut chart, &mut env, &header_rest, header_index + 1)?;
    }
    for (i, line) in lines.iter().enumerate().skip(header_index + 1) {
        read_line(&mut chart, &mut env, line, i + 1)?;
    }
    chart.preamble = env.preamble;
    if chart.points.is_empty()
        && chart.x_left.is_empty()
        && chart.y_bottom.is_empty()
        && [
            &chart.quadrant1,
            &chart.quadrant2,
            &chart.quadrant3,
            &chart.quadrant4,
        ]
        .iter()
        .all(|q| q.is_empty())
    {
        // A bare header draws a frame with nothing in it, which is the transparent-rectangle
        // failure stage 1 refuses (§6). A chart with *only* quadrant names is still a chart.
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "point or axis",
        });
    }
    Ok(chart)
}

fn read_line(
    chart: &mut QuadrantChart,
    env: &mut Envelope,
    line: &str,
    number: usize,
) -> Result<(), ParseError> {
    // `eol: NEWLINE | SEMI | EOF`.
    for stmt in line.split(';') {
        read_statement(chart, env, stmt, number)?;
    }
    Ok(())
}

fn read_statement(
    chart: &mut QuadrantChart,
    env: &mut Envelope,
    stmt: &str,
    number: usize,
) -> Result<(), ParseError> {
    if env.read(stmt, TitleSyntax::RawRestOfLine) {
        return Ok(());
    }
    let t = stmt.trim();
    if t.is_empty() {
        return Ok(());
    }
    if let Some(rest) = keyword(t, "classDef") {
        let mut words = rest.trim().splitn(2, char::is_whitespace);
        let name = words.next().unwrap_or("").to_string();
        if name.is_empty() {
            return Err(ParseError::Invalid {
                line: number,
                message: "classDef needs a name".to_string(),
            });
        }
        let styles = words
            .next()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        chart.class_defs.push((name, styles));
        return Ok(());
    }
    for (word, slot) in [
        ("quadrant-1", &mut chart.quadrant1),
        ("quadrant-2", &mut chart.quadrant2),
        ("quadrant-3", &mut chart.quadrant3),
        ("quadrant-4", &mut chart.quadrant4),
    ] {
        if let Some(rest) = keyword(t, word) {
            *slot = text_of(rest);
            return Ok(());
        }
    }
    for (word, is_x) in [("x-axis", true), ("y-axis", false)] {
        if let Some(rest) = keyword(t, word) {
            let (first, second) = split_arrow(rest);
            let (near, far) = match second {
                // `X-AXIS text AXIS-TEXT-DELIMITER text`.
                Some(s) if !s.trim().is_empty() => (text_of(first), text_of(s)),
                // `X-AXIS text AXIS-TEXT-DELIMITER` — upstream appends the arrow to the label.
                Some(_) => (format!("{} ⟶ ", text_of(first)), String::new()),
                // `X-AXIS text`.
                None => (text_of(first), String::new()),
            };
            if is_x {
                chart.x_left = near;
                chart.x_right = far;
            } else {
                chart.y_bottom = near;
                chart.y_top = far;
            }
            return Ok(());
        }
    }
    read_point(chart, t, number)
}

/// `label (:::class)? : [x, y] (, styles)?`
fn read_point(chart: &mut QuadrantChart, t: &str, number: usize) -> Result<(), ParseError> {
    // The lexer's `point_start` is `\s*\:\s*\[\s*`, so the split is at the **last** `:` that is
    // followed by a `[`: a label may contain a colon, and `:::` must not be mistaken for it.
    let Some(cut) = colon_before_bracket(t) else {
        return Err(ParseError::Unexpected {
            kind: KEYWORD,
            line: number,
            text: t.to_string(),
        });
    };
    let (head, tail) = t.split_at(cut);
    let (label_part, class) = match head.split_once(":::") {
        Some((l, c)) => (l, Some(c.trim().to_string())),
        None => (head, None),
    };
    let label = text_of(label_part);
    if label.is_empty() {
        return Err(ParseError::Invalid {
            line: number,
            message: "a point needs a label".to_string(),
        });
    }
    let open = tail.find('[').expect("colon_before_bracket found one");
    let after_open = &tail[open + 1..];
    let Some(close) = after_open.find(']') else {
        return Err(ParseError::Invalid {
            line: number,
            message: "a point's coordinates are never closed with `]`".to_string(),
        });
    };
    let inside = &after_open[..close];
    let Some((xs, ys)) = inside.split_once(',') else {
        return Err(ParseError::Invalid {
            line: number,
            message: "a point takes two coordinates, `[x, y]`".to_string(),
        });
    };
    let x = coordinate(xs.trim(), number)?;
    let y = coordinate(ys.trim(), number)?;
    let styles: Vec<String> = after_open[close + 1..]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    chart.points.push(QuadrantPoint {
        label,
        x,
        y,
        class,
        styles,
    });
    Ok(())
}

/// The `:` that opens a `point_start`, as a byte offset.
///
/// Scans from the right so that a label containing a colon still finds the coordinate list, and
/// steps over `:::` so a class selector is never taken for it.
fn colon_before_bracket(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    for i in (0..b.len()).rev() {
        if b[i] != b':' {
            continue;
        }
        if b.get(i + 1) == Some(&b':') || (i > 0 && b[i - 1] == b':') {
            continue;
        }
        if t[i + 1..].trim_start().starts_with('[') {
            return Some(i);
        }
    }
    None
}

/// `(1)|(0(.\d+)?)` — the whole of what a coordinate may be.
fn coordinate(text: &str, line: usize) -> Result<f64, ParseError> {
    const WHY: &str = "a point's coordinate must be `0`, `1`, or `0.` followed by digits";
    if text == "1" {
        return Ok(1.0);
    }
    if text == "0" {
        return Ok(0.0);
    }
    if let Some(frac) = text.strip_prefix("0.") {
        if !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()) {
            return text.parse::<f64>().map_err(|_| ParseError::BadNumber {
                line,
                text: text.to_string(),
                why: WHY,
            });
        }
    }
    Err(ParseError::BadNumber {
        line,
        text: text.to_string(),
        why: WHY,
    })
}

/// Splits an axis clause at the `--+>` delimiter, if there is one.
fn split_arrow(rest: &str) -> (&str, Option<&str>) {
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'-' {
            let mut j = i;
            while j < b.len() && b[j] == b'-' {
                j += 1;
            }
            // `\-\-+\>` — two or more dashes, then `>`.
            if j - i >= 2 && b.get(j) == Some(&b'>') {
                return (&rest[..i], Some(&rest[j + 1..]));
            }
            i = j;
            continue;
        }
        i += 1;
    }
    (rest, None)
}

/// `text` as this grammar spells it: a quoted string, or the raw run with its spaces kept.
fn text_of(rest: &str) -> String {
    let rest = rest.trim();
    let chars: Vec<char> = rest.chars().collect();
    match read_quoted(&chars, 0) {
        Some((text, _)) => text,
        None => rest.to_string(),
    }
}

/// `word` at the start of a statement, with the boundary the lexer's `" "*"x-axis"" "*` implies.
fn keyword<'a>(stmt: &'a str, word: &str) -> Option<&'a str> {
    let rest = super::strip_ci(stmt.trim_start(), word)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() || c == '"' || c == '\'' => Some(rest),
        Some(_) => None,
    }
}
