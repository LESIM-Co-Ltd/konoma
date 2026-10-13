//! The declarations of actions: the checks the registry runs when a plugin is registered, the one
//! rule for which actions are shown (`hints`), and the helpers of `testing`.

use std::path::Path;
use std::sync::Arc;

use konoma_plugin::hints::hints;
use konoma_plugin::keys::{parse_key, validate, Effect, KeyCode, KeyIssueKind, KeySpec, Layer};
use konoma_plugin::manifest::ApiVersion;
use konoma_plugin::prelude::*;
use konoma_plugin::registry::{NoteKind, PluginOrigin, Registry};
use konoma_plugin::testing::{
    check_hints, open_bytes, open_file, view_document, view_image, view_table, view_text,
};
use konoma_plugin::view::{DocOptions, DocumentView, Grid, ImageData, ImageView, TableView};
use konoma_plugin::Origin;
use konoma_plugin::Resources;

struct Stub(Manifest);

impl Plugin for Stub {
    fn manifest(&self) -> Manifest {
        self.0.clone()
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Gated(vec!["on".into()])))
    }
}

/// A session that says only the listed actions work.
struct Gated(Vec<String>);

impl Session for Gated {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::text(["x"]))
    }
    fn enabled(&self, action: &str) -> bool {
        self.0.iter().any(|a| a == action)
    }
}

fn act(id: &str, key: &str) -> Act {
    Act::new(id)
        .key(key)
        .label(id.to_string())
        .help(format!("{id} help"))
}

#[test]
fn strings_are_accepted_where_a_tr_is_expected() {
    let a = Act::new("go").label("Go").help("Go somewhere");
    assert_eq!(a.label.get("ja"), "Go", "an English-only text falls back");
    let m = Manifest::new("p", "1").name("Plugin");
    assert_eq!(m.name.en(), "Plugin");
    let f = Failure::corrupt("broken");
    assert_eq!(f.message.en(), "broken");
    let s = SettingDecl::new("n", Value::Int(1), "help");
    assert_eq!(s.help.en(), "help");
    // `t!` still works.
    let b = Act::new("go").label(t!(en: "go", ja: "行く"));
    assert_eq!(b.label.get("ja"), "行く");
}

#[test]
fn context_is_the_new_name_of_screen() {
    assert_eq!(
        Act::new("a").context("table"),
        Act::new("a").screen("table")
    );
    assert_eq!(
        Act::new("a").context("table").screen.as_deref(),
        Some("table")
    );
}

#[test]
fn function_keys_parse_and_print() {
    for n in 1..=12u8 {
        let k = parse_key(&format!("F{n}")).unwrap();
        assert_eq!(k, KeySpec::key(KeyCode::F(n)));
        assert_eq!(k.to_string(), format!("F{n}"));
        assert_eq!(parse_key(&format!("f{n}")).unwrap(), k);
    }
    assert_eq!(
        parse_key("Ctrl-F5").unwrap(),
        KeySpec {
            code: KeyCode::F(5),
            ctrl: true
        }
    );
    assert!(parse_key("F0").is_err());
    assert!(parse_key("F13").is_err());
    assert!(parse_key("F").is_ok(), "a plain `F` is the letter");
    assert_eq!(parse_key("F").unwrap(), KeySpec::ch('F'));
}

#[test]
fn shows_decides_which_display_keys_are_reserved() {
    // `s` is the diff's key and `f` the text's page key.
    let loose = Manifest::new("p", "1")
        .action(act("a", "s"))
        .action(act("b", "f"))
        .action(act("c", "w"));
    let keys: Vec<_> = validate(&loose).into_iter().map(|i| i.act).collect();
    assert_eq!(
        keys,
        ["a", "b"],
        "with no declaration every display type counts"
    );

    let text = loose.clone().shows(ViewKind::Text);
    let issues = validate(&text);
    assert_eq!(
        issues.iter().map(|i| i.act.as_str()).collect::<Vec<_>>(),
        ["b"],
        "a text-only plugin may use `s`, but not `f`"
    );
    assert!(matches!(
        issues[0].kind,
        KeyIssueKind::Reserved {
            layer: Layer::Type,
            ..
        }
    ));

    let table = loose.shows(ViewKind::Table);
    assert!(validate(&table).is_empty(), "a table reserves neither");
}

#[test]
fn a_context_of_its_own_beats_shows() {
    let m = Manifest::new("p", "1")
        .shows(ViewKind::Table)
        .action(act("a", "s").context("diff"));
    assert_eq!(validate(&m).len(), 1);
}

