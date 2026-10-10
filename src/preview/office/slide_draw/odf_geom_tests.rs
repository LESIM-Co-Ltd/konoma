//! Tests of the ODF custom-shape geometry: the equation evaluator, every path command family,
//! the viewBox / mirror / text-area mapping, budgets and hostile input. The survey and the
//! side-by-side comparison against LibreOffice live in `odf_geom_survey.rs` (ignored: they read
//! files that are not in the repository).

use super::*;
use crate::preview::office::docx_xml::{read_element, Budget, Tree};
use crate::preview::office::fmt_xlsx::XmlReader;
use quick_xml::events::Event;

/// Parses one XML element (the first one in `xml`), keeping the text of every element.
pub(super) fn parse(xml: &str) -> Node {
    let mut rd = XmlReader::new(xml.as_bytes());
    let mut buf = Vec::new();
    loop {
        let (e, empty) = match rd.read_event_into(&mut buf).expect("xml") {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::Eof => panic!("no element"),
            _ => continue,
        };
        let mut budget = Budget::odf(50_000_000, 1 << 30);
        match read_element(&mut rd, &e, empty, &mut budget).expect("read") {
            Tree::Ok(n) => return n,
            Tree::TooBig => panic!("too big"),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// A geometry element: `attrs` are extra attributes, `eqs` are `(name, formula)`.
fn geom_xml(vb: &str, attrs: &str, eqs: &[(&str, &str)], path: &str) -> String {
    let mut s = format!(
        r#"<draw:enhanced-geometry svg:viewBox="{vb}" {attrs} draw:enhanced-path="{}">"#,
        esc(path)
    );
    for (n, f) in eqs {
        s.push_str(&format!(
            r#"<draw:equation draw:name="{n}" draw:formula="{}"/>"#,
            esc(f)
        ));
    }
    s.push_str("</draw:enhanced-geometry>");
    s
}

const EMU: f64 = 360.0; // 1/100 mm; a 100 x 100 shape is 36000 EMU

fn run_with(
    vb: &str,
    attrs: &str,
    eqs: &[(&str, &str)],
    path: &str,
) -> Option<(Vec<GeomPath>, Option<Rect4>)> {
    let n = parse(&geom_xml(vb, attrs, eqs, path));
    enhanced_geometry(&n, 100.0 * EMU, 100.0 * EMU)
}

/// Like `run_with`, but builds the node by hand: the XML reader cuts an attribute at 4 KiB
/// (`docx_xml::MAX_ATTR_BYTES`), which a path over a budget would not survive.
fn run_big(path: &str) -> Option<(Vec<GeomPath>, Option<Rect4>)> {
    let n = Node {
        prefix: "draw".into(),
        name: "enhanced-geometry".into(),
        attrs: vec![
            ("svg:viewBox".into(), "0 0 100 100".into()),
            ("draw:enhanced-path".into(), path.into()),
        ],
        kids: Vec::new(),
    };
    enhanced_geometry(&n, 100.0 * EMU, 100.0 * EMU)
}

fn paths(path: &str) -> Vec<GeomPath> {
    run_with("0 0 100 100", "", &[], path)
        .expect("a geometry")
        .0
}

fn cmds(path: &str) -> Vec<PathCmd> {
    let mut p = paths(path);
    assert_eq!(p.len(), 1, "{p:?}");
    p.remove(0).cmds
}

fn pt(x: f64, y: f64) -> Pt {
    Pt::new(x, y)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn assert_pt(p: Pt, x: f64, y: f64) {
    assert!(close(p.x, x) && close(p.y, y), "{p:?} != ({x}, {y})");
}

/// The value of an expression: the x of `M ?x 0` with the equations given.
fn val_with(mods: &str, eqs: &[(&str, &str)], expr: &str) -> f64 {
    let mut e: Vec<(&str, &str)> = eqs.to_vec();
    e.push(("x", expr));
    let attrs = format!(r#"draw:modifiers="{mods}""#);
    let (p, _) = run_with("0 0 100 100", &attrs, &e, "M ?x 0").expect("a geometry");
    match p[0].cmds[0] {
        PathCmd::MoveTo(q) => q.x,
        ref c => panic!("{c:?}"),
    }
}

fn val(expr: &str) -> f64 {
    val_with("", &[], expr)
}

fn near(expr: &str, want: f64) {
    let v = val(expr);
    assert!(close(v, want), "{expr} = {v}, want {want}");
}

// ---------------------------------------------------------------------------------------------
// Evaluator
// ---------------------------------------------------------------------------------------------

#[test]
fn precedence_and_parentheses() {
    near("2+3*4", 14.0);
    near("(2+3)*4", 20.0);
    near("10-4-3", 3.0);
    near("100/10/5", 2.0);
    near("2*3+4*5", 26.0);
    near("2+3*4-6/2", 11.0);
    near("((1+2))*((3))", 9.0);
    near("  7  ", 7.0);
}

#[test]
fn unary_signs() {
    near("-5", -5.0);
    near("-(2+3)", -5.0);
    near("--3", 3.0);
    near("2*-3", -6.0);
    near("+4", 4.0);
    near("10--2", 12.0);
    near("-2*3", -6.0);
}

#[test]
fn numbers_and_exponents() {
    near("1e3", 1000.0);
    near("1.5E+2", 150.0);
    near("2.5e-1", 0.25);
    near(".5+.5", 1.0);
    near("12.", 12.0);
}

#[test]
fn division_by_zero_is_zero() {
    near("1/0", 0.0);
    near("5+1/0", 5.0);
    near("1/(2-2)", 0.0);
}

#[test]
fn nan_and_infinity_are_zero_and_huge_values_clamp() {
    near("sqrt(-4)", 0.0);
    near("sqrt(16)", 4.0);
    near("1e300", MAX_ABS);
    near("1e10*1e10", MAX_ABS);
    near("-1e10*1e10", -MAX_ABS);
    // an overflow to infinity is 0, like any other non-finite value
    near("1e300*1e300", 0.0);
    near("1e999", 0.0);
    near("tan(pi/2)*0", 0.0);
}

#[test]
fn functions() {
    near("abs(-3.5)", 3.5);
    near("min(3,2)", 2.0);
    near("max(3,2)", 3.0);
    near("min(-1, max(5, 7))", -1.0);
    near("sin(pi/2)", 1.0);
    near("sin(0)", 0.0);
    near("cos(pi)", -1.0);
    near("cos(0)", 1.0);
    near("tan(pi/4)", 1.0);
    near("atan(1)", PI / 4.0);
    near("sin(30*(pi/180))", 0.5);
    near("100*cos(60*(pi/180))", 50.0);
    // atan2(y, x): the first argument is the ordinate
    near("atan2(1,0)", PI / 2.0);
    near("atan2(0,-1)", PI);
    near("atan2(-1,0)", -PI / 2.0);
    near("atan2(0,0)", 0.0);
    // function names are not case sensitive, constants too
    near("SQRT(9)", 3.0);
    near("Pi", PI);
}

#[test]
fn if_is_taken_on_positive() {
    near("if(1,10,20)", 10.0);
    near("if(0.001,10,20)", 10.0);
    near("if(0,10,20)", 20.0);
    near("if(-5,10,20)", 20.0);
    near("if(3-5,10,20)", 20.0);
    near("if(2,1+1,0)*3", 6.0);
}

#[test]
fn modifiers() {
    let v = |e: &str| val_with("10 20.5 -3", &[], e);
    assert!(close(v("$0"), 10.0));
    assert!(close(v("$1"), 20.5));
    assert!(close(v("$2+$0"), 7.0));
    assert!(close(v("$3"), 0.0), "a modifier that is not there is 0");
    assert!(close(v("$99+1"), 1.0));
    assert!(close(v("-$0"), -10.0));
    assert!(
        close(val_with("abc 5", &[], "$0 + $1"), 5.0),
        "a bad modifier is 0"
    );
    assert!(
        close(val_with("1,2", &[], "$1"), 2.0),
        "commas separate too"
    );
}

#[test]
fn named_equations_forward_and_backward() {
    let eqs = [("a", "?b*2"), ("b", "?c+1"), ("c", "10")];
    assert!(close(val_with("", &eqs, "?a"), 22.0), "forward references");
    let eqs = [("c", "10"), ("b", "?c+1"), ("a", "?b*2")];
    assert!(close(val_with("", &eqs, "?a+?b+?c"), 22.0 + 11.0 + 10.0));
}

#[test]
fn unknown_names_and_functions_are_zero() {
    near("?nope", 0.0);
    near("?nope+5", 5.0);
    near("frobnicate(1)", 0.0);
    near("nothing", 0.0);
    near("sin(1,2)", 0.0);
    near("atan2(1)", 0.0);
    near("if(1,2)", 0.0);
    near("max()", 0.0);
    near("min(1,2,3)", 0.0);
}

#[test]
fn syntax_errors_make_the_equation_zero() {
    near("(1+2", 0.0);
    near("1+2)", 0.0);
    near("1+", 0.0);
    near("*3", 0.0);
    near("1 2", 0.0);
    near("$", 0.0);
    near("?", 0.0);
    near("sin(", 0.0);
    near("1..2", 0.0);
}

#[test]
fn cycles_do_not_loop() {
    // a = b + 1, b = a + 1: the cycle is cut at the equation being evaluated
    let eqs = [("a", "?b+1"), ("b", "?a+1")];
    let v = val_with("", &eqs, "?a");
    assert!(v == 2.0 || v == 1.0, "{v}");
    let eqs = [("a", "?a+5")];
    assert!(
        close(val_with("", &eqs, "?a"), 5.0),
        "a self reference is 0"
    );
    let eqs = [("a", "?b"), ("b", "?c"), ("c", "?a+7")];
    assert!(val_with("", &eqs, "?a").is_finite());
}

#[test]
fn constants_follow_the_view_box_and_the_shape() {
    let n = parse(&geom_xml(
        "10 20 200 80",
        r#"draw:path-stretchpoint-x="7" draw:path-stretchpoint-y="9""#,
        &[
            ("l", "left"),
            ("t", "top"),
            ("r", "right"),
            ("b", "bottom"),
            ("w", "width"),
            ("h", "height"),
            ("lw", "logwidth"),
            ("lh", "logheight"),
            ("xs", "xstretch"),
            ("ys", "ystretch"),
            ("hs", "hasstroke+hasfill"),
        ],
        "M ?l ?t L ?r ?b ?w ?h ?lw ?lh ?xs ?ys ?hs 0",
    ));
    // 720 x 360 EMU = 2 x 1 hundredths of a millimetre
    let (p, _) = enhanced_geometry(&n, 720.0, 360.0).unwrap();
    let want = [
        (10.0, 20.0),
        (210.0, 100.0),
        (200.0, 80.0),
        (2.0, 1.0),
        (7.0, 9.0),
        (2.0, 0.0),
    ];
    // the viewBox origin (10, 20) is subtracted from every coordinate
    let mut it = p[0].cmds.iter();
    assert_eq!(it.next(), Some(&PathCmd::MoveTo(pt(0.0, 0.0))));
    for (x, y) in want.iter().skip(1) {
        match it.next() {
            Some(PathCmd::LineTo(q)) => assert_pt(*q, x - 10.0, y - 20.0),
            c => panic!("{c:?}"),
        }
    }
}

#[test]
fn long_reference_chains_and_deep_nesting_do_not_overflow_the_stack() {
    // a chain of 3000 equations (over the equation budget: no geometry, but no crash either)
    let mut s = String::from(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 10 10" draw:enhanced-path="M ?f0 0">"#,
    );
    for i in 0..3000 {
        s.push_str(&format!(
            r#"<draw:equation draw:name="f{i}" draw:formula="?f{}+1"/>"#,
            i + 1
        ));
    }
    s.push_str("</draw:enhanced-geometry>");
    let n = parse(&s);
    let _ = enhanced_geometry(&n, 3600.0, 3600.0);
    // a chain of 1000: evaluated, the far end is cut
    let mut s = String::from(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 10 10" draw:enhanced-path="M ?f0 0">"#,
    );
    for i in 0..1000 {
        s.push_str(&format!(
            r#"<draw:equation draw:name="f{i}" draw:formula="?f{}+1"/>"#,
            i + 1
        ));
    }
    s.push_str("</draw:enhanced-geometry>");
    let n = parse(&s);
    let (p, _) = enhanced_geometry(&n, 3600.0, 3600.0).unwrap();
    assert!(matches!(p[0].cmds[0], PathCmd::MoveTo(_)));
    // deep parentheses are a syntax error (0), not a stack overflow
    let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
    near(&deep, 0.0);
    let ok = format!("{}1{}", "(".repeat(40), ")".repeat(40));
    near(&ok, 1.0);
    near(&format!("{}1", "-".repeat(100_000)), 0.0); // over the formula length
    near(&format!("{}1", "- ".repeat(500)), 1.0);
}

#[test]
fn a_formula_over_the_length_budget_is_zero() {
    let long = format!("1{}", "+1".repeat(MAX_FORMULA_BYTES));
    near(&long, 0.0);
}

// ---------------------------------------------------------------------------------------------
// Path commands
// ---------------------------------------------------------------------------------------------

#[test]
fn move_line_close() {
    let p = paths("M 10 20 L 30 20 30 40 Z N");
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].w, p[0].h), (100.0, 100.0));
    assert_eq!(p[0].fill_mode, PathFill::Norm);
    assert!(p[0].stroke);
    assert_eq!(
        p[0].cmds,
        vec![
            PathCmd::MoveTo(pt(10.0, 20.0)),
            PathCmd::LineTo(pt(30.0, 20.0)),
            PathCmd::LineTo(pt(30.0, 40.0)),
            PathCmd::Close,
        ]
    );
}

#[test]
fn without_n_the_sub_paths_share_one_path() {
    let p = paths("M 0 0 L 10 0 Z M 20 20 L 30 20 Z");
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].cmds.len(), 6);
}

