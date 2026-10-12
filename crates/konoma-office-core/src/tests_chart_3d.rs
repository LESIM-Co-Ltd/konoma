//! The 3-D parts of a chart part: `c:view3D`, the walls and the floor, `c:gapDepth`.

use super::chart_xml::*;
use super::docx_xml::Node;
use super::slide_draw::chart::*;
use super::slide_draw::{Fill, Rgba};

const NS: &str = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

fn resolver(n: &Node) -> Option<Rgba> {
    match n.name.as_str() {
        "srgbClr" => Rgba::from_hex(n.attr("val")?),
        _ => None,
    }
}

const PALETTE: [Rgba; 1] = [Rgba::rgb(1, 1, 1)];

fn parse(chart_inner: &str, plot: &str) -> ChartModel {
    let env = ChartEnv {
        resolve_color: &resolver,
        palette: &PALETTE,
        minor_font: None,
        major_font: None,
    };
    let xml = format!(
        r#"<c:chartSpace {NS}><c:chart>{chart_inner}<c:plotArea>{plot}</c:plotArea></c:chart></c:chartSpace>"#
    );
    parse_chart(xml.as_bytes(), &env).expect("parses").model
}

const BAR3D: &str = r#"<c:bar3DChart><c:barDir val="col"/><c:grouping val="standard"/><c:varyColors val="0"/><c:gapDepth val="80"/><c:shape val="box"/><c:axId val="1"/><c:axId val="2"/></c:bar3DChart>"#;

#[test]
fn view3d_is_read_with_all_its_properties() {
    let m = parse(
        r#"<c:view3D><c:rotX val="25"/><c:rotY val="40"/><c:depthPercent val="150"/><c:hPercent val="80"/><c:rAngAx val="1"/><c:perspective val="45"/></c:view3D>"#,
        BAR3D,
    );
    assert!(m.three_d);
    assert_eq!(
        m.view3d,
        Some(View3D {
            rot_x: 25.0,
            rot_y: 40.0,
            r_ang_ax: true,
            perspective: 45.0,
            depth_percent: 150.0,
            h_percent: Some(80.0),
        })
    );
}

#[test]
fn missing_view3d_properties_take_the_defaults_and_wild_ones_are_clamped() {
    let m = parse(r#"<c:view3D/>"#, BAR3D);
    assert_eq!(m.view3d, Some(View3D::default()));
    let m = parse(
        r#"<c:view3D><c:rotX val="500"/><c:rotY val="-5"/><c:depthPercent val="1"/><c:perspective val="9999"/><c:hPercent val="0"/></c:view3D>"#,
        BAR3D,
    );
    let v = m.view3d.unwrap();
    assert_eq!((v.rot_x, v.rot_y), (90.0, 0.0));
    assert_eq!(v.depth_percent, 20.0);
    assert_eq!(v.perspective, 240.0);
    assert_eq!(v.h_percent, Some(5.0));
    // not numbers: the defaults
    let m = parse(r#"<c:view3D><c:rotX val="x"/></c:view3D>"#, BAR3D);
    assert_eq!(m.view3d.unwrap().rot_x, View3D::default().rot_x);
}

#[test]
fn a_3d_group_without_view3d_is_three_d_with_no_view() {
    let m = parse("", BAR3D);
    assert!(m.three_d && m.groups[0].three_d);
    assert!(m.view3d.is_none());
    // a flat chart is neither
    let m = parse(
        "",
        r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:axId val="1"/><c:axId val="2"/></c:barChart>"#,
    );
    assert!(!m.three_d && m.view3d.is_none());
}

#[test]
fn gap_depth_is_read_and_bounded_and_defaults_to_150() {
    let m = parse("", BAR3D);
    assert_eq!(m.groups[0].gap_depth, 80.0);
    // a standard-grouping 3-D column is read as a clustered one
    assert_eq!(m.groups[0].grouping, Grouping::Clustered);
    let m = parse(
        "",
        r#"<c:bar3DChart><c:barDir val="col"/><c:gapDepth val="9999"/><c:axId val="1"/><c:axId val="2"/></c:bar3DChart>"#,
    );
    assert_eq!(m.groups[0].gap_depth, 500.0);
    let m = parse(
        "",
        r#"<c:bar3DChart><c:barDir val="col"/><c:axId val="1"/><c:axId val="2"/></c:bar3DChart>"#,
    );
    assert_eq!(m.groups[0].gap_depth, 150.0);
}

#[test]
fn walls_and_floor_are_read_from_their_sp_pr() {
    let m = parse(
        r#"<c:view3D/><c:floor><c:thickness val="0"/><c:spPr><a:solidFill><a:srgbClr val="D9D9D9"/></a:solidFill></c:spPr></c:floor><c:sideWall><c:spPr><a:noFill/><a:ln w="12700"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></c:spPr></c:sideWall><c:backWall><c:thickness val="0"/></c:backWall>"#,
        BAR3D,
    );
    assert_eq!(
        m.floor.unwrap().fill,
        Some(Fill::Solid(Rgba::rgb(0xD9, 0xD9, 0xD9)))
    );
    let side = m.side_wall.unwrap();
    assert!(side
        .line
        .as_ref()
        .is_some_and(|l| l.color == Some(Rgba::rgb(255, 0, 0))));
    assert!(!side.fill.as_ref().is_some_and(Fill::is_visible));
    // a wall element without spPr is there but empty
    assert_eq!(m.back_wall, Some(Wall::default()));
    // no element: no wall
    let m = parse("", BAR3D);
    assert!(m.floor.is_none() && m.side_wall.is_none() && m.back_wall.is_none());
}
