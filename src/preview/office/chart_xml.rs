//! Reading a chart part (`c:chartSpace`: `ppt/charts/chartN.xml`, `word/charts/..`, `xl/charts/..`;
//! the format is the same in all three) into a [`ChartModel`].
//!
//! * The data come from the caches in the part (`c:numCache`, `c:strCache`, `c:multiLvlStrCache`
//!   -- the first, innermost level --, `c:numLit`, `c:strLit`). **The embedded workbook is never
//!   opened.**
//! * Colours are resolved by the caller ([`ChartEnv::resolve_color`]): it gets the colour element
//!   itself (`a:srgbClr`, `a:schemeClr`, `a:sysClr`, ..., with its transform children) and returns
//!   the final colour; the theme belongs to whoever owns the file. The theme accents and fonts come
//!   in [`ChartEnv`] too.
//! * Out of scope: `c:userShapes` (shapes drawn over the chart), `cx:` charts (chartex: waterfall,
//!   funnel, treemap ... -- [`parse_chart`] says [`OfficeError::Unsupported`] and the caller draws
//!   the fallback picture), the axis-less surface charts, and everything about the embedded
//!   workbook. Stock and of-pie charts are read as line and pie charts, with
//!   [`ParsedChart::approximated`] set.
//!
//! # Budgets
//!
//! The part is read into a small element tree whose size is bounded as it is built: at most
//! [`MAX_XML_NODES`] elements, [`MAX_SERIES`] series and [`MAX_TOTAL_POINTS`] points of caches
//! (the points past the limits are skipped as they are read, never stored); a part over the node
//! budget is refused as a whole. A `ptCount` is never trusted for an allocation (lists grow by the
//! points that are really there, up to [`MAX_POINTS`]). Anything dropped sets
//! [`ParsedChart::truncated`].

// The pptx reader that consumes this module arrives in a later task; until then only the tests use it.
#![allow(dead_code)]

use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::XmlVersion;

use super::docx_xml::{Kid, Node};
use super::fmt_xlsx::{xml_err, XmlReader};
use super::slide_draw::chart::{
    Axis, AxisKind, AxisPos, BarDir, ChartGroup, ChartModel, ChartText, Crosses, DataLabels,
    DispBlanks, GroupKind, Grouping, LabelPos, Legend, LegendPos, ManualLayout, Marker,
    MarkerSymbol, PointFmt, RadarStyle, ScatterStyle, Series, Stroke, TextStyle, TickLabelPos,
    TickMark, MAX_CATEGORIES, MAX_LABEL_CHARS, MAX_POINTS, MAX_SERIES,
};
use super::slide_draw::{Dash, Fill, Gradient, Rgba};
use super::OfficeError;

/// Most elements the tree of one chart part may hold.
pub const MAX_XML_NODES: usize = 250_000;
/// Most cache points (over all series and caches of a chart) that are read.
pub const MAX_TOTAL_POINTS: usize = 100_000;
/// Most bytes of text kept (values, names, labels, format codes).
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// Longest attribute value kept.
const MAX_ATTR_BYTES: usize = 1024;

/// What the reader gets from its caller.
pub struct ChartEnv<'a> {
    /// Resolves a colour element (see the module documentation).
    pub resolve_color: &'a dyn Fn(&Node) -> Option<Rgba>,
    /// The theme accents `accent1` .. `accent6`.
    pub palette: &'a [Rgba],
    /// The theme's minor (body) Latin font (`+mn-lt`) and major one (`+mj-lt`).
    pub minor_font: Option<&'a str>,
    pub major_font: Option<&'a str>,
}

/// The result of reading a chart part.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedChart {
    pub model: ChartModel,
    /// Something was left out at a budget.
    pub truncated: bool,
    /// A chart type was read as the nearest one (stock -> line, of-pie -> pie) or skipped (surface).
    pub approximated: bool,
}

/// Reads a chart part.
pub fn parse_chart(xml: &[u8], env: &ChartEnv) -> Result<ParsedChart, OfficeError> {
    let (root, mut truncated) = build_tree(xml)?;
    if root.name != "chartSpace" || root.prefix == "cx" {
        return Err(OfficeError::Unsupported);
    }
    let mut p = Parser {
        env,
        approximated: false,
        truncated: false,
        ja: false,
    };
    let model = p.chart_space(&root);
    truncated |= p.truncated;
    Ok(ParsedChart {
        model,
        truncated,
        approximated: p.approximated,
    })
}

// ---------------------------------------------------------------------------------------------
// The element tree
// ---------------------------------------------------------------------------------------------

/// Elements whose text is data.
fn keeps_text(name: &str) -> bool {
    matches!(name, "v" | "t" | "formatCode" | "separator")
}

