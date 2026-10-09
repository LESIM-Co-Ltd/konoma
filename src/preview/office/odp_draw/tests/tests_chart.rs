//! Tests of the OpenDocument chart reading (`chart_read.rs`) and the frame hook (`chart.rs`):
//! charts written the way LibreOffice writes them (`Object N/content.xml`).

use super::chart_read::{parse, read_tree, LO_PALETTE};
use super::*;
use crate::preview::office::slide_draw::chart as ch;
use crate::preview::office::tests_odp::pns;

/// A chart part: `chart_class`, the plot area's style attributes (`chart:*`), the series, the axes
/// and the rows of the local table (a cell that parses as a number is a float).
struct Cp {
    class: &'static str,
    plot: String,
    axes: String,
    series: String,
    styles: String,
    rows: Vec<Vec<&'static str>>,
    legend: bool,
    title: Option<&'static str>,
}

impl Cp {
    fn new(class: &'static str) -> Cp {
        Cp {
            class,
            plot: String::new(),
            axes: String::new(),
            series: String::new(),
            styles: String::new(),
            rows: vec![
                vec!["", "2022", "2023"],
                vec!["North", "12", "15"],
                vec!["South", "9", "11"],
                vec!["East", "17", "14"],
            ],
            legend: true,
            title: Some("Sales"),
        }
    }

