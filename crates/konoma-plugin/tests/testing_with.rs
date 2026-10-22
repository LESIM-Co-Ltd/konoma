//! `view_canvas_with` / `send_keys_with` pass the caller's context, and `push_image` is the
//! `&mut` form of `image`.

use konoma_plugin::key::Key;
use konoma_plugin::prelude::*;
use konoma_plugin::session::{ColorDepth, TermCaps};
use konoma_plugin::testing::{
    open_bytes, send_keys, send_keys_with, view_canvas, view_canvas_with, CanvasText,
};
use konoma_plugin::view::{CanvasImage, CellRect, ImageData};
use std::sync::Arc;

struct Probe;
impl Plugin for Probe {
    fn manifest(&self) -> Manifest {
        Manifest::new("probe", "0.1.0")
            .bind(Bind::files(["*.probe"]))
            .shows(ViewKind::Canvas)
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ProbeSession {
            seen: String::new(),
        }))
    }
}

struct ProbeSession {
    seen: String,
}
impl Session for ProbeSession {
    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let mut c = CanvasView::new(req.cols, req.rows);
        c.set_string(0, 0, &format!("{:?}", cx.term.colors), Style::new());
        c.set_string(0, 1, &self.seen, Style::new());
        Ok(View::Canvas(c))
    }
    fn key(&mut self, _key: &Key, cx: &ActCx) -> Result<Outcome, Failure> {
        self.seen = format!("{:?}", cx.term.colors);
        Ok(Outcome::Redraw)
    }
}

#[test]
fn view_canvas_with_passes_the_context() {
    let (mut s, _f) = open_bytes(&Probe, "a.probe", b"x").unwrap();
    let req = ViewReq::new(20, 2);
    let bare = view_canvas(&mut *s, &req).unwrap();
    assert_eq!(CanvasText::of(&bare).rows[0], "Ansi16");
    let cx = ViewCx::bare().with_term(TermCaps::new().colors(ColorDepth::TrueColor));
    let c = view_canvas_with(&mut *s, &req, &cx).unwrap();
    assert_eq!(CanvasText::of(&c).rows[0], "TrueColor");
}

#[test]
fn send_keys_with_passes_the_context() {
    let (mut s, _f) = open_bytes(&Probe, "a.probe", b"x").unwrap();
    let req = ViewReq::new(20, 2);
    send_keys(&mut *s, &[Key::char('a')]).outcomes().unwrap();
    assert_eq!(
        CanvasText::of(&view_canvas(&mut *s, &req).unwrap()).rows[1],
        "Ansi16"
    );
    let cx = ActCx::bare().with_term(TermCaps::new().colors(ColorDepth::Ansi256));
    let out = send_keys_with(&mut *s, &[Key::char('a'), Key::char('b')], &cx)
        .outcomes()
        .unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(
        CanvasText::of(&view_canvas(&mut *s, &req).unwrap()).rows[1],
        "Ansi256"
    );
}

#[test]
fn push_image_is_the_mut_form_of_image() {
    let data = ImageData::Rgba {
        width: 1,
        height: 1,
        pixels: Arc::new(vec![0, 0, 0, 255]),
    };
    let mk = |id| CanvasImage::new(id, CellRect::new(0, 0, 1, 1), data.clone());
    let built = CanvasView::new(3, 1).image(mk(1)).image(mk(2));
    let mut pushed = CanvasView::new(3, 1);
    for id in [1, 2] {
        pushed.push_image(mk(id));
    }
    assert_eq!(built.images.len(), 2);
    assert_eq!(pushed.images.len(), 2);
    assert_eq!(built.images[1].id, pushed.images[1].id);
    assert_eq!(pushed.check(), Ok(()));
}

struct Flaky;
impl Plugin for Flaky {
    fn manifest(&self) -> Manifest {
        Manifest::new("flaky", "0.1.0")
            .bind(Bind::files(["*.flaky"]))
            .shows(ViewKind::Canvas)
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(FlakySession {
            typed: String::new(),
        }))
    }
}

struct FlakySession {
    typed: String,
}
impl Session for FlakySession {
    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let mut c = CanvasView::new(req.cols, req.rows);
        c.set_string(0, 0, &self.typed, Style::new());
        Ok(View::Canvas(c))
    }
    fn key(&mut self, key: &Key, _cx: &ActCx) -> Result<Outcome, Failure> {
        if key.code == KeyCode::Char('x') {
            return Err(Failure::unsupported("no x"));
        }
        if let KeyCode::Char(c) = key.code {
            self.typed.push(c);
        }
        Ok(Outcome::Redraw)
    }
}

#[test]
fn send_keys_does_not_stop_at_a_failure_as_the_host_does_not() {
    let (mut s, _f) = open_bytes(&Flaky, "a.flaky", b"").unwrap();
    let run = send_keys(&mut *s, &[Key::char('a'), Key::char('x'), Key::char('b')]);
    assert_eq!(run.results.len(), 3);
    assert!(!run.is_ok());
    assert_eq!(run.failures().len(), 1);
    assert_eq!(run.results[2], Ok(Outcome::Redraw));
    let c = view_canvas(&mut *s, &ViewReq::new(10, 1)).unwrap();
    assert_eq!(CanvasText::of(&c).rows[0], "ab");
    assert!(run.outcomes().is_err());
}
