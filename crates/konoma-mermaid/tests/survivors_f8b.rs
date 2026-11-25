//! Tests added after the second stage-2 mutation audit: every look at the cancel flag of the
//! mermaid plugin is there. Each test cancels at **one** look and shows what that look alone stops.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use konoma_mermaid::{MermaidPlugin, REPR_IMAGE, REPR_TEXT};
use konoma_plugin::view::ViewReq;
use konoma_plugin::{Cancel, FailureKind, Input, OpenCx, Plugin, Session, Settings, ViewCx};
use konoma_test_support::unique_tmp;

/// Says yes from the `n`th look (0-based) on.
fn cancel_from_look(n: usize) -> Cancel {
    let looks = Arc::new(AtomicUsize::new(0));
    Cancel::from_fn(move || looks.fetch_add(1, Ordering::SeqCst) >= n)
}

fn open_file(path: &std::path::Path, cancel: Cancel) -> Result<Box<dyn Session>, FailureKind> {
    let settings = Settings::new();
    let cx = OpenCx::new(&settings, "en").with_cancel(cancel);
    MermaidPlugin
        .open(
            Input::File {
                path: path.to_path_buf(),
                hint: None,
            },
            &cx,
        )
        .map_err(|f| f.kind)
}

fn view_of(session: &mut dyn Session, repr: &str, cancel: Cancel) -> Result<(), FailureKind> {
    let settings = Settings::new();
    let cx = ViewCx::new(&settings, "en").with_cancel(cancel);
    let mut req = ViewReq::new(80, 24);
    req.repr = Some(repr.into());
    session.view(&req, &cx).map(|_| ()).map_err(|f| f.kind)
}

fn diagram(name: &str) -> (std::path::PathBuf, konoma_test_support::TmpDir) {
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("d.mmd");
    std::fs::write(&file, "flowchart LR\n  A --> B\n").unwrap();
    (file, dir)
}

#[test]
fn a_cancelled_open_is_a_cancelled_open() {
    let (file, _dir) = diagram("konoma_mermaid_f8b_open");
    assert_eq!(
        open_file(&file, cancel_from_look(0)).err(),
        Some(FailureKind::Cancelled)
    );
    assert!(open_file(&file, Cancel::never()).is_ok());
}

/// The look at the start of a view comes before the file is read: a cancelled view of a file that
/// is gone is a cancelled view, not a read error.
#[test]
fn a_view_cancelled_at_its_start_does_not_read_the_file() {
    let (file, _dir) = diagram("konoma_mermaid_f8b_start");
    let mut session = open_file(&file, Cancel::never()).ok().unwrap();
    std::fs::remove_file(&file).unwrap();
    for repr in [REPR_IMAGE, REPR_TEXT] {
        assert_eq!(
            view_of(session.as_mut(), repr, cancel_from_look(0)),
            Err(FailureKind::Cancelled),
            "{repr}"
        );
        assert_eq!(
            view_of(session.as_mut(), repr, Cancel::never()),
            Err(FailureKind::Io),
            "{repr}: the file is really gone"
        );
    }
}

/// The look after the file is read: the text form has no look after it, so this one is what stops
/// a cancel that came during the read.
#[test]
fn a_view_cancelled_during_the_read_is_not_made_into_text() {
    let (file, _dir) = diagram("konoma_mermaid_f8b_read");
    let mut session = open_file(&file, Cancel::never()).ok().unwrap();
    assert_eq!(
        view_of(session.as_mut(), REPR_TEXT, cancel_from_look(1)),
        Err(FailureKind::Cancelled)
    );
    assert_eq!(
        view_of(session.as_mut(), REPR_TEXT, Cancel::never()),
        Ok(())
    );
}

/// The look before the drawing: a cancel that comes after the read stops the picture from being
/// drawn.
#[test]
fn a_view_cancelled_before_the_drawing_is_not_drawn() {
    let (file, _dir) = diagram("konoma_mermaid_f8b_draw");
    let mut session = open_file(&file, Cancel::never()).ok().unwrap();
    assert_eq!(
        view_of(session.as_mut(), REPR_IMAGE, cancel_from_look(2)),
        Err(FailureKind::Cancelled)
    );
    assert_eq!(
        view_of(session.as_mut(), REPR_IMAGE, Cancel::never()),
        Ok(())
    );
}
