//! The `konoma-markdown` program, run without a terminal: a file it does not show is refused with a
//! reason and a non-zero exit (the same exit code as an unknown option), and a missing file is
//! named. A file it shows gets as far as the terminal check, so no screen is ever taken.

use std::ffi::OsStr;
use std::path::Path;

use konoma_test_support::{run_program_without_terminal, unique_tmp};

const EXE: &str = env!("CARGO_BIN_EXE_konoma-markdown");

fn run(home: &Path, file: &Path) -> (Option<i32>, String) {
    let out = run_program_without_terminal(Path::new(EXE), &[OsStr::new(file)], home);
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_format_it_does_not_show_is_refused_with_a_reason() {
    let d = unique_tmp("markdown_bin_refuse");
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
    let d = unique_tmp("markdown_bin_text");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let f = d.as_path().join("x.zzz");
    std::fs::write(&f, "plain words\n").unwrap();
    let (code, err) = run(d.as_path(), &f);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("does not show this file"), "{err}");
}

#[test]
fn a_missing_file_is_named() {
    let d = unique_tmp("markdown_bin_missing");
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
    let d = unique_tmp("markdown_bin_accept");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let f = d.as_path().join("x.md");
    std::fs::write(&f, b"# t\n").unwrap();
    let (code, err) = run(d.as_path(), &f);
    assert!(!err.contains("does not show this file"), "{err}");
    // Really opened (a refusal would say "cannot open the file"), then stopped at the terminal
    // check: no terminal here, so no screen is taken.
    assert!(err.contains("must be a terminal"), "{err}");
    assert_eq!(code, Some(1), "{err}");
}

#[test]
fn the_bundled_destinations_are_not_listed_or_served() {
    let d = unique_tmp("markdown_bin_bundled_listing");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let args = |a: &[&str]| {
        let a: Vec<&OsStr> = a.iter().map(OsStr::new).collect();
        run_program_without_terminal(Path::new(EXE), &a, d.as_path())
    };
    // `--manifest` prints the program's own plugin only (the program has one, so no `--plugin`).
    let out = args(&["--manifest"]);
    assert_eq!(out.status.code(), Some(0));
    let json = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(json.contains("\"markdown\""), "{json}");
    assert!(
        !json.contains("mermaid") && !json.contains("\"math\""),
        "{json}"
    );
    // The others cannot be named.
    for id in ["mermaid", "math"] {
        let out = args(&["--plugin", id, "--manifest"]);
        assert_eq!(out.status.code(), Some(2), "{id}");
        assert!(out.stdout.is_empty(), "{id}");
    }
    // A bundled destination takes no file of its own: `.mmd` is not opened.
    let f = d.as_path().join("x.mmd");
    std::fs::write(&f, "graph TD\n A-->B\n").unwrap();
    let (code, err) = run(d.as_path(), &f);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("does not show this file"), "{err}");
}
