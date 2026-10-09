use super::super::model::{Fill, Geometry, Item, Line, Rgba, ShapeItem, Xfrm};
use super::*;
use crate::preview::office::docx_xml::{read_element, Budget, Node, Tree};
use crate::preview::office::fmt_xlsx::XmlReader;
use quick_xml::events::Event;

const NO_ADJ: &[(String, f64)] = &[];

fn adj(v: &[(&str, f64)]) -> Vec<(String, f64)> {
    v.iter().map(|(n, x)| (n.to_string(), *x)).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn pt_close(p: Pt, x: f64, y: f64) -> bool {
    close(p.x, x) && close(p.y, y)
}

fn node(xml: &str) -> Node {
    let mut rd = XmlReader::new(xml.as_bytes());
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (e, empty) = match rd.read_event_into(&mut buf).unwrap() {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => panic!("no element"),
            _ => continue,
        };
        let mut b = Budget::new(100_000, 1 << 20);
        match read_element(&mut rd, &e, empty, &mut b).unwrap() {
            Tree::Ok(n) => return n,
            Tree::TooBig => panic!("too big"),
        }
    }
}

fn geom(name: &str, a: &[(&str, f64)], w: f64, h: f64) -> ShapeGeom {
    preset(name, &adj(a), w, h).unwrap_or_else(|| panic!("no preset {name}"))
}

/// The points of the first path's `MoveTo` / `LineTo` commands.
fn line_pts(g: &ShapeGeom, path: usize) -> Vec<Pt> {
    g.paths[path]
        .cmds
        .iter()
        .filter_map(|c| match c {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => Some(*p),
            _ => None,
        })
        .collect()
}

fn all_coords_ok(g: &ShapeGeom) {
    let ok = |v: f64| v.is_finite() && v.abs() <= MAX_COORD;
    let okp = |p: &Pt| ok(p.x) && ok(p.y);
    for p in &g.paths {
        assert!(ok(p.w) && ok(p.h));
        for c in &p.cmds {
            let good = match c {
                PathCmd::MoveTo(a) | PathCmd::LineTo(a) => okp(a),
                PathCmd::QuadTo(a, b) => okp(a) && okp(b),
                PathCmd::CubicTo(a, b, c) => okp(a) && okp(b) && okp(c),
                PathCmd::ArcTo {
                    wr,
                    hr,
                    st_deg,
                    sw_deg,
                } => ok(*wr) && ok(*hr) && ok(*st_deg) && ok(*sw_deg),
                PathCmd::Close => true,
            };
            assert!(good, "bad command {c:?}");
        }
    }
    if let Some((l, t, r, b)) = g.text_rect {
        assert!(ok(l) && ok(t) && ok(r) && ok(b));
    }
    for (p, a) in &g.connections {
        assert!(okp(p) && ok(*a));
    }
}

// ---------------------------------------------------------------------------------------------
// formulas
// ---------------------------------------------------------------------------------------------

fn ev(f: &str) -> f64 {
    eval_formula(f, 1200.0, 600.0, &[])
}

#[test]
fn operators_basic() {
    assert_eq!(ev("val 7"), 7.0);
    assert_eq!(ev("val -7"), -7.0);
    assert_eq!(ev("*/ 10 20 4"), 50.0);
    assert_eq!(ev("*/ 10 20 0"), 0.0);
    assert_eq!(ev("+- 1 2 3"), 0.0);
    assert_eq!(ev("+- 10 5 3"), 12.0);
    assert_eq!(ev("+/ 1 2 3"), 1.0);
    assert_eq!(ev("+/ 1 2 0"), 0.0);
    assert_eq!(ev("?: 1 5 6"), 5.0);
    assert_eq!(ev("?: 0 5 6"), 6.0);
    assert_eq!(ev("?: -1 5 6"), 6.0);
    assert_eq!(ev("abs -5"), 5.0);
    assert_eq!(ev("abs 5"), 5.0);
    assert_eq!(ev("max 3 9"), 9.0);
    assert_eq!(ev("min 3 9"), 3.0);
    assert_eq!(ev("sqrt 16"), 4.0);
    assert_eq!(ev("sqrt -4"), 0.0);
}

#[test]
fn mod_is_the_length_of_three_components() {
    assert_eq!(ev("mod 3 4 0"), 5.0);
    assert_eq!(ev("mod 1 2 2"), 3.0);
    assert_eq!(ev("mod 0 0 0"), 0.0);
}

#[test]
fn pin_limits_the_middle_operand() {
    assert_eq!(ev("pin 0 5 10"), 5.0);
    assert_eq!(ev("pin 0 -5 10"), 0.0);
    assert_eq!(ev("pin 0 15 10"), 10.0);
    // x above z: the lower bound wins when y is below it, the upper bound otherwise
    assert_eq!(ev("pin 10 5 0"), 10.0);
    assert_eq!(ev("pin 10 20 0"), 0.0);
}

#[test]
fn angle_operators_use_sixty_thousandths_of_a_degree() {
    assert!(close(ev("cos 100 0"), 100.0));
    assert!(close(ev("cos 100 10800000"), -100.0));
    assert!(close(ev("cos 100 5400000"), 0.0));
    assert!(close(ev("sin 100 5400000"), 100.0));
    assert!(close(ev("sin 100 16200000"), -100.0));
    assert!(close(ev("tan 100 2700000"), 100.0));
    assert!(close(ev("tan 100 -2700000"), -100.0));
    assert!(close(ev("sin 100 21600000"), 0.0));
}

#[test]
fn at2_is_a_signed_angle_of_y_over_x() {
    assert!(close(ev("at2 1 1"), 2_700_000.0));
    assert!(close(ev("at2 1 0"), 0.0));
    assert!(close(ev("at2 0 1"), 5_400_000.0));
    assert!(close(ev("at2 -1 0"), 10_800_000.0));
    assert!(close(ev("at2 0 -1"), -5_400_000.0));
    assert!(close(ev("at2 -1 -1"), -8_100_000.0));
    assert_eq!(ev("at2 0 0"), 0.0);
}

#[test]
fn cat2_and_sat2_are_the_cosine_and_sine_of_the_atan2() {
    assert!(close(ev("cat2 100 1 1"), 100.0 * 0.5f64.sqrt()));
    assert!(close(ev("sat2 100 1 1"), 100.0 * 0.5f64.sqrt()));
    assert!(close(ev("cat2 100 0 1"), 0.0));
    assert!(close(ev("sat2 100 0 1"), 100.0));
    assert!(close(ev("cat2 100 -1 0"), -100.0));
    assert!(close(ev("sat2 100 0 -1"), -100.0));
    assert!(close(ev("cat2 100 0 0"), 100.0)); // atan2(0, 0) = 0
}

#[test]
fn bad_input_reads_as_zero() {
    assert_eq!(ev(""), 0.0);
    assert_eq!(ev("   "), 0.0);
    assert_eq!(ev("nope 1 2 3"), 0.0);
    assert_eq!(ev("val nosuchguide"), 0.0);
    assert_eq!(ev("+- 5"), 5.0); // missing operands are 0
    assert_eq!(ev("*/"), 0.0);
    assert_eq!(ev("val"), 0.0);
    assert_eq!(ev("*/ w h"), 0.0); // z missing: division by zero
    assert_eq!(ev("val nan"), 0.0);
    assert_eq!(ev("val inf"), 0.0);
    assert_eq!(ev("val 1e5"), 0.0); // not an integer literal of the specification
    assert_eq!(ev("val -"), 0.0);
    assert_eq!(ev("val +-"), 0.0);
    assert_eq!(ev("val 1.5.2"), 0.0);
    // extra operands are ignored
    assert_eq!(ev("val 3 4 5 6"), 3.0);
}

