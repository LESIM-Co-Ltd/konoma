use super::*;
use konoma_plugin::testing::check_manifest;
use konoma_plugin::{Settings, Value};

fn embed(kind: &str, latex: &str, display: bool) -> Input {
    Input::Embed {
        kind: kind.into(),
        source: latex.into(),
        display,
    }
}

fn view(input: Input, settings: &Settings) -> Result<View, Failure> {
    let mut s = MathPlugin.open(input, &OpenCx::bare()).expect("opens");
    s.view(&ViewReq::default(), &ViewCx::new(settings, "en"))
}

fn svg_of(v: View) -> Vec<u8> {
    match v {
        View::Image(iv) => match iv.data {
            ImageData::Svg { data, trusted, .. } => {
                assert!(trusted, "konoma's own renderer made it");
                data.to_vec()
            }
            other => panic!("not an SVG: {other:?}"),
        },
        other => panic!("not an image: {other:?}"),
    }
}

#[test]
fn the_manifest_is_clean_and_binds_the_two_embedded_kinds_but_no_file() {
    assert!(check_manifest(&MathPlugin).is_empty());
    let m = math_manifest();
    assert_eq!(m.id, MATH_ID);
    assert!(m.verbs.is_empty(), "a formula has no file to edit or open");
    assert_eq!(m.binds.len(), 2);
}

#[test]
fn a_formula_is_the_svg_the_core_draws_in_the_given_color() {
    let settings = Settings::new().with("color", Value::Str("#ff8800".into()));
    for display in [false, true] {
        let got = svg_of(view(embed("math", "x^2 + y^2", display), &settings).unwrap());
        let want = konoma_math_core::latex_to_svg("x^2 + y^2", display, "#ff8800").unwrap();
        assert_eq!(got, want.as_bytes(), "display = {display}");
    }
}

#[test]
fn the_default_color_is_the_light_gray() {
    let got = svg_of(view(embed("math", "x", false), Settings::empty()).unwrap());
    let want = konoma_math_core::latex_to_svg("x", false, DEFAULT_COLOR).unwrap();
    assert_eq!(got, want.as_bytes());
}

#[test]
fn a_math_fence_is_a_block_formula() {
    let from_fence =
        svg_of(view(embed(FENCE_KIND, "\\frac{a}{b}", false), Settings::empty()).unwrap());
    let block = svg_of(view(embed(MATH_KIND, "\\frac{a}{b}", true), Settings::empty()).unwrap());
    assert_eq!(from_fence, block);
}

#[test]
fn a_formula_that_cannot_be_drawn_is_an_unsupported_failure() {
    let e = view(embed("math", "\\frac{", false), Settings::empty())
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.kind, konoma_plugin::FailureKind::Unsupported);
    let e = view(embed("math", "   ", false), Settings::empty())
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.kind, konoma_plugin::FailureKind::Unsupported);
}

#[test]
fn mode_text_keeps_the_source_as_text() {
    let settings = Settings::new().with("mode", Value::Str("text".into()));
    match view(embed("math", "a+b", false), &settings).unwrap() {
        View::Text(TextView {
            source: TextSource::Lines(l),
            ..
        }) => assert_eq!(l[0].plain(), "a+b"),
        other => panic!("not text: {other:?}"),
    }
}

#[test]
fn it_opens_only_a_formula() {
    let file = Input::File {
        path: "a.tex".into(),
        hint: None,
    };
    assert!(MathPlugin.open(file, &OpenCx::bare()).is_err());
    assert!(MathPlugin
        .open(embed("fence:mermaid", "x", false), &OpenCx::bare())
        .is_err());
}

#[test]
fn a_cancelled_open_or_view_is_a_cancel() {
    use konoma_plugin::{Cancel, FailureKind};
    let cancelled = || Cancel::from_fn(|| true);
    let input = || embed(MATH_KIND, "x^2", false);
    let err = MathPlugin
        .open(input(), &OpenCx::bare().with_cancel(cancelled()))
        .err()
        .expect("a cancelled open");
    assert_eq!(err.kind, FailureKind::Cancelled);
    let mut session = MathPlugin.open(input(), &OpenCx::bare()).unwrap();
    let err = session
        .view(
            &ViewReq::default(),
            &ViewCx::bare().with_cancel(cancelled()),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Cancelled);
}
