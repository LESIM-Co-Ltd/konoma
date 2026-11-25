//! The `konoma-office` program, run without a terminal: a file it does not show is refused with a
//! reason and a non-zero exit (the same exit code as an unknown option), and a missing file is
//! named. A file it shows gets as far as the terminal check, so no screen is ever taken.

use std::ffi::OsStr;
use std::path::Path;

use konoma_test_support::{run_program_without_terminal, unique_tmp};

const EXE: &str = env!("CARGO_BIN_EXE_konoma-office");

fn run(home: &Path, file: &Path) -> (Option<i32>, String) {
    let out = run_program_without_terminal(Path::new(EXE), &[OsStr::new(file)], home);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_format_it_does_not_show_is_refused_with_a_reason() {
    let d = unique_tmp("office_bin_refuse");
    std::fs::create_dir_all(d.as_path()).unwrap();
    // Binary content with an extension no plugin of this program binds.
    let f = d.as_path().join("x.zzz");
    std::fs::write(&f, [0u8, 1, 2, 3]).unwrap();
    let (code, err) = run(d.as_path(), &f);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("does not show this file"), "{err}");
    assert!(err.contains("x.zzz"), "names the file: {err}");
}

#[test]
fn a_text_file_it_does_not_bind_is_refused_too() {
    let d = unique_tmp("office_bin_text");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let f = d.as_path().join("x.zzz");
    std::fs::write(&f, "plain words\n").unwrap();
    let (code, err) = run(d.as_path(), &f);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("does not show this file"), "{err}");
}

#[test]
fn a_missing_file_is_named() {
    let d = unique_tmp("office_bin_missing");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let (code, err) = run(d.as_path(), &d.as_path().join("nope.zzz"));
    assert_eq!(code, Some(2), "{err}");
    assert!(
        err.contains("no such file") && err.contains("nope.zzz"),
        "{err}"
    );
}

#[test]
fn a_format_it_shows_is_accepted_and_stops_at_the_terminal_check() {
    // One real file of each kind the program binds: a workbook, a Word document and a deck.
    let samples = konoma_test_support::repo_root().join("samples");
    for name in ["sample.xlsx", "sample.docx", "sample.pptx"] {
        let d = unique_tmp("office_bin_accept");
        std::fs::create_dir_all(d.as_path()).unwrap();
        let f = d.as_path().join(name);
        std::fs::copy(samples.join(name), &f).unwrap();
        let (code, err) = run(d.as_path(), &f);
        assert!(!err.contains("does not show this file"), "{name}: {err}");
        // Really opened (a refusal would say "cannot open the file"), then stopped at the terminal
        // check: no terminal here, so no screen is taken.
        assert!(err.contains("must be a terminal"), "{name}: {err}");
        assert_eq!(code, Some(1), "{name}: {err}");
    }
}

fn run_args(home: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let out = run_program_without_terminal(Path::new(EXE), &args, home);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_bundled_destinations_are_not_listed_or_served() {
    let d = unique_tmp("office_bin_bundled_listing");
    std::fs::create_dir_all(d.as_path()).unwrap();
    // `--version` lists the program's own plugins only.
    let (code, out, err) = run_args(d.as_path(), &["--version"]);
    assert_eq!(code, Some(0), "{err}");
    let ids: Vec<&str> = out
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(ids, ["spreadsheet", "document", "presentation"], "{out}");
    // `--manifest` and `--serve` cannot name a bundled one.
    for id in ["markdown", "mermaid", "math"] {
        let (code, out, err) = run_args(d.as_path(), &["--plugin", id, "--manifest"]);
        assert_eq!(code, Some(2), "{id}: {out}{err}");
        assert!(out.is_empty(), "{id}: {out}");
        assert!(err.contains("has no plugin"), "{id}: {err}");
    }
    // A bundled destination takes no file of its own: `.md` is not opened.
    let f = d.as_path().join("x.md");
    std::fs::write(&f, "# t\n").unwrap();
    let (code, err) = run(d.as_path(), &f);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("does not show this file"), "{err}");
}
