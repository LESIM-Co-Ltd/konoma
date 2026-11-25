//! The destinations the `konoma-office` program carries (Markdown, mermaid, math): a Word document is
//! reopened through them, and they never take a file of their own.

use std::path::Path;
use std::sync::Arc;

use konoma_plugin::app::{
    open_file_bundled, run_with, AppCx, AppError, AppEvent, AppKeys, ReopenSource,
};
use konoma_plugin::{Plugin, Settings, TermCaps};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn arcs(list: Vec<Box<dyn Plugin>>) -> Vec<Arc<dyn Plugin>> {
    list.into_iter().map(Arc::from).collect()
}

fn sample(name: &str) -> std::path::PathBuf {
    konoma_test_support::repo_root().join("samples").join(name)
}

/// The settled screen of the app on `file`, with `bundled` carried beside the office plugins.
fn screen(file: &Path, bundled: Vec<Arc<dyn Plugin>>) -> Result<String, AppError> {
    let mut opened = open_file_bundled(
        arcs(konoma_office::program()),
        bundled,
        file,
        "en",
        TermCaps::new(),
        &|_| Settings::new(),
    )?;
    let reopen =
        ReopenSource::from_registry(Arc::clone(&opened.registry), Box::new(|_| Settings::new()));
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: opened.shows.clone(),
        reopen: Some(reopen),
        ..AppCx::default()
    };
    let mut events = Vec::<AppEvent>::new().into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut events,
        &cx,
        &AppKeys::defaults(),
    )?;
    let buf = term.backend().buffer();
    Ok((0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

#[test]
fn the_program_carries_markdown_mermaid_and_math_as_destinations() {
    let ids: Vec<String> = konoma_office::bundled()
        .iter()
        .map(|p| p.manifest().id)
        .collect();
    assert_eq!(ids, ["markdown", "mermaid", "math"]);
    // None of them is one of the program's own plugins (those are the only ones `--serve`,
    // `--manifest` and the choice of a file see).
    let own: Vec<String> = konoma_office::program()
        .iter()
        .map(|p| p.manifest().id)
        .collect();
    assert!(ids.iter().all(|i| !own.contains(i)), "{own:?} vs {ids:?}");
}

#[test]
fn a_docx_is_reopened_as_markdown_through_the_bundled_plugin() {
    let shown = screen(&sample("sample.docx"), arcs(konoma_office::bundled())).expect("opens");
    assert!(
        !shown.contains("no plugin is available"),
        "the document was not reopened:\n{shown}"
    );
    assert!(
        shown.chars().filter(|c| c.is_alphanumeric()).count() > 40,
        "the document is on screen:\n{shown}"
    );
}

#[test]
fn without_the_bundled_plugins_the_same_docx_cannot_be_shown() {
    let shown = screen(&sample("sample.docx"), Vec::new()).expect("opens");
    assert!(
        shown.contains("no plugin is available to show this document"),
        "{shown}"
    );
}

#[test]
fn a_file_only_a_bundled_plugin_takes_is_not_opened() {
    let d = konoma_test_support::unique_tmp("office_bundled_md");
    std::fs::create_dir_all(d.as_path()).unwrap();
    for name in ["x.md", "x.mmd"] {
        let f = d.as_path().join(name);
        std::fs::write(&f, "# t\n").unwrap();
        match screen(&f, arcs(konoma_office::bundled())) {
            Err(AppError::NotHandled { .. }) => {}
            Err(e) => panic!("{name}: refused for another reason: {e}"),
            Ok(s) => panic!("{name}: opened by a bundled plugin:\n{s}"),
        }
    }
}