#[test]
fn huge_numbers_are_limited_and_finite() {
    let big = "9".repeat(120);
    assert_eq!(ev(&format!("val {big}")), MAX_VALUE);
    assert_eq!(ev(&format!("val -{big}")), -MAX_VALUE);
    // a product of two limited values is limited again, not infinite
    let sq = format!("*/ {big} {big} 1");
    assert_eq!(ev(&sq), MAX_VALUE);
    assert_eq!(ev(&format!("+- {big} {big} -{big}")), MAX_VALUE);
    assert!(ev("tan 1000000000 5400000").abs() <= MAX_VALUE);
    assert!(ev("sin 1000000000000 99999999999").is_finite());
    assert!(ev(&format!("mod {big} {big} {big}")) <= MAX_VALUE);
    // squares of ordinary coordinates must not be clamped (the presets multiply them)
    assert_eq!(ev("*/ 1000000000 1000000000 1"), 1e18);
    assert_eq!(ev("*/ 100000000000 100000000000 1"), 1e22);
}

#[test]
fn operands_are_names_or_literals() {
    let g = [("adj1", 25000.0), ("a", 2.0)];
    let f = |s: &str| eval_formula(s, 1200.0, 600.0, &g);
    assert_eq!(f("*/ w adj1 100000"), 300.0);
    assert_eq!(f("+- a a a"), 2.0);
    assert_eq!(f("val 3cd4"), 16_200_000.0); // a name that starts with a digit
    assert_eq!(f("val +5"), 5.0);
    assert_eq!(f("val 007"), 7.0);
    assert_eq!(f("val .5"), 0.5);
}

#[test]
fn a_guide_named_like_a_builtin_shadows_it() {
    assert_eq!(eval_formula("val w", 100.0, 50.0, &[("w", 7.0)]), 7.0);
}

#[test]
fn builtin_guides() {
    let (w, h) = (1200.0, 600.0);
    let f = |n: &str| eval_formula(&format!("val {n}"), w, h, &[]);
    let expect = [
        ("l", 0.0),
        ("t", 0.0),
        ("r", w),
        ("b", h),
        ("w", w),
        ("h", h),
        ("hc", 600.0),
        ("vc", 300.0),
        ("ss", 600.0),
        ("ls", 1200.0),
        ("wd2", 600.0),
        ("wd3", 400.0),
        ("wd4", 300.0),
        ("wd5", 240.0),
        ("wd6", 200.0),
        ("wd8", 150.0),
        ("wd10", 120.0),
        ("wd12", 100.0),
        ("wd32", 37.5),
        ("hd2", 300.0),
        ("hd3", 200.0),
        ("hd4", 150.0),
        ("hd5", 120.0),
        ("hd6", 100.0),
        ("hd8", 75.0),
        ("hd10", 60.0),
        ("hd12", 50.0),
        ("hd32", 600.0 / 32.0),
        ("ssd2", 300.0),
        ("ssd4", 150.0),
        ("ssd6", 100.0),
        ("ssd8", 75.0),
        ("ssd16", 37.5),
        ("ssd32", 600.0 / 32.0),
        ("cd2", 10_800_000.0),
        ("cd4", 5_400_000.0),
        ("cd8", 2_700_000.0),
        ("3cd4", 16_200_000.0),
        ("3cd8", 8_100_000.0),
        ("5cd8", 13_500_000.0),
        ("7cd8", 18_900_000.0),
    ];
    for (n, v) in expect {
        assert!(close(f(n), v), "{n}: {} != {v}", f(n));
    }
}

#[test]
fn non_finite_sizes_are_zero() {
    assert_eq!(eval_formula("val w", f64::NAN, 5.0, &[]), 0.0);
    assert_eq!(eval_formula("val h", 5.0, f64::INFINITY, &[]), 0.0);
    assert_eq!(eval_formula("val w", -5.0, 5.0, &[]), 0.0);
    assert_eq!(eval_formula("val w", 1e30, 5.0, &[]), MAX_COORD);
}

// ---------------------------------------------------------------------------------------------
// presets: all of them
// ---------------------------------------------------------------------------------------------

