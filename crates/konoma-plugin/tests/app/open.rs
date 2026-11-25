//! Choosing the plugin for a file, and the refusals.

use std::sync::Arc;

use konoma_plugin::app::{
    colors_of, install_panic_hook_with, key_from_crossterm, lang_of, open_file, AppError,
};
use konoma_plugin::prelude::*;
use konoma_plugin::TermCaps;
use konoma_test_support::unique_tmp;

use crate::plugin::TestPlugin;

fn dir(tag: &str) -> konoma_test_support::TmpDir {
    let d = unique_tmp(tag);
    std::fs::create_dir_all(d.as_path()).unwrap();
    d
}

fn open(
    plugins: Vec<Arc<dyn Plugin>>,
    path: &std::path::Path,
) -> Result<konoma_plugin::app::Opened, AppError> {
    open_file(plugins, path, "en", TermCaps::new())
}

fn all() -> Vec<Arc<dyn Plugin>> {
    vec![Arc::new(TestPlugin::all())]
}

fn err(r: Result<konoma_plugin::app::Opened, AppError>) -> AppError {
    match r {
        Ok(o) => panic!("opened (plugin {})", o.plugin_id),
        Err(e) => e,
    }
}

#[test]
fn a_file_the_plugin_binds_opens_and_the_title_is_its_name() {
    let d = dir("app_open_ok");
    let p = d.as_path().join("note.t");
    std::fs::write(&p, "x").unwrap();
    let o = open(all(), &p).expect("opens");
    assert_eq!(o.title, "note.t");
    assert_eq!(o.plugin_id, "app-test");
}

#[test]
fn a_relative_path_is_named_by_its_absolute_form() {
    // The session is given an absolute path (the proc protocol refuses relative ones), and so the
    // refusal names it. The working directory is not changed: other tests share the process under
    // a bare `cargo test`.
    let rel = std::path::Path::new("rel.t");
    let cwd = std::env::current_dir().unwrap();
    let e = err(open(all(), rel));
    match e {
        AppError::NoFile(p) => assert_eq!(p, cwd.join("rel.t")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_file_is_refused_with_its_name_and_exit_code_2() {
    let d = dir("app_open_missing");
    let e = err(open(all(), &d.as_path().join("nope.t")));
    assert!(matches!(e, AppError::NoFile(_)), "{e:?}");
    assert!(e.to_string().contains("nope.t"));
    assert!(e.to_string().contains("no such file"));
    assert_eq!(e.exit_code(), 2);
}

#[test]
fn a_directory_is_refused() {
    let d = dir("app_open_dir");
    let sub = d.as_path().join("sub.t");
    std::fs::create_dir_all(&sub).unwrap();
    let e = err(open(all(), &sub));
    assert!(matches!(e, AppError::NotAFile(_)), "{e:?}");
    assert!(e.to_string().contains("not a file"));
    assert_eq!(e.exit_code(), 2);
}

#[test]
fn a_file_that_cannot_be_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("app_open_perm");
    let p = d.as_path().join("locked.t");
    std::fs::write(&p, "x").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&p).is_ok() {
        // Running as root: the mode does not stop the read, so there is nothing to refuse.
        return;
    }
    let e = err(open(all(), &p));
    assert!(matches!(e, AppError::Unreadable(..)), "{e:?}");
    assert!(e.to_string().contains("cannot be read"));
    assert_eq!(e.exit_code(), 2);
}

#[test]
fn a_form_no_plugin_takes_is_refused_with_code_2_whether_text_or_binary() {
    let d = dir("app_open_unbound");
    let text = d.as_path().join("a.unknown");
    std::fs::write(&text, "plain words").unwrap();
    let bin = d.as_path().join("a.bin");
    std::fs::write(&bin, b"\x00\x01\x02binary").unwrap();
    let none = d.as_path().join("noext");
    std::fs::write(&none, "x").unwrap();
    for (p, said, not_said) in [
        (&text, "reads as text", "`.unknown`"),
        (&bin, "`.bin`", "reads as text"),
    ] {
        let e = err(open(all(), p));
        assert!(matches!(e, AppError::NotHandled { .. }), "{e:?}");
        assert_eq!(e.exit_code(), 2);
        let s = e.to_string();
        assert!(s.contains("does not show"), "{s}");
        // Text says it reads as text (the program does not show plain text); a binary names its
        // extension instead.
        assert!(s.contains(said), "{s}");
        assert!(!s.contains(not_said), "{s}");
    }
    let e = err(open(all(), &none));
    assert_eq!(e.exit_code(), 2);
}

#[test]
fn a_plugin_that_cannot_open_the_file_gives_its_message() {
    let d = dir("app_open_fail");
    let p = d.as_path().join("a.noopen");
    std::fs::write(&p, "x").unwrap();
    let e = err(open(all(), &p));
    assert!(matches!(e, AppError::Open(_)), "{e:?}");
    assert!(e.to_string().contains("the test plugin refuses this file"));
    assert_eq!(e.exit_code(), 1);
}

#[test]
fn the_message_follows_the_language() {
    struct Ja;
    impl Plugin for Ja {
        fn manifest(&self) -> Manifest {
            Manifest::new("ja-test", "0.0.1")
                .bind(Bind::files(["*.ja"]))
                .shows(ViewKind::Text)
        }
        fn open(&self, _i: Input, _c: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::corrupt(t!(en: "broken", ja: "壊れています")))
        }
    }
    let d = dir("app_open_lang");
    let p = d.as_path().join("a.ja");
    std::fs::write(&p, "x").unwrap();
    let ja = open_file(vec![Arc::new(Ja)], &p, "ja", TermCaps::new());
    assert!(err(ja).to_string().contains("壊れています"));
    let en = open_file(vec![Arc::new(Ja)], &p, "en", TermCaps::new());
    assert!(err(en).to_string().contains("broken"));
}

