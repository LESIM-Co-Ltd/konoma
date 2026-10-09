//! Tests of Fontwork (`fontwork.rs`): the text of a shape with a text path becomes glyph outlines
//! warped into the shape's guide curves.

use super::*;

const W_CM: f64 = 9.0;
const H_CM: f64 = 3.0;

fn shape_xml(text: &str, ty: &str, path: &str) -> String {
    format!(
        r##"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="{W_CM}cm" svg:height="{H_CM}cm"><text:p>{text}</text:p><draw:enhanced-geometry svg:viewBox="0 0 21600 21600" draw:text-path="true" draw:type="{ty}" draw:enhanced-path="{path}"/></draw:custom-shape>"##
    )
}

/// Top and bottom edge: the plain text type.
const PLAIN: &str = "M 0 0 L 21600 0 N M 0 21600 L 21600 21600 N";
/// Top edge sloping down to the right, bottom edge horizontal (a wedge).
const SLOPE: &str = "M 0 0 L 21600 10800 N M 0 21600 L 21600 21600 N";
/// A shallow arch, a cubic from the left, up over the middle and down to the right.
const SHALLOW: &str = "M 0 16000 C 7200 6000 14400 6000 21600 16000 N";
/// One clockwise arc from the left over the top to the right (a half ellipse).
const ARCH: &str = "M 0 10800 W 0 0 21600 21600 0 10800 21600 10800 N";

fn styled(sh: &str) -> ShapeItem {
    only_shape(&op(&slide_page(sh)).auto(&gr(
        r##"draw:fill="solid" draw:fill-color="#aa0000" draw:stroke="solid" svg:stroke-color="#00aa00""##,
    )))
}

fn points(s: &ShapeItem) -> Vec<(f64, f64)> {
    let Geometry::Paths(paths) = &s.geom else {
        panic!("{:?}", s.geom)
    };
    let mut v = Vec::new();
    for p in paths {
        assert_eq!((p.w, p.h), (0.0, 0.0), "box coordinates, no path scale");
        assert!(p.stroke);
        for c in &p.cmds {
            match c {
                sd::PathCmd::MoveTo(q) | sd::PathCmd::LineTo(q) => {
                    v.push((q.x / EMU_CM, q.y / EMU_CM));
                }
                sd::PathCmd::Close => {}
                c => panic!("{c:?}"),
            }
        }
    }
    v
}

fn bounds(v: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let f = |g: fn(&(f64, f64)) -> f64, min: bool| {
        v.iter()
            .map(g)
            .fold(if min { f64::MAX } else { f64::MIN }, |a, b| {
                if min {
                    a.min(b)
                } else {
                    a.max(b)
                }
            })
    };
    (
        f(|p| p.0, true),
        f(|p| p.1, true),
        f(|p| p.0, false),
        f(|p| p.1, false),
    )
}

#[test]
fn plain_fontwork_fills_the_box_with_the_outlines_of_its_text() {
    let s = styled(&shape_xml("Word", "fontwork-plain-text", PLAIN));
    // the text is outlines now: the shape keeps its own fill and line, and no text body
    assert!(s.text.is_none());
    assert_eq!(s.fill, Fill::Solid(Rgba::rgb(0xaa, 0, 0)));
    assert_eq!(
        s.line.as_ref().unwrap().fill,
        Fill::Solid(Rgba::rgb(0, 0xaa, 0))
    );
    let (x0, y0, x1, y1) = bounds(&points(&s));
    // the text spans the whole box, top to bottom and (the advance of the last letter apart)
    // left to right
    assert!((-0.01..0.5).contains(&x0), "{x0}");
    assert!(x1 <= W_CM + 0.01 && x1 > W_CM - 1.0, "{x1}");
    assert!(y0.abs() < 0.01 && (y1 - H_CM).abs() < 0.01, "{y0} {y1}");
}