fn attrs_of(e: &BytesStart<'_>) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false).flatten() {
        if attrs.len() >= 24 {
            break;
        }
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let val = if a.value.len() > MAX_ATTR_BYTES * 4 {
            String::new()
        } else {
            match a.normalized_value(XmlVersion::Implicit1_0) {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            }
        };
        let mut val = val;
        if val.len() > MAX_ATTR_BYTES {
            let mut n = MAX_ATTR_BYTES;
            while !val.is_char_boundary(n) {
                n -= 1;
            }
            val.truncate(n);
        }
        attrs.push((key, val));
    }
    attrs
}

fn node_of(e: &BytesStart<'_>) -> Node {
    let full = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let (prefix, name) = match full.split_once(':') {
        Some((p, n)) => (p.to_string(), n.to_string()),
        None => (String::new(), full),
    };
    Node {
        prefix,
        name,
        attrs: attrs_of(e),
        kids: Vec::new(),
    }
}

/// Counters of what the tree builder keeps.
struct Limits {
    nodes: usize,
    text: usize,
    series: usize,
    points: usize,
    truncated: bool,
}

impl Limits {
    /// Whether the element should be skipped: a series, or a cache point, past the budget.
    fn skip(&mut self, n: &Node) -> bool {
        match n.name.as_str() {
            "ser" => {
                if self.series >= MAX_SERIES {
                    self.truncated = true;
                    return true;
                }
                self.series += 1;
            }
            "pt" => {
                let idx = n.attr("idx").and_then(|v| v.parse::<usize>().ok());
                if self.points >= MAX_TOTAL_POINTS || idx.is_none_or(|i| i >= MAX_POINTS) {
                    self.truncated = true;
                    return true;
                }
                self.points += 1;
            }
            _ => {}
        }
        false
    }
}

