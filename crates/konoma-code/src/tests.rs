use super::*;
use konoma_plugin::testing::{check_manifest, open_file};
use konoma_plugin::{Settings, Value};

fn view_with(settings: &Settings, file: &str) -> TextView {
    let dir = konoma_test_support::unique_tmp("konoma_code_plugin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut session = open_file(&CodePlugin, &path).expect("a file opens");
    let cx = ViewCx::new(settings, "en");
    match session.view(&ViewReq::default(), &cx).expect("a view") {
        View::Text(t) => t,
        other => panic!("not a text view: {other:?}"),
    }
}

#[test]
fn the_manifest_is_clean_and_names_the_old_default_files() {
    assert!(check_manifest(&CodePlugin).is_empty());
    let m = code_manifest();
    assert_eq!(m.id, CODE_ID);
    assert!(m.settings.iter().any(|s| s.name == "syntax_highlight"));
}

#[test]
fn the_view_is_the_file_with_a_styler() {
    let v = view_with(Settings::empty(), "a.rs");
    assert!(matches!(v.source, TextSource::File(_)));
    assert!(v.file_lines);
    assert!(v.styler.is_some());
}

#[test]
fn syntax_highlight_off_returns_no_styler() {
    let off = Settings::new().with("syntax_highlight", Value::Bool(false));
    assert!(view_with(&off, "a.rs").styler.is_none());
}

#[test]
fn only_a_file_opens() {
    let r = CodePlugin.open(
        Input::Embed {
            kind: "x".into(),
            source: String::new(),
            display: false,
        },
        &OpenCx::bare(),
    );
    assert!(r.is_err());
}

#[test]
fn the_styler_gives_the_core_lines_with_colors_and_the_same_text() {
    let src = "fn main() {\n    let x = 42;\n}";
    let mut s = CodeStyler::new("a.rs", "TwoDark");
    let lines = s.style_block(0, src);
    assert_eq!(lines.len(), 3);
    let text: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(text, ["fn main() {", "    let x = 42;", "}"]);
    assert!(
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|sp| matches!(sp.style.fg, Some(Color::Rgb(..)))),
        "a Rust window is colored"
    );
    assert!(
        lines.iter().any(|l| l.spans.len() > 1),
        "keywords split a line into spans"
    );
}

#[test]
fn the_styler_matches_the_core_span_for_span() {
    let src = "// c\nconst A: u8 = 1;\n";
    let core = konoma_code_core::highlight(src, std::path::Path::new("a.rs"), "Dracula");
    let mut s = CodeStyler::new("a.rs", "Dracula");
    let got = s.style_block(0, src);
    assert_eq!(core.len(), got.len());
    for (c, g) in core.iter().zip(&got) {
        assert_eq!(c.spans.len(), g.spans.len());
        for (cs, gs) in c.spans.iter().zip(&g.spans) {
            assert_eq!(cs.content.as_ref(), gs.text);
            assert_eq!(color_of(cs.style.fg.unwrap()), gs.style.fg.unwrap());
            assert_eq!(modifier_of(cs.style.add_modifier), gs.style.add);
        }
    }
}

#[test]
fn a_file_name_without_an_extension_gets_its_grammar_from_the_name() {
    let mut s = CodeStyler::new("Makefile", "TwoDark");
    let lines = s.style_block(0, "all:\n\techo hi\n");
    assert!(lines.iter().any(|l| l.spans.len() > 1));
}

#[test]
fn ready_follows_the_grammar_warm_up() {
    // An extension no other test uses: cold until the styler highlights once.
    let dir = konoma_test_support::unique_tmp("konoma_code_ready");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.nim");
    std::fs::write(&f, "echo 1\n").unwrap();
    let mut s = CodeStyler::new(f.clone(), "TwoDark");
    assert!(!s.ready(), "not warm at first");
    konoma_code_core::warm_file("nim", &f);
    assert!(s.ready(), "warm after the background warm-up");
    let _ = s.style_block(0, "echo 1\n");
    assert!(s.ready());
}

#[test]
fn modifiers_survive_the_conversion() {
    use ratatui::style::Modifier as R;
    let m = modifier_of(R::BOLD | R::ITALIC | R::UNDERLINED);
    assert!(m.contains(Modifier::BOLD) && m.contains(Modifier::ITALIC));
    assert!(m.contains(Modifier::UNDERLINED));
    assert!(!m.contains(Modifier::DIM));
}

#[test]
fn a_cancelled_open_or_view_is_a_cancel() {
    use konoma_plugin::{Cancel, FailureKind};
    let cancelled = || Cancel::from_fn(|| true);
    let dir = konoma_test_support::unique_tmp("konoma_code_cancel");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let input = || Input::File {
        path: path.clone(),
        hint: None,
    };
    let err = CodePlugin
        .open(input(), &OpenCx::bare().with_cancel(cancelled()))
        .err()
        .expect("a cancelled open");
    assert_eq!(err.kind, FailureKind::Cancelled);
    let mut session = CodePlugin.open(input(), &OpenCx::bare()).unwrap();
    let err = session
        .view(
            &ViewReq::default(),
            &ViewCx::bare().with_cancel(cancelled()),
        )
        .unwrap_err();
    assert_eq!(err.kind, FailureKind::Cancelled);
}