#[test]
fn there_are_187_distinct_presets_one_of_them_made_in_code() {
    let names = preset_names();
    // the 186 of the Ecma file plus `upArrow`, which the file lacks
    assert_eq!(names.len(), 187, "{names:?}");
    for n in [
        "rect",
        "ellipse",
        "line",
        "upDownArrow",
        "upArrow",
        "wedgeRectCallout",
        "star5",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    assert!(preset("noSuchShape", NO_ADJ, 100.0, 100.0).is_none());
    assert!(preset("", NO_ADJ, 100.0, 100.0).is_none());
    assert!(preset("Rect", NO_ADJ, 100.0, 100.0).is_none()); // case matters
}

#[test]
fn the_embedded_file_is_the_ecma_file() {
    // byte-identical to what was downloaded (size and shape checked; the licence forbids edits)
    let xml = include_bytes!("presets/presetShapeDefinitions.xml");
    assert_eq!(xml.len(), 559_368);
    assert_eq!(&xml[..3], b"\xEF\xBB\xBF");
    let text = std::str::from_utf8(xml).unwrap();
    assert!(text.contains("<presetShapeDefinitons>"));
    let notice = include_str!("presets/NOTICE.md");
    assert!(notice.contains("© Ecma International"));
    assert!(notice.contains("not under konoma's MIT licence"));
    assert!(notice.contains("ECMA INTERNATIONAL DISCLAIMS ALL WARRANTIES"));
}

#[test]
fn up_down_arrow_keeps_the_first_definition() {
    // the file defines it twice, identically; there is exactly one entry and it works
    let g = geom("upDownArrow", &[], 1000.0, 1000.0);
    assert_eq!(line_pts(&g, 0).len(), 10);
}

/// Every value of `ST_ShapeType` (ECMA-376 Part 1, 20.1.10.56), as listed in the schema.
const SCHEMA_SHAPE_TYPES: &str = "line lineInv triangle rtTriangle rect diamond parallelogram trapezoid nonIsoscelesTrapezoid pentagon hexagon heptagon octagon decagon dodecagon star4 star5 star6 star7 star8 star10 star12 star16 star24 star32 roundRect round1Rect round2SameRect round2DiagRect snipRoundRect snip1Rect snip2SameRect snip2DiagRect plaque ellipse teardrop homePlate chevron pieWedge pie blockArc donut noSmoking rightArrow leftArrow upArrow downArrow stripedRightArrow notchedRightArrow bentUpArrow leftRightArrow upDownArrow leftUpArrow leftRightUpArrow quadArrow leftArrowCallout rightArrowCallout upArrowCallout downArrowCallout leftRightArrowCallout upDownArrowCallout quadArrowCallout bentArrow uturnArrow circularArrow leftCircularArrow leftRightCircularArrow curvedRightArrow curvedLeftArrow curvedUpArrow curvedDownArrow swooshArrow cube can lightningBolt heart sun moon smileyFace irregularSeal1 irregularSeal2 foldedCorner bevel frame halfFrame corner diagStripe chord arc leftBracket rightBracket leftBrace rightBrace bracketPair bracePair straightConnector1 bentConnector2 bentConnector3 bentConnector4 bentConnector5 curvedConnector2 curvedConnector3 curvedConnector4 curvedConnector5 callout1 callout2 callout3 accentCallout1 accentCallout2 accentCallout3 borderCallout1 borderCallout2 borderCallout3 accentBorderCallout1 accentBorderCallout2 accentBorderCallout3 wedgeRectCallout wedgeRoundRectCallout wedgeEllipseCallout cloudCallout cloud ribbon ribbon2 ellipseRibbon ellipseRibbon2 leftRightRibbon verticalScroll horizontalScroll wave doubleWave plus flowChartProcess flowChartDecision flowChartInputOutput flowChartPredefinedProcess flowChartInternalStorage flowChartDocument flowChartMultidocument flowChartTerminator flowChartPreparation flowChartManualInput flowChartManualOperation flowChartConnector flowChartPunchedCard flowChartPunchedTape flowChartSummingJunction flowChartOr flowChartCollate flowChartSort flowChartExtract flowChartMerge flowChartOfflineStorage flowChartOnlineStorage flowChartMagneticTape flowChartMagneticDisk flowChartMagneticDrum flowChartDisplay flowChartDelay flowChartAlternateProcess flowChartOffpageConnector actionButtonBlank actionButtonHome actionButtonHelp actionButtonInformation actionButtonForwardNext actionButtonBackPrevious actionButtonEnd actionButtonBeginning actionButtonReturn actionButtonDocument actionButtonSound actionButtonMovie gear6 gear9 funnel mathPlus mathMinus mathMultiply mathDivide mathEqual mathNotEqual cornerTabs squareTabs plaqueTabs chartX chartStar chartPlus";

#[test]
fn every_value_of_the_schemas_shape_type_is_a_preset() {
    let names = preset_names();
    let schema: Vec<&str> = SCHEMA_SHAPE_TYPES.split(' ').collect();
    assert_eq!(schema.len(), 187);
    for n in &schema {
        assert!(names.contains(n), "{n} is not a preset");
    }
    assert_eq!(
        names.len(),
        schema.len(),
        "a preset the schema does not know"
    );
}

#[test]
fn up_arrow_is_the_down_arrow_turned_upside_down() {
    // same adjust names and defaults
    assert_eq!(
        preset_adjust_names("upArrow"),
        preset_adjust_names("downArrow")
    );
    for &(w, h) in &[
        (1_000_000.0, 1_000_000.0),
        (400_000.0, 1_600_000.0),
        (1_600_000.0, 400_000.0),
    ] {
        for a in [
            vec![],
            vec![("adj1", 20_000.0), ("adj2", 80_000.0)],
            vec![("adj1", 100_000.0), ("adj2", 0.0)],
            vec![("adj1", 70_000.0), ("adj2", 1e9)],
        ] {
            let down = geom("downArrow", &a, w, h);
            let up = geom("upArrow", &a, w, h);
            assert_eq!(up.paths.len(), down.paths.len());
            let (d, u) = (line_pts(&down, 0), line_pts(&up, 0));
            assert_eq!(d.len(), u.len());
            for (p, q) in d.iter().zip(&u) {
                assert!(
                    close(p.x, q.x) && close(h - p.y, q.y),
                    "{p:?} {q:?} {w}x{h}"
                );
            }
            // the text rectangle: same left and right, top and bottom swapped about the middle
            let (dl, dt, dr, db) = down.text_rect.unwrap();
            let (ul, ut, ur, ub) = up.text_rect.unwrap();
            assert!(close(dl, ul) && close(dr, ur), "{w}x{h}");
            assert!(close(h - db, ut) && close(h - dt, ub), "{w}x{h}");
            // connection sites: mirrored position and angle
            assert_eq!(down.connections.len(), up.connections.len());
            for ((dp, da), (upp, ua)) in down.connections.iter().zip(&up.connections) {
                assert!(close(dp.x, upp.x) && close(h - dp.y, upp.y));
                assert!(
                    close((360.0 - da).rem_euclid(360.0), ua.rem_euclid(360.0)),
                    "{da} {ua}"
                );
            }
        }
    }
    // the tip points up: a point at the middle of the top edge, and the shaft reaches the bottom
    let g = geom("upArrow", &[], 1_000_000.0, 1_000_000.0);
    let pts = line_pts(&g, 0);
    assert!(pts.iter().any(|p| pt_close(*p, 500_000.0, 0.0)));
    assert!(pts.iter().any(|p| close(p.y, 1_000_000.0)));
}

#[test]
fn mirroring_a_spec_negates_arc_angles_and_uses_the_path_space() {
    let spec = CustomGeomSpec {
        paths: vec![PathSpec {
            w: 100.0,
            h: 40.0,
            cmds: vec![
                CmdSpec::Move(("0".into(), "10".into())),
                CmdSpec::Arc {
                    wr: "20".into(),
                    hr: "10".into(),
                    st_ang: "cd2".into(),
                    sw_ang: "cd4".into(),
                },
                CmdSpec::Quad(("5".into(), "5".into()), ("6".into(), "30".into())),
                CmdSpec::Cubic(
                    ("1".into(), "2".into()),
                    ("3".into(), "4".into()),
                    ("5".into(), "6".into()),
                ),
                CmdSpec::Close,
            ],
            ..PathSpec::default()
        }],
        text_rect: Some(["l".into(), "10".into(), "r".into(), "20".into()]),
        ..CustomGeomSpec::default()
    };
    let mirrored = super::super::geom_xml::mirrored_vertically(&spec);
    let a = custom(&spec, 1000.0, 400.0).unwrap();
    let b = custom(&mirrored, 1000.0, 400.0).unwrap();
    assert_eq!(b.paths[0].cmds.len(), a.paths[0].cmds.len());
    // path space: y of the move, 10 of 40, becomes 30 of 40
    match (a.paths[0].cmds[0], b.paths[0].cmds[0]) {
        (PathCmd::MoveTo(p), PathCmd::MoveTo(q)) => {
            assert!(close(p.x, q.x) && close(q.y, 40.0 - p.y), "{p:?} {q:?}");
        }
        other => panic!("{other:?}"),
    }
    match (a.paths[0].cmds[1], b.paths[0].cmds[1]) {
        (
            PathCmd::ArcTo {
                st_deg: s1,
                sw_deg: w1,
                ..
            },
            PathCmd::ArcTo {
                st_deg: s2,
                sw_deg: w2,
                ..
            },
        ) => {
            assert!(close(s1, 180.0) && close(w1, 90.0));
            assert!(close(s2, -180.0) && close(w2, -90.0), "{s2} {w2}");
        }
        other => panic!("{other:?}"),
    }
    match (a.paths[0].cmds[3], b.paths[0].cmds[3]) {
        (PathCmd::CubicTo(a1, a2, a3), PathCmd::CubicTo(b1, b2, b3)) => {
            for (p, q) in [(a1, b1), (a2, b2), (a3, b3)] {
                assert!(close(p.x, q.x) && close(40.0 - p.y, q.y));
            }
        }
        other => panic!("{other:?}"),
    }
    // text rectangle (box space): top 10 / bottom 20 of 400 become 380 / 390
    let (_, t, _, bt) = b.text_rect.unwrap();
    assert!(close(t, 380.0) && close(bt, 390.0), "{t} {bt}");
}

const SIZES: &[(f64, f64)] = &[
    (1_000_000.0, 1_000_000.0),
    (3_000_000.0, 1_000_000.0),
    (1_000_000.0, 3_000_000.0),
    (1.0, 1.0),
    (0.0, 0.0),
    (0.0, 1_000_000.0),
    (1_000_000.0, 0.0),
    (1e9, 1e9),
    (1e9, 7.0),
    (7.0, 1e9),
    (f64::NAN, f64::INFINITY),
    (-5.0, -5.0),
];

#[test]
fn every_preset_evaluates_cleanly_at_every_size() {
    for name in preset_names() {
        for &(w, h) in SIZES {
            let g = preset(name, NO_ADJ, w, h).unwrap_or_else(|| panic!("{name} {w}x{h}"));
            all_coords_ok(&g);
            assert!(!g.truncated, "{name}");
        }
    }
}

#[test]
fn every_preset_has_a_path_and_a_text_rect_or_is_a_line() {
    for name in preset_names() {
        let g = geom(name, &[], 1_000_000.0, 600_000.0);
        assert!(!g.paths.is_empty(), "{name} has no path");
        for p in &g.paths {
            assert!(!p.cmds.is_empty(), "{name} has an empty path");
        }
    }
}

#[test]
fn no_bundled_preset_uses_an_unknown_name_or_operator() {
    for name in preset_names() {
        let spec = super::super::geom_xml::preset_spec(name).unwrap();
        for &(w, h) in &[(1_000_000.0, 600_000.0), (0.0, 0.0)] {
            let (_, unknown) = spec.eval_counting(w, h, NO_ADJ).unwrap();
            // the Ecma file has one typo: curvedLeftArrow's third connection site says
            // ang="cd3", which is no guide name (read as 0, like any unknown name)
            let want = usize::from(name == "curvedLeftArrow");
            assert_eq!(unknown, want, "{name} {w}x{h}");
        }
    }
}

#[test]
fn every_preset_survives_extreme_adjust_values() {
    let extremes = [
        0.0,
        -1.0,
        1.0,
        50_000.0,
        -2_147_483_648.0,
        2_147_483_647.0,
        1e18,
        -1e18,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        21_600_000.0,
        -21_600_000.0,
    ];
    for name in preset_names() {
        let spec = super::super::geom_xml::preset_spec(name).unwrap();
        let names: Vec<String> = spec.adjusts.iter().map(|(n, _)| n.clone()).collect();
        for &(w, h) in &[(1_000_000.0, 600_000.0), (1.0, 1.0), (0.0, 0.0), (1e9, 1e9)] {
            for &x in &extremes {
                let all: Vec<(String, f64)> = names.iter().map(|n| (n.clone(), x)).collect();
                let g = preset(name, &all, w, h).unwrap();
                all_coords_ok(&g);
                // one at a time, the others at their defaults
                for n in &names {
                    let g = preset(name, &[(n.clone(), x)], w, h).unwrap();
                    all_coords_ok(&g);
                }
            }
        }
    }
}

#[test]
fn an_override_that_is_not_finite_keeps_the_default() {
    let a = geom("roundRect", &[], 1000.0, 1000.0);
    let b = geom("roundRect", &[("adj", f64::NAN)], 1000.0, 1000.0);
    assert_eq!(a, b);
}

#[test]
fn an_unknown_adjust_name_is_ignored() {
    let a = geom("roundRect", &[], 1000.0, 1000.0);
    let b = geom("roundRect", &[("nosuch", 5.0)], 1000.0, 1000.0);
    assert_eq!(a, b);
}

#[test]
fn evaluation_is_deterministic() {
    for name in ["cloud", "star24", "wedgeEllipseCallout", "gear9"] {
        let a = geom(name, &[], 1_234_567.0, 765_432.0);
        let b = geom(name, &[], 1_234_567.0, 765_432.0);
        assert_eq!(a, b);
    }
}

// ---------------------------------------------------------------------------------------------
// presets: exact values worked out from the definitions
// ---------------------------------------------------------------------------------------------

#[test]
fn rect_is_the_box() {
    let g = geom("rect", &[], 1000.0, 500.0);
    let p = line_pts(&g, 0);
    assert_eq!(p.len(), 4);
    assert!(pt_close(p[0], 0.0, 0.0));
    assert!(pt_close(p[1], 1000.0, 0.0));
    assert!(pt_close(p[2], 1000.0, 500.0));
    assert!(pt_close(p[3], 0.0, 500.0));
    assert_eq!(g.text_rect, Some((0.0, 0.0, 1000.0, 500.0)));
    assert!(g.paths[0].fill_mode == PathFill::Norm && g.paths[0].stroke);
    assert_eq!((g.paths[0].w, g.paths[0].h), (0.0, 0.0));
}

#[test]
fn round_rect_corner_radius_is_a_fraction_of_the_short_side() {
    // w 1,000,000 x h 500,000: ss = 500,000; adj 16667 -> x1 = ss * 16667 / 100000 = 83,335
    let g = geom("roundRect", &[], 1_000_000.0, 500_000.0);
    let c = &g.paths[0].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 83_335.0)));
    match c[1] {
        PathCmd::ArcTo {
            wr,
            hr,
            st_deg,
            sw_deg,
        } => assert!(
            close(wr, 83_335.0)
                && close(hr, 83_335.0)
                && close(st_deg, 180.0)
                && close(sw_deg, 90.0)
        ),
        ref o => panic!("{o:?}"),
    }
    assert!(matches!(c[2], PathCmd::LineTo(p) if pt_close(p, 916_665.0, 0.0)));
    assert!(matches!(c[3], PathCmd::ArcTo { st_deg, .. } if close(st_deg, 270.0)));
    assert!(matches!(c[4], PathCmd::LineTo(p) if pt_close(p, 1_000_000.0, 416_665.0)));
    assert!(matches!(c[5], PathCmd::ArcTo { st_deg, .. } if close(st_deg, 0.0)));
    assert!(matches!(c[6], PathCmd::LineTo(p) if pt_close(p, 83_335.0, 500_000.0)));
    assert!(matches!(c[7], PathCmd::ArcTo { st_deg, .. } if close(st_deg, 90.0)));
    assert!(matches!(c[8], PathCmd::Close));
    // text rectangle: il = x1 * 29289 / 100000
    let il = 83_335.0 * 29_289.0 / 100_000.0;
    let (l, t, r, b) = g.text_rect.unwrap();
    assert!(close(l, il) && close(t, il) && close(r, 1_000_000.0 - il) && close(b, 500_000.0 - il));
}

