use super::*;
use konoma_plugin::testing::check_manifest;
use konoma_plugin::{Settings, Value};

const FLOW: &str = "graph LR\n  A --> B\n";

fn embed(code: &str) -> Input {
    Input::Embed {
        kind: FENCE_KIND.into(),
        source: code.into(),
        display: false,
    }
}

fn view(input: Input, settings: &Settings, req: &ViewReq) -> Result<View, Failure> {
    let mut s = MermaidPlugin.open(input, &OpenCx::bare()).expect("opens");
    s.view(req, &ViewCx::new(settings, "en"))
}

fn text_of(v: View) -> Vec<String> {
    match v {
        View::Text(TextView {
            source: TextSource::Lines(lines),
            ..
        }) => lines.iter().map(Line::plain).collect(),
        other => panic!("not a text view: {other:?}"),
    }
}

#[test]
fn the_manifest_is_clean_and_binds_the_files_and_the_fence() {
    assert!(check_manifest(&MermaidPlugin).is_empty());
    let m = mermaid_manifest();
    assert_eq!(m.id, MERMAID_ID);
    for name in ["mode", "theme", "curve", "routing", "rows"] {
        assert!(m.settings.iter().any(|s| s.name == name), "{name}");
    }
}

#[test]
fn a_fence_gives_a_trusted_svg_that_zooms_sharply() {
    match view(embed(FLOW), Settings::empty(), &ViewReq::default()).expect("a view") {
        View::Image(iv) => {
            assert!(iv.sharp_zoom);
            match iv.data {
                ImageData::Svg { data, trusted, .. } => {
                    assert!(trusted, "konoma's own renderer made it");
                    assert!(std::str::from_utf8(&data).unwrap().contains("<svg"));
                }
                other => panic!("not an SVG: {other:?}"),
            }
        }
        other => panic!("not an image: {other:?}"),
    }
}

#[test]
fn the_picture_is_what_the_core_draws_with_the_settings() {
    let settings = Settings::new()
        .with("theme", Value::Str("light".into()))
        .with("curve", Value::Str("linear".into()))
        .with("routing", Value::Str("konoma-orthogonal".into()));
    let want =
        konoma_mermaid_core::mermaid_to_svg_flow(FLOW, "light", "linear", "konoma-orthogonal")
            .unwrap();
    let View::Image(iv) = view(embed(FLOW), &settings, &ViewReq::default()).unwrap() else {
        panic!("not an image");
    };
    let ImageData::Svg { data, .. } = iv.data else {
        panic!("not an SVG");
    };
    assert_eq!(&*data, want.as_bytes());
}

#[test]
fn a_file_is_one_diagram_read_when_the_view_is_made() {
    let dir = konoma_test_support::unique_tmp("konoma_mermaid_plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.mmd");
    std::fs::write(&path, FLOW).unwrap();
    let mut s = konoma_plugin::testing::open_file(&MermaidPlugin, &path).expect("opens");
    let cx = ViewCx::new(Settings::empty(), "en");
    assert!(matches!(
        s.view(&ViewReq::default(), &cx),
        Ok(View::Image(_))
    ));
    // An edit between two views shows in the second.
    std::fs::write(&path, "not a diagram at all").unwrap();
    assert!(s.view(&ViewReq::default(), &cx).is_err());
}

#[test]
fn a_missing_file_is_an_io_failure() {
    let mut s = konoma_plugin::testing::open_file(
        &MermaidPlugin,
        std::path::Path::new("/nonexistent/none.mmd"),
    )
    .expect("opens");
    let e = s
        .view(&ViewReq::default(), &ViewCx::bare())
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.kind, konoma_plugin::FailureKind::Io);
}

#[test]
fn a_diagram_that_cannot_be_drawn_is_an_unsupported_failure() {
    let e = view(
        embed("this is not mermaid"),
        Settings::empty(),
        &ViewReq::default(),
    )
    .map(|_| ())
    .unwrap_err();
    assert_eq!(e.kind, konoma_plugin::FailureKind::Unsupported);
}

