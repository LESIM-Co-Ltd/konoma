//! Tests of the chart part reader (`chart_xml`): each chart type, the caches, axes, legends,
//! labels, missing and hostile input.

use super::chart_xml::*;
use super::docx_xml::Node;
use super::slide_draw::chart::*;
use super::slide_draw::{Fill, Rgba};
use super::OfficeError;

const NS: &str = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

fn resolver(n: &Node) -> Option<Rgba> {
    match n.name.as_str() {
        "srgbClr" => Rgba::from_hex(n.attr("val")?),
        "schemeClr" => match n.attr("val")? {
            "accent1" => Some(Rgba::rgb(1, 1, 1)),
            "accent2" => Some(Rgba::rgb(2, 2, 2)),
            "tx1" => Some(Rgba::rgb(9, 9, 9)),
            _ => None,
        },
        _ => None,
    }
}

const PALETTE: [Rgba; 2] = [Rgba::rgb(1, 1, 1), Rgba::rgb(2, 2, 2)];

fn env<'a>() -> ChartEnv<'a> {
    ChartEnv {
        resolve_color: &resolver,
        palette: &PALETTE,
        minor_font: Some("Calibri"),
        major_font: Some("Cambria"),
    }
}

fn space(inner: &str) -> String {
    format!(r#"<c:chartSpace {NS}>{inner}</c:chartSpace>"#)
}

fn parse(xml: &str) -> ParsedChart {
    parse_chart(xml.as_bytes(), &env()).expect("parses")
}

fn chart(plot: &str) -> String {
    space(&format!(
        "<c:chart><c:plotArea>{plot}</c:plotArea></c:chart>"
    ))
}

fn str_cache(vals: &[&str]) -> String {
    let pts: String = vals
        .iter()
        .enumerate()
        .map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#))
        .collect();
    format!(
        r#"<c:strCache><c:ptCount val="{}"/>{pts}</c:strCache>"#,
        vals.len()
    )
}

fn num_cache(vals: &[&str], fmt: &str) -> String {
    let pts: String = vals
        .iter()
        .enumerate()
        .map(|(i, v)| format!(r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#))
        .collect();
    format!(
        r#"<c:numCache><c:formatCode>{fmt}</c:formatCode><c:ptCount val="{}"/>{pts}</c:numCache>"#,
        vals.len()
    )
}

fn ser(name: &str, cats: &[&str], vals: &[&str], extra: &str) -> String {
    format!(
        r#"<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:f>x</c:f>{}</c:strRef></c:tx>{extra}<c:cat><c:strRef><c:f>c</c:f>{}</c:strRef></c:cat><c:val><c:numRef><c:f>v</c:f>{}</c:numRef></c:val></c:ser>"#,
        str_cache(&[name]),
        str_cache(cats),
        num_cache(vals, "General")
    )
}

fn group_of(xml: &str) -> ChartGroup {
    parse(xml).model.groups.into_iter().next().expect("a group")
}

#[test]
fn bar_chart_with_caches() {
    let x = chart(&format!(
        r#"<c:barChart><c:barDir val="bar"/><c:grouping val="stacked"/><c:varyColors val="0"/>{}<c:gapWidth val="80"/><c:overlap val="100"/><c:axId val="1"/><c:axId val="2"/></c:barChart>"#,
        ser("Sales", &["a", "b", "c"], &["1.5", "2", "-3"], "")
    ));
    let p = parse(&x);
    assert!(!p.truncated && !p.approximated);
    let g = &p.model.groups[0];
    assert_eq!(g.kind, GroupKind::Bar);
    assert_eq!(g.bar_dir, BarDir::Bar);
    assert_eq!(g.grouping, Grouping::Stacked);
    assert_eq!((g.gap_width, g.overlap), (80.0, 100.0));
    assert_eq!(g.axis_ids, vec![1, 2]);
    let s = &g.series[0];
    assert_eq!(s.name.as_deref(), Some("Sales"));
    assert_eq!(s.cats, vec!["a", "b", "c"]);
    assert_eq!(s.values, vec![Some(1.5), Some(2.0), Some(-3.0)]);
    assert_eq!(s.format_code.as_deref(), Some("General"));
}

#[test]
fn defaults_of_a_bar_group() {
    let g = group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a"], &["1"], "")
    )));
    assert_eq!(g.grouping, Grouping::Clustered);
    assert_eq!(g.bar_dir, BarDir::Col);
    assert_eq!((g.gap_width, g.overlap), (150.0, 0.0));
    assert!(!g.vary_colors);
}

