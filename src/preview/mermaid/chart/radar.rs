//! mermaid's `radar-beta` language.
//!
//! Read from `radar.langium`, `db.ts` and `renderer.ts` at tag `mermaid@11.17.2`.
//!
//! * **`axis` and `curve` accumulate.** The grammar's body is a `*` loop over
//!   `'axis' axes+=Axis (',' axes+=Axis)*`, so two `axis` lines give the union of both, not the
//!   second. The crate konoma is replacing keeps only the last line, which draws a three-spoke
//!   chart for a six-axis source — a picture that is wrong rather than merely plain.
//! * **A curve's entries may be written two ways**, and they mean different things. Bare numbers
//!   are positional. `axisName: value` entries are *looked up*: `computeCurveEntries` walks the
//!   **axes** and finds each one's entry, so the source order of a detailed curve does not matter
//!   and a missing axis is an error ("Missing entry for axis …").
//! * **A curve's braces may span lines.** `Entries` has `NEWLINE*` between every element, which is
//!   why this parser joins lines while a `{` is open rather than reading one line at a time.
//! * **The scale is `min` to `max`, clamped**: `relativeRadius` is
//!   `radius * (clamp(v, min, max) - min) / (max - min)`, with `min` defaulting to 0 and `max`
//!   defaulting to the largest entry in any curve.
//! * `ticks` is capped at 32 upstream ("the gradient makes the diagram unreadable"), and konoma
//!   keeps the cap.

use super::{
    find_header, first_word, parse_number, read_quoted, Envelope, ParseError, Preamble, TitleSyntax,
};

/// The keyword.
pub const KEYWORD: &str = "radar-beta";

/// How the background rings are drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Graticule {
    /// Concentric circles. The default.
    #[default]
    Circle,
    /// Concentric copies of the axis polygon.
    Polygon,
}

/// One spoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Axis {
    /// The id other statements refer to it by.
    pub name: String,
    /// The text drawn at its end — the `["…"]` label, or the name when there is none.
    pub label: String,
}

/// One plotted series.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    /// The id.
    pub name: String,
    /// The legend text.
    pub label: String,
    /// One value per axis, in axis order.
    pub entries: Vec<f64>,
}

/// The `showLegend` / `ticks` / `max` / `min` / `graticule` settings, resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Whether the legend is drawn. Default true.
    pub show_legend: bool,
    /// How many rings. Default 5, capped at [`MAX_TICKS`].
    pub ticks: usize,
    /// The outer edge of the scale. `None` means "the largest value in any curve".
    pub max: Option<f64>,
    /// The centre of the scale. Default 0.
    pub min: f64,
    /// Ring shape.
    pub graticule: Graticule,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            show_legend: true,
            ticks: 5,
            max: None,
            min: 0.0,
            graticule: Graticule::Circle,
        }
    }
}

/// Upstream's ceiling on `ticks`.
pub const MAX_TICKS: usize = 32;

/// A parsed radar chart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Radar {
    /// Title and the accessibility statements.
    pub preamble: Preamble,
    /// The spokes, in declaration order — which is the order every curve's entries are in.
    pub axes: Vec<Axis>,
    /// The series.
    pub curves: Vec<Curve>,
    /// The resolved settings.
    pub options: Options,
}

