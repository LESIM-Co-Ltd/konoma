use super::*;
use ratatui::backend::{ClearType, TestBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use std::sync::{Arc, Mutex};

struct Probe {
    inner: TestBackend,
    clears: usize,
    draws: usize,
}

impl Backend for Probe {
    type Error = std::convert::Infallible;
    fn draw<'a, I: Iterator<Item = (u16, u16, &'a Cell)>>(
        &mut self,
        cells: I,
    ) -> Result<(), Self::Error> {
        self.inner.draw(cells)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(Position::new(0, 0))
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> Result<(), Self::Error> {
        Ok(())
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }
    fn clear_region(&mut self, t: ClearType) -> Result<(), Self::Error> {
        if matches!(t, ClearType::All) {
            self.clears += 1;
        }
        self.inner.clear_region(t)
    }
    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_up(region, line_count)
    }
    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        line_count: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_down(region, line_count)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        self.draws += 1;
        Ok(())
    }
}

#[test]
fn overlay_move_clears_once_and_worker_results_redraw_without_input() {
    let (d, laid, _rx) = super::super::doc_pictures::inline_overlay_tests::setup();
    let mut pictures = Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "test",
        std::path::Path::new("/x/doc.md"),
        800,
    )
    .with_out(Box::new(std::io::sink()));
    *pictures.doc_pics() = d;
    let mut frame = Frame::Document(super::super::document::DocFrame {
        name: "doc.md".into(),
        laid: Arc::new(laid),
        scroll: 0,
        wrap: true,
        hscroll: 0,
        lang: Lang::En,
        truncated: false,
    });
    let mut term = Terminal::new(Probe {
        inner: TestBackend::new(20, 10),
        clears: 0,
        draws: 0,
    })
    .unwrap();
    for (scroll, clears, passes) in [
        (0, 0, 1),
        (0, 0, 1),
        (1, 1, 2),
        (1, 1, 1),
        (20, 2, 2),
        (21, 2, 1),
    ] {
        let Frame::Document(d) = &mut frame else {
            unreachable!()
        };
        d.scroll = scroll;
        let before = term.backend().draws;
        draw_frame(&mut term, &mut frame, &[], None, Some(&mut pictures), None).unwrap();
        assert_eq!(term.backend().clears, clears, "scroll {scroll}");
        assert_eq!(
            term.backend().draws - before,
            passes,
            "clear is followed by a redraw"
        );
        assert_eq!(
            term.backend().inner.buffer().area,
            ratatui::layout::Rect::new(0, 0, 20, 10)
        );
        assert!(term
            .backend()
            .inner
            .buffer()
            .content
            .iter()
            .any(|c| c.symbol() == "┌"));
    }
}

struct Page(Arc<Mutex<usize>>);
impl Session for Page {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        *self.0.lock().unwrap() += 1;
        Ok(View::Document(crate::view::DocumentView {
            markdown: "plain".into(),
            origin: crate::input::Origin::File("/x/doc.md".into()),
            resources: Default::default(),
            options: Default::default(),
            title: None,
            truncated: false,
        }))
    }
}

struct CheckRelayout {
    turns: usize,
}

#[test]
fn overlay_worker_relayout_redraws_without_input() {
    let views = Arc::new(Mutex::new(0));
    let mut session = Page(Arc::clone(&views));
    let mut events = CheckRelayout { turns: 0 };
    let mut pictures = Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "test",
        std::path::Path::new("/x/doc.md"),
        800,
    )
    .with_out(Box::new(std::io::sink()));
    *pictures.doc_pics() = super::super::doc_pictures::inline_overlay_tests::relayout_ready();
    let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
    run_with_pictures(
        &mut term,
        &mut session,
        &mut events,
        &AppCx::default(),
        &AppKeys::new(&KeysConfig::default(), "vim"),
        Some(pictures),
    )
    .unwrap();
    assert_eq!(
        *views.lock().unwrap(),
        2,
        "the landed result requests one layout without a key"
    );
    assert_eq!(events.turns, 1);
}

impl EventSource for CheckRelayout {
    fn next(&mut self, _: Option<Duration>) -> Next {
        self.turns += 1;
        Next::End
    }
}