#[test]
fn n_starts_a_new_path_and_resets_the_flags() {
    let p = paths("M 0 0 L 10 0 10 10 Z F N M 20 20 L 30 30 S N M 1 1 L 2 2");
    assert_eq!(p.len(), 3);
    assert_eq!(p[0].fill_mode, PathFill::None);
    assert!(p[0].stroke);
    assert_eq!(p[1].fill_mode, PathFill::Norm);
    assert!(!p[1].stroke);
    assert_eq!(p[2].fill_mode, PathFill::Norm);
    assert!(p[2].stroke);
    // an N with nothing before it makes no empty path
    assert_eq!(paths("N N M 0 0 L 1 1 N N").len(), 1);
}

#[test]
fn n_forgets_the_current_point_so_the_next_command_starts_there() {
    let p = paths("M 5 5 L 6 6 N L 50 50 60 60");
    assert_eq!(
        p[1].cmds,
        vec![
            PathCmd::MoveTo(pt(50.0, 50.0)),
            PathCmd::LineTo(pt(60.0, 60.0))
        ]
    );
}

#[test]
fn implicit_repetition_and_dropped_remainders() {
    let c = cmds("M 0 0 L 1 1 2 2 3 3 4");
    assert_eq!(c.len(), 4, "the lone 4 is dropped: {c:?}");
    let c = cmds("M 0 0 5 5");
    assert_eq!(
        c,
        vec![PathCmd::MoveTo(pt(0.0, 0.0)), PathCmd::MoveTo(pt(5.0, 5.0))]
    );
    let c = cmds("M 0 0 C 1 1 2 2 3 3 4 4 5 5 6 6 7 7");
    assert_eq!(c.len(), 3, "{c:?}");
}