#[test]
fn a_sloping_top_edge_lifts_the_right_end_of_the_text_down() {
    let s = styled(&shape_xml("Word", "fontwork-slant-down", SLOPE));
    let pts = points(&s);
    let (x0, _, x1, _) = bounds(&pts);
    let top_at = |lo: f64, hi: f64| {
        pts.iter()
            .filter(|p| p.0 >= lo && p.0 <= hi)
            .map(|p| p.1)
            .fold(f64::MAX, f64::min)
    };
    let left = top_at(x0, x0 + 1.0);
    let right = top_at(x1 - 1.0, x1);
    // the top edge falls from y = 0 to y = H / 2 across the box
    assert!(left < 0.6, "{left}");
    assert!(right > H_CM / 2.0 - 0.8, "{right}");
}

#[test]
fn a_single_curve_is_the_baseline_the_letters_stand_on() {
    let s = styled(&shape_xml("Arch", "fontwork-arch-up-curve", SHALLOW));
    let pts = points(&s);
    let (x0, _, x1, _) = bounds(&pts);
    // the letters stand on a curve that rises towards the middle and falls at both ends: the
    // feet of the middle third are higher than the feet at the two ends
    let top = |lo: f64, hi: f64| {
        -pts.iter()
            .filter(|p| p.0 >= lo && p.0 <= hi)
            .map(|p| p.1)
            .fold(f64::MIN, f64::max)
    };
    let third = (x1 - x0) / 3.0;
    let middle = top(x0 + third, x0 + 2.0 * third);
    let left = top(x0, x0 + third * 0.5);
    let right = top(x1 - third * 0.5, x1);
    assert!(
        middle > left + 0.3 && middle > right + 0.3,
        "{left} {middle} {right}"
    );
    // the line is as long as the curve: the text reaches both of its ends
    assert!(x1 - x0 > W_CM * 0.8, "{x0} {x1}");
}

#[test]
fn several_lines_share_the_height() {
    let one = styled(&shape_xml("Ab", "fontwork-plain-text", PLAIN));
    let two = styled(&shape_xml(
        "Ab</text:p><text:p>Cd",
        "fontwork-plain-text",
        PLAIN,
    ));
    let (_, _, _, h1) = bounds(&points(&one));
    let (_, y0, _, h2) = bounds(&points(&two));
    assert!((h1 - H_CM).abs() < 0.01 && (h2 - H_CM).abs() < 0.01 && y0.abs() < 0.01);
    // the second line is in the lower half: there are points on both sides of the middle
    let pts = points(&two);
    assert!(pts.iter().any(|p| p.1 < H_CM / 2.0 - 0.3));
    assert!(pts.iter().any(|p| p.1 > H_CM / 2.0 + 0.3));
    // more outlines than with one line
    assert!(pts.len() > points(&one).len());
}

