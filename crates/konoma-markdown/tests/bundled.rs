//! The destinations the `konoma-markdown` program carries (mermaid, math): they are looked up for the
//! diagrams and formulas of a document and never take a file of their own.

use std::sync::Arc;

use konoma_plugin::app::{open_file_bundled, AppError};
use konoma_plugin::{Plugin, Settings, TermCaps};

fn arcs(list: Vec<Box<dyn Plugin>>) -> Vec<Arc<dyn Plugin>> {
    list.into_iter().map(Arc::from).collect()
}

fn open(name: &str) -> Result<konoma_plugin::app::Opened, AppError> {
    let d = konoma_test_support::unique_tmp("markdown_bundled");
    std::fs::create_dir_all(d.as_path()).unwrap();
    let f = d.as_path().join(name);
    std::fs::write(&f, "# t\n").unwrap();
    open_file_bundled(
        arcs(konoma_markdown::program()),
        arcs(konoma_markdown::bundled()),
        &f,
        "en",
        TermCaps::new(),
        &|_| Settings::new(),
    )
}

#[test]
fn the_program_carries_mermaid_and_math_as_destinations() {
    let ids: Vec<String> = konoma_markdown::bundled()
        .iter()
        .map(|p| p.manifest().id)
        .collect();
    assert_eq!(ids, ["mermaid", "math"]);
}

#[test]
fn a_markdown_file_is_taken_by_the_program_itself() {
    let opened = open("x.md").expect("opens");
    assert_eq!(opened.plugin_id, "markdown");
    // The embedded kinds are answered by the bundled plugins.
    assert_eq!(
        opened.registry.resolve_embed("fence:mermaid").as_deref(),
        Some("mermaid")
    );
    assert_eq!(
        opened.registry.resolve_embed("math").as_deref(),
        Some("math")
    );
}

#[test]
fn a_file_only_a_bundled_plugin_takes_is_not_opened() {
    for name in ["x.mmd", "x.mermaid"] {
        match open(name) {
            Err(AppError::NotHandled { .. }) => {}
            Err(e) => panic!("{name}: refused for another reason: {e}"),
            Ok(o) => panic!("{name}: opened by `{}`", o.plugin_id),
        }
    }
}

/// The app's inline-diagram height cap is spelled once on each side (the plugin crate cannot
/// depend on the mermaid plugin); the two must stay the same number.
#[test]
fn the_default_mermaid_rows_cap_is_the_mermaid_plugins_own_default() {
    assert_eq!(
        i64::from(konoma_plugin::document_pictures::DEFAULT_MERMAID_ROWS),
        konoma_mermaid::DEFAULT_ROWS
    );
}