#[test]
fn round_rect_adjust_is_pinned_to_half() {
    // adj 50000 -> radius = half the short side; larger values are pinned there
    for a in [50_000.0, 80_000.0, 1e12] {
        let g = geom("roundRect", &[("adj", a)], 1_000_000.0, 500_000.0);
        assert!(
            matches!(g.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 250_000.0)),
            "{a}"
        );
    }
    let g = geom("roundRect", &[("adj", -5.0)], 1_000_000.0, 500_000.0);
    assert!(matches!(g.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 0.0)));
}

#[test]
fn triangle_apex_follows_the_adjust() {
    let g = geom("triangle", &[], 1_000_000.0, 800_000.0);
    let p = line_pts(&g, 0);
    assert!(pt_close(p[0], 0.0, 800_000.0));
    assert!(pt_close(p[1], 500_000.0, 0.0));
    assert!(pt_close(p[2], 1_000_000.0, 800_000.0));
    let g = geom("triangle", &[("adj", 25_000.0)], 1_000_000.0, 800_000.0);
    let p = line_pts(&g, 0);
    assert!(pt_close(p[1], 250_000.0, 0.0));
    // text rectangle: x1 = w * a / 200000 = 125,000; r = x1 + wd2; t = vc
    assert_eq!(
        g.text_rect,
        Some((125_000.0, 400_000.0, 625_000.0, 800_000.0))
    );
    let g = geom("triangle", &[("adj", 200_000.0)], 1_000_000.0, 800_000.0);
    assert!(pt_close(line_pts(&g, 0)[1], 1_000_000.0, 0.0)); // pinned to 100000
}