#[test]
fn cubic_and_quadratic() {
    let c = cmds("M 0 0 C 10 0 20 10 30 30 Q 40 40 50 30");
    assert_eq!(
        c[1],
        PathCmd::CubicTo(pt(10.0, 0.0), pt(20.0, 10.0), pt(30.0, 30.0))
    );
    assert_eq!(c[2], PathCmd::QuadTo(pt(40.0, 40.0), pt(50.0, 30.0)));
}

#[test]
fn a_drawing_command_without_a_current_point_starts_at_its_first_point() {
    let c = cmds("L 5 6 7 8");
    assert_eq!(
        c,
        vec![PathCmd::MoveTo(pt(5.0, 6.0)), PathCmd::LineTo(pt(7.0, 8.0))]
    );
    let c = cmds("C 1 1 2 2 3 3");
    assert_eq!(c[0], PathCmd::MoveTo(pt(1.0, 1.0)));
    let c = cmds("Z L 1 1");
    assert_eq!(
        c,
        vec![PathCmd::MoveTo(pt(1.0, 1.0))],
        "a Z with nothing to close is dropped"
    );
}

#[test]
fn z_returns_to_the_start_of_the_sub_polygon() {
    // after Z the current point is the start: the X quadrant leaves from (10, 10)
    let c = cmds("M 10 10 L 50 10 Z X 20 20");
    match c[3] {
        PathCmd::CubicTo(a, _, e) => {
            assert_pt(a, 10.0 + KAPPA * 10.0, 10.0);
            assert_pt(e, 20.0, 20.0);
        }
        ref c => panic!("{c:?}"),
    }
}

