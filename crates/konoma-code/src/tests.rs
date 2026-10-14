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

#[test]
fn every_modifier_bit_converts_to_its_own_bit_and_nothing_else() {
    use ratatui::style::Modifier as R;
    for (r, p) in [
        (R::BOLD, Modifier::BOLD),
        (R::DIM, Modifier::DIM),
        (R::ITALIC, Modifier::ITALIC),
        (R::UNDERLINED, Modifier::UNDERLINED),
        (R::SLOW_BLINK, Modifier::SLOW_BLINK),
        (R::RAPID_BLINK, Modifier::RAPID_BLINK),
        (R::REVERSED, Modifier::REVERSED),
        (R::HIDDEN, Modifier::HIDDEN),
        (R::CROSSED_OUT, Modifier::CROSSED_OUT),
    ] {
        assert_eq!(modifier_of(r), p, "{r:?}");
    }
    assert_eq!(modifier_of(R::empty()), Modifier::EMPTY);
    // All of them at once.
    let all = R::BOLD
        | R::DIM
        | R::ITALIC
        | R::UNDERLINED
        | R::SLOW_BLINK
        | R::RAPID_BLINK
        | R::REVERSED
        | R::HIDDEN
        | R::CROSSED_OUT;
    let got = modifier_of(all);
    for p in [
        Modifier::BOLD,
        Modifier::DIM,
        Modifier::ITALIC,
        Modifier::UNDERLINED,
        Modifier::SLOW_BLINK,
        Modifier::RAPID_BLINK,
        Modifier::REVERSED,
        Modifier::HIDDEN,
        Modifier::CROSSED_OUT,
    ] {
        assert!(got.contains(p), "{p:?}");
    }
}

#[test]
fn a_style_keeps_what_it_adds_and_what_it_removes() {
    use ratatui::style::{Modifier as R, Style as RS};
    let s = style_of(
        RS::default()
            .add_modifier(R::BOLD)
            .remove_modifier(R::ITALIC | R::DIM),
    );
    assert_eq!(s.add, Modifier::BOLD);
    assert!(s.sub.contains(Modifier::ITALIC) && s.sub.contains(Modifier::DIM));
    assert!(!s.sub.contains(Modifier::BOLD));
    assert_eq!(style_of(RS::default()).sub, Modifier::EMPTY);
}

#[test]
fn the_code_theme_setting_reaches_the_colors() {
    let src = "fn main() { let x = \"s\"; }\n";
    let colors = |settings: &Settings| -> Vec<Option<Color>> {
        let mut v = view_with(settings, "a.rs");
        let styler = v.styler.as_mut().expect("a styler");
        styler
            .style_block(0, src)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.style.fg))
            .collect()
    };
    let dracula = Settings::new().with("code_theme", Value::Str("Dracula".into()));
    let nord = Settings::new().with("code_theme", Value::Str("Nord".into()));
    let default = colors(Settings::empty());
    let d = colors(&dracula);
    let n = colors(&nord);
    assert_ne!(d, default, "a theme of its own changes the colors");
    assert_ne!(n, d);
    // And they are the colors of that theme.
    let mut direct = CodeStyler::new("a.rs", "Dracula");
    let want: Vec<Option<Color>> = direct
        .style_block(0, src)
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.style.fg))
        .collect();
    assert_eq!(d, want);
    // Absent means the default theme.
    let mut def = CodeStyler::new("a.rs", DEFAULT_THEME);
    let want: Vec<Option<Color>> = def
        .style_block(0, src)
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.style.fg))
        .collect();
    assert_eq!(default, want);
}

#[test]
fn a_file_without_an_extension_is_never_warm_whatever_else_is() {
    // Warm `rs` first: a name with no extension must not read as `rs`.
    let mut rs = CodeStyler::new("a.rs", "TwoDark");
    let _ = rs.style_block(0, "fn main() {}\n");
    assert!(rs.ready());
    assert!(!CodeStyler::new("Makefile", "TwoDark").ready());
    assert!(!CodeStyler::new("noext", "TwoDark").ready());
}

#[test]
fn the_declared_default_of_syntax_highlight_is_what_the_view_does_without_it() {
    let m = code_manifest();
    let decl = m
        .settings
        .iter()
        .find(|s| s.name == "syntax_highlight")
        .unwrap();
    assert_eq!(decl.default, Value::Bool(true));
    assert_eq!(
        view_with(Settings::empty(), "a.rs").styler.is_some(),
        matches!(decl.default, Value::Bool(true))
    );
}