#[test]
fn with_several_plugins_the_file_goes_to_the_one_whose_bind_matches() {
    let a = TestPlugin {
        id: "plugin-a",
        globs: &["*.aa"],
        redraw_ms: 0,
    };
    let b = TestPlugin {
        id: "plugin-b",
        globs: &["*.bb", "*.t"],
        redraw_ms: 0,
    };
    let d = dir("app_open_many");
    for (name, want) in [
        ("x.aa", "plugin-a"),
        ("x.bb", "plugin-b"),
        ("x.t", "plugin-b"),
    ] {
        let p = d.as_path().join(name);
        std::fs::write(&p, "x").unwrap();
        let o = open(
            vec![
                Arc::new(TestPlugin {
                    id: a.id,
                    globs: a.globs,
                    redraw_ms: 0,
                }),
                Arc::new(TestPlugin {
                    id: b.id,
                    globs: b.globs,
                    redraw_ms: 0,
                }),
            ],
            &p,
        )
        .unwrap();
        assert_eq!(o.plugin_id, want, "{name}");
    }
}

#[test]
fn two_plugins_with_one_id_are_refused_before_the_terminal() {
    let d = dir("app_open_dup");
    let p = d.as_path().join("x.t");
    std::fs::write(&p, "x").unwrap();
    let e = err(open(
        vec![Arc::new(TestPlugin::all()), Arc::new(TestPlugin::all())],
        &p,
    ));
    assert!(matches!(e, AppError::Plugin(_)), "{e:?}");
    assert!(e.to_string().contains("already registered"), "{e}");
}

#[test]
fn a_plugin_with_a_bad_manifest_is_refused() {
    let bad = TestPlugin {
        id: "Bad Id",
        globs: &["*.t"],
        redraw_ms: 0,
    };
    let d = dir("app_open_bad");
    let p = d.as_path().join("x.t");
    std::fs::write(&p, "x").unwrap();
    let e = err(open(vec![Arc::new(bad)], &p));
    assert!(matches!(e, AppError::Plugin(_)), "{e:?}");
}