#[test]
fn a_fontwork_that_cannot_be_warped_stays_text_in_the_fill_colour() {
    // no enhanced path: no guide curves
    let sh = r##"<draw:custom-shape draw:style-name="gr1" svg:x="1cm" svg:y="1cm" svg:width="9cm" svg:height="3cm"><text:p>Word</text:p><draw:enhanced-geometry draw:type="fontwork-arch-up"/></draw:custom-shape>"##;
    let s = styled(sh);
    assert_eq!(s.fill, Fill::None);
    assert!(s.line.is_none());
    assert!(matches!(s.geom, Geometry::Rect));
    assert_eq!(
        s.text.unwrap().paragraphs[0].runs[0].fill,
        Fill::Solid(Rgba::rgb(0xaa, 0, 0))
    );
    // a guide that is not a curve (a single point): the same
    let one = shape_xml("Word", "fontwork-plain-text", "M 100 100 N");
    assert!(styled(&one).text.is_some());
    // text of spaces only: nothing to outline, the same
    let blank = shape_xml("   ", "fontwork-plain-text", PLAIN);
    let sc = scene(
        &op(&slide_page(&blank)).auto(&gr(r##"draw:fill="solid" draw:fill-color="#aa0000""##)),
    );
    assert!(shapes_of(&sc)
        .iter()
        .all(|s| matches!(s.geom, Geometry::Rect)));
}

#[test]
fn a_long_text_is_cut_at_the_budget_and_the_deck_says_so() {
    let long = "W".repeat(super::fontwork::MAX_CHARS + 50);
    let d = doc(
        &op(&slide_page(&shape_xml(&long, "fontwork-plain-text", PLAIN)))
            .auto(&gr(r##"draw:fill="solid" draw:fill-color="#aa0000""##)),
    );
    assert!(d.truncated);
    let s = shapes_of(&d.slide_scenes[0])[0].clone();
    assert!(!points(&s).is_empty());
}

#[test]
fn a_dot_below_the_baseline_stays_inside_the_box() {
    // descenders: the box of the glyphs, not of the em, is what fills the shape
    let s = styled(&shape_xml("gypq", "fontwork-plain-text", PLAIN));
    let (_, y0, _, y1) = bounds(&points(&s));
    assert!(y0 > -0.01 && y1 < H_CM + 0.01);
    assert!((y1 - H_CM).abs() < 0.01);
}

#[test]
#[ignore = "writes an image for a human to look at (FONTWORK_OUT)"]
fn fontwork_dump() {
    let Ok(dir) = std::env::var("FONTWORK_OUT") else {
        return;
    };
    for (name, ty, path) in [
        ("arch", "fontwork-arch-up-curve", ARCH),
        ("slope", "fontwork-slant-down", SLOPE),
        ("plain", "fontwork-plain-text", PLAIN),
    ] {
        let sc = scene(&op(&slide_page(&shape_xml("Arch Text", ty, path))).auto(&gr(
            r##"draw:fill="solid" draw:fill-color="#aa0000" draw:stroke="solid" svg:stroke-color="#00aa00""##,
        )));
        let r = sd::render_svg(&sc, &|_| None);
        let img = crate::preview::svg::rasterize_trusted(
            r.svg.as_bytes(),
            std::path::Path::new("/s.svg"),
            2400,
        )
        .unwrap();
        img.save(std::path::Path::new(&dir).join(format!("fontwork-{name}.png")))
            .unwrap();
    }
}

#[test]
fn the_fontwork_of_a_slide_shares_one_budget() {
    // Each shape's text is as long as a shape may warp; the slide warps three of them in full.
    let long = "W".repeat(fontwork::MAX_CHARS);
    let shapes: String = (0..5)
        .map(|_| shape_xml(&long, "fontwork-plain-text", PLAIN))
        .collect();
    let o = op(&slide_page(&shapes)).auto(&gr(
        r##"draw:fill="solid" draw:fill-color="#aa0000" draw:stroke="solid" svg:stroke-color="#00aa00""##,
    ));
    let sc = scene(&o);
    let warped: Vec<bool> = shapes_of(&sc)
        .iter()
        .map(|s| matches!(s.geom, Geometry::Paths(_)) && s.text.is_none())
        .collect();
    assert_eq!(warped, vec![true, true, true, false, false]);
    // The slide says it is not drawn as written; the text of the last two is kept as plain text.
    assert!(sc.truncated);
    assert!(text_of_shape(shapes_of(&sc)[4]).starts_with("WWW"));
}

#[test]
fn a_slide_within_the_fontwork_budget_is_not_marked() {
    let shapes: String = (0..3)
        .map(|_| shape_xml("Hello", "fontwork-plain-text", PLAIN))
        .collect();
    let o = op(&slide_page(&shapes)).auto(&gr(
        r##"draw:fill="solid" draw:fill-color="#aa0000" draw:stroke="solid" svg:stroke-color="#00aa00""##,
    ));
    let sc = scene(&o);
    assert!(!sc.truncated);
    assert!(shapes_of(&sc)
        .iter()
        .all(|s| matches!(s.geom, Geometry::Paths(_))));
}

#[test]
fn the_fontwork_budget_is_each_slides_own() {
    let long = "W".repeat(fontwork::MAX_CHARS);
    let shapes: String = (0..3)
        .map(|_| shape_xml(&long, "fontwork-plain-text", PLAIN))
        .collect();
    let o = op(&format!("{}{}", slide_page(&shapes), slide_page(&shapes))).auto(&gr(
        r##"draw:fill="solid" draw:fill-color="#aa0000" draw:stroke="solid" svg:stroke-color="#00aa00""##,
    ));
    for sc in scenes(&o) {
        assert!(!sc.truncated);
        assert!(shapes_of(&sc)
            .iter()
            .all(|s| matches!(s.geom, Geometry::Paths(_))));
    }
}
