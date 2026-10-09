//! Charts: a format-neutral [`ChartModel`] and [`draw_chart`], which lowers a model into the shapes
//! and text boxes of the slide drawing model (there is no chart item; see [`super::model`]).
//!
//! The OOXML reader that builds a [`ChartModel`] is `office::chart_xml`; the readers of other
//! formats may build models too. Nothing here knows about files, relationships or themes: colours
//! are already resolved to [`Rgba`] and the series palette is part of the model.
//!
//! # What is drawn
//!
//! Column / bar (clustered, stacked, percent-stacked; gap width and overlap), line (with or without
//! markers, smooth, stacked), area (standard, stacked, percent), pie (vary colours, first slice
//! angle, explosion), doughnut (hole size, one ring per series), scatter (markers / lines /
//! smooth), radar (standard, markers, filled) and bubble charts; the 3-D variants of column / bar,
//! line, area and pie charts get depth (see [`three_d`]: an oblique projection with painted side
//! and top faces, walls, and a tilted pie). Around them: the chart and plot area fills, axes with
//! ticks, tick labels (number formats through `numfmt`, category text), major and minor gridlines,
//! axis titles, the chart title, the legend (right, left, top, bottom, top-right; one entry per
//! series, or per point for vary-colours pie and doughnut charts) and data labels (value,
//! percentage, category name, series name, in the positions Office offers; best fit is
//! inside the slice when the text fits and outside otherwise).
//!
//! # Approximations (deliberate)
//!
//! * 3-D charts use the small oblique projection of [`three_d`] (no perspective, no `hPercent`,
//!   `rAngAx = 0` drawn like `rAngAx = 1`, 3-D columns with series in depth rows drawn clustered,
//!   box shapes only); stock, surface and of-pie charts are not drawn by the OOXML reader.
//! * Date axes are category axes (the labels are formatted dates but the points are evenly
//!   spaced); series (depth) axes are not drawn.
//! * Data labels of pies' outside positions are in two columns, one on each side of the pie: on a
//!   side the labels that would cover each other are stacked in blocks centred on where they
//!   wanted to be (inside the plot's height), they hug the rim at their own height, and a label
//!   that moved gets a gray leader line to its slice; the labels a leader line reaches past line
//!   up with the moved ones, so a leader line never crosses a label's text. Labels of points (line, scatter, bubble charts) that
//!   would cover one another move up (above-point labels) or down by whole label heights to the
//!   nearest free spot, without leader lines. Labels of bars stay in their bar; labels of the
//!   two kinds never avoid each other, and the room made for pie labels is one line's height, so
//!   a pie with many stacked labels can touch the title. "Best fit" is outside-end for bars and
//!   "right" for lines.
//! * `invertIfNegative` is ignored; gradient and pattern fills are used where a shape has a
//!   bounding box of its own (bars, markers) and approximated elsewhere.
//! * Text is measured with the faces `resvg` uses (see `fonts`), so label widths follow the
//!   fallback fonts, not the Office ones.
//!
//! # Defaults (what a file that says nothing gets)
//!
//! These are the Office 2007 "style 2" defaults, which is what a chart part without the properties
//! means: text 10 pt black, title 18 pt bold, axis titles 10 pt bold; series colours `accent1` ..
//! `accent6` of the theme, and for a series past the sixth the same accents darkened and lightened
//! in the order of Office's "colorful" palette (`lumMod 60 %`, `lumMod 80 % + lumOff 20 %`,
//! `lumMod 60 % + lumOff 40 %`, `lumMod 50 %`, then `lumMod 70 % + lumOff 30 %` repeated);
//! axis lines and gridlines `868686` at 0.75 pt; a chart or plot area without a fill is
//! transparent; pie and doughnut slices without an outline have none; line series are 2.25 pt.
//!
//! `c:style` selects the automatic colours as Office 2007 does: styles 2, 10, 18, ... are the
//! colourful palette above, 1, 9, ... are grays, and 3..8 (+8k) are monochrome ramps of
//! `accent1..6`: a single series (or point) gets the accent, several get lightness ramped from a
//! darker to a lighter tone of it (approximated linearly in HSL).
//!
//! `c:style` 9 to 16 outline bars, areas and slices that have no `a:ln` in white (see
//! `layout::style_outline`); a legend outline is drawn only when the file gives it a colour.
//!
//! # Axis scaling
//!
//! See [`scale`]: an automatic value axis follows Excel's rules (zero is included unless the data
//! sit in the top sixth of their range, the maximum leaves 5 % of the range above the data, the
//! major unit is 1, 2 or 5 times a power of ten giving about one tick per 32 px of a vertical axis
//! or 60 px of a horizontal one).
//!
//! # Budgets
//!
//! A chart is built from untrusted data. [`MAX_SERIES`], [`MAX_POINTS`], [`MAX_CATEGORIES`],
//! [`MAX_LABEL_CHARS`] and [`MAX_DRAWN`] bound what the model may hold and what is drawn; past a
//! limit the rest is dropped and [`draw_chart`] reports it. Values are sanitised: NaN and infinity
//! are blanks, magnitudes are clamped to [`MAX_VALUE`].

