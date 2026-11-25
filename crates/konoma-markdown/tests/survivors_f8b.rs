//! Tests added after the second stage-2 mutation audit: every look at the cancel flag of the
//! Markdown plugin is there. Each test cancels at **one** look and shows what that look alone
//! stops.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use konoma_markdown::{MarkdownPlugin, REPR_SOURCE};
use konoma_plugin::view::ViewReq;
use konoma_plugin::{Cancel, FailureKind, Input, OpenCx, Plugin, Settings, ViewCx};
use konoma_test_support::unique_tmp;

/// Says yes from the `n`th look (0-based) on.
fn cancel_from_look(n: usize) -> Cancel {
    let looks = Arc::new(AtomicUsize::new(0));
    Cancel::from_fn(move || looks.fetch_add(1, Ordering::SeqCst) >= n)
}

fn open(
    path: &std::path::Path,
    cancel: Cancel,
) -> Result<Box<dyn konoma_plugin::Session>, FailureKind> {
    let settings = Settings::new();
    let cx = OpenCx::new(&settings, "en").with_cancel(cancel);
    MarkdownPlugin
        .open(
            Input::File {
                path: path.to_path_buf(),
                hint: None,
            },
            &cx,
        )
        .map_err(|f| f.kind)
}

/// The look before anything is read: a cancelled open does not touch the file (a file that is not
/// there is a cancelled open, not a read error).
#[test]
fn a_cancelled_open_does_not_read_the_file() {
    let dir = unique_tmp("konoma_markdown_f8b_entry");
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("nothing-here.md");
    assert_eq!(
        open(&missing, cancel_from_look(0)).err(),
        Some(FailureKind::Cancelled)
    );
    // Without the cancel the same path is the read error it is.
    assert_eq!(open(&missing, Cancel::never()).err(), Some(FailureKind::Io));
}

/// The look after the read, before the text is wrapped up: a cancel that comes while the file is
/// being read is still a cancelled open.
#[test]
fn a_cancel_that_comes_during_the_read_is_a_cancelled_open() {
    let dir = unique_tmp("konoma_markdown_f8b_read");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.md");
    std::fs::write(&file, "# a\n").unwrap();
    assert_eq!(
        open(&file, cancel_from_look(1)).err(),
        Some(FailureKind::Cancelled)
    );
    assert!(open(&file, Cancel::never()).is_ok());
}

/// A view asked for after the cancel is a cancelled view, for both representations.
#[test]
fn a_cancelled_view_is_a_cancelled_view() {
    let dir = unique_tmp("konoma_markdown_f8b_view");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.md");
    std::fs::write(&file, "# a\n").unwrap();
    let mut session = open(&file, Cancel::never()).ok().unwrap();
    let settings = Settings::new();
    for repr in [None, Some(REPR_SOURCE.to_string())] {
        let mut req = ViewReq::new(80, 24);
        req.repr = repr.clone();
        let cx = ViewCx::new(&settings, "en").with_cancel(Cancel::from_fn(|| true));
        let err = session.view(&req, &cx).expect_err("cancelled");
        assert_eq!(err.kind, FailureKind::Cancelled, "{repr:?}");
        assert!(session.view(&req, &ViewCx::new(&settings, "en")).is_ok());
    }
}