#[test]
fn right_arrow_points() {
    // w 1,000,000 x h 500,000, adj1 = adj2 = 50000: dx1 = ss * a2 / 1e5 = 250,000;
    // x1 = 750,000; dy1 = h * a1 / 200000 = 125,000; y1 = 125,000; y2 = 375,000
    let g = geom("rightArrow", &[], 1_000_000.0, 500_000.0);
    let p = line_pts(&g, 0);
    let want = [
        (0.0, 125_000.0),
        (750_000.0, 125_000.0),
        (750_000.0, 0.0),
        (1_000_000.0, 250_000.0),
        (750_000.0, 500_000.0),
        (750_000.0, 375_000.0),
        (0.0, 375_000.0),
    ];
    assert_eq!(p.len(), want.len());
    for (q, (x, y)) in p.iter().zip(want) {
        assert!(pt_close(*q, x, y), "{q:?} != {x},{y}");
    }
    // the head cannot be longer than the shape: adj2 is pinned to maxAdj2 = 100000 * w / ss
    let g = geom("rightArrow", &[("adj2", 1e9)], 1_000_000.0, 500_000.0);
    assert!(pt_close(line_pts(&g, 0)[1], 0.0, 125_000.0));
}

#[test]
fn star5_first_vertex_and_symmetry() {
    // w = h = 1,000,000: swd2 = 525,730; dx1 = swd2 cos 18deg = 500,000 (the left tip is at x 0);
    // shd2 = 552,785; svc = 552,785; dy1 = shd2 sin 18deg; y1 = svc - dy1 = 381,966
    let g = geom("star5", &[], 1_000_000.0, 1_000_000.0);
    let p = line_pts(&g, 0);
    assert_eq!(p.len(), 10);
    assert!(
        p[0].x.abs() < 2.0 && (p[0].y - 381_966.0).abs() < 2.0,
        "{:?}",
        p[0]
    );
    assert!(pt_close(p[2], 500_000.0, 0.0)); // the top tip
    for i in 0..10 {
        let q = p[i];
        let m = p[(14 - i) % 10]; // mirror about the vertical axis (vertex 2 is the top tip)
        assert!(
            (q.x + m.x - 1_000_000.0).abs() < 2.0 && (q.y - m.y).abs() < 2.0,
            "{i}: {q:?} {m:?}"
        );
    }
}

#[test]
fn ellipse_is_four_quarter_arcs() {
    let g = geom("ellipse", &[], 1000.0, 600.0);
    let c = &g.paths[0].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 300.0)));
    for (i, st) in [180.0, 270.0, 0.0, 90.0].iter().enumerate() {
        match c[i + 1] {
            PathCmd::ArcTo {
                wr,
                hr,
                st_deg,
                sw_deg,
            } => assert!(
                close(wr, 500.0) && close(hr, 300.0) && close(st_deg, *st) && close(sw_deg, 90.0)
            ),
            ref o => panic!("{o:?}"),
        }
    }
    assert!(matches!(c[5], PathCmd::Close));
    // inscribed text rectangle: the box shrunk by cos 45deg
    let d = 500.0 * 0.5f64.sqrt();
    let e = 300.0 * 0.5f64.sqrt();
    let (l, t, r, b) = g.text_rect.unwrap();
    assert!(
        close(l, 500.0 - d) && close(r, 500.0 + d) && close(t, 300.0 - e) && close(b, 300.0 + e)
    );
}

#[test]
fn chevron_points() {
    // w 1,000,000 x h 500,000, adj 50000: x1 = ss * a / 1e5 = 250,000; x2 = r - x1 = 750,000
    let g = geom("chevron", &[], 1_000_000.0, 500_000.0);
    let p = line_pts(&g, 0);
    let want = [
        (0.0, 0.0),
        (750_000.0, 0.0),
        (1_000_000.0, 250_000.0),
        (750_000.0, 500_000.0),
        (0.0, 500_000.0),
        (250_000.0, 250_000.0),
    ];
    assert_eq!(p.len(), 6);
    for (q, (x, y)) in p.iter().zip(want) {
        assert!(pt_close(*q, x, y), "{q:?}");
    }
    // dx = x2 - x1 = 500,000 > 0: text from x1 to x2
    let (l, _, r, _) = g.text_rect.unwrap();
    assert!(close(l, 250_000.0) && close(r, 750_000.0));
}

#[test]
fn wedge_rect_callout_tip_and_notch() {
    // w 1,000,000 x h 500,000, adj1 -20833, adj2 62500: dxPos = -208,330, dyPos = 312,500;
    // the tip is below the box (dz > 0, dyPos > 0): (hc + dxPos, vc + dyPos) = (291,670; 562,500)
    let g = geom("wedgeRectCallout", &[], 1_000_000.0, 500_000.0);
    let p = line_pts(&g, 0);
    assert_eq!(p.len(), 16);
    assert!(
        p.iter().any(|q| pt_close(*q, 291_670.0, 562_500.0)),
        "{p:?}"
    );
    // the notch on the bottom edge: x1 = w * 2/12 and x2 = w * 5/12 (dxPos < 0)
    assert!(p
        .iter()
        .any(|q| pt_close(*q, 1_000_000.0 * 5.0 / 12.0, 500_000.0)));
    assert!(p
        .iter()
        .any(|q| pt_close(*q, 1_000_000.0 * 2.0 / 12.0, 500_000.0)));
    // the tip to the right of the box: the tip moves to the right edge
    let g = geom(
        "wedgeRectCallout",
        &[("adj1", 80_000.0), ("adj2", 0.0)],
        1_000_000.0,
        500_000.0,
    );
    let p = line_pts(&g, 0);
    assert!(
        p.iter().any(|q| pt_close(*q, 1_300_000.0, 250_000.0)),
        "{p:?}"
    );
}

#[test]
fn bent_connector3_follows_its_adjust() {
    let g = geom(
        "bentConnector3",
        &[("adj1", 25_000.0)],
        1_000_000.0,
        400_000.0,
    );
    let p = line_pts(&g, 0);
    let want = [
        (0.0, 0.0),
        (250_000.0, 0.0),
        (250_000.0, 400_000.0),
        (1_000_000.0, 400_000.0),
    ];
    assert_eq!(p.len(), 4);
    for (q, (x, y)) in p.iter().zip(want) {
        assert!(pt_close(*q, x, y));
    }
    assert_eq!(g.paths[0].fill_mode, PathFill::None);
    // the adjust is not clamped by this preset: a bend outside the box is legal
    let g = geom(
        "bentConnector3",
        &[("adj1", 150_000.0)],
        1_000_000.0,
        400_000.0,
    );
    assert!(pt_close(line_pts(&g, 0)[1], 1_500_000.0, 0.0));
}

#[test]
fn curved_connector3_control_points() {
    let g = geom("curvedConnector3", &[], 1_000_000.0, 400_000.0);
    let c = &g.paths[0].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 0.0)));
    match c[1] {
        PathCmd::CubicTo(a, b, e) => {
            assert!(pt_close(a, 250_000.0, 0.0));
            assert!(pt_close(b, 500_000.0, 100_000.0));
            assert!(pt_close(e, 500_000.0, 200_000.0));
        }
        ref o => panic!("{o:?}"),
    }
    match c[2] {
        PathCmd::CubicTo(a, b, e) => {
            assert!(pt_close(a, 500_000.0, 300_000.0));
            assert!(pt_close(b, 750_000.0, 400_000.0));
            assert!(pt_close(e, 1_000_000.0, 400_000.0));
        }
        ref o => panic!("{o:?}"),
    }
    let g = geom(
        "curvedConnector3",
        &[("adj1", 20_000.0)],
        1_000_000.0,
        400_000.0,
    );
    assert!(
        matches!(g.paths[0].cmds[1], PathCmd::CubicTo(a, _, e) if pt_close(a, 100_000.0, 0.0) && pt_close(e, 200_000.0, 200_000.0))
    );
}

