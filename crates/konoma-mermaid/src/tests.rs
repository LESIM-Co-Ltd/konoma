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
    let req = ViewReq::default().repr(REPR_TEXT);
    let b = text_of(view(embed(FLOW), Settings::empty(), &req).unwrap());
    assert_eq!(a, b);
    // And an explicit "image" beats mode = text.
    let req = ViewReq::default().repr(REPR_IMAGE);
    assert!(matches!(
        view(embed(FLOW), &text_mode, &req),
        Ok(View::Image(_))
    ));
}

#[test]
fn the_text_diagram_follows_the_requested_width() {
    let wide = "graph LR\n  A[a long first box] --> B[a long second box] --> C[a long third box]\n";
    let req = |cols| ViewReq::new(cols, 24).repr(REPR_TEXT);
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
fn a_diagram_file_over_the_limit_is_refused_not_read_into_memory() {
    let dir = konoma_test_support::unique_tmp("konoma_mermaid_limit");
    std::fs::create_dir_all(&dir).unwrap();
    // One byte over the limit: refused by its size, without reading it.
    let big = dir.join("big.mmd");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    let mut s = MermaidPlugin
        .open(
            Input::File {
                path: big,
                hint: None,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let err = s.view(&ViewReq::default(), &ViewCx::bare()).unwrap_err();
    assert_eq!(err.kind, konoma_plugin::FailureKind::TooLarge, "{err}");
    // At the limit it is read.
    let ok = dir.join("ok.mmd");
    let mut text = String::from("graph LR\n  A --> B\n");
    text.push_str(&"%".repeat(1000));
    std::fs::write(&ok, &text).unwrap();
    let mut s = MermaidPlugin
        .open(
            Input::File {
                path: ok,
                hint: None,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    assert!(s.view(&ViewReq::default(), &ViewCx::bare()).is_ok());
}

#[test]
fn a_diagram_that_is_not_a_regular_file_is_refused() {
    let dir = konoma_test_support::unique_tmp("konoma_mermaid_not_a_file");
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = MermaidPlugin
        .open(
            Input::File {
                path: dir.to_path_buf(),
                hint: None,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    assert!(s.view(&ViewReq::default(), &ViewCx::bare()).is_err());
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

#[test]
fn the_session_offers_the_picture_first_then_the_text() {
    let s = MermaidPlugin.open(embed(FLOW), &OpenCx::bare()).unwrap();
    let layout = s.layout();
    assert_eq!(layout.default_repr.as_deref(), Some(REPR_IMAGE));
    let names: Vec<&str> = layout.reprs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(names, [REPR_IMAGE, REPR_TEXT]);
    assert_eq!(layout.default_repr_without_pixels, None);
}

#[test]
fn trailing_newlines_do_not_change_the_text_diagram() {
    let plain = text_diagram("graph LR\n  A --> B", 0);
    for tail in ["\n", "\n\n", "\n\n\n"] {
        let got = text_diagram(&format!("graph LR\n  A --> B{tail}"), 0);
        assert_eq!(
            got.iter().map(Line::plain).collect::<Vec<_>>(),
            plain.iter().map(Line::plain).collect::<Vec<_>>(),
            "{tail:?}"
        );
    }
}

/// The declared defaults are what the plugin does when a setting is absent: a view made with no
/// settings is the view made with every declared default written out.
#[test]
fn the_declared_defaults_are_what_an_absent_setting_does() {
    let m = mermaid_manifest();
    let mut explicit = Settings::new();
    for d in &m.settings {
        explicit.set(d.name.clone(), d.default.clone());
    }
    let bytes = |v: View| match v {
        View::Image(iv) => match iv.data {
            ImageData::Svg { data, .. } => data.to_vec(),
            other => panic!("{other:?}"),
        },
        other => panic!("not a picture: {other:?}"),
    };
    let a = bytes(view(embed(FLOW), Settings::empty(), &ViewReq::default()).unwrap());
    let b = bytes(view(embed(FLOW), &explicit, &ViewReq::default()).unwrap());
    assert_eq!(a, b);
    // And every declared default is the constant the module names.
    let get = |n: &str| {
        m.settings
            .iter()
            .find(|s| s.name == n)
            .unwrap()
            .default
            .clone()
    };
    assert_eq!(get("mode"), Value::Str(DEFAULT_MODE.into()));
    assert_eq!(get("theme"), Value::Str(DEFAULT_THEME.into()));
    assert_eq!(get("curve"), Value::Str(DEFAULT_CURVE.into()));
    assert_eq!(get("routing"), Value::Str(DEFAULT_ROUTING.into()));
    assert_eq!(get("rows"), Value::Int(DEFAULT_ROWS));
}
