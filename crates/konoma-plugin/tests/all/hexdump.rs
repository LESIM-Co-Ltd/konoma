//! The hello-world plugin of the design (4-10), written against the public API only.

use konoma_plugin::prelude::*;
use konoma_plugin::testing::{check_manifest, open_file, view_text};
use konoma_test_support::unique_tmp;

struct Hexdump;

impl Plugin for Hexdump {
    fn manifest(&self) -> Manifest {
        Manifest::new("hexdump", env!("CARGO_PKG_VERSION"))
            .bind(Bind::files(["*.bin"]))
            .shows(ViewKind::Text)
            .action(
                Act::new("width")
                    .key("w")
                    .label(t!(en: "width", ja: "幅"))
                    .help(
                        t!(en: "Switch 8/16 bytes per row", ja: "1 行のバイト数を 8/16 で切り替え"),
                    ),
            )
    }
    fn open(&self, input: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Dump {
            bytes: input.read_all()?,
            width: 16,
        }))
    }
}

struct Dump {
    bytes: Vec<u8>,
    width: usize,
}

fn hex_line(chunk: &[u8]) -> String {
    chunk
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Session for Dump {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::text(self.bytes.chunks(self.width).map(hex_line)))
    }
    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        if action == "width" {
            self.width = if self.width == 16 { 8 } else { 16 };
            return Ok(Outcome::Redraw);
        }
        Ok(Outcome::Unhandled)
    }
}

fn sample() -> (konoma_test_support::TmpDir, std::path::PathBuf) {
    let dir = unique_tmp("konoma_plugin_hexdump");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.bin");
    std::fs::write(&path, (0u8..20).collect::<Vec<_>>()).unwrap();
    (dir, path)
}

#[test]
fn opens_and_shows_sixteen_bytes_per_row() {
    let (_dir, path) = sample();
    let mut s = open_file(&Hexdump, &path).unwrap();
    let lines = view_text(s.as_mut(), &ViewReq::default()).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f");
    assert_eq!(lines[1], "10 11 12 13");
}

#[test]
fn width_action_toggles_between_16_and_8() {
    let (_dir, path) = sample();
    let mut s = open_file(&Hexdump, &path).unwrap();
    assert_eq!(s.act("width", &ActCx::bare()).unwrap(), Outcome::Redraw);
    assert_eq!(view_text(s.as_mut(), &ViewReq::default()).unwrap().len(), 3);
    s.act("width", &ActCx::bare()).unwrap();
    assert_eq!(view_text(s.as_mut(), &ViewReq::default()).unwrap().len(), 2);
}

#[test]
fn unknown_action_is_unhandled_and_enabled_defaults_true() {
    let (_dir, path) = sample();
    let mut s = open_file(&Hexdump, &path).unwrap();
    assert_eq!(s.act("nope", &ActCx::bare()).unwrap(), Outcome::Unhandled);
    assert!(s.enabled("width"));
}

#[test]
fn manifest_has_no_key_issues_and_help_in_both_languages() {
    assert!(check_manifest(&Hexdump).is_empty());
    let m = Hexdump.manifest();
    assert_eq!(m.actions[0].label.get("ja"), "幅");
    assert_eq!(m.actions[0].label.get("en"), "width");
}

#[test]
fn missing_file_is_an_io_failure() {
    let err = open_file(&Hexdump, std::path::Path::new("/nonexistent/x.bin"))
        .err()
        .unwrap();
    assert_eq!(err.kind, konoma_plugin::FailureKind::Io);
}

#[test]
fn a_plugin_given_files_without_shows_is_reported() {
    use konoma_plugin::registry::{NoteKind, PluginOrigin, Registry};
    use konoma_plugin::testing::ManifestIssue;
    use std::sync::Arc;

    /// The hello world as it was written before `shows` was needed.
    struct NoShows;
    impl Plugin for NoShows {
        fn manifest(&self) -> Manifest {
            Manifest::new("noshows", "0.1.0").bind(Bind::files(["*.bin"]))
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::unsupported(Tr::new("unused")))
        }
    }
    assert_eq!(
        check_manifest(&NoShows),
        vec![ManifestIssue::ShowsNotDeclared]
    );
    let mut r = Registry::new();
    r.register(Arc::new(NoShows), PluginOrigin::User).unwrap();
    assert!(r
        .notes()
        .iter()
        .any(|n| n.plugin == "noshows" && n.kind == NoteKind::ShowsNotDeclared));
    // Declared, it is clean; a plugin with no files at all (an embed, a verb) is not asked to.
    r.register(Arc::new(Hexdump), PluginOrigin::User).unwrap();
    assert!(!r.notes().iter().any(|n| n.plugin == "hexdump"));
}