    fn xml(&self) -> String {
        let mut table = String::new();
        for r in &self.rows {
            table.push_str("<table:table-row>");
            for c in r {
                if c.parse::<f64>().is_ok() && !(c.len() == 4 && c.starts_with("20")) {
                    table.push_str(&format!(
                        r#"<table:table-cell office:value-type="float" office:value="{c}"><text:p>{c}</text:p></table:table-cell>"#
                    ));
                } else {
                    table.push_str(&format!(
                        r#"<table:table-cell office:value-type="string"><text:p>{c}</text:p></table:table-cell>"#
                    ));
                }
            }
            table.push_str("</table:table-row>");
        }
        let legend = if self.legend {
            r#"<chart:legend chart:legend-position="end" chart:style-name="chL"/>"#
        } else {
            ""
        };
        let title = self.title.map_or(String::new(), |t| {
            format!(r#"<chart:title chart:style-name="chT"><text:p>{t}</text:p></chart:title>"#)
        });
        format!(
            r##"<?xml version="1.0" encoding="UTF-8"?><office:document-content {} office:version="1.3"><office:automatic-styles>
<style:style style:name="ch1" style:family="chart"><style:graphic-properties draw:stroke="none"/></style:style>
<style:style style:name="chT" style:family="chart"><style:chart-properties chart:auto-position="true" style:rotation-angle="0"/><style:text-properties fo:font-size="13pt"/></style:style>
<style:style style:name="chL" style:family="chart"><style:graphic-properties draw:fill="solid" draw:fill-color="#d9d9d9"/><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="chP" style:family="chart"><style:chart-properties {}/></style:style>
<style:style style:name="chW" style:family="chart"><style:graphic-properties draw:fill="solid" draw:fill-color="#d9d9d9"/></style:style>
<number:number-style style:name="N0"><number:number number:min-integer-digits="1"/></number:number-style>
<number:number-style style:name="N2"><number:number number:decimal-places="2" number:min-integer-digits="1" number:grouping="true"/></number:number-style>
<number:percentage-style style:name="NP"><number:number number:decimal-places="0"/><number:text>%</number:text></number:percentage-style>
<number:date-style style:name="ND"><number:year number:style="long"/><number:text>/</number:text><number:month number:style="long"/><number:text>/</number:text><number:day number:style="long"/></number:date-style>
{}</office:automatic-styles><office:body><office:chart><chart:chart svg:width="12cm" svg:height="7cm" chart:class="chart:{}" chart:style-name="ch1">{title}{legend}<chart:plot-area chart:style-name="chP">{}{}<chart:wall chart:style-name="chW"/></chart:plot-area><table:table table:name="local-table">{table}</table:table></chart:chart></office:chart></office:body></office:document-content>"##,
            pns(),
            self.plot,
            self.styles,
            self.class,
            self.axes,
            self.series,
        )
    }
}

fn series(style: &str, values: &str, label: &str, class: Option<&str>) -> String {
    let class = class.map_or(String::new(), |c| format!(r#" chart:class="chart:{c}""#));
    format!(
        r#"<chart:series chart:style-name="{style}" chart:values-cell-range-address="{values}" chart:label-cell-address="{label}"{class}><chart:data-point chart:repeated="3"/></chart:series>"#
    )
}

const XAXIS: &str = r#"<chart:axis chart:dimension="x" chart:name="primary-x" chart:style-name="chAx"><chart:categories table:cell-range-address="local-table.$A$2:.$A$4"/></chart:axis><chart:axis chart:dimension="y" chart:name="primary-y" chart:style-name="chAy"><chart:grid chart:style-name="chG" chart:class="major"/></chart:axis>"#;

const AXIS_STYLES: &str = r##"<style:style style:name="chAx" style:family="chart" style:data-style-name="N0"><style:chart-properties chart:display-label="true" chart:link-data-style-to-source="true"/><style:graphic-properties svg:stroke-color="#b3b3b3"/><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="chAy" style:family="chart" style:data-style-name="N2"><style:chart-properties chart:display-label="true" chart:minimum="0" chart:maximum="30" chart:interval-major="10" chart:interval-minor-divisor="2" chart:logarithmic="false" chart:gap-width="50" chart:overlap="-10"/><style:graphic-properties svg:stroke-color="#b3b3b3"/></style:style>
<style:style style:name="chG" style:family="chart"><style:graphic-properties svg:stroke-color="#b3b3b3"/></style:style>
<style:style style:name="s1" style:family="chart"><style:graphic-properties draw:stroke="none" draw:fill-color="#004586"/></style:style>
<style:style style:name="s2" style:family="chart"><style:graphic-properties draw:stroke="none" draw:fill-color="#ff420e"/></style:style>"##;

fn full(class: &'static str, plot: &str) -> Cp {
    let mut c = Cp::new(class);
    c.plot = plot.to_string();
    c.axes = XAXIS.to_string();
    c.styles = AXIS_STYLES.to_string();
    c.series = series("s1", "local-table.$B$2:.$B$4", "local-table.$B$1", None)
        + &series("s2", "local-table.$C$2:.$C$4", "local-table.$C$1", None);
    c
}

fn model_of(c: &Cp) -> ch::ChartModel {
    let root = read_tree(c.xml().as_bytes()).expect("tree");
    parse(&root, None).expect("a chart").model
}

fn nums(v: &[Option<f64>]) -> Vec<f64> {
    v.iter().map(|x| x.unwrap_or(f64::NAN)).collect()
}

// ---- classes -------------------------------------------------------------------------------------

#[test]
fn a_clustered_column_chart_with_its_data_axes_legend_and_title() {
    let m = model_of(&full("bar", ""));
    assert_eq!(m.groups.len(), 1);
    let g = &m.groups[0];
    assert_eq!(g.kind, ch::GroupKind::Bar);
    assert_eq!(g.bar_dir, ch::BarDir::Col);
    assert_eq!(g.grouping, ch::Grouping::Clustered);
    assert_eq!(g.series.len(), 2);
    assert_eq!(nums(&g.series[0].values), vec![12.0, 9.0, 17.0]);
    assert_eq!(nums(&g.series[1].values), vec![15.0, 11.0, 14.0]);
    assert_eq!(g.series[0].name.as_deref(), Some("2022"));
    assert_eq!(g.series[0].cats, vec!["North", "South", "East"]);
    // the colours of the series style
    assert_eq!(
        g.series[0].fill,
        Some(Fill::Solid(Rgba::rgb(0, 0x45, 0x86)))
    );
    assert_eq!(
        g.series[1].fill,
        Some(Fill::Solid(Rgba::rgb(0xff, 0x42, 0x0e)))
    );
    // the gap and the overlap of the value axis' style
    assert_eq!((g.gap_width, g.overlap), (50.0, -10.0));
    // the title, 13 pt and not bold
    let t = m.title.as_ref().unwrap();
    assert_eq!(t.text, "Sales");
    assert_eq!(t.style.size_pt, Some(13.0));
    assert_eq!(t.style.bold, Some(false));
    // the legend on the right, with its fill and a grey outline of its own
    let l = m.legend.as_ref().unwrap();
    assert_eq!(l.pos, ch::LegendPos::Right);
    assert_eq!(l.fill, Some(Fill::Solid(Rgba::rgb(0xd9, 0xd9, 0xd9))));
    assert!(l.line.is_some());
    // the wall is the plot's fill
    assert_eq!(m.plot_fill, Some(Fill::Solid(Rgba::rgb(0xd9, 0xd9, 0xd9))));
    // the axes: a category axis at the bottom, a value axis with scale and a major grid
    assert_eq!(m.axes.len(), 2);
    let (x, y) = (&m.axes[0], &m.axes[1]);
    assert_eq!((x.kind, x.pos), (ch::AxisKind::Cat, ch::AxisPos::Bottom));
    assert_eq!((y.kind, y.pos), (ch::AxisKind::Val, ch::AxisPos::Left));
    assert_eq!(
        (y.min, y.max, y.major_unit, y.minor_unit),
        (Some(0.0), Some(30.0), Some(10.0), Some(5.0))
    );
    assert!(y.major_grid.is_some() && y.minor_grid.is_none());
    assert_eq!(y.num_fmt.as_ref().map(|f| f.0.as_str()), Some("#,##0.00"));
    assert_eq!(g.axis_ids, vec![x.id, y.id]);
}

#[test]
fn stacked_percent_and_horizontal_bars() {
    let m = model_of(&full("bar", r#"chart:stacked="true""#));
    assert_eq!(m.groups[0].grouping, ch::Grouping::Stacked);
    let m = model_of(&full(
        "bar",
        r#"chart:stacked="true" chart:percentage="true""#,
    ));
    assert_eq!(m.groups[0].grouping, ch::Grouping::PercentStacked);
    // a percent chart's value axis is labelled in percent
    assert_eq!(
        m.axes[1].num_fmt.as_ref().map(|f| f.0.as_str()),
        Some("#,##0.00")
    );
    let m = model_of(&full("bar", r#"chart:vertical="true""#));
    assert_eq!(m.groups[0].bar_dir, ch::BarDir::Bar);
    assert_eq!(
        (m.axes[0].pos, m.axes[1].pos),
        (ch::AxisPos::Left, ch::AxisPos::Bottom)
    );
}

#[test]
fn a_percent_chart_without_a_number_format_is_labelled_in_percent() {
    let mut c = full("bar", r#"chart:stacked="true" chart:percentage="true""#);
    c.styles = c.styles.replace(r#" style:data-style-name="N2""#, "");
    let m = model_of(&c);
    assert_eq!(m.axes[1].num_fmt.as_ref().map(|f| f.0.as_str()), Some("0%"));
}

#[test]
fn line_charts_symbols_and_interpolation() {
    let m = model_of(&full("line", r#"chart:symbol-type="automatic""#));
    let g = &m.groups[0];
    assert_eq!(g.kind, ch::GroupKind::Line);
    assert_eq!(g.grouping, ch::Grouping::Standard);
    assert!(g.markers);
    assert_ne!(
        g.series[0].marker.as_ref().unwrap().symbol,
        ch::MarkerSymbol::None
    );
    assert_eq!(g.series[0].smooth, Some(false));
    // the line is in the series' stroke colour (here: the palette's)
    assert_eq!(
        g.series[0].line.as_ref().unwrap().color,
        Some(LO_PALETTE[0])
    );
    let m = model_of(&full(
        "line",
        r#"chart:symbol-type="none" chart:interpolation="cubic-spline""#,
    ));
    let g = &m.groups[0];
    assert!(!g.markers);
    assert_eq!(
        g.series[0].marker.as_ref().unwrap().symbol,
        ch::MarkerSymbol::None
    );
    assert_eq!(g.series[0].smooth, Some(true));
    // a named symbol of a series style
    let mut c = full("line", "");
    c.styles += r##"<style:style style:name="s3" style:family="chart"><style:chart-properties chart:symbol-type="named-symbol" chart:symbol-name="square" chart:symbol-width="0.5cm"/><style:graphic-properties svg:stroke-width="0.08cm" svg:stroke-color="#ff0000" draw:fill-color="#00ff00"/></style:style>"##;
    c.series = series("s3", "local-table.$B$2:.$B$4", "local-table.$B$1", None);
    let s = &model_of(&c).groups[0].series[0];
    let mk = s.marker.as_ref().unwrap();
    assert_eq!(mk.symbol, ch::MarkerSymbol::Square);
    assert!((mk.size_pt.unwrap() - 0.5 * 360_000.0 / 12_700.0).abs() < 1e-6);
    assert_eq!(mk.fill, Some(Fill::Solid(Rgba::rgb(0, 255, 0))));
    let l = s.line.as_ref().unwrap();
    assert_eq!(l.color, Some(Rgba::rgb(255, 0, 0)));
    assert!((l.width.unwrap() - 0.08 * 360_000.0).abs() < 1.0);
    // `chart:lines="false"`: markers only
    let m = model_of(&full(
        "line",
        r#"chart:symbol-type="automatic" chart:lines="false""#,
    ));
    assert!(m.groups[0].series[0].line.as_ref().unwrap().none);
}

#[test]
fn area_charts_overlapping_are_painted_back_to_front() {
    let m = model_of(&full("area", ""));
    let g = &m.groups[0];
    assert_eq!(g.kind, ch::GroupKind::Area);
    // (reversed: the first series is in front in LibreOffice)
    assert_eq!(g.series[0].name.as_deref(), Some("2023"));
    assert_eq!(g.series[1].name.as_deref(), Some("2022"));
    // the category axis of an area chart has its points on the ticks
    assert!(!m.axes[0].between && !m.axes[1].between);
    // stacked ones keep their order
    let m = model_of(&full("area", r#"chart:stacked="true""#));
    assert_eq!(m.groups[0].series[0].name.as_deref(), Some("2022"));
    assert_eq!(m.groups[0].grouping, ch::Grouping::Stacked);
}

#[test]
fn pie_and_ring_charts_have_a_colour_per_slice_and_run_counter_clockwise() {
    let mut c = Cp::new("circle");
    c.styles = AXIS_STYLES.to_string();
    c.plot = r#"chart:data-label-number="value""#.to_string();
    c.series = r#"<chart:series chart:style-name="s1" chart:values-cell-range-address="local-table.$B$2:.$B$4" chart:label-cell-address="local-table.$B$1" chart:class="chart:circle"><chart:data-point chart:style-name="s2"/><chart:data-point/></chart:series>"#.to_string();
    c.axes = XAXIS.to_string();
    let m = model_of(&c);
    assert!(m.axes.is_empty());
    let g = &m.groups[0];
    assert_eq!(g.kind, ch::GroupKind::Pie);
    assert!(g.vary_colors && g.counter_clockwise);
    assert_eq!(g.first_slice_ang, 0.0);
    let s = &g.series[0];
    let fill = |i: usize| {
        s.points
            .iter()
            .find(|p| p.idx == i)
            .and_then(|p| p.fill.clone())
    };
    // the first slice has its style's colour, the others the palette's by position
    assert_eq!(fill(0), Some(Fill::Solid(Rgba::rgb(0xff, 0x42, 0x0e))));
    assert_eq!(fill(1), Some(Fill::Solid(LO_PALETTE[1])));
    assert_eq!(fill(2), Some(Fill::Solid(LO_PALETTE[2])));
    assert!(s.labels.as_ref().unwrap().show_value);
    // a ring: the chart's class decides (its series say `circle`)
    let mut r = c;
    r.class = "ring";
    let m = model_of(&r);
    assert_eq!(m.groups[0].kind, ch::GroupKind::Doughnut);
    // the start angle: 90 degrees (LibreOffice's default) is twelve o'clock
    let mut a = Cp::new("circle");
    a.plot = r#"chart:angle-offset="180""#.to_string();
    a.series = series("s1", "local-table.$B$2:.$B$4", "local-table.$B$1", None);
    assert_eq!(model_of(&a).groups[0].first_slice_ang, 270.0);
}

#[test]
fn scatter_charts_take_x_values_from_the_domain_and_draw_lines_by_default() {
    let mut c = Cp::new("scatter");
    c.rows = vec![
        vec!["x", "y1"],
        vec!["1", "2"],
        vec!["2", "4"],
        vec!["3", "6"],
    ];
    c.styles = AXIS_STYLES.to_string();
    c.plot = r#"chart:symbol-type="automatic""#.to_string();
    c.series = r#"<chart:series chart:style-name="s1" chart:values-cell-range-address="local-table.$B$2:.$B$4" chart:label-cell-address="local-table.$B$1"><chart:domain table:cell-range-address="local-table.$A$2:.$A$4"/></chart:series>"#.to_string();
    c.axes = XAXIS.to_string();
    let m = model_of(&c);
    let g = &m.groups[0];
    assert_eq!(g.kind, ch::GroupKind::Scatter);
    assert_eq!(nums(&g.series[0].x_values), vec![1.0, 2.0, 3.0]);
    assert_eq!(nums(&g.series[0].values), vec![2.0, 4.0, 6.0]);
    assert_eq!(g.scatter_style, ch::ScatterStyle::LineMarker);
    // both axes are value axes
    assert_eq!(m.axes[0].kind, ch::AxisKind::Val);
    // `chart:lines="false"`: points only
    c.plot += r#" chart:lines="false""#;
    assert_eq!(
        model_of(&c).groups[0].scatter_style,
        ch::ScatterStyle::Marker
    );
}

#[test]
fn radar_filled_radar_and_bubble() {
    let m = model_of(&full("radar", r#"chart:symbol-type="none""#));
    assert_eq!(m.groups[0].kind, ch::GroupKind::Radar);
    assert_eq!(m.groups[0].radar_style, ch::RadarStyle::Standard);
    let m = model_of(&full("filled-radar", ""));
    assert_eq!(m.groups[0].radar_style, ch::RadarStyle::Filled);
    assert!(m.groups[0].series[0].fill.is_some());
    let mut c = Cp::new("bubble");
    c.rows = vec![
        vec!["x", "y", "size"],
        vec!["1", "2", "10"],
        vec!["2", "4", "20"],
    ];
    c.styles = AXIS_STYLES.to_string();
    c.axes = XAXIS.to_string();
    c.series = r#"<chart:series chart:style-name="s1" chart:values-cell-range-address="local-table.$C$2:.$C$3" chart:label-cell-address="local-table.$C$1"><chart:domain table:cell-range-address="local-table.$A$2:.$A$3"/><chart:domain table:cell-range-address="local-table.$B$2:.$B$3"/></chart:series>"#.to_string();
    let g = &model_of(&c).groups[0];
    assert_eq!(g.kind, ch::GroupKind::Bubble);
    assert_eq!(nums(&g.series[0].sizes), vec![10.0, 20.0]);
    assert_eq!(nums(&g.series[0].x_values), vec![1.0, 2.0]);
    assert_eq!(nums(&g.series[0].values), vec![2.0, 4.0]);
}

#[test]
fn a_column_and_line_chart_has_a_group_per_class_and_a_secondary_axis_per_attachment() {
    let mut c = full("bar", "");
    c.series = series("s1", "local-table.$B$2:.$B$4", "local-table.$B$1", None)
        + &series(
            "s2",
            "local-table.$C$2:.$C$4",
            "local-table.$C$1",
            Some("line"),
        );
    let m = model_of(&c);
    assert_eq!(m.groups.len(), 2);
    assert_eq!(m.groups[0].kind, ch::GroupKind::Bar);
    assert_eq!(m.groups[1].kind, ch::GroupKind::Line);
    assert_eq!(m.groups[0].axis_ids, m.groups[1].axis_ids);
    assert_eq!(m.axes.len(), 2);
    // attached to the secondary axis
    c.series = series("s1", "local-table.$B$2:.$B$4", "local-table.$B$1", None)
        + &series(
            "s2",
            "local-table.$C$2:.$C$4",
            "local-table.$C$1",
            Some("line"),
        )
        .replace(
            "<chart:series ",
            r#"<chart:series chart:attached-axis="secondary-y" "#,
        );
    let m = model_of(&c);
    assert_eq!(m.axes.len(), 3);
    assert_ne!(m.groups[0].axis_ids[1], m.groups[1].axis_ids[1]);
    assert_eq!(m.axes[2].pos, ch::AxisPos::Right);
    assert_eq!(m.axes[2].crosses, ch::Crosses::Max);
}

#[test]
fn unsupported_classes_are_not_drawn() {
    for class in ["stock", "gantt", "surface", "nonsense"] {
        let root = read_tree(full(class, "").xml().as_bytes()).unwrap();
        assert!(parse(&root, None).is_none(), "{class}");
    }
}

// ---- styles and data labels ---------------------------------------------------------------------

#[test]
fn data_labels_follow_the_styles_and_default_by_chart_type() {
    let value = |plot: &str| {
        let m = model_of(&full("bar", plot));
        m.groups[0].series[0].labels.clone()
    };
    assert!(value("").is_none());
    let l = value(r#"chart:data-label-number="value""#).unwrap();
    assert!(l.show_value && !l.show_percent && !l.show_category);
    assert_eq!(l.pos, Some(ch::LabelPos::OutsideEnd));
    // stacked bars are labelled in the middle
    let l = value(r#"chart:stacked="true" chart:data-label-number="value""#).unwrap();
    assert_eq!(l.pos, Some(ch::LabelPos::Center));
    let l = value(r#"chart:data-label-number="value-and-percentage" chart:data-label-text="true" chart:label-position="inside""#)
        .unwrap();
    assert!(l.show_value && l.show_percent && l.show_category);
    assert_eq!(l.pos, Some(ch::LabelPos::InsideEnd));
    assert!(value(r#"chart:data-label-number="none""#).is_none());
}

#[test]
fn colours_without_a_style_come_from_the_default_palette() {
    let mut c = full("bar", "");
    c.series = series("none", "local-table.$B$2:.$B$4", "local-table.$B$1", None)
        + &series("none", "local-table.$C$2:.$C$4", "local-table.$C$1", None);
    let g = &model_of(&c).groups[0];
    assert_eq!(g.series[0].fill, Some(Fill::Solid(LO_PALETTE[0])));
    assert_eq!(g.series[1].fill, Some(Fill::Solid(LO_PALETTE[1])));
    assert_eq!(LO_PALETTE[0], Rgba::rgb(0x00, 0x45, 0x86));
    assert_eq!(LO_PALETTE[11], Rgba::rgb(0x00, 0x84, 0xd1));
}

#[test]
fn axis_options_are_read() {
    let mut c = full("bar", "");
    c.styles += r##"<style:style style:name="chAz" style:family="chart" style:data-style-name="NP"><style:chart-properties chart:display-label="false" chart:logarithmic="true" chart:reverse-direction="true" chart:tick-marks-major-inner="true" chart:tick-marks-major-outer="false" chart:axis-position="end"/><style:graphic-properties draw:stroke="none"/><style:text-properties fo:font-size="8pt" fo:color="#ff0000" fo:font-weight="bold"/></style:style>
<style:style style:name="chA2" style:family="chart"><style:chart-properties chart:axis-position="3.5"/></style:style>"##;
    c.axes = r#"<chart:axis chart:dimension="x" chart:name="primary-x" chart:style-name="chAz"><chart:title chart:style-name="chT"><text:p>Cats</text:p></chart:title></chart:axis><chart:axis chart:dimension="y" chart:name="primary-y" chart:style-name="chA2"><chart:grid chart:class="minor" chart:style-name="chG"/></chart:axis>"#.to_string();
    let m = model_of(&c);
    let x = &m.axes[0];
    assert_eq!(x.tick_label_pos, ch::TickLabelPos::None);
    assert!(x.reversed);
    assert_eq!(x.major_tick, ch::TickMark::In);
    assert_eq!(x.crosses, ch::Crosses::Max);
    assert!(x.line.as_ref().unwrap().none);
    assert_eq!(x.text.size_pt, Some(8.0));
    assert_eq!(x.text.bold, Some(true));
    assert_eq!(x.text.color, Some(Rgba::rgb(255, 0, 0)));
    assert_eq!(x.title.as_ref().unwrap().text, "Cats");
    assert_eq!(x.num_fmt.as_ref().unwrap().0, "0%");
    let y = &m.axes[1];
    assert_eq!(y.crosses, ch::Crosses::At(3.5));
    assert!(y.minor_grid.is_some() && y.major_grid.is_none());
    let mut c = full("bar", "");
    c.styles += r##"<style:style style:name="chLog" style:family="chart"><style:chart-properties chart:logarithmic="true"/></style:style>"##;
    c.axes = r#"<chart:axis chart:dimension="y" chart:name="primary-y" chart:style-name="chLog"/>"#
        .to_string();
    let m = model_of(&c);
    assert_eq!(m.axes[1].log_base, Some(10.0));
    // no x axis element: that axis is not drawn
    assert!(m.axes[0].deleted);
}

#[test]
fn a_date_style_and_a_currency_style_become_format_codes() {
    let mut c = full("bar", "");
    c.styles += r##"<number:currency-style style:name="NC"><number:currency-symbol>EUR</number:currency-symbol><number:number number:decimal-places="2"/></number:currency-style>
<number:number-style style:name="NS"><number:scientific-number number:decimal-places="1"/></number:number-style>
<style:style style:name="chAd" style:family="chart" style:data-style-name="ND"/>
<style:style style:name="chAc" style:family="chart" style:data-style-name="NC"/>"##;
    c.axes = r#"<chart:axis chart:dimension="x" chart:name="primary-x" chart:style-name="chAd"/><chart:axis chart:dimension="y" chart:name="primary-y" chart:style-name="chAc"/>"#.to_string();
    let m = model_of(&c);
    assert_eq!(m.axes[0].num_fmt.as_ref().unwrap().0, "yyyy/mm/dd");
    assert_eq!(m.axes[1].num_fmt.as_ref().unwrap().0, "\"EUR\"0.00");
}

#[test]
fn chart_area_wall_title_and_legend_positions() {
    let mut c = full("bar", "");
    c.styles += r##"<style:style style:name="chBg" style:family="chart"><style:graphic-properties draw:fill="solid" draw:fill-color="#ffeedd" draw:stroke="solid" svg:stroke-color="#000000"/></style:style>"##;
    let xml = c
        .xml()
        .replace(r#"chart:style-name="ch1""#, r#"chart:style-name="chBg""#);
    let root = read_tree(xml.as_bytes()).unwrap();
    let m = parse(&root, None).unwrap().model;
    assert_eq!(m.chart_fill, Some(Fill::Solid(Rgba::rgb(0xff, 0xee, 0xdd))));
    assert!(!m.chart_line.as_ref().unwrap().none);
    for (pos, want) in [
        ("start", ch::LegendPos::Left),
        ("top", ch::LegendPos::Top),
        ("bottom", ch::LegendPos::Bottom),
        ("top-end", ch::LegendPos::TopRight),
        ("end", ch::LegendPos::Right),
    ] {
        let x = full("bar", "").xml().replace(
            r#"chart:legend-position="end""#,
            &format!(r#"chart:legend-position="{pos}""#),
        );
        let root = read_tree(x.as_bytes()).unwrap();
        assert_eq!(
            parse(&root, None).unwrap().model.legend.unwrap().pos,
            want,
            "{pos}"
        );
    }
    // no legend element, no legend; a subtitle is a second line of the title
    let mut c = full("bar", "");
    c.legend = false;
    assert!(model_of(&c).legend.is_none());
    let x = c.xml().replace("</chart:title>", "</chart:title><chart:subtitle chart:style-name=\"chT\"><text:p>Sub</text:p></chart:subtitle>");
    let root = read_tree(x.as_bytes()).unwrap();
    assert_eq!(
        parse(&root, None).unwrap().model.title.unwrap().text,
        "Sales\nSub"
    );
    // a title written in a span takes the span's size
    let x = c
        .xml()
        .replace("<text:p>Sales</text:p>", r#"<text:p><text:span text:style-name="T1">Sales</text:span></text:p>"#)
        .replace("</office:automatic-styles>", r#"<style:style style:name="T1" style:family="text"><style:text-properties fo:font-size="9pt" fo:font-weight="bold"/></style:style></office:automatic-styles>"#);
    let root = read_tree(x.as_bytes()).unwrap();
    let t = parse(&root, None).unwrap().model.title.unwrap();
    assert_eq!((t.style.size_pt, t.style.bold), (Some(9.0), Some(true)));
}

#[test]
fn an_axis_title_turns_by_its_styles_rotation() {
    let mut c = full("bar", "");
    c.styles += r##"<style:style style:name="chTR" style:family="chart"><style:chart-properties style:rotation-angle="90"/></style:style>"##;
    c.axes = r#"<chart:axis chart:dimension="y" chart:name="primary-y" chart:style-name="chAy"><chart:title chart:style-name="chTR"><text:p>Value</text:p></chart:title></chart:axis>"#.to_string();
    let m = model_of(&c);
    assert_eq!(m.axes[1].title.as_ref().unwrap().style.rot_deg, Some(-90.0));
}

// ---- the local table and its addresses -------------------------------------------------------------

#[test]
fn ranges_resolve_against_the_table_as_written() {
    let mut c = full("bar", "");
    // a row range, a single cell and a range past the table
    c.series = series("s1", "local-table.$B$2:.$C$2", "local-table.$A$3", None)
        + &series("s2", "local-table.$B$4:.$B$9", "local-table.$Z$99", None)
        + &series("s3", "garbage", "also garbage", None)
        + &series("s4", "local-table.$B$2", "local-table.$B$1", None);
    let g = &model_of(&c).groups[0];
    assert_eq!(nums(&g.series[0].values), vec![12.0, 15.0]);
    assert_eq!(g.series[0].name.as_deref(), Some("South"));
    // past the table: empty cells; the name cell is outside
    assert_eq!(g.series[1].values.len(), 6);
    assert_eq!(g.series[1].values[0], Some(17.0));
    assert!(g.series[1].values[1..].iter().all(Option::is_none));
    assert_eq!(g.series[1].name, None);
    assert!(g.series[2].values.is_empty());
    assert_eq!(nums(&g.series[3].values), vec![12.0]);
}

#[test]
fn hostile_ranges_are_cut_or_ignored() {
    let mut c = full("bar", "");
    c.series = series(
        "s1",
        "local-table.$A$1:.$ZZZZ$99999999999",
        "local-table.$A$1",
        None,
    ) + &series("s2", "local-table.$B$1:.$B$99999999", "x", None)
        + &series(
            "s3",
            "local-table.$XFE$1:.$XFE$1 local-table.$B$2:.$B$3",
            "x",
            None,
        )
        + &series("s4", &"local-table.$A$1 ".repeat(1000), "x", None);
    let m = model_of(&c);
    for s in &m.groups[0].series {
        assert!(s.values.len() <= ch::MAX_POINTS);
    }
    assert_eq!(m.groups[0].series.len(), 4);
}

#[test]
fn the_local_table_is_cut_at_its_budgets() {
    let mut c = full("bar", "");
    c.series = series("s1", "local-table.$A$1:.$A$30000", "x", None);
    // a table with a huge repeated row and cell count
    let x = c.xml().replace(
        "<table:table-row>",
        r#"<table:table-row table:number-rows-repeated="1000000"><table:table-cell table:number-columns-repeated="100000" office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row><table:table-row>"#,
    );
    let root = read_tree(x.as_bytes()).unwrap();
    let p = parse(&root, None).unwrap();
    assert!(p.truncated);
    assert!(p.model.groups[0].series[0].values.len() <= ch::MAX_POINTS);
}

#[test]
fn too_many_series_are_cut() {
    let mut c = full("bar", "");
    c.series = (0..ch::MAX_SERIES + 5)
        .map(|_| series("s1", "local-table.$B$2:.$B$4", "local-table.$B$1", None))
        .collect();
    let p = parse(&read_tree(c.xml().as_bytes()).unwrap(), None).unwrap();
    assert!(p.truncated);
    assert_eq!(p.model.groups[0].series.len(), ch::MAX_SERIES);
}

#[test]
fn a_chart_without_plot_area_or_series_is_not_a_chart() {
    let x = Cp::new("bar")
        .xml()
        .replace("chart:plot-area", "chart:other");
    assert!(parse(&read_tree(x.as_bytes()).unwrap(), None).is_none());
    let c = Cp::new("bar");
    assert!(parse(&read_tree(c.xml().as_bytes()).unwrap(), None).is_none());
}

#[test]
fn damaged_xml_and_other_documents_are_not_charts() {
    assert!(read_tree(b"").is_none());
    assert!(read_tree(b"<<<").is_none());
    let math = r#"<math:math xmlns:math="http://www.w3.org/1998/Math/MathML"><math:mi>x</math:mi></math:math>"#;
    let root = read_tree(math.as_bytes()).unwrap();
    assert!(parse(&root, None).is_none());
    // an element tree over the node budget is refused
    let big = format!(
        "<a>{}</a>",
        "<b/>".repeat(super::chart_read::MAX_CHART_NODES + 10)
    );
    assert!(read_tree(big.as_bytes()).is_none());
}

#[test]
fn styles_xml_of_the_object_supplies_default_styles() {
    let mut c = full("bar", "");
    c.series = series("sx", "local-table.$B$2:.$B$4", "local-table.$B$1", None);
    let styles = format!(
        r##"<office:document-styles {}><office:styles><style:style style:name="sx" style:family="chart"><style:graphic-properties draw:fill-color="#123456"/></style:style></office:styles></office:document-styles>"##,
        pns()
    );
    let content = read_tree(c.xml().as_bytes()).unwrap();
    let st = read_tree(styles.as_bytes()).unwrap();
    let m = parse(&content, Some(&st)).unwrap().model;
    assert_eq!(
        m.groups[0].series[0].fill,
        Some(Fill::Solid(Rgba::rgb(0x12, 0x34, 0x56)))
    );
}

// ---- the frame ---------------------------------------------------------------------------------------

fn object_frame(href: &str, replacement: Option<&str>) -> String {
    let rep = replacement.map_or(String::new(), |r| {
        format!(r#"<draw:image xlink:href="{r}"/>"#)
    });
    pic(&format!(r#"<draw:object xlink:href="{href}"/>{rep}"#))
}

fn chart_op(c: &Cp, replacement: Option<(&str, &[u8])>) -> Op {
    let rep = replacement.map(|r| r.0);
    let mut o = op(&slide_page(&object_frame("./Object 1", rep)))
        .auto(&gr(""))
        .object("Object 1", &c.xml());
    if let Some((name, bytes)) = replacement {
        o = o.part(name.trim_start_matches("./"), bytes);
    }
    o
}

fn groups_of(sc: &sd::SlideScene) -> Vec<&sd::GroupItem> {
    sc.items
        .iter()
        .filter_map(|i| match i {
            Item::Group(g) => Some(g),
            _ => None,
        })
        .collect()
}

#[test]
fn a_chart_object_is_drawn_in_a_group_at_the_frame_and_its_replacement_is_not() {
    let png = png(8, 8);
    let o = chart_op(
        &full("bar", ""),
        Some(("./ObjectReplacements/Object 1", &png)),
    );
    let sc = scene(&o);
    let g = groups_of(&sc);
    assert_eq!(g.len(), 1);
    assert!(close(g[0].xfrm.x, 2.0 * EMU_CM) && close(g[0].xfrm.y, EMU_CM));
    assert!(close(g[0].xfrm.w, 6.0 * EMU_CM) && close(g[0].xfrm.h, 3.0 * EMU_CM));
    assert_eq!(g[0].child_ext, (g[0].xfrm.w, g[0].xfrm.h));
    // bars, text boxes and lines
    assert!(g[0].items.len() > 10, "{}", g[0].items.len());
    assert!(pictures_of(&sc).is_empty());
    // the title is among the texts
    let texts: Vec<String> = g[0]
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Shape(s) => Some(text_of_shape(s)),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "Sales"), "{texts:?}");
    // the text view still has the chart
    assert!(doc(&o).markdown.contains("chart"));
}

#[test]
fn a_missing_object_and_a_broken_chart_fall_back_to_the_replacement_or_the_placeholder() {
    let png = png(8, 8);
    // no object part at all, a PNG beside it: the PNG is drawn
    let o = op(&slide_page(&object_frame(
        "./Object 9",
        Some("./ObjectReplacements/Object 9"),
    )))
    .auto(&gr(""))
    .part("ObjectReplacements/Object 9", &png);
    let sc = scene(&o);
    assert!(groups_of(&sc).is_empty());
    let p = pictures_of(&sc);
    assert_eq!(p.len(), 1);
    assert_ne!(p[0].image.key, MISSING_PICTURE);
    // LibreOffice's own metafile (`VCLMTF`) cannot be drawn: the placeholder
    let o = op(&slide_page(&object_frame(
        "./Object 9",
        Some("./ObjectReplacements/Object 9"),
    )))
    .auto(&gr(""))
    .part("ObjectReplacements/Object 9", b"VCLMTF\x01\x00");
    let sc = scene(&o);
    assert_eq!(pictures_of(&sc)[0].image.key, MISSING_PICTURE);
    // a formula object (not a chart) with a replacement
    let math = r#"<math:math xmlns:math="http://www.w3.org/1998/Math/MathML"><math:mi>x</math:mi></math:math>"#;
    let o = op(&slide_page(&object_frame(
        "./Object 1",
        Some("./ObjectReplacements/Object 1"),
    )))
    .auto(&gr(""))
    .object("Object 1", math)
    .part("ObjectReplacements/Object 1", &png);
    let sc = scene(&o);
    assert!(groups_of(&sc).is_empty());
    assert_eq!(pictures_of(&sc).len(), 1);
    // a chart class that is not drawn (stock) falls back too
    let o = chart_op(
        &full("stock", ""),
        Some(("./ObjectReplacements/Object 1", &png)),
    );
    let sc = scene(&o);
    assert!(groups_of(&sc).is_empty());
    assert_eq!(pictures_of(&sc).len(), 1);
}

#[test]
fn the_replacement_may_be_svg_or_jpeg_by_its_bytes() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;
    let o = chart_op(
        &full("stock", ""),
        Some(("./ObjectReplacements/Object 1", svg)),
    );
    let p = pictures_of(&scene(&o)).len();
    assert_eq!(p, 1);
    let o = chart_op(
        &full("stock", ""),
        Some((
            "./ObjectReplacements/Object 1",
            &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0],
        )),
    );
    // (a JPEG that cannot be decoded is not a picture)
    let _ = scene(&o);
}

#[test]
fn an_oversized_chart_part_is_refused_and_flagged() {
    let mut c = full("bar", "");
    c.title = Some("T");
    let big = c.xml().replace(
        "<chart:legend ",
        &format!("<!--{}--><chart:legend ", "x".repeat(600 * 1024)),
    );
    let o = op(&slide_page(&object_frame("./Object 1", None)))
        .auto(&gr(""))
        .object("Object 1", &big);
    let sc = scene(&o);
    assert!(groups_of(&sc).is_empty());
    assert!(sc.truncated);
}

#[test]
fn the_item_budget_cuts_a_chart() {
    let opts = DocOptions {
        max_slide_shapes: 6,
        ..DocOptions::default()
    };
    let o = chart_op(&full("bar", ""), None);
    let d = load_op(&o, &opts).unwrap();
    let sc = &d.slide_scenes[0];
    assert!(sc.truncated);
    let n: usize = groups_of(sc).iter().map(|g| g.items.len()).sum();
    assert!(n <= 6, "{n}");
}

#[test]
fn an_object_path_that_leaves_the_package_is_not_followed() {
    for href in ["../Object 1", "/Object 1", "http://x/Object 1", ""] {
        let o = op(&slide_page(&object_frame(href, None)))
            .auto(&gr(""))
            .object("Object 1", &full("bar", "").xml());
        assert!(groups_of(&scene(&o)).is_empty(), "{href}");
    }
}

#[test]
fn named_symbols_keep_their_direction_and_shape() {
    use super::chart_read::symbol_named as named;
    use ch::MarkerSymbol as M;
    for (name, want) in [
        ("arrow-up", M::Triangle),
        ("arrow-down", M::TriangleDown),
        ("arrow-left", M::TriangleLeft),
        ("arrow-right", M::TriangleRight),
        ("bowtie", M::Bowtie),
        ("sandglass", M::Sandglass),
        ("horizontal-bar", M::Dash),
        ("vertical-bar", M::VBar),
        ("x", M::X),
        ("asterisk", M::Star),
        ("square", M::Square),
        ("nonsense", M::Diamond),
    ] {
        assert_eq!(named(name), want, "{name}");
    }
}