#[test]
fn line_area_chart_kinds_and_groupings() {
    for (tag, kind) in [
        ("lineChart", GroupKind::Line),
        ("line3DChart", GroupKind::Line),
        ("areaChart", GroupKind::Area),
        ("area3DChart", GroupKind::Area),
        ("bar3DChart", GroupKind::Bar),
    ] {
        let p = parse(&chart(&format!(
            r#"<c:{tag}><c:grouping val="percentStacked"/>{}</c:{tag}>"#,
            ser("s", &["a"], &["1"], "")
        )));
        let g = &p.model.groups[0];
        assert_eq!(g.kind, kind, "{tag}");
        assert_eq!(g.grouping, Grouping::PercentStacked, "{tag}");
        assert_eq!(g.three_d, tag.contains("3D"), "{tag}");
        assert_eq!(p.model.three_d, tag.contains("3D"), "{tag}");
    }
    let g = group_of(&chart(&format!(
        r#"<c:lineChart><c:grouping val="standard"/><c:marker val="0"/>{}</c:lineChart>"#,
        ser("s", &["a"], &["1"], "<c:smooth val=\"1\"/>")
    )));
    assert_eq!(g.grouping, Grouping::Standard);
    assert!(!g.markers);
    assert_eq!(g.series[0].smooth, Some(true));
}

#[test]
fn pie_and_doughnut() {
    let g = group_of(&chart(&format!(
        r#"<c:pieChart><c:firstSliceAng val="30"/>{}</c:pieChart>"#,
        ser("s", &["a", "b"], &["1", "2"], "<c:explosion val=\"12\"/><c:dPt><c:idx val=\"1\"/><c:explosion val=\"20\"/><c:spPr><a:solidFill><a:srgbClr val=\"FF0000\"/></a:solidFill></c:spPr></c:dPt>")
    )));
    assert_eq!(g.kind, GroupKind::Pie);
    assert!(g.vary_colors, "pie colours vary unless told otherwise");
    assert_eq!(g.first_slice_ang, 30.0);
    assert_eq!(g.series[0].explosion, Some(12.0));
    assert_eq!(g.series[0].points[0].idx, 1);
    assert_eq!(g.series[0].points[0].explosion, Some(20.0));
    assert_eq!(
        g.series[0].points[0].fill,
        Some(Fill::Solid(Rgba::rgb(255, 0, 0)))
    );
    let g = group_of(&chart(&format!(
        r#"<c:doughnutChart><c:varyColors val="0"/>{}<c:holeSize val="65"/></c:doughnutChart>"#,
        ser("s", &["a"], &["1"], "")
    )));
    assert_eq!(g.kind, GroupKind::Doughnut);
    assert_eq!(g.hole_size, 65.0);
    assert!(!g.vary_colors);
}

#[test]
fn scatter_and_bubble_series() {
    let sc = format!(
        r#"<c:scatterChart><c:scatterStyle val="smoothMarker"/><c:ser><c:idx val="0"/><c:order val="0"/><c:xVal><c:numRef><c:f>x</c:f>{}</c:numRef></c:xVal><c:yVal><c:numRef><c:f>y</c:f>{}</c:numRef></c:yVal></c:ser></c:scatterChart>"#,
        num_cache(&["1", "2", "3"], "General"),
        num_cache(&["4", "5", "6"], "0.0")
    );
    let g = group_of(&chart(&sc));
    assert_eq!(g.kind, GroupKind::Scatter);
    assert_eq!(g.scatter_style, ScatterStyle::SmoothMarker);
    let s = &g.series[0];
    assert_eq!(s.x_values, vec![Some(1.0), Some(2.0), Some(3.0)]);
    assert_eq!(s.values, vec![Some(4.0), Some(5.0), Some(6.0)]);
    assert_eq!(s.format_code.as_deref(), Some("0.0"));
    let bub = String::from(
        r#"<c:bubbleChart><c:bubbleScale val="50"/><c:sizeRepresents val="w"/><c:ser><c:idx val="0"/><c:order val="0"/><c:xVal><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>1</c:v></c:pt></c:numLit></c:xVal><c:yVal><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>2</c:v></c:pt></c:numLit></c:yVal><c:bubbleSize><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>9</c:v></c:pt></c:numLit></c:bubbleSize></c:ser></c:bubbleChart>"#,
    );
    let g = group_of(&chart(&bub));
    assert_eq!(g.kind, GroupKind::Bubble);
    assert_eq!(g.bubble_scale, 50.0);
    assert!(g.size_is_width);
    assert_eq!(g.series[0].sizes, vec![Some(9.0)]);
    assert_eq!(g.series[0].x_values, vec![Some(1.0)]);
    // Text x values give no numbers (the points are numbered).
    let txt = r#"<c:scatterChart><c:ser><c:idx val="0"/><c:order val="0"/><c:xVal><c:strRef><c:f>x</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>a</c:v></c:pt></c:strCache></c:strRef></c:xVal></c:ser></c:scatterChart>"#;
    assert!(group_of(&chart(txt)).series[0].x_values.is_empty());
}

