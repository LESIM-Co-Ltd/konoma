//! Tests added after the second stage-2 mutation audit: every look at the cancel flag of the math
//! plugin is there. Each test cancels at **one** look and shows what that look alone stops.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use konoma_math::{MathPlugin, MATH_KIND};
use konoma_plugin::view::ViewReq;
use konoma_plugin::{Cancel, FailureKind, Input, OpenCx, Plugin, Session, Settings, Value, ViewCx};

/// Says yes from the `n`th look (0-based) on.
fn cancel_from_look(n: usize) -> Cancel {
    let looks = Arc::new(AtomicUsize::new(0));
    Cancel::from_fn(move || looks.fetch_add(1, Ordering::SeqCst) >= n)
}

fn open_with(cancel: Cancel) -> Result<Box<dyn Session>, FailureKind> {
    let settings = Settings::new();
    let cx = OpenCx::new(&settings, "en").with_cancel(cancel);
    MathPlugin
        .open(
            Input::Embed {
                kind: MATH_KIND.into(),
                source: "x^2".into(),
                display: false,
            },
            &cx,
        )
        .map_err(|f| f.kind)
}

fn view_with(session: &mut dyn Session, mode: &str, cancel: Cancel) -> Result<(), FailureKind> {
    let settings = Settings::new().with("mode", Value::Str(mode.into()));
    let cx = ViewCx::new(&settings, "en").with_cancel(cancel);
    session
        .view(&ViewReq::new(80, 24), &cx)
        .map(|_| ())
        .map_err(|f| f.kind)
}

#[test]
fn a_cancelled_open_is_a_cancelled_open() {
    assert_eq!(
        open_with(cancel_from_look(0)).err(),
        Some(FailureKind::Cancelled)
    );
    assert!(open_with(Cancel::never()).is_ok());
}

/// The look at the start of a view stops the plugin's text mode as well as the picture: the text
/// branch has no look of its own after it.
#[test]
fn a_view_cancelled_at_its_start_stops_in_text_mode_too() {
    let mut session = open_with(Cancel::never()).ok().unwrap();
    assert_eq!(
        view_with(session.as_mut(), "text", cancel_from_look(0)),
        Err(FailureKind::Cancelled)
    );
    assert_eq!(view_with(session.as_mut(), "text", Cancel::never()), Ok(()));
}

/// The look before the drawing: a cancel that comes after the start, in the picture mode, stops
/// the formula from being drawn.
#[test]
fn a_view_cancelled_before_the_drawing_is_not_drawn() {
    let mut session = open_with(Cancel::never()).ok().unwrap();
    assert_eq!(
        view_with(session.as_mut(), "image", cancel_from_look(1)),
        Err(FailureKind::Cancelled)
    );
    assert_eq!(
        view_with(session.as_mut(), "image", Cancel::never()),
        Ok(())
    );
}
