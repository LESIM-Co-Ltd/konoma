use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use konoma_plugin::input::Resources;
use konoma_plugin::prelude::*;
use konoma_plugin::settings::{Settings, Value};
use konoma_plugin::style::Modifier;
use konoma_plugin::view::{DocOptions, Layout};
use konoma_plugin::{Cancel, Failure, FailureKind, Verb, VerbDecl};

#[test]
fn tr_falls_back_to_english() {
    let t = t!(en: "width", ja: "幅");
    assert_eq!(t.get("en"), "width");
    assert_eq!(t.get("ja"), "幅");
    assert_eq!(t.get("fr"), "width");
    let only = t!(en: "x");
    assert_eq!(only.get("ja"), "x");
    assert_eq!(Tr::new("a").with("de", "b").get("de"), "b");
    assert_eq!(Tr::new("a").with("en", "c").get("en"), "c");
    assert_eq!(Tr::new("a").ja("1").ja("2").get("ja"), "2");
    assert_eq!(Tr::new("a"), Tr::new(String::from("a")));
    let _from: Tr = "lit".into();
}

#[test]
fn modifier_bits_match_ratatui() {
    // ratatui 0.30 `Modifier` values.
    assert_eq!(Modifier::BOLD.bits(), 0b0000_0000_0001);
    assert_eq!(Modifier::DIM.bits(), 0b0000_0000_0010);
    assert_eq!(Modifier::ITALIC.bits(), 0b0000_0000_0100);
    assert_eq!(Modifier::UNDERLINED.bits(), 0b0000_0000_1000);
    assert_eq!(Modifier::SLOW_BLINK.bits(), 0b0000_0001_0000);
    assert_eq!(Modifier::RAPID_BLINK.bits(), 0b0000_0010_0000);
    assert_eq!(Modifier::REVERSED.bits(), 0b0000_0100_0000);
    assert_eq!(Modifier::HIDDEN.bits(), 0b0000_1000_0000);
    assert_eq!(Modifier::CROSSED_OUT.bits(), 0b0001_0000_0000);
    assert_eq!(Modifier::EMPTY.bits(), 0);
}

#[test]
fn modifier_operations() {
    let mut m = Modifier::BOLD | Modifier::ITALIC;
    assert!(m.contains(Modifier::BOLD) && m.contains(Modifier::ITALIC));
    assert!(!m.contains(Modifier::DIM));
    assert!(m.contains(Modifier::EMPTY));
    m.insert(Modifier::DIM);
    assert!(m.contains(Modifier::BOLD | Modifier::DIM));
    assert!(Modifier::EMPTY.is_empty());
}

#[test]
fn style_span_line_builders() {
    let s = Style::new()
        .fg(Color::Rgb(1, 2, 3))
        .bg(Color::Indexed(9))
        .bold()
        .sub_modifier(Modifier::DIM);
    assert_eq!(s.fg, Some(Color::Rgb(1, 2, 3)));
    assert!(s.add.contains(Modifier::BOLD) && s.sub.contains(Modifier::DIM));
    let line = Line::from(vec![Span::raw("ab"), Span::styled("cd", s)]);
    assert_eq!(line.plain(), "abcd");
    assert_eq!(Line::raw("x").spans.len(), 1);
    assert_eq!(Line::from("y"), Line::raw("y"));
    assert_eq!(Style::default().fg, None);
}

#[test]
fn color_variants_are_in_ratatui_order() {
    // The host maps by name, but the declaration order is part of the contract in the design.
    use konoma_plugin::style::Color::*;
    let order = [
        Reset,
        Black,
        Red,
        Green,
        Yellow,
        Blue,
        Magenta,
        Cyan,
        Gray,
        DarkGray,
        LightRed,
        LightGreen,
        LightYellow,
        LightBlue,
        LightMagenta,
        LightCyan,
        White,
    ];
    assert_eq!(order.len(), 17);
    let idx = |c: konoma_plugin::style::Color| order.iter().position(|x| *x == c);
    assert_eq!(idx(Gray), Some(8));
    assert_eq!(idx(White), Some(16));
}