#[test]
fn radar_styles() {
    for (v, st) in [
        ("standard", RadarStyle::Standard),
        ("marker", RadarStyle::Marker),
        ("filled", RadarStyle::Filled),
    ] {
        let g = group_of(&chart(&format!(
            r#"<c:radarChart><c:radarStyle val="{v}"/>{}</c:radarChart>"#,
            ser("s", &["a", "b", "c"], &["1", "2", "3"], "")
        )));
        assert_eq!(g.kind, GroupKind::Radar);
        assert_eq!(g.radar_style, st);
    }
}

#[test]
fn approximated_and_skipped_types() {
    let p = parse(&chart(&format!(
        "<c:stockChart>{}</c:stockChart>",
        ser("s", &["a"], &["1"], "")
    )));
    assert!(p.approximated);
    assert_eq!(p.model.groups[0].kind, GroupKind::Line);
    let p = parse(&chart(&format!(
        "<c:ofPieChart>{}</c:ofPieChart>",
        ser("s", &["a"], &["1"], "")
    )));
    assert!(p.approximated);
    assert_eq!(p.model.groups[0].kind, GroupKind::Pie);
    let p = parse(&chart(&format!(
        "<c:surfaceChart>{}</c:surfaceChart>",
        ser("s", &["a"], &["1"], "")
    )));
    assert!(p.approximated);
    assert!(p.model.groups.is_empty());
}

#[test]
fn combination_chart_has_two_groups_and_axes() {
    let plot = format!(
        r#"<c:barChart>{}<c:axId val="10"/><c:axId val="20"/></c:barChart><c:lineChart>{}<c:axId val="30"/><c:axId val="40"/></c:lineChart><c:catAx><c:axId val="10"/><c:crossAx val="20"/></c:catAx><c:valAx><c:axId val="20"/><c:crossAx val="10"/></c:valAx><c:valAx><c:axId val="40"/><c:crossAx val="30"/><c:axPos val="r"/><c:crosses val="max"/></c:valAx>"#,
        ser("a", &["x"], &["1"], ""),
        ser("b", &["x"], &["2"], "")
    );
    let m = parse(&chart(&plot)).model;
    assert_eq!(m.groups.len(), 2);
    assert_eq!(m.axes.len(), 3);
    assert_eq!(m.axes[2].pos, AxisPos::Right);
    assert_eq!(m.axes[2].crosses, Crosses::Max);
    assert_eq!(m.groups[1].axis_ids, vec![30, 40]);
}

#[test]
fn literals_and_multi_level_categories() {
    let s = r#"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Lit</c:v></c:tx><c:cat><c:strLit><c:ptCount val="2"/><c:pt idx="0"><c:v>p</c:v></c:pt><c:pt idx="1"><c:v>q</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit><c:formatCode>0%</c:formatCode><c:ptCount val="2"/><c:pt idx="0"><c:v>0.5</c:v></c:pt><c:pt idx="1"><c:v>0.25</c:v></c:pt></c:numLit></c:val></c:ser></c:barChart>"#;
    let g = group_of(&chart(s));
    assert_eq!(g.series[0].name.as_deref(), Some("Lit"));
    assert_eq!(g.series[0].cats, vec!["p", "q"]);
    assert_eq!(g.series[0].format_code.as_deref(), Some("0%"));
    let ml = r#"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:multiLvlStrRef><c:f>x</c:f><c:multiLvlStrCache><c:ptCount val="3"/><c:lvl><c:pt idx="0"><c:v>leaf0</c:v></c:pt><c:pt idx="1"><c:v>leaf1</c:v></c:pt><c:pt idx="2"><c:v>leaf2</c:v></c:pt></c:lvl><c:lvl><c:pt idx="0"><c:v>group</c:v></c:pt></c:lvl></c:multiLvlStrCache></c:multiLvlStrRef></c:cat></c:ser></c:barChart>"#;
    let g = group_of(&chart(ml));
    assert_eq!(g.series[0].cats, vec!["leaf0", "leaf1", "leaf2"]);
}

