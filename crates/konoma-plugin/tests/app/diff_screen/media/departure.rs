use super::*;
use konoma_plugin::app::{DocPics, InProcessSvg, Pictures};
use konoma_widgets::inline_images::{InlineImageCache, MdImgEntry};
use ratatui::{
    backend::{Backend, ClearType, TestBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Rect, Size},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct Probe {
    inner: TestBackend,
    clears: Arc<AtomicUsize>,
}
impl Backend for Probe {
    type Error = std::convert::Infallible;
    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
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
            let previous = self.clears.fetch_add(1, Ordering::SeqCst);
            assert!(
                previous < 2,
                "one stable overlay must not trigger repeated clears before input"
            );
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
        Ok(())
    }
}
pub(super) fn check() {
    let dir = konoma_test_support::unique_tmp("media_departure_frame");
    let mut prepared = support::prepared(&dir);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let requests = Arc::new(Mutex::new(vec![]));
    let captured = requests.clone();
    let worker = MediaDiffWorker::with_compute(Arc::new(move |r| {
        count.fetch_add(1, Ordering::SeqCst);
        captured.lock().unwrap().push(r.raster_px);
        computed(r)
    }));
    let mut state = prepared
        .diff
        .take()
        .unwrap()
        .with_media_worker(worker, mpsc::channel().1);
    let path = dir.join("side.svg");
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut pictures =
        Pictures::new(picker.clone(), "fixture", &path, 128).with_out(Box::new(std::io::sink()));
    let area = Rect::new(0, 1, 54, 12);
    state.prepare_frame(area, Some(pictures.doc_pics()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let px = requests.lock().unwrap()[0];
    assert_eq!(px, (576, 256), "64px raster quantization precedes layout");
    let key = format!("media-diff://o/1/{}x{}", px.0, px.1);
    let entry = MdImgEntry::default();
    let ticket = entry.wish.ticket();
    let mut cache = InlineImageCache::default();
    cache.attach_picker(&picker);
    let (tx, _rx) = mpsc::channel();
    cache.enc_tx = Some(tx);
    cache.cache.insert(key.into(), entry);
    // Replace the backing cache under a landed outcome: only the new side is now absent.
    *pictures.doc_pics() = DocPics::new(&picker, "fixture", &path, 128, Arc::new(InProcessSvg))
        .with_cache(cache, mpsc::channel().1);
    prepared.diff = Some(state);
    let clears = Arc::new(AtomicUsize::new(0));
    struct Events {
        step: usize,
        calls: Arc<AtomicUsize>,
        clears: Arc<AtomicUsize>,
        ticket: Arc<konoma_svg::image::DecodeTicket>,
    }
    impl EventSource for Events {
        fn next(&mut self, _: Option<Duration>) -> Next {
            let step = self.step;
            self.step += 1;
            match step {
                0 => {
                    assert_eq!(
                        self.calls.load(Ordering::SeqCst),
                        2,
                        "missing cache key re-kicks before drawing"
                    );
                    assert_eq!(
                        self.clears.load(Ordering::SeqCst),
                        0,
                        "first appearance must not clear"
                    );
                    assert!(!self.ticket.is_stale());
                    Next::Event(AppEvent::Key(Key::char('q')))
                }
                1 => {
                    assert!(
                        self.ticket.is_stale(),
                        "q prunes media before the next project frame"
                    );
                    assert_eq!(
                        self.clears.load(Ordering::SeqCst),
                        1,
                        "departure clears exactly once"
                    );
                    Next::Event(AppEvent::Resize)
                }
                _ => {
                    assert_eq!(
                        self.clears.load(Ordering::SeqCst),
                        1,
                        "returning must not clear twice"
                    );
                    assert_eq!(
                        self.calls.load(Ordering::SeqCst),
                        2,
                        "closed diff never re-kicks"
                    );
                    Next::End
                }
            }
        }
    }
    let mut terminal = ratatui::Terminal::new(Probe {
        inner: TestBackend::new(54, 14),
        clears: clears.clone(),
    })
    .unwrap();
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut prepared,
        &mut Events {
            step: 0,
            calls,
            clears,
            ticket,
        },
        &Default::default(),
        Some(pictures),
    )
    .unwrap();
}