use super::model::*;

mod cartesian;
mod labels;
pub use labels::format_number;
mod layout;
mod pie;
mod radar;
pub mod scale;
mod shapes;
mod text;
mod three_d;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_more;
#[cfg(test)]
mod tests_pie_labels;
#[cfg(test)]
mod tests_three_d;

/// Most series a chart may hold (Excel's own limit per chart is 255). The reader and the drawing
/// both stop here.
pub const MAX_SERIES: usize = 255;
/// Most points of one series that are kept or drawn.
pub const MAX_POINTS: usize = 20_000;
/// Most categories of a chart that are kept or drawn.
pub const MAX_CATEGORIES: usize = 20_000;
/// Longest text (title, label, series name, category) kept, in characters; the rest is cut.
pub const MAX_LABEL_CHARS: usize = 256;
/// Most primitives (bars, markers, label boxes, ...) one chart draws; a polyline or an area counts
/// as one however many points it has (its points are bounded by [`MAX_POINTS`]).
pub const MAX_DRAWN: usize = 20_000;
/// Values are clamped to this magnitude before any arithmetic, so that `1e308` neither overflows
/// the axis scaling nor turns a bar into an infinite rectangle.
pub const MAX_VALUE: f64 = 1e15;
/// Most tick marks (and gridlines) per axis.
pub const MAX_TICKS: usize = 200;
/// Most legend entries drawn.
pub const MAX_LEGEND_ENTRIES: usize = 255;

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

/// A whole chart.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartModel {
    pub title: Option<ChartText>,
    pub legend: Option<Legend>,
    /// `plotArea/layout`: where the plot goes (fractions of the chart box), when set by hand.
    pub plot_layout: Option<ManualLayout>,
    /// The chart groups of the plot area, in drawing order (`c:barChart`, `c:lineChart`, ...).
    pub groups: Vec<ChartGroup>,
    pub axes: Vec<Axis>,
    pub disp_blanks_as: DispBlanks,
    /// The chart area (`None` = not painted).
    pub chart_fill: Option<Fill>,
    pub chart_line: Option<Stroke>,
    /// The plot area.
    pub plot_fill: Option<Fill>,
    pub plot_line: Option<Stroke>,
    /// The text style every element inherits (`chartSpace/txPr`).
    pub text: TextStyle,
    /// The series colours: theme `accent1` .. `accent6` (empty = Office's default accents).
    pub palette: Vec<Rgba>,
    /// `c:style` (1..=48; 0 = not given, the colourful style 2): decides how series get their
    /// automatic colours (see the module documentation of `layout`).
    pub style: u8,
    /// The chart was a 3-D one (see [`three_d`]).
    pub three_d: bool,
    /// `c:view3D`: how a 3-D chart is turned (`None` = the defaults of [`View3D`]).
    pub view3d: Option<View3D>,
    /// The walls and the floor of a 3-D chart (`c:floor`, `c:sideWall`, `c:backWall`).
    pub floor: Option<Wall>,
    pub side_wall: Option<Wall>,
    pub back_wall: Option<Wall>,
    /// The default font (the theme's minor Latin font).
    pub default_font: Option<String>,
    /// Display language of dates / month names in number formats.
    pub locale_ja: bool,
}