#[test]
fn elliptical_quadrants() {
    // X leaves along the x axis: from the top of an ellipse to its left side
    let c = cmds("M 50 0 X 0 50");
    match c[1] {
        PathCmd::CubicTo(a, b, e) => {
            assert_pt(a, 50.0 - KAPPA * 50.0, 0.0);
            assert_pt(b, 0.0, 50.0 - KAPPA * 50.0);
            assert_pt(e, 0.0, 50.0);
        }
        ref c => panic!("{c:?}"),
    }
    // Y leaves along the y axis: from the left side back to the bottom
    let c = cmds("M 0 50 Y 50 100");
    match c[1] {
        PathCmd::CubicTo(a, b, e) => {
            assert_pt(a, 0.0, 50.0 + KAPPA * 50.0);
            assert_pt(b, 50.0 - KAPPA * 50.0, 100.0);
            assert_pt(e, 50.0, 100.0);
        }
        ref c => panic!("{c:?}"),
    }
    // repetition alternates nothing: each pair is the same kind
    let c = cmds("M 50 0 X 0 50 50 100");
    assert_eq!(c.len(), 3);
    // the quadrant is on the ellipse: the middle of the Bezier is at 45 degrees
    let c = cmds("M 100 50 Y 50 0");
    match c[1] {
        PathCmd::CubicTo(a, b, e) => {
            let mid = bez(pt(100.0, 50.0), a, b, e, 0.5);
            let r = ((mid.x - 50.0).powi(2) + (mid.y - 50.0).powi(2)).sqrt();
            assert!((r - 50.0).abs() < 0.05, "{r}");
        }
        ref c => panic!("{c:?}"),
    }
}

fn bez(p0: Pt, p1: Pt, p2: Pt, p3: Pt, t: f64) -> Pt {
    let u = 1.0 - t;
    let f = |a: f64, b: f64, c: f64, d: f64| {
        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
    };
    Pt::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y))
}

/// Every cubic of the commands, with its start point.
fn cubics(c: &[PathCmd]) -> Vec<(Pt, Pt, Pt, Pt)> {
    let mut out = Vec::new();
    let mut cur = pt(0.0, 0.0);
    for x in c {
        match *x {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => cur = p,
            PathCmd::CubicTo(a, b, e) => {
                out.push((cur, a, b, e));
                cur = e;
            }
            _ => {}
        }
    }
    out
}

fn on_circle(c: &[PathCmd], cx: f64, cy: f64, r: f64) {
    for (p0, a, b, e) in cubics(c) {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let q = bez(p0, a, b, e, t);
            let d = ((q.x - cx).powi(2) + (q.y - cy).powi(2)).sqrt();
            assert!((d - r).abs() < r * 0.001, "off the circle: {q:?} {d}");
        }
    }
}

#[test]
fn angle_ellipse_full_circle() {
    let c = cmds("U 50 50 40 40 0 360 Z N");
    assert_eq!(c[0], PathCmd::MoveTo(pt(90.0, 50.0)));
    assert_eq!(cubics(&c).len(), 4);
    on_circle(&c, 50.0, 50.0, 40.0);
    let (_, _, _, last) = *cubics(&c).last().unwrap();
    assert_pt(last, 90.0, 50.0);
    assert_eq!(c.last(), Some(&PathCmd::Close));
    // 16.16 fixed point, as LibreOffice writes for the cloud callout: 0 .. 360 * 65536
    let c = cmds("U 50 50 40 40 0 23592960 Z N");
    assert_eq!(cubics(&c).len(), 4);
    on_circle(&c, 50.0, 50.0, 40.0);
    // more than a whole turn is one whole turn
    assert_eq!(cubics(&cmds("U 50 50 40 40 0 720")).len(), 4);
}

#[test]
fn angle_ellipse_partial_and_to() {
    // a quarter from 0 to 90 degrees (clockwise on screen): from the right to the bottom
    let c = cmds("U 50 50 40 40 0 90");
    assert_eq!(c[0], PathCmd::MoveTo(pt(90.0, 50.0)));
    assert_eq!(cubics(&c).len(), 1);
    let (_, _, _, e) = cubics(&c)[0];
    assert_pt(e, 50.0, 90.0);
    on_circle(&c, 50.0, 50.0, 40.0);
    // an end before the start goes on round: 270 -> 90 is half a turn through 0
    let c = cmds("U 50 50 40 40 270 90");
    let (_, _, _, e) = *cubics(&c).last().unwrap();
    assert_pt(e, 50.0, 90.0);
    assert_eq!(cubics(&c).len(), 2);
    // T joins the current point with a line, U does not
    let c = cmds("M 0 0 T 50 50 40 40 0 90");
    assert_eq!(c[1], PathCmd::LineTo(pt(90.0, 50.0)));
    let c = cmds("M 0 0 U 50 50 40 40 0 90");
    assert_eq!(c[1], PathCmd::MoveTo(pt(90.0, 50.0)));
    // an elliptical one
    let c = cmds("U 50 50 40 20 0 90");
    let (_, _, _, e) = cubics(&c)[0];
    assert_pt(e, 50.0, 70.0);
    // zero radius draws nothing
    assert!(run_with("0 0 100 100", "", &[], "U 50 50 0 20 0 90").is_none());
}

#[test]
fn arcs_by_two_points_counter_clockwise_and_clockwise() {
    // box 0 0 100 100, from the right (100, 50) to the left (0, 50)
    let ccw = cmds("B 0 0 100 100 100 50 0 50");
    assert_eq!(ccw[0], PathCmd::MoveTo(pt(100.0, 50.0)));
    assert_eq!(cubics(&ccw).len(), 2);
    on_circle(&ccw, 50.0, 50.0, 50.0);
    // counter-clockwise on screen from the right goes through the top (y = 0)
    let (p0, a, b, e) = cubics(&ccw)[0];
    assert_pt(e, 50.0, 0.0);
    assert!(bez(p0, a, b, e, 0.5).y < 50.0);
    let (_, _, _, end) = cubics(&ccw)[1];
    assert_pt(end, 0.0, 50.0);

    let cw = cmds("V 0 0 100 100 100 50 0 50");
    on_circle(&cw, 50.0, 50.0, 50.0);
    let (p0, a, b, e) = cubics(&cw)[0];
    assert_pt(e, 50.0, 100.0);
    assert!(bez(p0, a, b, e, 0.5).y > 50.0);

    // the same arcs by the other pair of letters
    assert_eq!(cmds("A 0 0 100 100 100 50 0 50"), {
        let mut v = vec![PathCmd::MoveTo(pt(100.0, 50.0))];
        v.extend(ccw[1..].iter().copied());
        v
    });
    assert_eq!(cmds("W 0 0 100 100 100 50 0 50")[1..], cw[1..]);
}