impl Radar {
    /// The outer edge of the scale: `max` if given, else the largest value anywhere.
    pub fn max(&self) -> f64 {
        if let Some(m) = self.options.max {
            return m;
        }
        self.curves
            .iter()
            .flat_map(|c| c.entries.iter().copied())
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

/// A `curve` whose entries have not yet been matched against the axes.
///
/// Held back on purpose: upstream requires the axes to be declared *before* any curve that names
/// them (`computeCurveEntries` throws "Axes must be populated before curves") and konoma does not,
/// because in a declarative source that order carries no meaning.
struct PendingCurve {
    /// 1-based line of the `curve` statement, for the error message.
    line: usize,
    /// The id.
    name: String,
    /// The legend text.
    label: String,
    /// `(axis name, value)` as written — the name is `None` for a positional entry.
    entries: Vec<(Option<String>, f64)>,
}

/// Whether this source is a radar chart.
pub fn is_radar(src: &str) -> bool {
    find_header(src, &[KEYWORD]).is_some()
}

/// Parses a mermaid radar chart.
pub fn parse(src: &str) -> Result<Radar, ParseError> {
    let Some(source) = find_header(src, &[KEYWORD]) else {
        return Err(ParseError::NotThisChart {
            expected: KEYWORD,
            header: first_word(src),
        });
    };

    let mut radar = Radar::default();
    let mut env = Envelope::new(source.front_matter_title);
    // `showLegend`, `ticks`, … are read into these first so a later statement can override an
    // earlier one the way `setOptions`'s option map does.
    let mut options = Options::default();
    let mut ticks_seen: Option<usize> = None;
    // Detailed entries are resolved against the axes *after* the whole source is read, so a curve
    // written before its axes still works — upstream requires the axes first and throws otherwise,
    // which is a needless order dependency in a declarative language.
    let mut pending: Vec<PendingCurve> = Vec::new();

    // A curve's `{ … }` may span lines, so statements are gathered rather than taken one line at
    // a time.
    let mut buffer = String::new();
    let mut buffer_line = 0usize;
    let mut depth = 0usize;

    let header_rest = source.header_rest.trim().trim_start_matches(':').trim();
    let mut feed: Vec<(usize, String)> = Vec::new();
    if !header_rest.is_empty() {
        feed.push((source.header_index + 1, header_rest.to_string()));
    }
    for (i, line) in source
        .lines
        .iter()
        .enumerate()
        .skip(source.header_index + 1)
    {
        feed.push((i + 1, line.clone()));
    }

    for (number, line) in feed {
        if depth == 0 && env.read(&line, TitleSyntax::Terminal) {
            continue;
        }
        if depth == 0 {
            buffer.clear();
            buffer_line = number;
        }
        if !buffer.is_empty() {
            buffer.push(' ');
        }
        buffer.push_str(line.trim());
        depth = (depth + line.matches('{').count()).saturating_sub(line.matches('}').count());
        if depth == 0 {
            let stmt = buffer.trim().to_string();
            if !stmt.is_empty() {
                statement(
                    &stmt,
                    buffer_line,
                    &mut radar,
                    &mut options,
                    &mut ticks_seen,
                    &mut pending,
                )?;
            }
            buffer.clear();
        }
    }
    if depth != 0 {
        return Err(ParseError::Invalid {
            line: buffer_line,
            message: "a curve's `{` is never closed".to_string(),
        });
    }

    radar.preamble = env.preamble;
    radar.options = options;
    if let Some(t) = ticks_seen {
        radar.options.ticks = t.min(MAX_TICKS);
    }

    for c in pending {
        let values = resolve_entries(&radar.axes, &c.entries, c.line)?;
        radar.curves.push(Curve {
            name: c.name,
            label: c.label,
            entries: values,
        });
    }

    if radar.axes.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "axis",
        });
    }
    if radar.curves.is_empty() {
        return Err(ParseError::NoData {
            kind: KEYWORD,
            wanted: "curve",
        });
    }
    if radar.max() <= radar.options.min {
        return Err(ParseError::Invalid {
            line: 0,
            message: format!(
                "the scale has no width: max {} is not above min {}",
                radar.max(),
                radar.options.min
            ),
        });
    }
    Ok(radar)
}

fn statement(
    stmt: &str,
    number: usize,
    radar: &mut Radar,
    options: &mut Options,
    ticks_seen: &mut Option<usize>,
    pending: &mut Vec<PendingCurve>,
) -> Result<(), ParseError> {
    if let Some(rest) = word(stmt, "axis") {
        for part in split_commas(rest) {
            let (name, label, tail) = name_and_label(&part, number)?;
            if !tail.trim().is_empty() {
                return Err(ParseError::Unexpected {
                    kind: KEYWORD,
                    line: number,
                    text: tail.trim().to_string(),
                });
            }
            radar.axes.push(Axis { name, label });
        }
        return Ok(());
    }
    if let Some(rest) = word(stmt, "curve") {
        for part in split_curves(rest) {
            let (name, label, tail) = name_and_label(&part, number)?;
            let tail = tail.trim();
            let Some(body) = tail.strip_prefix('{').and_then(|b| b.strip_suffix('}')) else {
                return Err(ParseError::Invalid {
                    line: number,
                    message: format!("curve `{name}` needs its values in `{{ … }}`"),
                });
            };
            let mut entries = Vec::new();
            for item in split_commas(body) {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                entries.push(entry(item, number)?);
            }
            if entries.is_empty() {
                return Err(ParseError::Invalid {
                    line: number,
                    message: format!("curve `{name}` has no values"),
                });
            }
            pending.push(PendingCurve {
                line: number,
                name,
                label,
                entries,
            });
        }
        return Ok(());
    }
    // The five options. `(',' options+=Option)*` lets them share a line.
    let mut any = false;
    for part in split_commas(stmt) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if !option(part, number, options, ticks_seen)? {
            return Err(ParseError::Unexpected {
                kind: KEYWORD,
                line: number,
                text: part.to_string(),
            });
        }
        any = true;
    }
    if any {
        return Ok(());
    }
    Err(ParseError::Unexpected {
        kind: KEYWORD,
        line: number,
        text: stmt.to_string(),
    })
}