#[test]
fn mode_text_and_the_text_representation_give_the_box_drawing_diagram() {
    let text_mode = Settings::new().with("mode", Value::Str("text".into()));
    let a = text_of(view(embed(FLOW), &text_mode, &ViewReq::default()).unwrap());
    assert!(!a.is_empty());
    // The same lines come from asking for the representation by name, whatever the mode.
    let req = ViewReq {
        repr: Some(REPR_TEXT.into()),
        ..ViewReq::default()
    };
    let b = text_of(view(embed(FLOW), Settings::empty(), &req).unwrap());
    assert_eq!(a, b);
    // And an explicit "image" beats mode = text.
    let req = ViewReq {
        repr: Some(REPR_IMAGE.into()),
        ..ViewReq::default()
    };
    assert!(matches!(
        view(embed(FLOW), &text_mode, &req),
        Ok(View::Image(_))
    ));
}

#[test]
fn the_text_diagram_follows_the_requested_width() {
    let wide = "graph LR\n  A[a long first box] --> B[a long second box] --> C[a long third box]\n";
    let req = |cols| ViewReq {
        repr: Some(REPR_TEXT.into()),
        cols,
        ..ViewReq::default()
    };
    let at = |cols| text_of(view(embed(wide), Settings::empty(), &req(cols)).unwrap());
    let unlimited = at(0);
    let narrow = at(30);
    assert_ne!(unlimited, narrow, "a narrow width changes the layout");
    assert_eq!(text_diagram(wide, 30).len(), narrow.len());
}

#[test]
fn an_undrawable_text_diagram_is_the_dimmed_source_under_a_note() {
    let lines = text_diagram("not a diagram\nsecond line", 80);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert!(plain[0].starts_with("[cannot render mermaid"), "{plain:?}");
    assert_eq!(plain[1], "  not a diagram");
    assert_eq!(plain[2], "  second line");
    for l in &lines {
        assert!(l.spans.iter().all(|s| s.style.add.contains(Modifier::DIM)));
    }
}

#[test]
fn it_opens_only_a_file_or_a_mermaid_fence() {
    for input in [
        Input::Embed {
            kind: "math".into(),
            source: "x".into(),
            display: false,
        },
        Input::Content {
            content_type: "markdown".into(),
            text: String::new(),
            resources: Default::default(),
            origin: konoma_plugin::Origin::File("a.md".into()),
        },
    ] {
        assert!(MermaidPlugin.open(input, &OpenCx::bare()).is_err());
    }
}

#[test]
fn the_layout_names_a_picture_and_a_text_representation() {
    let s = MermaidPlugin.open(embed(FLOW), &OpenCx::bare()).unwrap();
    let ids: Vec<String> = s.layout().reprs.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, [REPR_IMAGE, REPR_TEXT]);
}

#[test]
fn an_empty_line_of_the_text_diagram_has_no_span() {
    // The Markdown renderer's own text diagram gives spanless empty lines; the host compares them.
    let lines = text_diagram("pie title P\n  \"a\" : 3\n  \"b\" : 4\n", 0);
    assert!(lines.iter().any(|l| l.spans.is_empty()), "{lines:?}");
    assert!(lines
        .iter()
        .all(|l| !l.spans.iter().any(|s| s.text.is_empty())));
}

#[test]
fn a_cancelled_open_or_view_is_a_cancel() {
    use konoma_plugin::{Cancel, FailureKind};
    let cancelled = || Cancel::from_fn(|| true);
    let err = MermaidPlugin
        .open(embed(FLOW), &OpenCx::bare().with_cancel(cancelled()))
        .err()
        .expect("a cancelled open");
    assert_eq!(err.kind, FailureKind::Cancelled);
    let mut session = MermaidPlugin.open(embed(FLOW), &OpenCx::bare()).unwrap();
    let err = session
        .view(
            &ViewReq::default(),
            &ViewCx::bare().with_cancel(cancelled()),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Cancelled);
}