#[test]
fn arc_to_is_joined_to_the_current_point_by_a_line() {
    let c = cmds("M 10 10 A 0 0 100 100 100 50 0 50");
    assert_eq!(c[1], PathCmd::LineTo(pt(100.0, 50.0)));
    let c = cmds("M 10 10 W 0 0 100 100 100 50 0 50");
    assert_eq!(c[1], PathCmd::LineTo(pt(100.0, 50.0)));
    // without the "to" the arc starts a new polygon
    let c = cmds("M 10 10 B 0 0 100 100 100 50 0 50");
    assert_eq!(c[1], PathCmd::MoveTo(pt(100.0, 50.0)));
    // with no current point the "to" arc just starts
    let c = cmds("A 0 0 100 100 100 50 0 50");
    assert_eq!(c[0], PathCmd::MoveTo(pt(100.0, 50.0)));
}

#[test]
fn arc_points_are_projected_onto_the_ellipse_by_their_ray() {
    // a start point well outside the ellipse still selects the ray from the centre
    let c = cmds("B 0 0 100 100 500 50 50 -300");
    assert_eq!(c[0], PathCmd::MoveTo(pt(100.0, 50.0)));
    let (_, _, _, e) = *cubics(&c).last().unwrap();
    assert_pt(e, 50.0, 0.0);
    // stretched: the ray through the corner of the box meets the ellipse at 45 degrees of the
    // parametrisation (not at the visual 45 degrees)
    let c = cmds("B 0 0 200 100 200 100 100 0");
    let s = 0.5f64.sqrt();
    assert_eq!(
        c[0],
        PathCmd::MoveTo(pt(100.0 + 100.0 * s, 50.0 + 50.0 * s))
    );
}

#[test]
fn coincident_arc_points_make_a_whole_ellipse() {
    let c = cmds("B 0 0 100 100 100 50 100 50");
    assert_eq!(cubics(&c).len(), 4);
    on_circle(&c, 50.0, 50.0, 50.0);
    let c = cmds("V 0 0 100 100 100 50 100 50");
    assert_eq!(cubics(&c).len(), 4);
}

#[test]
fn a_degenerate_arc_box_is_a_point() {
    let c = cmds("M 0 0 A 10 10 10 50 0 0 5 5");
    assert_eq!(
        c,
        vec![PathCmd::MoveTo(pt(0.0, 0.0)), PathCmd::LineTo(pt(5.0, 5.0))]
    );
    let c = cmds("M 0 0 B 10 10 10 50 0 0 5 5");
    assert_eq!(
        c,
        vec![PathCmd::MoveTo(pt(0.0, 0.0)), PathCmd::MoveTo(pt(5.0, 5.0))]
    );
}

#[test]
fn libreoffice_arc_to_extension_is_a_drawingml_arc_to() {
    let c = cmds("M 0 50 G 50 50 180 180 G 50 50 0 180");
    assert_eq!(c[0], PathCmd::MoveTo(pt(0.0, 50.0)));
    assert_eq!(
        c[1],
        PathCmd::ArcTo {
            wr: 50.0,
            hr: 50.0,
            st_deg: 180.0,
            sw_deg: 180.0
        }
    );
    // the second arc continues from where the first ended: (100, 50) is at 0 degrees
    assert_eq!(
        c[2],
        PathCmd::ArcTo {
            wr: 50.0,
            hr: 50.0,
            st_deg: 0.0,
            sw_deg: 180.0
        }
    );
    // no current point: ignored
    assert!(run_with("0 0 100 100", "", &[], "G 50 50 0 90").is_none());
}

#[test]
fn drawooo_path_is_preferred() {
    let n = parse(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 100 100" draw:enhanced-path="M 0 0 Z N" drawooo:enhanced-path="M 1 2 L 3 4 Z N"/>"#,
    );
    let (p, _) = enhanced_geometry(&n, 3600.0, 3600.0).unwrap();
    assert_eq!(p[0].cmds[0], PathCmd::MoveTo(pt(1.0, 2.0)));
    let n = parse(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 100 100" drawooo:enhanced-path="M 7 8 Z N" draw:enhanced-path="M 0 0 Z N"/>"#,
    );
    let (p, _) = enhanced_geometry(&n, 3600.0, 3600.0).unwrap();
    assert_eq!(p[0].cmds[0], PathCmd::MoveTo(pt(7.0, 8.0)));
}

#[test]
fn star_with_equations_end_to_end() {
    // LibreOffice's `block-arc` with its default modifiers: a half ring (the top half)
    let eqs = [
        ("f0", "10800*cos($0 *(pi/180))"),
        ("f1", "10800*sin($0 *(pi/180))"),
        ("f2", "?f0 +10800"),
        ("f3", "?f1 +10800"),
        ("f4", "21600-?f2 "),
        ("f5", "10800-$1 "),
        ("f6", "10800+$1 "),
    ];
    let (p, _) = run_with(
        "0 0 21600 21600",
        r#"draw:modifiers="180 5400""#,
        &eqs,
        "B 0 0 21600 21600 ?f4 ?f3 ?f2 ?f3 W ?f5 ?f5 ?f6 ?f6 ?f2 ?f3 ?f4 ?f3 Z N",
    )
    .unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].w, p[0].h), (21600.0, 21600.0));
    let c = &p[0].cmds;
    // f2 = 10800*cos(180) + 10800 = 0, f3 = 10800, f4 = 21600
    assert_pt(
        match c[0] {
            PathCmd::MoveTo(q) => q,
            ref x => panic!("{x:?}"),
        },
        21600.0,
        10800.0,
    );
    // outer arc: right -> left over the top; inner (W, clockwise): left -> right over the top
    let cs = cubics(c);
    assert_eq!(cs.len(), 4);
    let (p0, a, b, e) = cs[0];
    assert_pt(e, 10800.0, 0.0);
    assert!(bez(p0, a, b, e, 0.5).y < 10800.0);
    // the inner arc's radius is 10800 - 5400
    let (_, _, _, inner_top) = cs[2];
    assert_pt(inner_top, 10800.0, 5400.0);
    assert_eq!(c.last(), Some(&PathCmd::Close));
}