fn option(
    part: &str,
    number: usize,
    options: &mut Options,
    ticks_seen: &mut Option<usize>,
) -> Result<bool, ParseError> {
    if let Some(v) = word(part, "showLegend") {
        options.show_legend = match v.trim() {
            "true" => true,
            "false" => false,
            other => {
                return Err(ParseError::Invalid {
                    line: number,
                    message: format!("showLegend takes `true` or `false`, not `{other}`"),
                })
            }
        };
        return Ok(true);
    }
    if let Some(v) = word(part, "graticule") {
        options.graticule = match v.trim() {
            "circle" => Graticule::Circle,
            "polygon" => Graticule::Polygon,
            other => {
                return Err(ParseError::Invalid {
                    line: number,
                    message: format!("graticule takes `circle` or `polygon`, not `{other}`"),
                })
            }
        };
        return Ok(true);
    }
    for name in ["ticks", "max", "min"] {
        if let Some(v) = word(part, name) {
            let text = v.trim();
            // `NUMBER` in `common.langium` carries no sign, so a negative bound is not a number
            // here — which is worth saying rather than silently accepting.
            let Some(n) = parse_number(text, false) else {
                return Err(ParseError::BadNumber {
                    line: number,
                    text: text.to_string(),
                    why: "expected a number with no sign",
                });
            };
            match name {
                "ticks" => *ticks_seen = Some(n.max(0.0) as usize),
                "max" => options.max = Some(n),
                _ => options.min = n,
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// `value` or `axisName: value` / `axisName value`.
fn entry(item: &str, number: usize) -> Result<(Option<String>, f64), ParseError> {
    if let Some(v) = parse_number(item, false) {
        return Ok((None, v));
    }
    let (name, rest) = match item.split_once(':') {
        Some((n, r)) => (n.trim(), r.trim()),
        None => match item.split_once(char::is_whitespace) {
            Some((n, r)) => (n.trim(), r.trim()),
            None => {
                return Err(ParseError::BadNumber {
                    line: number,
                    text: item.to_string(),
                    why: "expected a number, or `axis: number`",
                })
            }
        },
    };
    let Some(v) = parse_number(rest, false) else {
        return Err(ParseError::BadNumber {
            line: number,
            text: rest.to_string(),
            why: "expected a number",
        });
    };
    Ok((Some(name.to_string()), v))
}

/// `computeCurveEntries`: positional entries stay put, referenced ones are gathered **per axis**.
fn resolve_entries(
    axes: &[Axis],
    entries: &[(Option<String>, f64)],
    number: usize,
) -> Result<Vec<f64>, ParseError> {
    if entries.first().is_some_and(|(a, _)| a.is_none()) {
        return Ok(entries.iter().map(|(_, v)| *v).collect());
    }
    if axes.is_empty() {
        return Err(ParseError::Invalid {
            line: number,
            message: "a curve names its axes but the chart declares none".to_string(),
        });
    }
    axes.iter()
        .map(|axis| {
            entries
                .iter()
                .find(|(name, _)| name.as_deref() == Some(axis.name.as_str()))
                .map(|(_, v)| *v)
                .ok_or_else(|| ParseError::Invalid {
                    line: number,
                    message: format!("no entry for axis `{}`", axis.label),
                })
        })
        .collect()
}

/// `name` followed by an optional `["label"]`. Returns the two and whatever is left.
fn name_and_label(part: &str, number: usize) -> Result<(String, String, String), ParseError> {
    let part = part.trim();
    let name: String = part
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if name.is_empty() {
        return Err(ParseError::Invalid {
            line: number,
            message: format!("`{part}` does not start with a name"),
        });
    }
    let rest = part[name.len()..].trim_start();
    let Some(after_open) = rest.strip_prefix('[') else {
        return Ok((name.clone(), name, rest.to_string()));
    };
    let chars: Vec<char> = after_open.trim_start().chars().collect();
    let Some((label, used)) = read_quoted(&chars, 0) else {
        return Err(ParseError::Invalid {
            line: number,
            message: format!("the label of `{name}` must be quoted"),
        });
    };
    let consumed: usize = chars[..used].iter().map(|c| c.len_utf8()).sum();
    let after = after_open.trim_start();
    let Some(tail) = after[consumed..].trim_start().strip_prefix(']') else {
        return Err(ParseError::Invalid {
            line: number,
            message: format!("the label of `{name}` is never closed with `]`"),
        });
    };
    Ok((name, label, tail.to_string()))
}

/// `keyword` followed by whitespace or a `[`.
fn word<'a>(stmt: &'a str, kw: &str) -> Option<&'a str> {
    let rest = super::strip_ci(stmt.trim_start(), kw)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() || c == '[' || c == '{' => Some(rest),
        Some(_) => None,
    }
}

/// Splits on commas outside quotes and braces.
fn split_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '{' | '[' => {
                    depth += 1;
                    cur.push(c);
                }
                '}' | ']' => {
                    depth = depth.saturating_sub(1);
                    cur.push(c);
                }
                ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out
}

/// `curve a{1,2}, b{3,4}` — split between curves, which [`split_commas`] already does because it
/// counts braces. Named for what it means at the call site.
fn split_curves(s: &str) -> Vec<String> {
    split_commas(s)
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .collect()
}