/// `c:view3D`: the view of a 3-D chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View3D {
    /// Elevation, degrees (`rotX`): how far the chart is tilted toward the viewer.
    pub rot_x: f64,
    /// Rotation about the vertical axis, degrees (`rotY`).
    pub rot_y: f64,
    /// `rAngAx`: the axes stay at right angles (an oblique projection).
    pub r_ang_ax: bool,
    /// `perspective`, degrees x 2 (30 = 15 degrees); not used by the drawing.
    pub perspective: f64,
    /// `depthPercent`: depth as a percentage of the base (20..2000, default 100).
    pub depth_percent: f64,
    /// `hPercent`: height as a percentage of the base (`None` = automatic); not used.
    pub h_percent: Option<f64>,
}

impl Default for View3D {
    fn default() -> Self {
        View3D {
            rot_x: 15.0,
            rot_y: 20.0,
            r_ang_ax: false,
            perspective: 30.0,
            depth_percent: 100.0,
            h_percent: None,
        }
    }
}

/// A wall or the floor of a 3-D chart.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Wall {
    pub fill: Option<Fill>,
    pub line: Option<Stroke>,
}

/// The text properties a chart element can set; `None` inherits.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextStyle {
    pub size_pt: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub color: Option<Rgba>,
    pub font: Option<String>,
    /// `bodyPr/@rot` in degrees, clockwise (axis labels and titles).
    pub rot_deg: Option<f64>,
}

impl TextStyle {
    /// This style, with whatever it leaves unset taken from `base`.
    pub fn over(&self, base: &TextStyle) -> TextStyle {
        TextStyle {
            size_pt: self.size_pt.or(base.size_pt),
            bold: self.bold.or(base.bold),
            italic: self.italic.or(base.italic),
            color: self.color.or(base.color),
            font: self.font.clone().or_else(|| base.font.clone()),
            rot_deg: self.rot_deg.or(base.rot_deg),
        }
    }
}

/// A title: its text (paragraphs separated by `\n`) and style.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartText {
    pub text: String,
    pub style: TextStyle,
    pub layout: Option<ManualLayout>,
    /// Drawn over the plot instead of making room for itself.
    pub overlay: bool,
}