// ---------------------------------------------------------------------------------------------
// viewBox, mirror, text areas
// ---------------------------------------------------------------------------------------------

#[test]
fn view_box_sets_the_path_size_and_its_origin_is_subtracted() {
    let (p, _) = run_with("100 200 50 80", "", &[], "M 100 200 L 150 280 Z N").unwrap();
    assert_eq!((p[0].w, p[0].h), (50.0, 80.0));
    assert_eq!(p[0].cmds[0], PathCmd::MoveTo(pt(0.0, 0.0)));
    assert_eq!(p[0].cmds[1], PathCmd::LineTo(pt(50.0, 80.0)));
    // negative origin, comma separated
    let (p, _) = run_with("-10,-20,30,40", "", &[], "M 0 0 Z").unwrap();
    assert_eq!((p[0].w, p[0].h), (30.0, 40.0));
    assert_eq!(p[0].cmds[0], PathCmd::MoveTo(pt(10.0, 20.0)));
}

#[test]
fn a_missing_or_malformed_view_box_is_the_default_space() {
    // the `mso-spt100` pie is written without one; its numbers are in 21600 units
    let n = parse(r#"<draw:enhanced-geometry draw:enhanced-path="M 0 0 L 21600 10800"/>"#);
    let (p, _) = enhanced_geometry(&n, 3600.0, 3600.0).unwrap();
    assert_eq!((p[0].w, p[0].h), (21600.0, 21600.0));
    let (p, _) = run_with("0 0 100", "", &[], "M 0 0").unwrap();
    assert_eq!((p[0].w, p[0].h), (21600.0, 21600.0));
    // four unparsable numbers are four zeros: a view box of size 0 (the shape size)
    let (p, _) = run_with("a b c d", "", &[], "M 0 0 L 5 5").unwrap();
    assert_eq!((p[0].w, p[0].h), (100.0, 100.0));
}

#[test]
fn a_zero_view_box_means_the_shape_size_in_hundredths_of_a_millimetre() {
    // 100 x 50 hundredths of a millimetre
    let n = parse(&geom_xml(
        "0 0 0 0",
        "",
        &[("f0", "logwidth/2"), ("f1", "logheight/2")],
        "M 0 ?f1 L ?f0 0",
    ));
    let (p, _) = enhanced_geometry(&n, 100.0 * EMU, 50.0 * EMU).unwrap();
    assert_eq!((p[0].w, p[0].h), (100.0, 50.0));
    assert_eq!(p[0].cmds[0], PathCmd::MoveTo(pt(0.0, 25.0)));
    assert_eq!(p[0].cmds[1], PathCmd::LineTo(pt(50.0, 0.0)));
    // ... and with no shape size there is nothing to take it from
    assert!(enhanced_geometry(&n, 0.0, 0.0).is_none());
    assert!(enhanced_geometry(&n, f64::NAN, 5.0).is_none());
}

#[test]
fn mirror_flips_about_the_middle_of_the_view_box() {
    let p = |attrs: &str| {
        run_with("10 20 100 50", attrs, &[], "M 10 20 L 40 30 Z N")
            .unwrap()
            .0
    };
    let plain = p("");
    assert_eq!(plain[0].cmds[1], PathCmd::LineTo(pt(30.0, 10.0)));
    let h = p(r#"draw:mirror-horizontal="true""#);
    assert_eq!(h[0].cmds[0], PathCmd::MoveTo(pt(100.0, 0.0)));
    assert_eq!(h[0].cmds[1], PathCmd::LineTo(pt(70.0, 10.0)));
    let v = p(r#"draw:mirror-vertical="true""#);
    assert_eq!(v[0].cmds[0], PathCmd::MoveTo(pt(0.0, 50.0)));
    assert_eq!(v[0].cmds[1], PathCmd::LineTo(pt(30.0, 40.0)));
    let hv = p(r#"draw:mirror-horizontal="true" draw:mirror-vertical="true""#);
    assert_eq!(hv[0].cmds[1], PathCmd::LineTo(pt(70.0, 40.0)));
    let off = p(r#"draw:mirror-horizontal="false" draw:mirror-vertical="false""#);
    assert_eq!(off, plain);
}

#[test]
fn mirror_flips_arc_to_angles() {
    let at = |attrs: &str| {
        let (p, _) = run_with("0 0 100 100", attrs, &[], "M 100 50 G 50 50 0 90").unwrap();
        p[0].cmds[1]
    };
    let arc = |st: f64, sw: f64| PathCmd::ArcTo {
        wr: 50.0,
        hr: 50.0,
        st_deg: st,
        sw_deg: sw,
    };
    assert_eq!(at(""), arc(0.0, 90.0));
    assert_eq!(at(r#"draw:mirror-horizontal="true""#), arc(180.0, -90.0));
    assert_eq!(at(r#"draw:mirror-vertical="true""#), arc(0.0, -90.0));
    assert_eq!(
        at(r#"draw:mirror-horizontal="true" draw:mirror-vertical="true""#),
        arc(-180.0, 90.0)
    );
}

#[test]
fn mirrored_bezier_curves_follow_their_points() {
    // the LibreOffice moon is mirrored horizontally
    let (p, _) = run_with(
        "0 0 100 100",
        r#"draw:mirror-horizontal="true""#,
        &[],
        "M 100 0 C 80 10 20 50 20 50",
    )
    .unwrap();
    assert_eq!(
        p[0].cmds[1],
        PathCmd::CubicTo(pt(20.0, 10.0), pt(80.0, 50.0), pt(80.0, 50.0))
    );
}

#[test]
fn text_area_is_scaled_to_the_box_and_mirrored() {
    // viewBox 100 x 100 -> box 200 x 100 EMU * 360
    let n = |attrs: &str| {
        parse(&geom_xml(
            "0 0 100 100",
            &format!(r#"draw:text-areas="10 20 ?r 80" {attrs}"#),
            &[("r", "right-30")],
            "M 0 0 L 1 1",
        ))
    };
    let (w, h) = (200.0 * EMU, 100.0 * EMU);
    let (_, t) = enhanced_geometry(&n(""), w, h).unwrap();
    let t = t.unwrap();
    assert!(close(t.0, 20.0 * EMU) && close(t.1, 20.0 * EMU), "{t:?}");
    assert!(close(t.2, 140.0 * EMU) && close(t.3, 80.0 * EMU), "{t:?}");
    let (_, t) = enhanced_geometry(&n(r#"draw:mirror-horizontal="true""#), w, h).unwrap();
    let t = t.unwrap();
    assert!(close(t.0, 60.0 * EMU) && close(t.2, 180.0 * EMU), "{t:?}");
    assert!(t.0 < t.2 && t.1 < t.3, "ordered");
    let (_, t) = enhanced_geometry(&n(r#"draw:mirror-vertical="true""#), w, h).unwrap();
    let t = t.unwrap();
    assert!(close(t.1, 20.0 * EMU) && close(t.3, 80.0 * EMU), "{t:?}");
    // only the first area is read
    let m = parse(&geom_xml(
        "0 0 100 100",
        r#"draw:text-areas="0 0 50 50 60 60 90 90""#,
        &[],
        "M 0 0 L 1 1",
    ));
    let (_, t) = enhanced_geometry(&m, 100.0 * EMU, 100.0 * EMU).unwrap();
    assert_eq!(t, Some((0.0, 0.0, 50.0 * EMU, 50.0 * EMU)));
    // fewer than four numbers: no text area
    let m = parse(&geom_xml(
        "0 0 100 100",
        r#"draw:text-areas="0 0 50""#,
        &[],
        "M 0 0 L 1 1",
    ));
    assert_eq!(enhanced_geometry(&m, 3600.0, 3600.0).unwrap().1, None);
    // view box origin
    let m = parse(&geom_xml(
        "50 50 100 100",
        r#"draw:text-areas="60 60 90 90""#,
        &[],
        "M 50 50 L 51 51",
    ));
    let (_, t) = enhanced_geometry(&m, 100.0 * EMU, 100.0 * EMU).unwrap();
    assert_eq!(t, Some((10.0 * EMU, 10.0 * EMU, 40.0 * EMU, 40.0 * EMU)));
}

// ---------------------------------------------------------------------------------------------
// Missing paths, hostile input
// ---------------------------------------------------------------------------------------------

#[test]
fn no_enhanced_path_is_none() {
    let n =
        parse(r#"<draw:enhanced-geometry svg:viewBox="0 0 10 10" draw:type="ooxml-rightArrow"/>"#);
    assert!(enhanced_geometry(&n, 3600.0, 3600.0).is_none());
    assert!(run_with("0 0 10 10", "", &[], "").is_none());
    assert!(run_with("0 0 10 10", "", &[], "   ").is_none());
    assert!(
        run_with("0 0 10 10", "", &[], "N").is_none(),
        "a path with no drawing is nothing"
    );
}

#[test]
fn garbage_in_a_path_is_skipped() {
    let c = cmds("M 1 2 !!! xyz ,, L 3 4 ## 5 6 @");
    assert_eq!(
        c,
        vec![
            PathCmd::MoveTo(pt(1.0, 2.0)),
            PathCmd::LineTo(pt(3.0, 4.0)),
            PathCmd::LineTo(pt(5.0, 6.0))
        ]
    );
    // unknown command letters take their parameters with them
    let c = cmds("M 1 2 K 9 9 9 L 3 4");
    assert_eq!(
        c,
        vec![PathCmd::MoveTo(pt(1.0, 2.0)), PathCmd::LineTo(pt(3.0, 4.0))]
    );
    // parameters before any command
    let c = cmds("7 8 M 1 2");
    assert_eq!(c, vec![PathCmd::MoveTo(pt(1.0, 2.0))]);
    // unresolvable parameters are 0
    let c = cmds("M ?nope $9");
    assert_eq!(c, vec![PathCmd::MoveTo(pt(0.0, 0.0))]);
    // glued numbers and commands
    let c = cmds("M10 20L30 40Z");
    assert_eq!(c.len(), 3);
    assert_eq!(c[1], PathCmd::LineTo(pt(30.0, 40.0)));
    // signs: "10-5" is two numbers
    let c = cmds("M 10-5 L-3+4");
    assert_eq!(
        c,
        vec![
            PathCmd::MoveTo(pt(10.0, -5.0)),
            PathCmd::LineTo(pt(-3.0, 4.0))
        ]
    );
    // non-ASCII bytes in a path
    let c = cmds("M 1 2 \u{3042}\u{1F600} L 3 4");
    assert_eq!(c.len(), 2);
}

#[test]
fn huge_numbers_are_clamped_not_propagated() {
    let c = cmds("M 1e300 -1e300 L 1e999 5");
    assert_eq!(c[0], PathCmd::MoveTo(pt(MAX_ABS, -MAX_ABS)));
    assert_eq!(c[1], PathCmd::LineTo(pt(0.0, 5.0)));
    for cmd in cmds("U 1e300 1e300 1e300 1e300 -1e300 1e300 B -1e300 -1e300 1e300 1e300 1e300 1e300 -1e300 3 Q 1e308 1e308 -1e308 5") {
        let ok = |p: Pt| p.x.is_finite() && p.y.is_finite();
        match cmd {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => assert!(ok(p)),
            PathCmd::CubicTo(a, b, e) => assert!(ok(a) && ok(b) && ok(e)),
            PathCmd::QuadTo(a, e) => assert!(ok(a) && ok(e)),
            _ => {}
        }
    }
}

#[test]
fn nan_in_the_view_box_and_shape_size_is_harmless() {
    let n = parse(&geom_xml("NaN 0 10 10", "", &[], "M 1 1 L 2 2"));
    let _ = enhanced_geometry(&n, f64::NAN, f64::INFINITY);
    let n = parse(&geom_xml("0 0 1e300 1e300", "", &[], "M 1 1 L 2 2"));
    let _ = enhanced_geometry(&n, 3600.0, 3600.0);
    let n = parse(&geom_xml("0 0 -5 10", "", &[], "M 1 1 L 2 2"));
    let _ = enhanced_geometry(&n, 3600.0, 3600.0);
}

#[test]
fn budgets_give_nothing_instead_of_a_half_shape() {
    // commands (a Z with a current point is one command and one token)
    let long = "Z ".repeat(MAX_COMMANDS + 10);
    assert!(run_big(&format!("M 0 0 {long}")).is_none());
    let ok = "Z ".repeat(MAX_COMMANDS - 1);
    assert!(run_big(&format!("M 0 0 {ok}")).is_some());
    // sub-paths
    let many = "M 0 0 L 1 1 N ".repeat(MAX_SUBPATHS + 1);
    assert!(run_big(&many).is_none());
    let ok = "M 0 0 L 1 1 N ".repeat(MAX_SUBPATHS);
    assert_eq!(run_big(&ok).unwrap().0.len(), MAX_SUBPATHS);
    // bytes
    let big = format!("M 0 0 {} L 1 1", " ".repeat(MAX_PATH_BYTES + 1));
    assert!(run_big(&big).is_none());
    // tokens
    let toks = "1 ".repeat(MAX_PATH_TOKENS + 1);
    assert!(run_big(&format!("M {toks}")).is_none());
    // a single enormous parameter
    let tok = format!("M {} 0", "9".repeat(MAX_TOKEN_BYTES + 1));
    assert!(run_big(&tok).is_none());
    // arcs: the command count is bounded by the arc count (five commands each)
    let arcs = "U 0 0 5 5 0 360 ".repeat(25_000);
    assert!(run_big(&arcs).is_none());
}

#[test]
fn too_many_equations_give_nothing() {
    let mut s = String::from(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 10 10" draw:enhanced-path="M 0 0 L 1 1">"#,
    );
    for i in 0..=MAX_EQUATIONS {
        s.push_str(&format!(
            r#"<draw:equation draw:name="f{i}" draw:formula="1"/>"#
        ));
    }
    s.push_str("</draw:enhanced-geometry>");
    assert!(enhanced_geometry(&parse(&s), 3600.0, 3600.0).is_none());
    let mut s = String::from(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 10 10" draw:enhanced-path="M ?f4095 0 L 1 1">"#,
    );
    for i in 0..MAX_EQUATIONS {
        s.push_str(&format!(
            r#"<draw:equation draw:name="f{i}" draw:formula="{i}"/>"#
        ));
    }
    s.push_str("</draw:enhanced-geometry>");
    let (p, _) = enhanced_geometry(&parse(&s), 3600.0, 3600.0).unwrap();
    assert_eq!(p[0].cmds[0], PathCmd::MoveTo(pt(4095.0, 0.0)));
}

#[test]
fn equations_without_names_or_formulas_and_duplicates() {
    let n = parse(
        r#"<draw:enhanced-geometry svg:viewBox="0 0 10 10" draw:enhanced-path="M ?a ?b">
            <draw:equation draw:formula="5"/>
            <draw:equation draw:name="a"/>
            <draw:equation draw:name="a" draw:formula="3"/>
            <draw:equation draw:name="b" draw:formula="4"/>
            <draw:equation draw:name="b" draw:formula="9"/>
        </draw:enhanced-geometry>"#,
    );
    let (p, _) = enhanced_geometry(&n, 3600.0, 3600.0).unwrap();
    assert_eq!(
        p[0].cmds[0],
        PathCmd::MoveTo(pt(3.0, 4.0)),
        "the first of a name wins"
    );
}

#[test]
fn many_modifiers_are_capped() {
    let m = (0..MAX_MODIFIERS + 50)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(close(val_with(&m, &[], "$1023"), 1023.0));
    assert!(close(val_with(&m, &[], "$1024"), 0.0));
}

#[test]
fn deterministic() {
    let a = run_with(
        "0 0 100 100",
        "",
        &[("f", "?g+1"), ("g", "?f+1")],
        "M ?f ?g L ?g ?f",
    );
    let b = run_with(
        "0 0 100 100",
        "",
        &[("f", "?g+1"), ("g", "?f+1")],
        "M ?f ?g L ?g ?f",
    );
    assert_eq!(a, b);
}

#[test]
fn the_xml_reader_cuts_a_path_at_four_kibibytes() {
    // Documented limit of the shared tree reader: a longer path arrives truncated, and the
    // truncated path still draws (what was read), never panics.
    let path = format!("M 0 0 {}", "L 1 1 ".repeat(2000));
    let n = parse(&geom_xml("0 0 100 100", "", &[], &path));
    let (p, _) = enhanced_geometry(&n, 3600.0, 3600.0).unwrap();
    assert!(p[0].cmds.len() < 2001 && p[0].cmds.len() > 100);
}