#[test]
fn donut_has_an_outer_and_a_counter_clockwise_inner_ring() {
    // w 1,000,000 x h 500,000, adj 25000: dr = ss * a / 1e5 = 125,000
    let g = geom("donut", &[], 1_000_000.0, 500_000.0);
    let c = &g.paths[0].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 250_000.0)));
    assert!(matches!(c[5], PathCmd::Close));
    assert!(matches!(c[6], PathCmd::MoveTo(p) if pt_close(p, 125_000.0, 250_000.0)));
    match c[7] {
        PathCmd::ArcTo {
            wr,
            hr,
            st_deg,
            sw_deg,
        } => assert!(
            close(wr, 375_000.0)
                && close(hr, 125_000.0)
                && close(st_deg, 180.0)
                && close(sw_deg, -90.0)
        ),
        ref o => panic!("{o:?}"),
    }
    assert_eq!(c.len(), 12);
}

#[test]
fn arc_preset_default_is_the_top_right_quarter() {
    let g = geom("arc", &[], 1_000_000.0, 600_000.0);
    assert_eq!(g.paths.len(), 2);
    // the filled wedge has no stroke, the open arc has no fill
    assert!(!g.paths[0].stroke && g.paths[0].fill_mode == PathFill::Norm);
    assert!(g.paths[1].stroke && g.paths[1].fill_mode == PathFill::None);
    let c = &g.paths[1].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 500_000.0, 0.0)));
    match c[1] {
        PathCmd::ArcTo {
            wr,
            hr,
            st_deg,
            sw_deg,
        } => assert!(
            close(wr, 500_000.0)
                && close(hr, 300_000.0)
                && close(st_deg, 270.0)
                && close(sw_deg, 90.0)
        ),
        ref o => panic!("{o:?}"),
    }
    // start 90deg, end 180deg: a quarter again, starting at the bottom
    let g = geom(
        "arc",
        &[("adj1", 5_400_000.0), ("adj2", 10_800_000.0)],
        1_000_000.0,
        600_000.0,
    );
    match g.paths[1].cmds[1] {
        PathCmd::ArcTo { st_deg, sw_deg, .. } => {
            assert!(close(st_deg, 90.0) && close(sw_deg, 90.0))
        }
        ref o => panic!("{o:?}"),
    }
    assert!(matches!(g.paths[1].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 500_000.0, 600_000.0)));
    // end before start wraps by a full turn
    let g = geom(
        "arc",
        &[("adj1", 10_800_000.0), ("adj2", 5_400_000.0)],
        1_000_000.0,
        600_000.0,
    );
    match g.paths[1].cmds[1] {
        PathCmd::ArcTo { sw_deg, .. } => assert!(close(sw_deg, 270.0)),
        ref o => panic!("{o:?}"),
    }
}

#[test]
fn pie_default_is_three_quarters() {
    let g = geom("pie", &[], 1_000_000.0, 600_000.0);
    let c = &g.paths[0].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 1_000_000.0, 300_000.0)));
    match c[1] {
        PathCmd::ArcTo { st_deg, sw_deg, .. } => {
            assert!(close(st_deg, 0.0) && close(sw_deg, 270.0))
        }
        ref o => panic!("{o:?}"),
    }
    assert!(matches!(c[2], PathCmd::LineTo(p) if pt_close(p, 500_000.0, 300_000.0)));
    assert!(matches!(c[3], PathCmd::Close));
    let g = geom(
        "pie",
        &[("adj1", 5_400_000.0), ("adj2", 10_800_000.0)],
        1_000_000.0,
        600_000.0,
    );
    match g.paths[0].cmds[1] {
        PathCmd::ArcTo { st_deg, sw_deg, .. } => {
            assert!(close(st_deg, 90.0) && close(sw_deg, 90.0))
        }
        ref o => panic!("{o:?}"),
    }
}

#[test]
fn preset_paths_keep_their_own_coordinate_space() {
    // the cloud is drawn on a 43200 x 43200 grid
    let g = geom("cloud", &[], 2_000_000.0, 1_000_000.0);
    assert_eq!((g.paths[0].w, g.paths[0].h), (43_200.0, 43_200.0));
    assert!(matches!(g.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 3900.0, 14370.0)));
}

#[test]
fn lighten_and_darken_path_modes_are_read() {
    let g = geom("cube", &[], 1_000_000.0, 1_000_000.0);
    let modes: Vec<PathFill> = g.paths.iter().map(|p| p.fill_mode).collect();
    assert!(
        modes.contains(&PathFill::Darken) || modes.contains(&PathFill::DarkenLess),
        "{modes:?}"
    );
    assert!(
        modes.contains(&PathFill::LightenLess) || modes.contains(&PathFill::Lighten),
        "{modes:?}"
    );
}

#[test]
fn connection_sites_carry_a_direction_in_degrees() {
    let g = geom("rect", &[], 1000.0, 600.0);
    assert_eq!(g.connections.len(), 4);
    assert!(g
        .connections
        .iter()
        .any(|(p, a)| pt_close(*p, 1000.0, 300.0) && close(*a, 0.0)));
    assert!(g
        .connections
        .iter()
        .any(|(p, a)| pt_close(*p, 500.0, 600.0) && close(*a, 90.0)));
    assert!(g
        .connections
        .iter()
        .any(|(p, a)| pt_close(*p, 0.0, 300.0) && close(*a, 180.0)));
    assert!(g
        .connections
        .iter()
        .any(|(p, a)| pt_close(*p, 500.0, 0.0) && close(*a, 270.0)));
}

// ---------------------------------------------------------------------------------------------
// custom geometry
// ---------------------------------------------------------------------------------------------

const POLY: &str = r#"<a:custGeom xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/>
  <a:rect l="0" t="0" r="r" b="b"/>
  <a:pathLst>
    <a:path w="100" h="50">
      <a:moveTo><a:pt x="0" y="0"/></a:moveTo>
      <a:lnTo><a:pt x="100" y="0"/></a:lnTo>
      <a:lnTo><a:pt x="50" y="50"/></a:lnTo>
      <a:close/>
    </a:path>
  </a:pathLst>
</a:custGeom>"#;

#[test]
fn custom_polygon_keeps_its_path_space() {
    let spec = custom_from_xml(&node(POLY)).unwrap();
    let g = custom(&spec, 2000.0, 1000.0).unwrap();
    assert_eq!(g.paths.len(), 1);
    assert_eq!((g.paths[0].w, g.paths[0].h), (100.0, 50.0));
    assert!(g.paths[0].stroke);
    assert_eq!(g.paths[0].fill_mode, PathFill::Norm);
    let p = line_pts(&g, 0);
    assert!(pt_close(p[1], 100.0, 0.0) && pt_close(p[2], 50.0, 50.0));
    assert!(matches!(g.paths[0].cmds[3], PathCmd::Close));
    assert_eq!(g.text_rect, Some((0.0, 0.0, 2000.0, 1000.0)));
    assert!(g.connections.is_empty());
}

#[test]
fn custom_bezier_and_path_attributes() {
    let n = node(
        r#"<a:custGeom><a:pathLst>
        <a:path w="10" h="10" fill="darkenLess" stroke="0" extrusionOk="false">
          <a:moveTo><a:pt x="0" y="0"/></a:moveTo>
          <a:quadBezTo><a:pt x="5" y="10"/><a:pt x="10" y="0"/></a:quadBezTo>
          <a:cubicBezTo><a:pt x="1" y="2"/><a:pt x="3" y="4"/><a:pt x="5" y="6"/></a:cubicBezTo>
        </a:path>
        <a:path fill="none"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="1" y="1"/></a:lnTo></a:path>
        </a:pathLst></a:custGeom>"#,
    );
    let spec = custom_from_xml(&n).unwrap();
    assert!(!spec.paths[0].stroke && !spec.paths[0].extrusion_ok);
    let g = custom(&spec, 100.0, 100.0).unwrap();
    assert_eq!(g.paths[0].fill_mode, PathFill::DarkenLess);
    assert!(!g.paths[0].stroke);
    assert!(
        matches!(g.paths[0].cmds[1], PathCmd::QuadTo(a, b) if pt_close(a, 5.0, 10.0) && pt_close(b, 10.0, 0.0))
    );
    assert!(
        matches!(g.paths[0].cmds[2], PathCmd::CubicTo(a, b, c) if pt_close(a, 1.0, 2.0) && pt_close(b, 3.0, 4.0) && pt_close(c, 5.0, 6.0))
    );
    assert_eq!(g.paths[1].fill_mode, PathFill::None);
    assert_eq!((g.paths[1].w, g.paths[1].h), (0.0, 0.0));
    assert_eq!(g.text_rect, None);
}