/// A manual layout: fractions of the chart box. `x`/`y` are either the position (edge mode) or an
/// offset from the default position (factor mode); `w`/`h` likewise.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ManualLayout {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub w: Option<f64>,
    pub h: Option<f64>,
    pub x_edge: bool,
    pub y_edge: bool,
    pub w_edge: bool,
    pub h_edge: bool,
    /// `layoutTarget inner`: the rectangle is the plot without its tick labels.
    pub inner: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegendPos {
    #[default]
    Right,
    Left,
    Top,
    Bottom,
    TopRight,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Legend {
    pub pos: LegendPos,
    pub overlay: bool,
    pub layout: Option<ManualLayout>,
    pub style: TextStyle,
    pub fill: Option<Fill>,
    pub line: Option<Stroke>,
    /// Entries hidden (`c:legendEntry/c:delete`): indices of the entries in legend order.
    pub deleted: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispBlanks {
    #[default]
    Gap,
    Zero,
    Span,
}

/// An outline; every field left unset is the automatic one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stroke {
    /// `a:noFill`: no line at all.
    pub none: bool,
    pub color: Option<Rgba>,
    /// EMU.
    pub width: Option<f64>,
    pub dash: Dash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupKind {
    #[default]
    Bar,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
    Radar,
    Bubble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarDir {
    /// Vertical bars.
    #[default]
    Col,
    /// Horizontal bars.
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// Lines and areas: every series on its own.
    #[default]
    Standard,
    Clustered,
    Stacked,
    PercentStacked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScatterStyle {
    None,
    Line,
    #[default]
    LineMarker,
    Marker,
    Smooth,
    SmoothMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadarStyle {
    #[default]
    Standard,
    Marker,
    Filled,
}

/// One chart group (`c:barChart`, `c:lineChart`, ...): a type, its options and its series.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartGroup {
    pub kind: GroupKind,
    pub bar_dir: BarDir,
    pub grouping: Grouping,
    pub vary_colors: bool,
    /// Percent of a bar's width between clusters (default 150).
    pub gap_width: f64,
    /// Percent of a bar's width by which the bars of a cluster overlap (-100..100; default 0).
    pub overlap: f64,
    /// Doughnut hole, percent of the radius (default 50).
    pub hole_size: f64,
    /// Pie / doughnut: degrees clockwise from 12 o'clock where the first slice starts.
    pub first_slice_ang: f64,
    /// Pie / doughnut: the slices run counter-clockwise from the start (LibreOffice's order;
    /// Office's is clockwise).
    pub counter_clockwise: bool,
    pub scatter_style: ScatterStyle,
    pub radar_style: RadarStyle,
    /// Bubble: scale of the biggest bubble, percent (default 100).
    pub bubble_scale: f64,
    /// Bubble: the size is the width, not the area.
    pub size_is_width: bool,
    /// Line charts: the group shows markers by default (`c:marker val`).
    pub markers: bool,
    /// Axis ids this group is plotted against (category / x first, value / y second).
    pub axis_ids: Vec<i64>,
    pub series: Vec<Series>,
    /// Group-level data labels, which a series without its own inherits.
    pub labels: Option<DataLabels>,
    pub three_d: bool,
    /// 3-D bars, lines and areas: the gap between rows in depth, percent of a row's depth
    /// (`c:gapDepth`, default 150).
    pub gap_depth: f64,
}

impl Default for ChartGroup {
    fn default() -> Self {
        ChartGroup {
            kind: GroupKind::Bar,
            bar_dir: BarDir::Col,
            grouping: Grouping::Standard,
            vary_colors: false,
            gap_width: 150.0,
            overlap: 0.0,
            hole_size: 50.0,
            first_slice_ang: 0.0,
            counter_clockwise: false,
            scatter_style: ScatterStyle::LineMarker,
            radar_style: RadarStyle::Standard,
            bubble_scale: 100.0,
            size_is_width: false,
            markers: true,
            axis_ids: Vec::new(),
            series: Vec::new(),
            labels: None,
            three_d: false,
            gap_depth: 150.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkerSymbol {
    #[default]
    Auto,
    None,
    Circle,
    Square,
    Diamond,
    Triangle,
    /// ODF `arrow-down` / `arrow-left` / `arrow-right` (`Triangle` points up).
    TriangleDown,
    TriangleLeft,
    TriangleRight,
    /// ODF `bowtie` (two triangles meeting at their tips, side by side) and `sandglass` (the same,
    /// one above the other).
    Bowtie,
    Sandglass,
    /// ODF `vertical-bar` (`Dash` is the horizontal one).
    VBar,
    X,
    Star,
    Dot,
    Dash,
    Plus,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Marker {
    pub symbol: MarkerSymbol,
    /// Points (2..72); `None` = 5.
    pub size_pt: Option<f64>,
    pub fill: Option<Fill>,
    pub line: Stroke,
}

/// Where a data label goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPos {
    Center,
    InsideEnd,
    InsideBase,
    OutsideEnd,
    BestFit,
    Left,
    Right,
    Above,
    Below,
}

/// Data label settings of a group, a series or a point.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DataLabels {
    /// `c:delete`: no labels.
    pub delete: bool,
    pub show_value: bool,
    pub show_category: bool,
    pub show_series: bool,
    pub show_percent: bool,
    pub show_legend_key: bool,
    pub pos: Option<LabelPos>,
    /// A format code that is not linked to the source.
    pub num_fmt: Option<String>,
    pub separator: Option<String>,
    pub style: TextStyle,
    /// Per-point overrides: `(point index, labels)`; a point with a `delete` is unlabelled.
    pub points: Vec<(usize, DataLabels)>,
    /// Custom text of this label (a point's `c:tx/c:rich`).
    pub text: Option<String>,
}

/// Formatting of one data point.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PointFmt {
    pub idx: usize,
    pub fill: Option<Fill>,
    pub line: Option<Stroke>,
    pub marker: Option<Marker>,
    pub explosion: Option<f64>,
}

/// One series. Every list is indexed by point; a missing value is `None`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Series {
    pub name: Option<String>,
    /// Category labels (first level of multi-level categories).
    pub cats: Vec<String>,
    /// The categories as numbers (dates, numeric categories), where they are numbers.
    pub cat_nums: Vec<Option<f64>>,
    /// The format code of the numeric categories.
    pub cat_format: Option<String>,
    pub values: Vec<Option<f64>>,
    /// The format code of the values (`c:numCache/c:formatCode`).
    pub format_code: Option<String>,
    /// Scatter and bubble: the x values.
    pub x_values: Vec<Option<f64>>,
    /// Bubble: the sizes.
    pub sizes: Vec<Option<f64>>,
    pub fill: Option<Fill>,
    pub line: Option<Stroke>,
    pub marker: Option<Marker>,
    pub smooth: Option<bool>,
    /// Pie: explosion of the whole series, percent.
    pub explosion: Option<f64>,
    pub points: Vec<PointFmt>,
    pub labels: Option<DataLabels>,
    /// The series was cut at a budget while it was read.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisKind {
    #[default]
    Cat,
    Val,
    Date,
    Ser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisPos {
    #[default]
    Bottom,
    Left,
    Top,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Crosses {
    #[default]
    AutoZero,
    Min,
    Max,
    At(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TickMark {
    None,
    In,
    #[default]
    Out,
    Cross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TickLabelPos {
    #[default]
    NextTo,
    Low,
    High,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    /// The axis id (Mac PowerPoint writes negative ones).
    pub id: i64,
    pub kind: AxisKind,
    /// The axis this one crosses.
    pub cross_ax: i64,
    pub pos: AxisPos,
    pub deleted: bool,
    /// `orientation maxMin`: runs backwards.
    pub reversed: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Log base (2..1000).
    pub log_base: Option<f64>,
    pub major_unit: Option<f64>,
    pub minor_unit: Option<f64>,
    /// Where this axis crosses its `cross_ax`.
    pub crosses: Crosses,
    pub major_tick: TickMark,
    pub minor_tick: TickMark,
    pub tick_label_pos: TickLabelPos,
    /// `(format code, source linked)`.
    pub num_fmt: Option<(String, bool)>,
    pub major_grid: Option<Stroke>,
    pub minor_grid: Option<Stroke>,
    pub title: Option<ChartText>,
    pub line: Option<Stroke>,
    pub text: TextStyle,
    /// Value axes: the cross axis' categories sit between the ticks (`crossBetween between`).
    pub between: bool,
    /// Category axes: label every n-th category (0 = automatic).
    pub label_skip: usize,
    /// Category axes: a tick mark at every n-th category (0 = automatic).
    pub tick_skip: usize,
}

impl Default for Axis {
    fn default() -> Self {
        Axis {
            id: 0,
            kind: AxisKind::Cat,
            cross_ax: 0,
            pos: AxisPos::Bottom,
            deleted: false,
            reversed: false,
            min: None,
            max: None,
            log_base: None,
            major_unit: None,
            minor_unit: None,
            crosses: Crosses::AutoZero,
            major_tick: TickMark::Out,
            minor_tick: TickMark::None,
            tick_label_pos: TickLabelPos::NextTo,
            num_fmt: None,
            major_grid: None,
            minor_grid: None,
            title: None,
            line: None,
            text: TextStyle::default(),
            between: true,
            label_skip: 0,
            tick_skip: 0,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

/// Draws a chart into a box of `w` x `h` EMU (origin top-left): the shapes and text boxes, in paint
/// order, and whether anything was left out because of a budget (see the module documentation).
/// Never panics, whatever the model holds.
pub fn draw_chart(model: &ChartModel, w: f64, h: f64) -> (Vec<Item>, bool) {
    let (w, h) = (clamp_len(w), clamp_len(h));
    let mut cx = shapes::Out::new(w, h);
    if cx.w < 8.0 || cx.h < 8.0 {
        return (Vec::new(), false);
    }
    let model = sanitize(model);
    layout::draw(&mut cx, &model);
    cx.finish()
}

fn clamp_len(emu: f64) -> f64 {
    if emu.is_finite() {
        (emu / EMU_PER_PX).clamp(0.0, 20_000.0)
    } else {
        0.0
    }
}

/// A finite value within [`MAX_VALUE`], or `None`.
pub(crate) fn clean(v: f64) -> Option<f64> {
    v.is_finite().then(|| v.clamp(-MAX_VALUE, MAX_VALUE))
}

/// The model with every number made finite and every list within its budget. The drawing code
/// then never meets a NaN, an infinity or an oversized list.
fn sanitize(m: &ChartModel) -> ChartModel {
    let mut m = m.clone();
    m.groups.truncate(32);
    m.axes.truncate(64);
    let mut total_series = 0usize;
    for g in &mut m.groups {
        let room = MAX_SERIES.saturating_sub(total_series);
        g.series.truncate(room);
        total_series += g.series.len();
        g.axis_ids.truncate(4);
        for v in [
            &mut g.gap_width,
            &mut g.overlap,
            &mut g.hole_size,
            &mut g.first_slice_ang,
            &mut g.bubble_scale,
        ] {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        g.gap_width = g.gap_width.clamp(0.0, 500.0);
        g.overlap = g.overlap.clamp(-100.0, 100.0);
        g.hole_size = g.hole_size.clamp(10.0, 90.0);
        g.bubble_scale = g.bubble_scale.clamp(0.0, 300.0);
        for s in &mut g.series {
            s.cats.truncate(MAX_CATEGORIES);
            s.cat_nums.truncate(MAX_CATEGORIES);
            for list in [&mut s.values, &mut s.x_values, &mut s.sizes] {
                list.truncate(MAX_POINTS);
                for v in list.iter_mut() {
                    *v = v.and_then(clean);
                }
            }
            for c in s.cat_nums.iter_mut() {
                *c = c.and_then(clean);
            }
            s.points.truncate(MAX_POINTS);
            if let Some(n) = &mut s.name {
                cut_chars(n, MAX_LABEL_CHARS);
            }
            for c in &mut s.cats {
                cut_chars(c, MAX_LABEL_CHARS);
            }
        }
    }
    for a in &mut m.axes {
        for v in [&mut a.min, &mut a.max, &mut a.major_unit, &mut a.minor_unit] {
            *v = v.and_then(clean);
        }
        if let Crosses::At(v) = &mut a.crosses {
            *v = clean(*v).unwrap_or(0.0);
        }
        if let Some(t) = &mut a.title {
            cut_chars(&mut t.text, MAX_LABEL_CHARS);
        }
    }
    if let Some(t) = &mut m.title {
        cut_chars(&mut t.text, MAX_LABEL_CHARS);
    }
    m
}

/// Cuts a string to at most `max` characters.
pub(crate) fn cut_chars(s: &mut String, max: usize) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
    }
}