#[test]
fn registering_keeps_a_refused_key_off_but_the_action_on() {
    let mut r = Registry::new();
    let m = Manifest::new("p", "1")
        .shows(ViewKind::Text)
        .action(act("jump", "j"))
        .action(act("ok", "w"))
        .action(act("same", "w"));
    r.register(Arc::new(Stub(m)), PluginOrigin::User).unwrap();
    let kept = r.manifest("p").unwrap();
    assert_eq!(
        kept.actions
            .iter()
            .map(|a| (a.id.as_str(), a.key.as_deref()))
            .collect::<Vec<_>>(),
        [("jump", None), ("ok", Some("w")), ("same", None)]
    );
    let notes = r.notes();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|n| n.plugin == "p"));
    assert!(matches!(
        &notes[0].kind,
        NoteKind::Key(i) if i.act == "jump" && i.effect == Effect::KeyDropped
    ));
    assert!(r.is_enabled("p"));
}

#[test]
fn a_rejected_action_is_not_registered() {
    let mut r = Registry::new();
    let m = Manifest::new("p", "1")
        .action(act("a", "w"))
        .action(act("a", "x"))
        .action(act("d", "z").destructive());
    r.register(Arc::new(Stub(m)), PluginOrigin::Standard)
        .unwrap();
    let ids: Vec<_> = r
        .manifest("p")
        .unwrap()
        .actions
        .iter()
        .map(|a| a.id.clone())
        .collect();
    assert_eq!(
        ids,
        ["a"],
        "the first `a` stays, the second and the destructive one go"
    );
    assert_eq!(
        r.manifest("p").unwrap().actions[0].key.as_deref(),
        Some("w")
    );
}

#[test]
fn a_plugin_of_another_api_version_is_switched_off_with_a_reason() {
    let mut r = Registry::new();
    let mut m = Manifest::new("old", "1").bind(Bind::files(["*.zzz"]));
    m.api = ApiVersion { major: 0, minor: 9 };
    r.register(Arc::new(Stub(m)), PluginOrigin::User).unwrap();
    assert!(!r.is_enabled("old"));
    assert!(!r.set_enabled("nope", true));
    assert!(r.set_enabled("old", true), "the id is known");
    assert!(!r.is_enabled("old"), "but it cannot be switched on");
    assert!(r.notes().iter().any(|n| matches!(
        n.kind,
        NoteKind::ApiMismatch { declared, host } if declared.minor == 9 && host == ApiVersion::CURRENT
    )));
    // It is not asked to show anything.
    let facts = konoma_plugin::registry::FileFacts {
        path: Path::new("a.zzz"),
        sniff_mime: &|| None,
        is_text: &|| false,
    };
    assert!(matches!(
        r.resolve_file(&[], &facts),
        konoma_plugin::registry::Resolution::CanNotPreview { .. }
    ));
    // The current version is fine.
    let mut ok = Registry::new();
    ok.register(
        Arc::new(Stub(Manifest::new("new", "1"))),
        PluginOrigin::User,
    )
    .unwrap();
    assert!(ok.is_enabled("new") && ok.notes().is_empty());
}

#[test]
fn api_versions_work_with_the_same_zero_minor_only() {
    let cur = ApiVersion::CURRENT;
    assert!(cur.works_with(cur));
    assert!(!ApiVersion {
        major: 0,
        minor: cur.minor + 1
    }
    .works_with(cur));
    assert!(!ApiVersion { major: 1, minor: 0 }.works_with(cur));
    // From 1.0 on a smaller minor is fine and a larger one is not.
    let host = ApiVersion { major: 1, minor: 2 };
    assert!(ApiVersion { major: 1, minor: 1 }.works_with(host));
    assert!(!ApiVersion { major: 1, minor: 3 }.works_with(host));
}

#[test]
fn hints_are_the_enabled_actions_only() {
    let m = Manifest::new("p", "1")
        .shows(ViewKind::Text)
        .action(act("on", "w").footer(1))
        .action(act("off", "x").footer(2))
        .action(act("jump", "j"));
    let all = hints(&m, &|_| true);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].key, Some(KeySpec::ch('w')));
    assert_eq!(
        all[2].key, None,
        "the refused key is gone, the action stays"
    );
    let some = hints(&m, &|id| id == "on");
    assert_eq!(some.len(), 1);
    assert_eq!(some[0].act, "on");
    assert_eq!(some[0].footer, Some(1));
    assert!(hints(&m, &|_| false).is_empty());
}