#[test]
fn custom_arcs_with_guides_and_adjusts() {
    // a half-disc: radius = adjust, centred on the bottom edge
    let n = node(
        r#"<a:custGeom>
        <a:avLst><a:gd name="adj" fmla="val 25000"/></a:avLst>
        <a:gdLst>
          <a:gd name="rad" fmla="*/ ss adj 100000"/>
          <a:gd name="half" fmla="*/ rad 1 2"/>
          <a:gd name="ang" fmla="val 10800000"/>
        </a:gdLst>
        <a:cxnLst><a:cxn ang="cd4"><a:pos x="hc" y="b"/></a:cxn></a:cxnLst>
        <a:rect l="half" t="half" r="rad" b="b"/>
        <a:pathLst><a:path>
          <a:moveTo><a:pt x="l" y="b"/></a:moveTo>
          <a:arcTo wR="rad" hR="half" stAng="ang" swAng="cd2"/>
          <a:close/>
        </a:path></a:pathLst></a:custGeom>"#,
    );
    let spec = custom_from_xml(&n).unwrap();
    let g = custom(&spec, 1000.0, 800.0).unwrap();
    // ss = 800; rad = 200; half = 100
    match g.paths[0].cmds[1] {
        PathCmd::ArcTo {
            wr,
            hr,
            st_deg,
            sw_deg,
        } => assert!(
            close(wr, 200.0) && close(hr, 100.0) && close(st_deg, 180.0) && close(sw_deg, 180.0)
        ),
        ref o => panic!("{o:?}"),
    }
    assert_eq!(g.text_rect, Some((100.0, 100.0, 200.0, 800.0)));
    assert_eq!(g.connections.len(), 1);
    assert!(pt_close(g.connections[0].0, 500.0, 800.0) && close(g.connections[0].1, 90.0));
    // the same spec evaluates differently for another box
    let g2 = custom(&spec, 400.0, 400.0).unwrap();
    assert!(matches!(g2.paths[0].cmds[1], PathCmd::ArcTo { wr, .. } if close(wr, 100.0)));
}

#[test]
fn custom_adjust_overrides_by_name() {
    let n = node(
        r#"<a:custGeom><a:avLst><a:gd name="adj" fmla="val 10000"/></a:avLst>
        <a:gdLst><a:gd name="x" fmla="*/ w adj 100000"/></a:gdLst>
        <a:pathLst><a:path><a:moveTo><a:pt x="x" y="0"/></a:moveTo></a:path></a:pathLst></a:custGeom>"#,
    );
    let spec = custom_from_xml(&n).unwrap();
    let a = spec.eval(1000.0, 1000.0, &[]).unwrap();
    assert!(matches!(a.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 100.0, 0.0)));
    let b = spec
        .eval(1000.0, 1000.0, &adj(&[("adj", 50_000.0)]))
        .unwrap();
    assert!(matches!(b.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 500.0, 0.0)));
}

#[test]
fn guides_see_only_earlier_guides() {
    let spec = CustomGeomSpec {
        guides: vec![
            ("p".into(), "val q".into()), // q is defined later: 0
            ("q".into(), "val 5".into()),
            ("c".into(), "+- p q 0".into()),
        ],
        paths: vec![PathSpec {
            cmds: vec![CmdSpec::Move(("p".into(), "c".into()))],
            ..PathSpec::default()
        }],
        ..CustomGeomSpec::default()
    };
    let g = custom(&spec, 10.0, 10.0).unwrap();
    assert!(matches!(g.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 0.0, 5.0)));
}

#[test]
fn custom_from_xml_rejects_other_elements() {
    assert!(custom_from_xml(&node("<a:prstGeom prst=\"rect\"/>")).is_none());
    assert!(custom_from_xml(&node("<a:sp/>")).is_none());
}

#[test]
fn malformed_custom_geometry_does_not_panic() {
    let cases = [
        "<a:custGeom/>",
        "<a:custGeom><a:pathLst/></a:custGeom>",
        "<a:custGeom><a:pathLst><a:path/></a:pathLst></a:custGeom>",
        // commands without points / with the wrong number of points
        "<a:custGeom><a:pathLst><a:path><a:moveTo/><a:lnTo><a:pt x=\"1\" y=\"1\"/><a:pt x=\"2\" y=\"2\"/></a:lnTo><a:quadBezTo><a:pt x=\"1\" y=\"1\"/></a:quadBezTo><a:cubicBezTo/><a:bogus/><a:arcTo/></a:path></a:pathLst></a:custGeom>",
        // points with missing / garbage coordinates
        "<a:custGeom><a:pathLst><a:path w=\"x\" h=\"-5\"><a:moveTo><a:pt/></a:moveTo><a:lnTo><a:pt x=\"zz\" y=\"1e400\"/></a:lnTo></a:path></a:pathLst></a:custGeom>",
        // guides with garbage
        "<a:custGeom><a:gdLst><a:gd/><a:gd name=\"a\"/><a:gd fmla=\"*/ 1\"/><a:gd name=\"b\" fmla=\"bogus b b b\"/><a:gd name=\"c\" fmla=\"*/ c c c\"/></a:gdLst></a:custGeom>",
        "<a:custGeom><a:rect/><a:cxnLst><a:cxn/><a:cxn><a:pos/></a:cxn></a:cxnLst></a:custGeom>",
        "<a:custGeom><a:pathLst><a:path w=\"1e999\" h=\"9999999999999999999999\"><a:moveTo><a:pt x=\"99999999999999999999999\" y=\"-99999999999999999999999\"/></a:moveTo><a:arcTo wR=\"99999999999999999999\" hR=\"1\" stAng=\"99999999999999999\" swAng=\"-9999999999999999\"/></a:path></a:pathLst></a:custGeom>",
    ];
    for c in cases {
        let spec = custom_from_xml(&node(c)).unwrap();
        for &(w, h) in SIZES {
            let g = custom(&spec, w, h).unwrap();
            all_coords_ok(&g);
        }
    }
}

#[test]
fn a_self_referencing_guide_is_zero_not_a_loop() {
    let spec = CustomGeomSpec {
        guides: vec![("a".into(), "+- a 1 0".into())],
        paths: vec![PathSpec {
            cmds: vec![CmdSpec::Move(("a".into(), "a".into()))],
            ..PathSpec::default()
        }],
        ..CustomGeomSpec::default()
    };
    let g = custom(&spec, 10.0, 10.0).unwrap();
    // `a` is not defined while its own formula runs: 0 + 1 - 0
    assert!(matches!(g.paths[0].cmds[0], PathCmd::MoveTo(p) if pt_close(p, 1.0, 1.0)));
}