#[test]
fn numeric_categories_are_formatted_with_the_cache_code() {
    let s = format!(
        r#"<c:lineChart><c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:numRef><c:f>x</c:f>{}</c:numRef></c:cat></c:ser></c:lineChart>"#,
        num_cache(&["44927", "44958"], "yyyy-mm-dd")
    );
    let s = group_of(&chart(&s)).series.remove(0);
    assert_eq!(s.cats, vec!["2023-01-01", "2023-02-01"]);
    assert_eq!(s.cat_nums, vec![Some(44927.0), Some(44958.0)]);
    assert_eq!(s.cat_format.as_deref(), Some("yyyy-mm-dd"));
}

#[test]
fn sparse_points_and_bad_numbers() {
    let s = r##"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>v</c:f><c:numCache><c:ptCount val="5"/><c:pt idx="1"><c:v>7</c:v></c:pt><c:pt idx="3"><c:v>#N/A</c:v></c:pt><c:pt idx="4"><c:v>NaN</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart>"##;
    let v = group_of(&chart(s)).series[0].values.clone();
    assert_eq!(v, vec![None, Some(7.0), None, None, None]);
    // inf as text is not a number.
    let s = r#"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numLit><c:ptCount val="1"/><c:pt idx="0"><c:v>inf</c:v></c:pt></c:numLit></c:val></c:ser></c:barChart>"#;
    assert_eq!(group_of(&chart(s)).series[0].values, vec![None]);
}

#[test]
fn colours_come_from_the_callback_and_fills() {
    let sp = r#"<c:spPr><a:solidFill><a:schemeClr val="accent2"><a:lumMod val="50000"/></a:schemeClr></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill><a:prstDash val="dash"/></a:ln></c:spPr>"#;
    let g = group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a"], &["1"], sp)
    )));
    let s = &g.series[0];
    assert_eq!(s.fill, Some(Fill::Solid(Rgba::rgb(2, 2, 2))));
    let l = s.line.as_ref().unwrap();
    assert_eq!(l.width, Some(12700.0));
    assert_eq!(l.color, Some(Rgba::rgb(0, 255, 0)));
    assert_eq!(l.dash, super::slide_draw::Dash::Dash);
    // noFill, gradient, pattern.
    let no = r#"<c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr>"#;
    let s = &group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a"], &["1"], no)
    )))
    .series[0];
    assert_eq!(s.fill, Some(Fill::None));
    assert!(s.line.as_ref().unwrap().none);
    let grad = r#"<c:spPr><a:gradFill><a:gsLst><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs></a:gsLst><a:lin ang="5400000"/></a:gradFill></c:spPr>"#;
    let s = &group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a"], &["1"], grad)
    )))
    .series[0];
    match &s.fill {
        Some(Fill::Gradient(g)) => {
            assert_eq!(g.stops[0], (0.0, Rgba::rgb(255, 0, 0)));
            assert_eq!(g.stops[1], (1.0, Rgba::rgb(0, 0, 255)));
        }
        o => panic!("{o:?}"),
    }
    let patt = r#"<c:spPr><a:pattFill prst="ltUpDiag"><a:fgClr><a:srgbClr val="010203"/></a:fgClr><a:bgClr><a:srgbClr val="FFFFFF"/></a:bgClr></a:pattFill></c:spPr>"#;
    let s = &group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a"], &["1"], patt)
    )))
    .series[0];
    assert!(matches!(&s.fill, Some(Fill::Pattern { preset, .. }) if preset == "ltUpDiag"));
    // A colour the callback does not know leaves the fill automatic.
    let unknown = r#"<c:spPr><a:solidFill><a:schemeClr val="weird"/></a:solidFill></c:spPr>"#;
    let s = &group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a"], &["1"], unknown)
    )))
    .series[0];
    assert_eq!(s.fill, None);
}

#[test]
fn markers_and_points() {
    let extra = r#"<c:marker><c:symbol val="diamond"/><c:size val="9"/><c:spPr><a:solidFill><a:srgbClr val="112233"/></a:solidFill></c:spPr></c:marker><c:dPt><c:idx val="0"/><c:marker><c:symbol val="none"/></c:marker></c:dPt>"#;
    let g = group_of(&chart(&format!(
        "<c:lineChart>{}</c:lineChart>",
        ser("s", &["a"], &["1"], extra)
    )));
    let s = &g.series[0];
    let m = s.marker.as_ref().unwrap();
    assert_eq!(m.symbol, MarkerSymbol::Diamond);
    assert_eq!(m.size_pt, Some(9.0));
    assert_eq!(m.fill, Some(Fill::Solid(Rgba::rgb(0x11, 0x22, 0x33))));
    assert_eq!(
        s.points[0].marker.as_ref().unwrap().symbol,
        MarkerSymbol::None
    );
}