#[test]
fn settings_getters_fall_back_on_missing_or_wrong_type() {
    let s = Settings::new()
        .with("on", Value::Bool(true))
        .with("n", Value::Int(7))
        .with("f", Value::Float(1.5))
        .with("name", Value::Str("x".into()))
        .with(
            "list",
            Value::List(vec![
                Value::Str("a".into()),
                Value::Int(1),
                Value::Str("b".into()),
            ]),
        );
    assert!(s.bool("on", false));
    assert!(s.bool("missing", true));
    assert!(!s.bool("n", false));
    assert_eq!(s.int("n", 0), 7);
    assert_eq!(s.int("name", 3), 3);
    assert_eq!(s.float("f", 0.0), 1.5);
    assert_eq!(s.float("n", 0.0), 7.0);
    assert_eq!(s.str("name", "d"), "x");
    assert_eq!(s.str("n", "d"), "d");
    assert_eq!(s.list_str("list"), vec!["a", "b"]);
    assert!(s.list_str("name").is_empty());
    assert!(Settings::empty().is_empty());
    assert!(s.get("nope").is_none());
}

#[test]
fn cancel_reads_the_flag() {
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let c = Cancel::from_fn(move || f2.load(Ordering::SeqCst));
    let c2 = c.clone();
    assert!(!c.is_cancelled());
    flag.store(true, Ordering::SeqCst);
    assert!(c.is_cancelled() && c2.is_cancelled());
    assert!(!Cancel::never().is_cancelled());
}

#[test]
fn failure_constructors_and_io_conversion() {
    let kinds = [
        (Failure::unsupported(Tr::new("a")), FailureKind::Unsupported),
        (Failure::corrupt(Tr::new("a")), FailureKind::Corrupt),
        (Failure::too_large(Tr::new("a")), FailureKind::TooLarge),
        (Failure::too_heavy(Tr::new("a")), FailureKind::TooHeavy),
        (Failure::timeout(Tr::new("a")), FailureKind::Timeout),
        (Failure::cancelled(Tr::new("a")), FailureKind::Cancelled),
        (Failure::io(Tr::new("a")), FailureKind::Io),
        (Failure::external(Tr::new("a")), FailureKind::External),
    ];
    for (f, k) in kinds {
        assert_eq!(f.kind, k);
        assert!(f.detail.is_none());
    }
    let e = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
    let f: Failure = e.into();
    assert_eq!(f.kind, FailureKind::Io);
    assert_eq!(f.detail.as_deref(), Some("gone"));
    assert!(f.to_string().contains("gone"));
    assert_eq!(f.message.get("ja"), "入出力エラー");
}

#[test]
fn read_all_only_reads_files() {
    let i = Input::Embed {
        kind: "math".into(),
        source: "x".into(),
        display: false,
    };
    assert_eq!(i.read_all().unwrap_err().kind, FailureKind::Unsupported);
}

#[test]
fn manifest_builder_defaults() {
    let m = Manifest::new("id", "1.2.3");
    assert_eq!(m.api, konoma_plugin::ApiVersion { major: 0, minor: 1 });
    assert_eq!(m.name.en(), "id");
    assert!(m.binds.is_empty() && m.actions.is_empty() && m.screens.is_empty());
    let m = m
        .name(t!(en: "N", ja: "名"))
        .verb(
            VerbDecl::new(Verb::Edit)
                .launcher(konoma_plugin::Launcher::Editor)
                .line_token(),
        )
        .screen("main");
    assert_eq!(m.name.get("ja"), "名");
    assert!(m.verbs[0].line_token);
    let a = Act::new("x").key("w").footer(3).writes();
    assert_eq!(a.footer, Some(3));
    assert!(a.writes && !a.destructive);
}

#[test]
fn view_defaults() {
    assert_eq!(Layout::single(), Layout::default());
    let o = DocOptions::default();
    assert_eq!(
        (
            o.details.as_str(),
            o.table_align.as_str(),
            o.image_align.as_str()
        ),
        ("auto", "left", "center")
    );
    assert_eq!(o.task_states, vec![' ', 'x']);
    let r = ViewReq::default();
    assert_eq!((r.cols, r.rows, r.repr, r.page), (80, 24, None, None));
    assert!(Resources::new().is_empty());
    match View::text(["a", "b"]) {
        View::Text(t) => assert!(!t.file_lines),
        other => panic!("{other:?}"),
    }
}