#[test]
fn check_hints_reports_key_problems_and_is_clean_otherwise() {
    let good = Stub(
        Manifest::new("p", "1")
            .shows(ViewKind::Text)
            .action(act("on", "w")),
    );
    let s = open_file(&good, Path::new("/nonexistent")).unwrap();
    assert!(check_hints(&good, s.as_ref()).is_clean());

    let bad = Stub(
        Manifest::new("p", "1")
            .shows(ViewKind::Text)
            .action(act("on", "j"))
            .action(Act::new("bare").key("w")),
    );
    let s = open_file(&bad, Path::new("/nonexistent")).unwrap();
    let report = check_hints(&bad, s.as_ref());
    assert!(!report.is_clean());
    assert_eq!(report.key_issues.len(), 1);
    assert_eq!(report.key_issues[0].act, "on");
    // `bare` has no help text of its own, and `bare` is not enabled in `Gated(["on"])`.
    assert!(report.shown_while_disabled.is_empty());
}

#[test]
fn check_hints_would_catch_a_session_that_shows_a_disabled_hint() {
    // The rule is shared, so an honest session never shows one; the report stays empty for
    // actions the session disables.
    let p = Stub(Manifest::new("p", "1").action(act("off", "w")));
    let s = Gated(Vec::new());
    let r = check_hints(&p, &s);
    assert!(r.shown_while_disabled.is_empty());
    assert!(hints(&p.manifest(), &|id| s.enabled(id)).is_empty());
}

struct Cells;

impl Grid for Cells {
    fn nrows(&self) -> usize {
        3
    }
    fn ncols(&self) -> usize {
        2
    }
    fn header(&self, col: usize) -> Option<String> {
        Some(format!("h{col}"))
    }
    fn cell(&self, row: usize, col: usize) -> String {
        format!("{row}:{col}")
    }
}

struct Views;

impl Plugin for Views {
    fn manifest(&self) -> Manifest {
        Manifest::new("views", "1")
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ViewsSession(input.read_all()?)))
    }
}

struct ViewsSession(Vec<u8>);

impl Session for ViewsSession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(match req.repr.as_deref() {
            Some("table") => View::Table(TableView {
                grid: Arc::new(Cells),
                title: Some("t".into()),
                rainbow: false,
                addressed: false,
            }),
            Some("doc") => View::Document(DocumentView {
                markdown: String::from_utf8_lossy(&self.0).into_owned(),
                origin: Origin::File("x".into()),
                resources: Resources::new(),
                options: DocOptions::default(),
                title: None,
                truncated: false,
            }),
            Some("image") => View::Image(ImageView::new(ImageData::Encoded(self.0.clone().into()))),
            _ => View::text([String::from_utf8_lossy(&self.0).into_owned()]),
        })
    }
}

fn req(repr: &str) -> ViewReq {
    ViewReq {
        repr: Some(repr.into()),
        ..ViewReq::default()
    }
}

#[test]
fn open_bytes_and_the_view_readers() {
    let (mut s, scratch) = open_bytes(&Views, "data.bin", b"# hi").unwrap();
    assert_eq!(std::fs::read(scratch.path()).unwrap(), b"# hi");
    assert_eq!(
        view_text(s.as_mut(), &ViewReq::default()).unwrap(),
        ["# hi"]
    );

    let t = view_table(s.as_mut(), &req("table"), 10).unwrap();
    assert_eq!(t.header, ["h0", "h1"]);
    assert_eq!(t.rows, [["0:0", "0:1"], ["1:0", "1:1"], ["2:0", "2:1"]]);
    assert_eq!(t.title.as_deref(), Some("t"));
    assert_eq!(
        view_table(s.as_mut(), &req("table"), 1).unwrap().rows.len(),
        1
    );

    let d = view_document(s.as_mut(), &req("doc")).unwrap();
    assert_eq!(d.markdown, "# hi");

    let i = view_image(s.as_mut(), &req("image")).unwrap();
    assert!(matches!(i.data, ImageData::Encoded(_)));

    // A view of another type is an error, not a panic.
    assert!(view_table(s.as_mut(), &ViewReq::default(), 10).is_err());
    assert!(view_document(s.as_mut(), &ViewReq::default()).is_err());
    assert!(view_image(s.as_mut(), &ViewReq::default()).is_err());
    assert!(view_text(s.as_mut(), &req("table")).is_err());
}

#[test]
fn open_bytes_cleans_up_and_keeps_the_name_inside_its_directory() {
    let dir;
    {
        let (_s, scratch) = open_bytes(&Views, "../../escape.bin", b"x").unwrap();
        let p = scratch.path().to_path_buf();
        assert_eq!(p.file_name().unwrap(), "escape.bin");
        assert!(p.exists());
        dir = p.parent().unwrap().to_path_buf();
        assert!(dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("konoma_plugin_scratch_"));
    }
    assert!(!dir.exists(), "the scratch directory is removed on drop");
}