#[test]
fn axes_are_read_completely() {
    let ax = r#"<c:catAx><c:axId val="-1884094432"/><c:scaling><c:orientation val="maxMin"/></c:scaling><c:delete val="0"/><c:axPos val="t"/><c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:t>Cats</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title><c:numFmt formatCode="0.0" sourceLinked="0"/><c:majorTickMark val="cross"/><c:minorTickMark val="in"/><c:tickLblPos val="low"/><c:crossAx val="-5"/><c:crosses val="max"/><c:tickLblSkip val="3"/><c:tickMarkSkip val="2"/></c:catAx><c:valAx><c:axId val="-5"/><c:scaling><c:logBase val="10"/><c:orientation val="minMax"/><c:max val="1000"/><c:min val="1"/></c:scaling><c:delete val="1"/><c:axPos val="l"/><c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="AAAAAA"/></a:solidFill></a:ln></c:spPr></c:majorGridlines><c:minorGridlines/><c:numFmt formatCode="0%" sourceLinked="1"/><c:crossAx val="-1884094432"/><c:crossesAt val="5"/><c:crossBetween val="midCat"/><c:majorUnit val="10"/><c:minorUnit val="2"/></c:valAx>"#;
    let m = parse(&chart(ax)).model;
    let c = &m.axes[0];
    assert_eq!((c.id, c.cross_ax, c.kind), (-1884094432, -5, AxisKind::Cat));
    assert!(c.reversed && !c.deleted);
    assert_eq!(c.pos, AxisPos::Top);
    assert_eq!(c.title.as_ref().unwrap().text, "Cats");
    assert_eq!(c.num_fmt, Some(("0.0".into(), false)));
    assert_eq!(
        (c.major_tick, c.minor_tick),
        (TickMark::Cross, TickMark::In)
    );
    assert_eq!(c.tick_label_pos, TickLabelPos::Low);
    assert_eq!(c.crosses, Crosses::Max);
    assert_eq!((c.label_skip, c.tick_skip), (3, 2));
    let v = &m.axes[1];
    assert_eq!(v.kind, AxisKind::Val);
    assert!(v.deleted);
    assert_eq!(
        (v.min, v.max, v.log_base),
        (Some(1.0), Some(1000.0), Some(10.0))
    );
    assert_eq!(v.crosses, Crosses::At(5.0));
    assert!(!v.between);
    assert_eq!((v.major_unit, v.minor_unit), (Some(10.0), Some(2.0)));
    assert_eq!(v.num_fmt, Some(("0%".into(), true)));
    let g = v.major_grid.as_ref().unwrap();
    assert_eq!(g.width, Some(6350.0));
    assert_eq!(g.color, Some(Rgba::rgb(0xAA, 0xAA, 0xAA)));
    assert_eq!(v.minor_grid, Some(Stroke::default()));
    // No gridlines element: none.
    assert!(parse(&chart(r#"<c:valAx><c:axId val="1"/></c:valAx>"#))
        .model
        .axes[0]
        .major_grid
        .is_none());
}

#[test]
fn title_legend_layouts_and_areas() {
    let x = space(
        r#"<c:roundedCorners val="0"/><c:lang val="ja-JP"/><c:chart><c:title><c:tx><c:rich><a:bodyPr rot="-5400000"/><a:p><a:pPr><a:defRPr sz="1400" b="1"><a:solidFill><a:srgbClr val="102030"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:pPr><a:r><a:rPr sz="2000" i="1"/><a:t>Hello</a:t></a:r></a:p><a:p><a:r><a:t>World</a:t></a:r><a:br/><a:r><a:t>!</a:t></a:r></a:p></c:rich></c:tx><c:layout><c:manualLayout><c:xMode val="edge"/><c:yMode val="edge"/><c:x val="0.1"/><c:y val="0.2"/></c:manualLayout></c:layout><c:overlay val="1"/></c:title><c:plotArea><c:layout><c:manualLayout><c:layoutTarget val="inner"/><c:xMode val="edge"/><c:yMode val="edge"/><c:x val="0.1"/><c:y val="0.2"/><c:w val="0.7"/><c:h val="0.6"/></c:manualLayout></c:layout><c:spPr><a:solidFill><a:srgbClr val="EEEEEE"/></a:solidFill></c:spPr></c:plotArea><c:legend><c:legendPos val="tr"/><c:legendEntry><c:idx val="1"/><c:delete val="1"/></c:legendEntry><c:overlay val="1"/><c:spPr><a:ln><a:solidFill><a:srgbClr val="000000"/></a:solidFill></a:ln></c:spPr><c:txPr><a:bodyPr/><a:p><a:pPr><a:defRPr sz="900"/></a:pPr></a:p></c:txPr></c:legend><c:dispBlanksAs val="span"/></c:chart><c:spPr><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></c:spPr><c:txPr><a:bodyPr/><a:p><a:pPr><a:defRPr sz="1100"/></a:pPr></a:p></c:txPr>"#,
    );
    let m = parse(&x).model;
    let t = m.title.as_ref().unwrap();
    assert_eq!(t.text, "Hello\nWorld\n!");
    // The run's properties win over the paragraph's defaults.
    assert_eq!(t.style.size_pt, Some(20.0));
    assert_eq!(t.style.bold, Some(true));
    assert_eq!(t.style.italic, Some(true));
    assert_eq!(t.style.color, Some(Rgba::rgb(0x10, 0x20, 0x30)));
    assert_eq!(t.style.font.as_deref(), Some("Cambria"));
    assert_eq!(t.style.rot_deg, Some(-90.0));
    assert!(t.overlay);
    let l = t.layout.unwrap();
    assert!(l.x_edge && l.y_edge && l.x == Some(0.1) && l.y == Some(0.2));
    let pl = m.plot_layout.unwrap();
    assert!(pl.inner && pl.w == Some(0.7) && pl.h == Some(0.6));
    assert_eq!(m.plot_fill, Some(Fill::Solid(Rgba::rgb(0xEE, 0xEE, 0xEE))));
    let lg = m.legend.as_ref().unwrap();
    assert_eq!(lg.pos, LegendPos::TopRight);
    assert!(lg.overlay);
    assert_eq!(lg.deleted, vec![1]);
    assert_eq!(lg.style.size_pt, Some(9.0));
    assert!(lg.line.is_some());
    assert_eq!(m.disp_blanks_as, DispBlanks::Span);
    assert_eq!(m.chart_fill, Some(Fill::Solid(Rgba::WHITE)));
    assert_eq!(m.text.size_pt, Some(11.0));
    assert!(m.locale_ja);
    assert_eq!(m.default_font.as_deref(), Some("Calibri"));
    assert_eq!(m.palette.len(), 2);
}

#[test]
fn automatic_title_is_the_only_series_name() {
    let mk = |series: &str, deleted: bool| {
        parse(&space(&format!(
            r#"<c:chart><c:title><c:overlay val="0"/></c:title><c:autoTitleDeleted val="{}"/><c:plotArea><c:barChart>{series}</c:barChart></c:plotArea></c:chart>"#,
            deleted as u8
        )))
        .model
        .title
    };
    let one = ser("Only", &["a"], &["1"], "");
    assert_eq!(mk(&one, false).unwrap().text, "Only");
    let two = format!("{one}{}", ser("Other", &["a"], &["1"], ""));
    assert!(mk(&two, false).is_none());
}

#[test]
fn style_from_the_fallback() {
    let x = space(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="c14" xmlns:c14="x"><c14:style val="104"/></mc:Choice><mc:Fallback><c:style val="4"/></mc:Fallback></mc:AlternateContent><c:chart/>"#,
    );
    assert_eq!(parse(&x).model.style, 4);
    assert_eq!(parse(&space(r#"<c:style val="99"/>"#)).model.style, 0);
    assert_eq!(parse(&space(r#"<c:style val="7"/>"#)).model.style, 7);
}

#[test]
fn data_labels() {
    let d = r#"<c:dLbls><c:dLbl><c:idx val="1"/><c:tx><c:rich><a:p><a:r><a:t>custom</a:t></a:r></a:p></c:rich></c:tx><c:showVal val="1"/></c:dLbl><c:dLbl><c:idx val="2"/><c:delete val="1"/></c:dLbl><c:numFmt formatCode="0.0" sourceLinked="0"/><c:txPr><a:bodyPr/><a:p><a:pPr><a:defRPr sz="800"/></a:pPr></a:p></c:txPr><c:dLblPos val="inEnd"/><c:showLegendKey val="0"/><c:showVal val="1"/><c:showCatName val="1"/><c:showSerName val="0"/><c:showPercent val="1"/><c:separator>
</c:separator></c:dLbls>"#;
    let g = group_of(&chart(&format!(
        "<c:barChart>{}</c:barChart>",
        ser("s", &["a", "b", "c"], &["1", "2", "3"], d)
    )));
    let l = g.series[0].labels.as_ref().unwrap();
    assert!(l.show_value && l.show_category && l.show_percent && !l.show_series);
    assert_eq!(l.pos, Some(LabelPos::InsideEnd));
    assert_eq!(l.num_fmt.as_deref(), Some("0.0"));
    assert_eq!(l.separator.as_deref(), Some("\n"));
    assert_eq!(l.style.size_pt, Some(8.0));
    assert_eq!(l.points.len(), 2);
    assert_eq!(l.points[0].0, 1);
    assert_eq!(l.points[0].1.text.as_deref(), Some("custom"));
    assert!(l.points[1].1.delete);
    // A linked number format is not an override; group-level labels are read too.
    let gl = format!(
        r#"<c:barChart>{}<c:dLbls><c:numFmt formatCode="General" sourceLinked="1"/><c:showVal val="1"/></c:dLbls></c:barChart>"#,
        ser("s", &["a"], &["1"], "")
    );
    let g = group_of(&chart(&gl));
    let l = g.labels.as_ref().unwrap();
    assert!(l.show_value && l.num_fmt.is_none());
    assert!(g.series[0].labels.is_none());
    for (v, p) in [
        ("ctr", LabelPos::Center),
        ("inBase", LabelPos::InsideBase),
        ("outEnd", LabelPos::OutsideEnd),
        ("bestFit", LabelPos::BestFit),
        ("l", LabelPos::Left),
        ("r", LabelPos::Right),
        ("t", LabelPos::Above),
        ("b", LabelPos::Below),
    ] {
        let x = chart(&format!(
            r#"<c:barChart><c:dLbls><c:dLblPos val="{v}"/><c:showVal val="1"/></c:dLbls>{}</c:barChart>"#,
            ser("s", &["a"], &["1"], "")
        ));
        assert_eq!(group_of(&x).labels.unwrap().pos, Some(p), "{v}");
    }
}

#[test]
fn missing_and_odd_parts() {
    // No chart at all.
    let m = parse(&space("")).model;
    assert!(m.groups.is_empty() && m.axes.is_empty() && m.title.is_none() && m.legend.is_none());
    let m = parse(&space("<c:chart/>")).model;
    assert!(m.groups.is_empty());
    // A series without data, a group without series, an axis without ids.
    let g = group_of(&chart("<c:barChart><c:ser/></c:barChart>"));
    assert!(g.series[0].values.is_empty() && g.series[0].cats.is_empty());
    assert!(group_of(&chart("<c:pieChart/>")).series.is_empty());
    let m = parse(&chart("<c:catAx/><c:valAx/>")).model;
    assert_eq!(m.axes.len(), 2);
    // Unknown elements and extension lists are ignored.
    let x = chart(&format!(
        r#"<c:barChart>{}<c:extLst><c:ext uri="x"><foo/></c:ext></c:extLst></c:barChart><c:foo/>"#,
        ser("s", &["a"], &["1"], "")
    ));
    assert_eq!(parse(&x).model.groups.len(), 1);
}

#[test]
fn not_a_chart() {
    let env = env();
    let e = parse_chart(b"<cx:chartSpace xmlns:cx=\"x\"/>", &env).unwrap_err();
    assert!(matches!(e, OfficeError::Unsupported));
    let e = parse_chart(b"<html/>", &env).unwrap_err();
    assert!(matches!(e, OfficeError::Unsupported));
    assert!(parse_chart(b"", &env).is_err());
    assert!(parse_chart(b"not xml at all", &env).is_err());
    // A part cut off in the middle is an error, not a half chart.
    assert!(parse_chart(b"<c:chartSpace><c:chart>", &env).is_err());
}

// ---- hostile input ----

fn big_series(points: usize) -> String {
    let mut pts = String::new();
    for i in 0..points {
        pts.push_str(&format!(r#"<c:pt idx="{i}"><c:v>{}</c:v></c:pt>"#, i % 10));
    }
    format!(
        r#"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:val><c:numRef><c:f>v</c:f><c:numCache><c:ptCount val="{points}"/>{pts}</c:numCache></c:numRef></c:val></c:ser></c:barChart>"#
    )
}

#[test]
fn a_hundred_thousand_points_are_cut_without_storing_them() {
    let t = std::time::Instant::now();
    let p = parse(&chart(&big_series(100_000)));
    assert!(t.elapsed().as_secs() < 10, "{:?}", t.elapsed());
    assert!(p.truncated);
    let v = &p.model.groups[0].series[0].values;
    assert!(v.len() <= MAX_POINTS, "{}", v.len());
    assert!(!v.is_empty());
    // And it can still be drawn.
    let (items, _) = draw_chart(&p.model, 4_000_000.0, 3_000_000.0);
    assert!(!items.is_empty());
}

#[test]
fn a_declared_count_is_never_trusted() {
    let s = r#"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:cat><c:strRef><c:f>c</c:f><c:strCache><c:ptCount val="4000000000"/><c:pt idx="0"><c:v>a</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:f>v</c:f><c:numCache><c:ptCount val="4000000000"/><c:pt idx="4000000000"><c:v>1</c:v></c:pt><c:pt idx="99999999999999999999"><c:v>1</c:v></c:pt><c:pt idx="2"><c:v>5</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart>"#;
    let p = parse(&chart(s));
    let ser = &p.model.groups[0].series[0];
    assert!(ser.values.len() <= MAX_POINTS);
    assert!(ser.cats.len() <= MAX_CATEGORIES);
    assert_eq!(ser.values[2], Some(5.0));
    assert!(p.truncated || ser.truncated);
}

#[test]
fn too_many_series_are_dropped() {
    let one = ser("s", &["a"], &["1"], "");
    let many = one.repeat(400);
    let p = parse(&chart(&format!("<c:barChart>{many}</c:barChart>")));
    assert_eq!(p.model.groups[0].series.len(), MAX_SERIES);
    assert!(p.truncated);
}

#[test]
fn too_many_elements_are_refused() {
    let junk = "<c:x/>".repeat(300_000);
    let e = parse_chart(chart(&junk).as_bytes(), &env()).unwrap_err();
    assert!(matches!(e, OfficeError::TooLarge { .. }), "{e:?}");
}

#[test]
fn deep_nesting_is_refused() {
    let deep = "<c:x>".repeat(400) + &"</c:x>".repeat(400);
    let e = parse_chart(chart(&deep).as_bytes(), &env()).unwrap_err();
    assert!(matches!(e, OfficeError::TooLarge { .. }), "{e:?}");
}

#[test]
fn hostile_text_and_numbers() {
    let long = "x".repeat(100_000);
    let s = format!(
        r#"<c:barChart><c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>{long}</c:v></c:tx><c:cat><c:strLit><c:ptCount val="1"/><c:pt idx="0"><c:v>{long}</c:v></c:pt></c:strLit></c:cat><c:val><c:numLit><c:ptCount val="2"/><c:pt idx="0"><c:v>1e999</c:v></c:pt><c:pt idx="1"><c:v>-1e308</c:v></c:pt></c:numLit></c:val></c:ser></c:barChart>"#
    );
    let p = parse(&chart(&s));
    let ser = &p.model.groups[0].series[0];
    assert!(ser.name.as_ref().unwrap().chars().count() <= MAX_LABEL_CHARS);
    assert!(ser.cats[0].chars().count() <= MAX_LABEL_CHARS);
    assert_eq!(ser.values[0], None);
    assert_eq!(ser.values[1], Some(-1e308));
    let (items, _) = draw_chart(&p.model, 4_000_000.0, 3_000_000.0);
    assert!(!items.is_empty());
}

#[test]
fn entities_in_text_are_resolved() {
    let s = format!(
        "<c:barChart>{}</c:barChart>",
        ser("A &amp; B &lt;1&gt; &#x41;", &["x"], &["1"], "")
    );
    let g = group_of(&chart(&s));
    assert_eq!(g.series[0].name.as_deref(), Some("A & B <1> A"));
}

#[test]
fn every_parsed_chart_type_draws() {
    for tag in [
        "barChart",
        "lineChart",
        "areaChart",
        "pieChart",
        "doughnutChart",
        "radarChart",
        "bar3DChart",
        "stockChart",
    ] {
        let p = parse(&chart(&format!(
            r#"<c:{tag}>{}<c:axId val="1"/><c:axId val="2"/></c:{tag}><c:catAx><c:axId val="1"/><c:crossAx val="2"/></c:catAx><c:valAx><c:axId val="2"/><c:crossAx val="1"/></c:valAx>"#,
            ser("s", &["a", "b", "c"], &["1", "2", "3"], "")
        )));
        let (items, trunc) = draw_chart(&p.model, 4_000_000.0, 3_000_000.0);
        assert!(!items.is_empty(), "{tag}");
        assert!(!trunc, "{tag}");
    }
}