#[test]
fn error_text_carries_no_control_character() {
    let e = AppError::NoFile("a\u{1b}[31mb".into());
    assert!(!e.to_string().contains('\u{1b}'));
    let e = AppError::Open("x\u{7}y".into());
    assert!(!e.to_string().contains('\u{7}'));
}

#[test]
fn the_exit_codes() {
    assert_eq!(AppError::NoFile("x".into()).exit_code(), 2);
    assert_eq!(AppError::NotAFile("x".into()).exit_code(), 2);
    assert_eq!(
        AppError::Unreadable("x".into(), std::io::Error::other("e")).exit_code(),
        2
    );
    assert_eq!(AppError::Open("x".into()).exit_code(), 1);
    assert_eq!(AppError::NoTerminal.exit_code(), 1);
    assert_eq!(AppError::Terminal("x".into()).exit_code(), 1);
    assert_eq!(AppError::Plugin("x".into()).exit_code(), 1);
    assert_eq!(
        AppError::NotHandled {
            path: "x".into(),
            why: "y".into()
        }
        .exit_code(),
        2
    );
}

#[test]
fn language_and_colors_come_from_the_environment_values() {
    assert_eq!(lang_of("ja_JP.UTF-8"), "ja");
    assert_eq!(lang_of("ja"), "ja");
    assert_eq!(lang_of("en_US.UTF-8"), "en");
    assert_eq!(lang_of(""), "en");
    assert_eq!(lang_of("C"), "en");
    assert_eq!(colors_of("truecolor", "xterm"), ColorDepth::TrueColor);
    assert_eq!(colors_of("24bit", "xterm"), ColorDepth::TrueColor);
    assert_eq!(colors_of("", "xterm-256color"), ColorDepth::Ansi256);
    assert_eq!(colors_of("", "xterm"), ColorDepth::Ansi16);
    assert_eq!(colors_of("", ""), ColorDepth::Ansi16);
    // truecolor wins over a 256color TERM.
    assert_eq!(
        colors_of("truecolor", "xterm-256color"),
        ColorDepth::TrueColor
    );
}

mod keys {
    use super::*;
    use crossterm::event::{
        KeyCode as C, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers as M,
    };

    fn ev(code: C, mods: M, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind,
            state: KeyEventState::NONE,
        }
    }

    fn k(code: C, mods: M) -> Option<Key> {
        key_from_crossterm(&ev(code, mods, KeyEventKind::Press))
    }

    #[test]
    fn plain_keys_map_one_to_one() {
        assert_eq!(k(C::Char('j'), M::NONE), Some(Key::char('j')));
        assert_eq!(k(C::Down, M::NONE), Some(Key::new(KeyCode::Down)));
        assert_eq!(k(C::PageUp, M::NONE), Some(Key::new(KeyCode::PageUp)));
        assert_eq!(k(C::Esc, M::NONE), Some(Key::new(KeyCode::Esc)));
        assert_eq!(k(C::F(5), M::NONE), Some(Key::new(KeyCode::F(5))));
    }

    #[test]
    fn shift_is_in_the_case_not_in_a_bit_for_chars_and_backtab() {
        assert_eq!(k(C::Char('G'), M::SHIFT), Some(Key::char('G')));
        assert_eq!(k(C::BackTab, M::SHIFT), Some(Key::new(KeyCode::BackTab)));
        // Other keys keep Shift.
        assert_eq!(
            k(C::Left, M::SHIFT),
            Some(Key::new(KeyCode::Left).with_mods(KeyMods::SHIFT))
        );
    }

    #[test]
    fn control_and_alt_are_kept() {
        assert_eq!(k(C::Char('c'), M::CONTROL), Some(Key::ctrl('c')));
        assert_eq!(
            k(C::Char('x'), M::ALT | M::CONTROL),
            Some(Key::char('x').with_mods(KeyMods::ALT | KeyMods::CTRL))
        );
    }

    #[test]
    fn a_release_and_an_unnamed_key_are_dropped() {
        assert_eq!(
            key_from_crossterm(&ev(C::Char('j'), M::NONE, KeyEventKind::Release)),
            None
        );
        assert_eq!(k(C::Insert, M::NONE), None);
        assert_eq!(k(C::CapsLock, M::NONE), None);
        // A repeat is a press.
        assert_eq!(
            key_from_crossterm(&ev(C::Char('j'), M::NONE, KeyEventKind::Repeat)),
            Some(Key::char('j'))
        );
    }
}