fn build_tree(xml: &[u8]) -> Result<(Node, bool), OfficeError> {
    let mut rd = XmlReader::new(xml);
    let mut lim = Limits {
        nodes: MAX_XML_NODES,
        text: MAX_TEXT_BYTES,
        series: 0,
        points: 0,
        truncated: false,
    };
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => {
                let n = node_of(&e);
                if root.is_some() {
                    break;
                }
                if !stack.is_empty() && lim.skip(&n) {
                    skip_element(&mut rd, e.name().as_ref())?;
                    continue;
                }
                if lim.nodes == 0 {
                    return Err(OfficeError::TooLarge {
                        what: "chart nodes",
                    });
                }
                lim.nodes -= 1;
                stack.push(n);
            }
            Event::Empty(e) => {
                let n = node_of(&e);
                if stack.is_empty() {
                    if root.is_none() {
                        root = Some(n);
                    }
                    continue;
                }
                if lim.skip(&n) {
                    continue;
                }
                if lim.nodes == 0 {
                    return Err(OfficeError::TooLarge {
                        what: "chart nodes",
                    });
                }
                lim.nodes -= 1;
                if let Some(top) = stack.last_mut() {
                    top.kids.push(Kid::N(n));
                }
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else { break };
                match stack.last_mut() {
                    Some(parent) => parent.kids.push(Kid::N(done)),
                    None => {
                        root = Some(done);
                        break;
                    }
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    if keeps_text(&top.name) {
                        let s = match t.xml10_content() {
                            Ok(s) => s.into_owned(),
                            Err(_) => String::from_utf8_lossy(&t).into_owned(),
                        };
                        push_text(top, &s, &mut lim);
                    }
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    if keeps_text(&top.name) {
                        let s = String::from_utf8_lossy(&t).into_owned();
                        push_text(top, &s, &mut lim);
                    }
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    if keeps_text(&top.name) {
                        let mut s = String::new();
                        if let Ok(name) = r.decode() {
                            if let Some(x) = quick_xml::escape::resolve_xml_entity(&name) {
                                s.push_str(x);
                            } else if let Ok(Some(c)) = r.resolve_char_ref() {
                                s.push(c);
                            }
                        }
                        push_text(top, &s, &mut lim);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    match root {
        Some(r) => Ok((r, lim.truncated)),
        None => Err(OfficeError::Corrupt("chart: no root element".into())),
    }
}

fn push_text(top: &mut Node, s: &str, lim: &mut Limits) {
    if s.is_empty() {
        return;
    }
    if lim.text < s.len() {
        lim.text = 0;
        lim.truncated = true;
        return;
    }
    lim.text -= s.len();
    match top.kids.last_mut() {
        Some(Kid::T(t)) => t.push_str(s),
        _ => top.kids.push(Kid::T(s.to_string())),
    }
}

/// Reads to the end of the element whose start tag was just read.
fn skip_element<R: BufRead>(rd: &mut XmlReader<R>, name: &[u8]) -> Result<(), OfficeError> {
    let mut buf = Vec::new();
    rd.read_to_end_into(quick_xml::name::QName(name), &mut buf)
        .map_err(xml_err)
}

// ---------------------------------------------------------------------------------------------
// Reading the model
// ---------------------------------------------------------------------------------------------

struct Parser<'a, 'e> {
    env: &'a ChartEnv<'e>,
    approximated: bool,
    truncated: bool,
    /// Dates and month names are Japanese.
    ja: bool,
}

fn val<'n>(n: &'n Node, child: &str) -> Option<&'n str> {
    n.child(child).and_then(|c| c.attr("val"))
}

fn num(n: &Node, child: &str) -> Option<f64> {
    val(n, child)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// A boolean child: a bare element means true.
fn flag(n: &Node, child: &str) -> Option<bool> {
    let c = n.child(child)?;
    Some(match c.attr("val") {
        None => true,
        Some(v) => matches!(v.trim(), "1" | "true" | "on"),
    })
}

fn attr_num(n: &Node, name: &str) -> Option<f64> {
    n.attr(name)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

const COLOR_ELEMENTS: [&str; 6] = [
    "srgbClr",
    "schemeClr",
    "sysClr",
    "prstClr",
    "hslClr",
    "scrgbClr",
];

impl Parser<'_, '_> {
    fn color_in(&self, n: &Node) -> Option<Rgba> {
        n.nodes()
            .find(|c| COLOR_ELEMENTS.contains(&c.name.as_str()))
            .and_then(|c| (self.env.resolve_color)(c))
    }

    fn font(&self, typeface: &str) -> Option<String> {
        match typeface {
            "+mn-lt" | "+mn-ea" | "+mn-cs" => self.env.minor_font.map(str::to_string),
            "+mj-lt" | "+mj-ea" | "+mj-cs" => self.env.major_font.map(str::to_string),
            "" => None,
            t => Some(t.to_string()),
        }
    }

    // ---- fills and lines ----

    /// The fill an `spPr` (or `a:ln`'s own `solidFill` holder) specifies: `None` = automatic.
    fn fill_of(&self, sp: &Node) -> Option<Fill> {
        for c in sp.nodes() {
            match c.name.as_str() {
                "noFill" => return Some(Fill::None),
                "solidFill" => return self.color_in(c).map(Fill::Solid),
                "gradFill" => return self.gradient(c),
                "pattFill" => {
                    let fg = c.child("fgClr").and_then(|n| self.color_in(n));
                    let bg = c.child("bgClr").and_then(|n| self.color_in(n));
                    return fg.map(|fg| Fill::Pattern {
                        preset: c.attr("prst").unwrap_or("pct50").to_string(),
                        fg,
                        bg: bg.unwrap_or(Rgba::WHITE),
                    });
                }
                _ => {}
            }
        }
        None
    }

    fn gradient(&self, g: &Node) -> Option<Fill> {
        let mut stops: Vec<(f64, Rgba)> = Vec::new();
        for gs in g
            .child("gsLst")?
            .nodes()
            .filter(|n| n.name == "gs")
            .take(32)
        {
            let pos = (attr_num(gs, "pos").unwrap_or(0.0) / 100_000.0).clamp(0.0, 1.0);
            if let Some(c) = self.color_in(gs) {
                stops.push((pos, c));
            }
        }
        stops.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        match stops.len() {
            0 => None,
            1 => Some(Fill::Solid(stops[0].1)),
            _ => {
                let ang = g
                    .child("lin")
                    .and_then(|l| attr_num(l, "ang"))
                    .map_or(0.0, |a| a / 60_000.0);
                Some(Fill::Gradient(Gradient::linear(ang, stops)))
            }
        }
    }

    fn stroke_of(&self, sp: &Node) -> Option<Stroke> {
        let ln = sp.child("ln")?;
        let mut s = Stroke {
            width: attr_num(ln, "w").map(|w| w.clamp(0.0, 5_000_000.0)),
            ..Stroke::default()
        };
        match self.fill_of(ln) {
            Some(Fill::None) => s.none = true,
            Some(Fill::Solid(c)) => s.color = Some(c),
            Some(Fill::Gradient(g)) => s.color = g.stops.first().map(|x| x.1),
            Some(Fill::Pattern { fg, .. }) => s.color = Some(fg),
            _ => {}
        }
        s.dash = match ln.child("prstDash").and_then(|d| d.attr("val")) {
            Some("dot") | Some("sysDot") => Dash::Dot,
            Some("dash") => Dash::Dash,
            Some("sysDash") => Dash::SysDash,
            Some("lgDash") => Dash::LgDash,
            Some("dashDot") => Dash::DashDot,
            Some("sysDashDot") => Dash::SysDashDot,
            Some("lgDashDot") => Dash::LgDashDot,
            Some("lgDashDotDot") => Dash::LgDashDotDot,
            Some("sysDashDotDot") => Dash::SysDashDotDot,
            _ => Dash::Solid,
        };
        Some(s)
    }

    // ---- text ----

    /// The text style of a `c:txPr` or `c:rich` body: the paragraph's `defRPr`, over which the
    /// first run's `rPr` applies.
    fn text_style(&self, body: &Node) -> TextStyle {
        let mut st = TextStyle::default();
        if let Some(bp) = body.child("bodyPr") {
            // Office writes `rot="-60000000"` (a turn far outside +-360 degrees) for "automatic".
            st.rot_deg = attr_num(bp, "rot")
                .filter(|r| r.abs() <= 21_600_000.0)
                .map(|r| r / 60_000.0);
        }
        let Some(p) = body.child("p") else { return st };
        let mut layers: Vec<&Node> = Vec::new();
        if let Some(d) = p.child("pPr").and_then(|pp| pp.child("defRPr")) {
            layers.push(d);
        }
        if let Some(r) = p
            .nodes()
            .find(|n| n.name == "r")
            .and_then(|r| r.child("rPr"))
        {
            layers.push(r);
        }
        for l in layers {
            if let Some(sz) = attr_num(l, "sz") {
                st.size_pt = Some(sz / 100.0);
            }
            if let Some(b) = l.attr("b") {
                st.bold = Some(matches!(b, "1" | "true"));
            }
            if let Some(i) = l.attr("i") {
                st.italic = Some(matches!(i, "1" | "true"));
            }
            if let Some(f) = l.child("solidFill") {
                if let Some(c) = self.color_in(f) {
                    st.color = Some(c);
                }
            }
            if let Some(t) = l.child("latin").and_then(|x| x.attr("typeface")) {
                if let Some(f) = self.font(t) {
                    st.font = Some(f);
                }
            }
        }
        st
    }

    fn tx_pr(&self, parent: &Node) -> TextStyle {
        parent
            .child("txPr")
            .map(|t| self.text_style(t))
            .unwrap_or_default()
    }

    /// The paragraphs of a rich text as one string (`\n` between paragraphs and at breaks).
    fn rich_text(&self, rich: &Node) -> String {
        let mut out = String::new();
        for (pi, p) in rich.nodes().filter(|n| n.name == "p").enumerate().take(64) {
            if pi > 0 {
                out.push('\n');
            }
            for r in p.nodes() {
                match r.name.as_str() {
                    "r" | "fld" => {
                        if let Some(t) = r.child("t") {
                            out.push_str(&t.text());
                        }
                    }
                    "br" => out.push('\n'),
                    _ => {}
                }
            }
            if out.len() > MAX_LABEL_CHARS * 4 {
                break;
            }
        }
        cut(out)
    }

    /// A `c:tx` holding a rich text or a string reference.
    fn tx_text(&self, tx: &Node) -> Option<String> {
        if let Some(r) = tx.child("rich") {
            return Some(self.rich_text(r));
        }
        if let Some(r) = tx.child("strRef") {
            return first_pt(r.child("strCache")?);
        }
        tx.child("v").map(|v| cut(v.text()))
    }

    fn title(&self, t: &Node) -> ChartText {
        let rich = t.child("tx").and_then(|tx| tx.child("rich"));
        let mut style = rich.map(|r| self.text_style(r)).unwrap_or_default();
        let text = t
            .child("tx")
            .and_then(|tx| self.tx_text(tx))
            .unwrap_or_default();
        if style == TextStyle::default() {
            style = self.tx_pr(t);
        }
        ChartText {
            text,
            style,
            layout: t.child("layout").and_then(manual_layout),
            overlay: flag(t, "overlay").unwrap_or(false),
        }
    }

    // ---- the chart ----

    fn chart_space(&mut self, root: &Node) -> ChartModel {
        self.ja = val(root, "lang").is_some_and(|l| l.starts_with("ja"));
        let mut m = ChartModel {
            palette: self.env.palette.to_vec(),
            default_font: self.env.minor_font.map(str::to_string),
            locale_ja: self.ja,
            ..ChartModel::default()
        };
        if let Some(sp) = root.child("spPr") {
            m.chart_fill = self.fill_of(sp);
            m.chart_line = self.stroke_of(sp);
        }
        m.text = self.tx_pr(root);
        m.style = chart_style(root);
        let Some(chart) = root.child("chart") else {
            return m;
        };
        m.disp_blanks_as = match val(chart, "dispBlanksAs") {
            Some("zero") => DispBlanks::Zero,
            Some("span") => DispBlanks::Span,
            _ => DispBlanks::Gap,
        };
        m.three_d = chart.child("view3D").is_some();
        let title = chart.child("title").map(|t| self.title(t));
        let title_deleted = flag(chart, "autoTitleDeleted").unwrap_or(false);
        if let Some(pa) = chart.child("plotArea") {
            self.plot_area(pa, &mut m);
        }
        if let Some(l) = chart.child("legend") {
            m.legend = Some(self.legend(l));
        }
        // An automatic title is the name of the only series.
        m.title = match title {
            Some(mut t) if t.text.trim().is_empty() => {
                let series: Vec<&Series> = m.groups.iter().flat_map(|g| g.series.iter()).collect();
                match (series.as_slice(), t.layout.is_some() || !title_deleted) {
                    ([s], true) => {
                        t.text = s.name.clone().unwrap_or_default();
                        (!t.text.is_empty()).then_some(t)
                    }
                    _ => None,
                }
            }
            t => t,
        };
        m
    }

    fn legend(&self, l: &Node) -> Legend {
        let mut out = Legend {
            pos: match val(l, "legendPos") {
                Some("l") => LegendPos::Left,
                Some("t") => LegendPos::Top,
                Some("b") => LegendPos::Bottom,
                Some("tr") => LegendPos::TopRight,
                _ => LegendPos::Right,
            },
            overlay: flag(l, "overlay").unwrap_or(false),
            layout: l.child("layout").and_then(manual_layout),
            style: self.tx_pr(l),
            ..Legend::default()
        };
        if let Some(sp) = l.child("spPr") {
            out.fill = self.fill_of(sp);
            out.line = self.stroke_of(sp);
        }
        for e in l.nodes().filter(|n| n.name == "legendEntry").take(300) {
            if flag(e, "delete").unwrap_or(false) {
                if let Some(i) = attr_num_child(e, "idx") {
                    out.deleted.push(i);
                }
            }
        }
        out
    }

    fn plot_area(&mut self, pa: &Node, m: &mut ChartModel) {
        m.plot_layout = pa.child("layout").and_then(manual_layout);
        if let Some(sp) = pa.child("spPr") {
            m.plot_fill = self.fill_of(sp);
            m.plot_line = self.stroke_of(sp);
        }
        for c in pa.nodes() {
            let kind = match c.name.as_str() {
                "barChart" | "bar3DChart" => Some(GroupKind::Bar),
                "lineChart" | "line3DChart" => Some(GroupKind::Line),
                "stockChart" => {
                    self.approximated = true;
                    Some(GroupKind::Line)
                }
                "areaChart" | "area3DChart" => Some(GroupKind::Area),
                "pieChart" | "pie3DChart" => Some(GroupKind::Pie),
                "ofPieChart" => {
                    self.approximated = true;
                    Some(GroupKind::Pie)
                }
                "doughnutChart" => Some(GroupKind::Doughnut),
                "scatterChart" => Some(GroupKind::Scatter),
                "radarChart" => Some(GroupKind::Radar),
                "bubbleChart" => Some(GroupKind::Bubble),
                "surfaceChart" | "surface3DChart" => {
                    self.approximated = true;
                    None
                }
                "catAx" | "valAx" | "dateAx" | "serAx" => {
                    if m.axes.len() < 64 {
                        m.axes.push(self.axis(c));
                    }
                    None
                }
                _ => None,
            };
            if let Some(k) = kind {
                if m.groups.len() < 32 {
                    let g = self.group(c, k);
                    m.three_d |= g.three_d;
                    m.groups.push(g);
                }
            }
        }
    }

    // ---- groups and series ----

    fn group(&mut self, c: &Node, kind: GroupKind) -> ChartGroup {
        let mut g = ChartGroup {
            kind,
            three_d: c.name.ends_with("3DChart"),
            vary_colors: flag(c, "varyColors")
                .unwrap_or(matches!(kind, GroupKind::Pie | GroupKind::Doughnut)),
            ..ChartGroup::default()
        };
        g.bar_dir = if val(c, "barDir") == Some("bar") {
            BarDir::Bar
        } else {
            BarDir::Col
        };
        g.grouping = match val(c, "grouping") {
            Some("stacked") => Grouping::Stacked,
            Some("percentStacked") => Grouping::PercentStacked,
            Some("clustered") => Grouping::Clustered,
            _ if kind == GroupKind::Bar => Grouping::Clustered,
            _ => Grouping::Standard,
        };
        if let Some(v) = num(c, "gapWidth") {
            g.gap_width = v;
        }
        if let Some(v) = num(c, "overlap") {
            g.overlap = v;
        }
        if let Some(v) = num(c, "holeSize") {
            g.hole_size = v;
        }
        if let Some(v) = num(c, "firstSliceAng") {
            g.first_slice_ang = v;
        }
        if let Some(v) = num(c, "bubbleScale") {
            g.bubble_scale = v;
        }
        g.size_is_width = val(c, "sizeRepresents") == Some("w");
        g.scatter_style = match val(c, "scatterStyle") {
            Some("none") => ScatterStyle::None,
            Some("line") => ScatterStyle::Line,
            Some("marker") => ScatterStyle::Marker,
            Some("smooth") => ScatterStyle::Smooth,
            Some("smoothMarker") => ScatterStyle::SmoothMarker,
            _ => ScatterStyle::LineMarker,
        };
        g.radar_style = match val(c, "radarStyle") {
            Some("marker") => RadarStyle::Marker,
            Some("filled") => RadarStyle::Filled,
            _ => RadarStyle::Standard,
        };
        if kind == GroupKind::Line || kind == GroupKind::Radar {
            g.markers = flag(c, "marker").unwrap_or(true);
        }
        for ax in c.nodes().filter(|n| n.name == "axId").take(4) {
            if let Some(id) = ax.attr("val").and_then(|v| v.trim().parse::<i64>().ok()) {
                g.axis_ids.push(id);
            }
        }
        g.labels = c.child("dLbls").map(|d| self.data_labels(d));
        for s in c.nodes().filter(|n| n.name == "ser") {
            let ser = self.series(s, kind);
            self.truncated |= ser.truncated;
            g.series.push(ser);
        }
        // A group of a bar-in-a-3D-box with a standard grouping is a plain clustered one.
        if kind == GroupKind::Bar && g.grouping == Grouping::Standard {
            g.grouping = Grouping::Clustered;
        }
        g
    }

    fn series(&mut self, s: &Node, kind: GroupKind) -> Series {
        let mut out = Series {
            name: s.child("tx").and_then(|tx| self.tx_text(tx)).map(cut),
            ..Series::default()
        };
        if let Some(sp) = s.child("spPr") {
            out.fill = self.fill_of(sp);
            out.line = self.stroke_of(sp);
        }
        out.marker = s.child("marker").map(|m| self.marker(m));
        out.smooth = flag(s, "smooth");
        out.explosion = num(s, "explosion");
        for d in s.nodes().filter(|n| n.name == "dPt").take(MAX_POINTS) {
            let Some(idx) = attr_num_child(d, "idx") else {
                continue;
            };
            let mut pf = PointFmt {
                idx,
                explosion: num(d, "explosion"),
                marker: d.child("marker").map(|m| self.marker(m)),
                ..PointFmt::default()
            };
            if let Some(sp) = d.child("spPr") {
                pf.fill = self.fill_of(sp);
                pf.line = self.stroke_of(sp);
            }
            out.points.push(pf);
        }
        out.labels = s.child("dLbls").map(|d| self.data_labels(d));

        match kind {
            GroupKind::Scatter | GroupKind::Bubble => {
                if let Some(x) = s.child("xVal") {
                    let (xs, _, trunc) = num_data(x);
                    out.truncated |= trunc;
                    out.x_values = xs;
                }
                if let Some(y) = s.child("yVal") {
                    let (ys, fmt, trunc) = num_data(y);
                    out.truncated |= trunc;
                    out.values = ys;
                    out.format_code = fmt;
                }
                if let Some(b) = s.child("bubbleSize") {
                    let (bs, _, trunc) = num_data(b);
                    out.truncated |= trunc;
                    out.sizes = bs;
                }
            }
            _ => {
                if let Some(v) = s.child("val") {
                    let (vs, fmt, trunc) = num_data(v);
                    out.truncated |= trunc;
                    out.values = vs;
                    out.format_code = fmt;
                }
                if let Some(c) = s.child("cat") {
                    let (labels, nums, fmt, trunc) = self.cat_data(c);
                    out.truncated |= trunc;
                    out.cats = labels;
                    out.cat_nums = nums;
                    out.cat_format = fmt;
                }
            }
        }
        out
    }

    fn marker(&self, m: &Node) -> Marker {
        let mut mk = Marker {
            symbol: match val(m, "symbol") {
                Some("none") => MarkerSymbol::None,
                Some("circle") => MarkerSymbol::Circle,
                Some("square") => MarkerSymbol::Square,
                Some("diamond") => MarkerSymbol::Diamond,
                Some("triangle") => MarkerSymbol::Triangle,
                Some("x") => MarkerSymbol::X,
                Some("star") => MarkerSymbol::Star,
                Some("dot") => MarkerSymbol::Dot,
                Some("dash") => MarkerSymbol::Dash,
                Some("plus") => MarkerSymbol::Plus,
                _ => MarkerSymbol::Auto,
            },
            size_pt: num(m, "size"),
            ..Marker::default()
        };
        if let Some(sp) = m.child("spPr") {
            mk.fill = self.fill_of(sp);
            if let Some(s) = self.stroke_of(sp) {
                mk.line = s;
            }
        }
        mk
    }

    /// The categories of a series: labels, numbers (where they are numbers) and the format code.
    fn cat_data(&self, c: &Node) -> (Vec<String>, Vec<Option<f64>>, Option<String>, bool) {
        if let Some(r) = c.child("multiLvlStrRef") {
            if let Some(cache) = r.child("multiLvlStrCache") {
                // The first `lvl` is the innermost level: the leaf labels.
                if let Some(l) = cache.child("lvl") {
                    let (v, t) = str_pts(l, cache.child("ptCount").or_else(|| l.child("ptCount")));
                    return (v, Vec::new(), None, t);
                }
            }
        }
        let (num_node, str_node) = (
            c.child("numRef")
                .and_then(|r| r.child("numCache"))
                .or_else(|| c.child("numLit")),
            c.child("strRef")
                .and_then(|r| r.child("strCache"))
                .or_else(|| c.child("strLit")),
        );
        if let Some(n) = num_node {
            let (vals, fmt, trunc) = num_pts(n);
            let ja = self.ja;
            let labels: Vec<String> = vals
                .iter()
                .map(|v| match v {
                    Some(v) => cut(super::slide_draw::chart::format_number(
                        fmt.as_deref(),
                        *v,
                        ja,
                    )),
                    None => String::new(),
                })
                .collect();
            return (labels, vals, fmt, trunc);
        }
        if let Some(s) = str_node {
            let (v, t) = str_pts(s, s.child("ptCount"));
            return (v, Vec::new(), None, t);
        }
        (Vec::new(), Vec::new(), None, false)
    }

    // ---- data labels ----

    fn data_labels(&self, d: &Node) -> DataLabels {
        let mut out = self.label_settings(d);
        for l in d.nodes().filter(|n| n.name == "dLbl").take(MAX_POINTS) {
            let Some(idx) = attr_num_child(l, "idx") else {
                continue;
            };
            let mut pl = self.label_settings(l);
            if let Some(tx) = l.child("tx") {
                if let Some(r) = tx.child("rich") {
                    let t = self.rich_text(r);
                    if !t.is_empty() {
                        pl.text = Some(t);
                    }
                }
            }
            out.points.push((idx, pl));
        }
        out
    }

    fn label_settings(&self, d: &Node) -> DataLabels {
        let mut l = DataLabels {
            delete: flag(d, "delete").unwrap_or(false),
            show_value: flag(d, "showVal").unwrap_or(false),
            show_category: flag(d, "showCatName").unwrap_or(false),
            show_series: flag(d, "showSerName").unwrap_or(false),
            show_percent: flag(d, "showPercent").unwrap_or(false),
            show_legend_key: flag(d, "showLegendKey").unwrap_or(false),
            pos: match val(d, "dLblPos") {
                Some("ctr") => Some(LabelPos::Center),
                Some("inEnd") => Some(LabelPos::InsideEnd),
                Some("inBase") => Some(LabelPos::InsideBase),
                Some("outEnd") => Some(LabelPos::OutsideEnd),
                Some("bestFit") => Some(LabelPos::BestFit),
                Some("l") => Some(LabelPos::Left),
                Some("r") => Some(LabelPos::Right),
                Some("t") => Some(LabelPos::Above),
                Some("b") => Some(LabelPos::Below),
                _ => None,
            },
            separator: d.child("separator").map(|s| s.text()),
            style: self.tx_pr(d),
            ..DataLabels::default()
        };
        if let Some(nf) = d.child("numFmt") {
            if nf.attr("sourceLinked") != Some("1") {
                l.num_fmt = nf.attr("formatCode").map(str::to_string);
            }
        }
        l
    }

    // ---- axes ----

    fn axis(&self, a: &Node) -> Axis {
        let mut ax = Axis {
            id: a
                .child("axId")
                .and_then(|n| n.attr("val"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0),
            kind: match a.name.as_str() {
                "valAx" => AxisKind::Val,
                "dateAx" => AxisKind::Date,
                "serAx" => AxisKind::Ser,
                _ => AxisKind::Cat,
            },
            cross_ax: a
                .child("crossAx")
                .and_then(|n| n.attr("val"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0),
            pos: match val(a, "axPos") {
                Some("l") => AxisPos::Left,
                Some("r") => AxisPos::Right,
                Some("t") => AxisPos::Top,
                _ => AxisPos::Bottom,
            },
            deleted: flag(a, "delete").unwrap_or(false),
            ..Axis::default()
        };
        if let Some(sc) = a.child("scaling") {
            ax.reversed = val(sc, "orientation") == Some("maxMin");
            ax.min = num(sc, "min");
            ax.max = num(sc, "max");
            ax.log_base = num(sc, "logBase");
        }
        ax.major_unit = num(a, "majorUnit");
        ax.minor_unit = num(a, "minorUnit");
        ax.crosses = if let Some(v) = num(a, "crossesAt") {
            Crosses::At(v)
        } else {
            match val(a, "crosses") {
                Some("min") => Crosses::Min,
                Some("max") => Crosses::Max,
                _ => Crosses::AutoZero,
            }
        };
        let tick = |name: &str, dflt: TickMark| match val(a, name) {
            Some("none") => TickMark::None,
            Some("in") => TickMark::In,
            Some("out") => TickMark::Out,
            Some("cross") => TickMark::Cross,
            _ => dflt,
        };
        ax.major_tick = tick("majorTickMark", TickMark::Cross);
        ax.minor_tick = tick("minorTickMark", TickMark::None);
        ax.tick_label_pos = match val(a, "tickLblPos") {
            Some("low") => TickLabelPos::Low,
            Some("high") => TickLabelPos::High,
            Some("none") => TickLabelPos::None,
            _ => TickLabelPos::NextTo,
        };
        if let Some(nf) = a.child("numFmt") {
            if let Some(code) = nf.attr("formatCode") {
                ax.num_fmt = Some((code.to_string(), nf.attr("sourceLinked") == Some("1")));
            }
        }
        ax.major_grid = a.child("majorGridlines").map(|g| {
            g.child("spPr")
                .and_then(|sp| self.stroke_of(sp))
                .unwrap_or_default()
        });
        ax.minor_grid = a.child("minorGridlines").map(|g| {
            g.child("spPr")
                .and_then(|sp| self.stroke_of(sp))
                .unwrap_or_default()
        });
        ax.title = a.child("title").map(|t| self.title(t));
        ax.line = a.child("spPr").and_then(|sp| self.stroke_of(sp));
        ax.text = self.tx_pr(a);
        ax.between = val(a, "crossBetween") != Some("midCat");
        ax.label_skip = num(a, "tickLblSkip").map_or(0, |v| v.max(0.0) as usize);
        ax.tick_skip = num(a, "tickMarkSkip").map_or(0, |v| v.max(0.0) as usize);
        ax
    }
}

fn attr_num_child(n: &Node, child: &str) -> Option<usize> {
    n.child(child)
        .and_then(|c| c.attr("val"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|i| *i < MAX_POINTS)
}

fn cut(mut s: String) -> String {
    if let Some((i, _)) = s.char_indices().nth(MAX_LABEL_CHARS) {
        s.truncate(i);
    }
    s
}

fn first_pt(cache: &Node) -> Option<String> {
    cache
        .nodes()
        .find(|n| n.name == "pt")
        .and_then(|p| p.child("v"))
        .map(|v| cut(v.text()))
}

/// `(idx, text)` of the `pt` children of a cache, as a list indexed by `idx` (missing = empty).
fn str_pts(cache: &Node, count: Option<&Node>) -> (Vec<String>, bool) {
    let declared = count
        .and_then(|c| c.attr("val"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut out: Vec<String> = Vec::new();
    let mut trunc = declared > MAX_CATEGORIES;
    for p in cache.nodes().filter(|n| n.name == "pt") {
        let Some(i) = p.attr("idx").and_then(|v| v.trim().parse::<usize>().ok()) else {
            continue;
        };
        if i >= MAX_CATEGORIES {
            trunc = true;
            continue;
        }
        if out.len() <= i {
            out.resize(i + 1, String::new());
        }
        out[i] = p.child("v").map(|v| cut(v.text())).unwrap_or_default();
    }
    // Points the cache declares but does not list are blank labels.
    let want = declared.min(MAX_CATEGORIES);
    if out.len() < want && want <= out.len() + MAX_CATEGORIES {
        out.resize(want, String::new());
    }
    (out, trunc)
}

/// The numbers of a `c:val` / `c:xVal` / `c:yVal` / `c:bubbleSize`: values by point index, the
/// format code, and whether points were dropped.
fn num_data(n: &Node) -> (Vec<Option<f64>>, Option<String>, bool) {
    let cache = n
        .child("numRef")
        .and_then(|r| r.child("numCache"))
        .or_else(|| n.child("numLit"));
    match cache {
        Some(c) => num_pts(c),
        // A string reference in a place of numbers (text x values): no numbers.
        None => (Vec::new(), None, false),
    }
}

fn num_pts(cache: &Node) -> (Vec<Option<f64>>, Option<String>, bool) {
    let fmt = cache
        .child("formatCode")
        .map(|f| f.text())
        .filter(|f| !f.is_empty());
    let declared = cache
        .child("ptCount")
        .and_then(|c| c.attr("val"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut out: Vec<Option<f64>> = Vec::new();
    let mut trunc = declared > MAX_POINTS;
    for p in cache.nodes().filter(|n| n.name == "pt") {
        let Some(i) = p.attr("idx").and_then(|v| v.trim().parse::<usize>().ok()) else {
            continue;
        };
        if i >= MAX_POINTS {
            trunc = true;
            continue;
        }
        if out.len() <= i {
            out.resize(i + 1, None);
        }
        out[i] = p
            .child("v")
            .and_then(|v| v.text().trim().parse::<f64>().ok())
            .filter(|v| v.is_finite());
    }
    let want = declared.min(MAX_POINTS);
    if out.len() < want && want <= out.len() + MAX_POINTS {
        out.resize(want, None);
    }
    (out, fmt, trunc)
}

fn manual_layout(l: &Node) -> Option<ManualLayout> {
    let ml = l.child("manualLayout")?;
    let edge = |name: &str| val(ml, name) == Some("edge");
    Some(ManualLayout {
        x: num(ml, "x"),
        y: num(ml, "y"),
        w: num(ml, "w"),
        h: num(ml, "h"),
        x_edge: edge("xMode"),
        y_edge: edge("yMode"),
        w_edge: edge("wMode"),
        h_edge: edge("hMode"),
        inner: val(ml, "layoutTarget") == Some("inner"),
    })
}

/// `c:style val` of the chart space, directly or in the fallback of an `mc:AlternateContent`
/// (whose `c14:style` counts 100 more); 0 when absent or out of range.
fn chart_style(root: &Node) -> u8 {
    let ok = |v: &str| v.trim().parse::<u8>().ok().filter(|v| (1..=48).contains(v));
    for c in root.nodes() {
        if c.name == "style" && c.prefix == "c" {
            if let Some(v) = c.attr("val").and_then(ok) {
                return v;
            }
        }
        if c.name == "AlternateContent" {
            for fb in c.nodes().filter(|n| n.name == "Fallback") {
                if let Some(v) = val(fb, "style").and_then(ok) {
                    return v;
                }
            }
        }
    }
    0
}
