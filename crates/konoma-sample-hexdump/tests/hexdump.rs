use konoma_plugin::prelude::*;
use konoma_plugin::testing::{check_hints, check_manifest, open_bytes, send_keys, view_text};
use konoma_sample_hexdump::Hexdump;

#[test]
fn hex_lines_include_a_short_final_row() {
    let (mut session, _file) =
        open_bytes(&Hexdump, "a.bin", b"ABCDEFGHIJKLMNOP\0\x7f\xff ").unwrap();
    assert_eq!(
        view_text(&mut *session, &ViewReq::new(100, 20)).unwrap(),
        vec![
            "00000000  41 42 43 44 45 46 47 48 49 4a 4b 4c 4d 4e 4f 50  |ABCDEFGHIJKLMNOP|",
            "00000010  00 7f ff 20                                      |... |",
        ]
    );
}

#[test]
fn empty_file_has_no_rows() {
    let (mut session, _file) = open_bytes(&Hexdump, "empty.bin", b"").unwrap();
    assert!(view_text(&mut *session, &ViewReq::new(100, 20))
        .unwrap()
        .is_empty());
}

#[test]
fn width_action_toggles_and_redraws() {
    let (mut session, _file) = open_bytes(&Hexdump, "a.bin", b"ABCDEFGHIJKLMNOP").unwrap();
    let manifest = Hexdump.manifest();
    let action = manifest
        .actions
        .iter()
        .find(|a| a.key.as_deref() == Some("w"))
        .unwrap();
    assert_eq!(action.id, "width");
    let req = ViewReq::new(100, 20);
    let before = view_text(&mut *session, &req).unwrap();
    assert!(matches!(
        session.act(&action.id, &ActCx::bare()).unwrap(),
        Outcome::Redraw
    ));
    assert_eq!(
        view_text(&mut *session, &req).unwrap(),
        vec![
            "00000000  41 42 43 44 45 46 47 48  |ABCDEFGH|",
            "00000008  49 4a 4b 4c 4d 4e 4f 50  |IJKLMNOP|",
        ]
    );
    assert!(matches!(
        session.act("width", &ActCx::bare()).unwrap(),
        Outcome::Redraw
    ));
    assert_eq!(view_text(&mut *session, &req).unwrap(), before);
}

#[test]
fn unknown_action_is_unhandled() {
    let (mut session, _file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    assert!(matches!(
        session.act("unknown", &ActCx::bare()).unwrap(),
        Outcome::Unhandled
    ));
}

#[test]
fn manifest_and_hints_are_clean() {
    let (session, _file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    assert!(check_manifest(&Hexdump).is_empty());
    assert!(check_hints(&Hexdump, &*session).is_clean());
    let action = &Hexdump.manifest().actions[0];
    assert_eq!(action.label.get("en"), "width");
    assert_eq!(action.label.get("ja"), "幅");
    assert_eq!(action.help.get("en"), "Switch 8/16 bytes per row");
    assert_eq!(action.help.get("ja"), "1行のバイト数を8/16に切り替え");
}

#[test]
fn sdk_helpers_open_view_and_send_raw_keys() {
    let (mut session, file) = open_bytes(&Hexdump, "a.bin", b"A").unwrap();
    let mut reopened = konoma_plugin::testing::open_file(&Hexdump, file.path()).unwrap();
    assert_eq!(
        view_text(&mut *reopened, &ViewReq::new(100, 20))
            .unwrap()
            .len(),
        1
    );
    let outcomes = send_keys(&mut *session, &[Key::char('x')])
        .outcomes()
        .unwrap();
    assert!(matches!(outcomes.as_slice(), [Outcome::Unhandled]));
}