#[test]
fn the_panic_hook_restores_the_terminal_before_the_message_is_printed() {
    use std::sync::Mutex;
    static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    fn restore() {
        ORDER.lock().unwrap().push("restore");
    }
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| ORDER.lock().unwrap().push("previous hook")));
    install_panic_hook_with(restore);
    let r = std::panic::catch_unwind(|| panic!("boom"));
    // Put the test harness's hook back before asserting.
    std::panic::set_hook(original);
    assert!(r.is_err());
    assert_eq!(*ORDER.lock().unwrap(), vec!["restore", "previous hook"]);
}

/// A plugin bound by content type only (like `image` and `video`), as the media program has.
struct MimePlugin;

impl Plugin for MimePlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("mime-test", "0.0.1")
            .bind(Bind::mime("image/*"))
            .shows(ViewKind::Text)
    }

    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Err(Failure::unsupported(t!(en: "opened")))
    }
}

fn mime_only() -> Vec<Arc<dyn Plugin>> {
    vec![Arc::new(MimePlugin)]
}

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff";

#[test]
fn a_plugin_bound_by_content_type_takes_a_file_by_its_content_whatever_its_name() {
    let d = dir("app_open_mime");
    for (name, bytes) in [
        ("pic", PNG),
        ("pic.txt", PNG),
        ("pic.png", PNG),
        ("anim", GIF),
        ("anim.dat", GIF),
    ] {
        let p = d.as_path().join(name);
        std::fs::write(&p, bytes).unwrap();
        // The plugin is picked (then its open fails on purpose): the refusal is the plugin's own.
        let e = err(open(mime_only(), &p));
        assert!(
            matches!(e, AppError::Open(ref m) if m == "opened"),
            "{name}: {e}"
        );
        assert_eq!(
            konoma_plugin::app::pick_plugin(mime_only(), &p).unwrap(),
            "mime-test",
            "{name}"
        );
    }
}

#[test]
fn a_file_that_is_not_a_picture_is_not_taken_by_its_name() {
    let d = dir("app_open_mime_no");
    let p = d.as_path().join("fake.png");
    std::fs::write(&p, "plain words\n").unwrap();
    assert!(matches!(
        err(open(mime_only(), &p)),
        AppError::NotHandled { .. }
    ));
}

#[test]
fn the_text_judgment_is_the_one_konoma_makes() {
    let d = dir("app_open_textjudge");
    let why = |bytes: &[u8]| {
        let p = d.as_path().join("f.zzz");
        std::fs::write(&p, bytes).unwrap();
        match err(open(all(), &p)) {
            AppError::NotHandled { why, .. } => why,
            e => panic!("{e}"),
        }
    };
    // An empty file is text.
    assert!(why(b"").contains("reads as text"));
    assert!(why(b"words\n").contains("reads as text"));
    // A NUL in the leading bytes makes it binary; one after the leading 8 KiB does not.
    assert!(!why(&[b'a', 0, b'b']).contains("reads as text"));
    let mut late = vec![b'a'; 9000];
    late[8500] = 0;
    assert!(why(&late).contains("reads as text"));
    let mut early = vec![b'a'; 9000];
    early[8000] = 0;
    assert!(!why(&early).contains("reads as text"));
}