#[test]
fn too_many_guides_refuse_the_geometry() {
    let mut xml = String::from("<a:custGeom><a:gdLst>");
    for i in 0..=MAX_GUIDES {
        xml.push_str(&format!("<a:gd name=\"g{i}\" fmla=\"val {i}\"/>"));
    }
    xml.push_str("</a:gdLst></a:custGeom>");
    assert!(custom_from_xml(&node(&xml)).is_none());
    // exactly at the limit is fine
    let mut xml = String::from("<a:custGeom><a:gdLst>");
    for i in 0..MAX_GUIDES {
        xml.push_str(&format!("<a:gd name=\"g{i}\" fmla=\"val {i}\"/>"));
    }
    xml.push_str("</a:gdLst></a:custGeom>");
    let spec = custom_from_xml(&node(&xml)).unwrap();
    assert!(custom(&spec, 1.0, 1.0).is_some());
    // a hand-made spec over the limit is refused by the evaluator too
    let big = CustomGeomSpec {
        guides: vec![("a".into(), "val 1".into()); MAX_GUIDES + 1],
        ..CustomGeomSpec::default()
    };
    assert!(custom(&big, 1.0, 1.0).is_none());
}

#[test]
fn adjusts_count_against_the_guide_budget() {
    let big = CustomGeomSpec {
        adjusts: vec![("a".into(), "val 1".into()); MAX_GUIDES],
        guides: vec![("b".into(), "val 1".into())],
        ..CustomGeomSpec::default()
    };
    assert!(custom(&big, 1.0, 1.0).is_none());
}

#[test]
fn too_many_paths_are_dropped_and_flagged() {
    let mut xml = String::from("<a:custGeom><a:pathLst>");
    for _ in 0..MAX_PATHS + 5 {
        xml.push_str("<a:path><a:moveTo><a:pt x=\"1\" y=\"1\"/></a:moveTo></a:path>");
    }
    xml.push_str("</a:pathLst></a:custGeom>");
    let spec = custom_from_xml(&node(&xml)).unwrap();
    assert!(spec.truncated);
    let g = custom(&spec, 10.0, 10.0).unwrap();
    assert_eq!(g.paths.len(), MAX_PATHS);
    assert!(g.truncated);
}

#[test]
fn too_many_commands_are_dropped_and_flagged() {
    // built by hand: reading that many nodes from XML is the reader's budget, not ours
    let cmds = vec![CmdSpec::Line(("1".into(), "1".into())); MAX_COMMANDS + 10];
    let spec = CustomGeomSpec {
        paths: vec![
            PathSpec {
                cmds,
                ..PathSpec::default()
            },
            PathSpec {
                cmds: vec![CmdSpec::Close],
                ..PathSpec::default()
            },
        ],
        ..CustomGeomSpec::default()
    };
    let g = custom(&spec, 10.0, 10.0).unwrap();
    assert!(g.truncated);
    let total: usize = g.paths.iter().map(|p| p.cmds.len()).sum();
    assert_eq!(total, MAX_COMMANDS);
    // exactly at the limit is not truncated
    let spec = CustomGeomSpec {
        paths: vec![PathSpec {
            cmds: vec![CmdSpec::Close; MAX_COMMANDS],
            ..PathSpec::default()
        }],
        ..CustomGeomSpec::default()
    };
    assert!(!custom(&spec, 1.0, 1.0).unwrap().truncated);
}

#[test]
fn xml_reader_drops_commands_over_the_budget() {
    let mut xml = String::from("<a:custGeom><a:pathLst><a:path>");
    for _ in 0..MAX_COMMANDS + 3 {
        xml.push_str("<a:close/>");
    }
    xml.push_str("</a:path></a:pathLst></a:custGeom>");
    let mut rd = XmlReader::new(xml.as_bytes());
    let mut buf = Vec::new();
    let Event::Start(e) = rd.read_event_into(&mut buf).unwrap() else {
        panic!()
    };
    let e = e.into_owned();
    let mut b = Budget::new(MAX_COMMANDS * 4, 8 << 20);
    let Tree::Ok(n) = read_element(&mut rd, &e, false, &mut b).unwrap() else {
        panic!()
    };
    let spec = custom_from_xml(&n).unwrap();
    assert!(spec.truncated);
    assert_eq!(spec.paths[0].cmds.len(), MAX_COMMANDS);
}

#[test]
fn deep_guide_chains_evaluate_in_linear_time() {
    // 4000 guides each referring to the previous one
    let mut guides = vec![("g0".to_string(), "val 1".to_string())];
    for i in 1..4000 {
        guides.push((format!("g{i}"), format!("+- g{} 1 0", i - 1)));
    }
    let spec = CustomGeomSpec {
        guides,
        paths: vec![PathSpec {
            cmds: vec![CmdSpec::Move(("g3999".into(), "0".into()))],
            ..PathSpec::default()
        }],
        ..CustomGeomSpec::default()
    };
    let g = custom(&spec, 1.0, 1.0).unwrap();
    assert!(matches!(g.paths[0].cmds[0], PathCmd::MoveTo(p) if close(p.x, 4000.0)));
}

#[test]
fn text_rect_defaults_to_the_box_when_attributes_are_missing() {
    let spec = custom_from_xml(&node("<a:custGeom><a:rect l=\"hc\"/></a:custGeom>")).unwrap();
    let g = custom(&spec, 100.0, 60.0).unwrap();
    assert_eq!(g.text_rect, Some((50.0, 0.0, 100.0, 60.0)));
}

#[test]
fn circular_arrow_matches_an_independent_evaluation() {
    // Values from a separate (Python) evaluation of the Ecma guide list at 1,008,000 x 720,000.
    // The inner guides multiply squares of coordinates (about 1e28): the evaluator must not clamp
    // them.
    let g = geom("circularArrow", &[], 1_008_000.0, 720_000.0);
    let c = &g.paths[0].cmds;
    assert!(matches!(c[0], PathCmd::MoveTo(p) if pt_close(p, 45_000.0, 360_000.0)));
    match c[1] {
        PathCmd::ArcTo { st_deg, sw_deg, .. } => {
            assert!(
                close(st_deg, 180.0) && (sw_deg - 9_797_106.852_460_768 / 60_000.0).abs() < 1e-6
            )
        }
        ref o => panic!("{o:?}"),
    }
    assert!(
        matches!(c[2], PathCmd::LineTo(p) if (p.x - 959_931.391_773_559_7).abs() < 1e-3 && (p.y - 233_723.540_339_967_8).abs() < 1e-3)
    );
    assert!(
        matches!(c[5], PathCmd::LineTo(p) if (p.x - 809_406.820_858_367_3).abs() < 1e-3 && (p.y - 233_723.540_339_967_9).abs() < 1e-3)
    );
    match c[6] {
        PathCmd::ArcTo { st_deg, sw_deg, .. } => {
            assert!((st_deg - 20_252_183.356_265_39 / 60_000.0).abs() < 1e-6);
            assert!((sw_deg + 9_452_183.356_265_388 / 60_000.0).abs() < 1e-6);
        }
        ref o => panic!("{o:?}"),
    }
}

#[test]
fn an_outline_path_drawn_before_the_fill_path_stays_visible() {
    // chartPlus draws its cross first and the box (fill only) second: the outlines are painted
    // after all the fills, so the cross is not buried under the box
    use super::super::tests::{at, e, raster, scene};
    let g = geom("chartPlus", &[], e(200.0), e(200.0));
    let mut s = ShapeItem::new(
        Xfrm::rect(e(100.0), e(100.0), e(200.0), e(200.0)),
        Geometry::Paths(g.paths),
    );
    s.fill = Fill::Solid(Rgba::rgb(255, 0, 0));
    s.line = Some(Line::solid(e(4.0), Rgba::rgb(0, 0, 255)));
    let r = super::super::render_svg(&scene(vec![Item::Shape(s)]), &|_| None);
    let img = raster(&r);
    assert_eq!(at(&img, 150, 150), [255, 0, 0]);
    let c = at(&img, 200, 150); // on the vertical bar of the cross
    assert!(c[2] > 200 && c[0] < 60, "{c:?}");
    let c = at(&img, 150, 200); // on the horizontal bar
    assert!(c[2] > 200 && c[0] < 60, "{c:?}");
}
