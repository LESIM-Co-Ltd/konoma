//! Preview publication is controlled by channels, never by elapsed time.
use super::*;
use crate::app::looping::{run_file_with_preview, AppEvent, EventSource, Next};
use crate::app::{AppKeys, Display};
use crate::host::project::md_diff::{MdDiffOutcome, MdDiffResult};
use crate::input::Origin;
use crate::view::{DocumentView, TextSource, TextView};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Page(bool, String, PathBuf);
impl Session for Page {
    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
        Ok(if self.0 {
            View::Document(DocumentView {
                markdown: self.1.clone(),
                origin: Origin::File(self.2.clone()),
                resources: Default::default(),
                options: Default::default(),
                title: None,
                truncated: false,
            })
        } else {
            View::Text(TextView {
                source: TextSource::Owned(self.1.clone()),
                styler: None,
                file_lines: true,
                syntax_name: None,
            })
        })
    }
}

#[test]
fn preview_git_gutter_off_does_not_dispatch_markdown_worker() {
    let path = PathBuf::from("doc.md");
    let mut gutter = PreviewGutter::new(path.clone(), PathBuf::new(), false, Default::default());
    let view = Page(true, "row 0\n\nrow 1\n".into(), path.clone())
        .view(&ViewReq::default(), &ViewCx::bare())
        .unwrap();
    gutter.prepare(&view);
    assert!(
        !gutter.markdown.pending_for(&path),
        "gutter OFF never dispatches a Markdown gutter worker"
    );
    assert!(
        gutter.reservation.is_none(),
        "Markdown OFF consumes the reservation without moving, as the host does"
    );
    assert!(gutter.wake_in().is_none());
}

struct Release {
    release: Option<mpsc::Sender<()>>,
    published: mpsc::Receiver<()>,
    step: usize,
    marked: bool,
}
impl EventSource for Release {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        let frame = crate::app::draw::test_frame();
        let marked = frame.content.iter().any(|c| c.symbol() == "▌");
        match self.step {
            0 => {
                assert!(
                    wait.is_some(),
                    "worker deadline participates in the minimum wait"
                );
                assert!(!marked, "computing frame has no gutter");
                assert_eq!(frame[(1, 1)].symbol(), "0", "computing frame stays at top");
                self.release.take().unwrap().send(()).unwrap();
                self.published
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                self.step += 1;
                Next::Timeout
            }
            1 => {
                assert_eq!(marked, self.marked, "gutter lands without input");
                assert!(
                    frame.content.iter().any(|c| c.symbol() == "X"),
                    "first change below fold is visible"
                );
                self.step += 1;
                Next::Event(AppEvent::Key(crate::Key::char('g')))
            }
            2 => {
                assert_eq!(
                    frame[(1 + u16::from(self.marked), 1)].symbol(),
                    "0",
                    "user scroll is not undone"
                );
                self.step += 1;
                Next::Timeout
            }
            _ => Next::End,
        }
    }
}

#[test]
fn preview_workers_land_without_input_and_consume_only_their_reservation() {
    for markdown in [true, false] {
        for enabled in [true, false] {
            if markdown && !enabled {
                continue;
            }
            let dir = konoma_test_support::unique_tmp("preview_worker");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(if markdown { "doc.md" } else { "file.txt" });
            let old = (0..40).map(|i| format!("{i}\n\n")).collect::<String>();
            let new = old.replacen("25\n", "X\n", 1);
            std::fs::write(&path, &new).unwrap();
            let (release, released) = mpsc::channel();
            let (publish, published) = mpsc::channel();
            let config = konoma_host_core::shared::SharedConfig::default();
            let mut gutter = PreviewGutter::new(
                path.clone(),
                dir.to_path_buf(),
                enabled,
                super::super::ProjectBackend::Git.diff_read_context(),
            );
            if markdown {
                let released = Mutex::new(released);
                gutter.markdown = crate::host::project::md_diff::MdDiffWorker::with_compute(
                    Arc::new(move |req| {
                        released.lock().unwrap().recv().unwrap();
                        let mut req = req.clone();
                        req.baseline = crate::host::project::md_diff::DiffBaseline::FollowSnapshot(
                            old.as_bytes().to_vec(),
                        );
                        let outcome =
                            crate::host::project::md_diff::MdDiffWorker::compute_md_diff(&req);
                        publish.send(()).unwrap();
                        outcome
                    }),
                );
                // Fence after the reply, not merely after computation, to make the next frame deterministic.
                let (tx, rx) = mpsc::channel();
                gutter.markdown.attach(tx);
                let (landed, fence) = mpsc::channel();
                let (tx, reply) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = rx.recv().unwrap();
                    tx.send(result).unwrap();
                    landed.send(()).unwrap();
                });
                gutter.markdown_rx = reply;
                let _ = published;
                run_case(
                    gutter,
                    Page(true, new, path),
                    release,
                    fence,
                    enabled,
                    &config,
                );
            } else {
                let (tx, rx) = mpsc::channel();
                gutter.raw = Some(rx);
                std::thread::spawn(move || {
                    released.recv().unwrap();
                    tx.send(Ok(crate::app::diff_screen::FileDiffData {
                        lines: konoma_vcs_core::vcs::diff_contents(&old, &new),
                        paths: Vec::new(),
                    }))
                    .unwrap();
                    publish.send(()).unwrap();
                });
                let text = std::fs::read_to_string(&path).unwrap();
                run_case(
                    gutter,
                    Page(false, text, path),
                    release,
                    published,
                    enabled,
                    &config,
                );
            }
        }
    }
}

fn run_case(
    mut gutter: PreviewGutter,
    mut page: Page,
    release: mpsc::Sender<()>,
    published: mpsc::Receiver<()>,
    marked: bool,
    config: &konoma_host_core::shared::SharedConfig,
) {
    let cx = AppCx {
        path: page.2.clone(),
        display: Display::of(config),
        ..Default::default()
    };
    let keys = AppKeys::defaults();
    let mut runner = FileLoopState::new(&cx, &keys);
    let mut terminal = Terminal::new(TestBackend::new(32, 11)).unwrap();
    let mut events = Release {
        release: Some(release),
        published,
        step: 0,
        marked,
    };
    run_file_with_preview(
        &mut terminal,
        &mut page,
        &mut events,
        &cx,
        &mut runner,
        None,
        Some(&mut gutter),
    )
    .unwrap();
    assert!(
        gutter.reservation.is_none(),
        "reservation consumed exactly once"
    );
    let current = gutter.markdown.landed_for(
        &gutter.path,
        crate::host::project::md_diff::MdDiffKind::Gutter,
    );
    let stale = MdDiffResult {
        gen: gutter.markdown.generation_for_test().wrapping_sub(1),
        path: gutter.path.clone(),
        kind: crate::host::project::md_diff::MdDiffKind::Gutter,
        outcome: MdDiffOutcome::NoBaseline,
    };
    assert!(!gutter.adopt(stale), "old generation cannot land");
    let stale = MdDiffResult {
        gen: *gutter.markdown.generation_for_test(),
        path: "old.md".into(),
        kind: crate::host::project::md_diff::MdDiffKind::Gutter,
        outcome: MdDiffOutcome::NoBaseline,
    };
    assert!(
        !gutter.adopt(stale),
        "old path cannot land even at the current generation"
    );
    assert_eq!(
        current.is_some(),
        gutter
            .markdown
            .landed_for(
                &gutter.path,
                crate::host::project::md_diff::MdDiffKind::Gutter
            )
            .is_some()
    );

    // A later file has a new owner, and a converted document cancels the old owner's want.
    let other = gutter.path.with_file_name("unrelated.md");
    let mut unrelated = Page(true, "# Unrelated\n\nunchanged\n".into(), other);
    let mut fresh_runner = FileLoopState::new(&cx, &keys);
    run_file_with_preview(
        &mut terminal,
        &mut unrelated,
        &mut Vec::<AppEvent>::new().into_iter(),
        &cx,
        &mut fresh_runner,
        None,
        Some(&mut gutter),
    )
    .unwrap();
    assert!(gutter.reservation.is_none());
    assert!(
        !crate::app::draw::test_frame()
            .content
            .iter()
            .any(|c| c.symbol() == "▌"),
        "an unrelated Preview never inherits the old gutter or reservation"
    );
}

#[test]
fn preview_reopen_uses_destination_settings_and_cancels_the_reservation() {
    use crate::{Plugin, Settings, Value};
    struct Converter;
    impl Session for Converter {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
            Ok(View::Reopen(crate::view::Reopen {
                content_type: "markdown".into(),
                text: "converted".into(),
                resources: Default::default(),
                origin: Origin::Converted {
                    from: "source.conv".into(),
                },
                truncated: false,
            }))
        }
    }
    struct Destination(Arc<Mutex<usize>>);
    impl Plugin for Destination {
        fn manifest(&self) -> Manifest {
            Manifest::new("destination", "0").shows(ViewKind::Document)
        }
        fn open(&self, _: Input, cx: &OpenCx) -> Result<Box<dyn Session>, crate::Failure> {
            assert!(cx.settings.bool("destination", false));
            Ok(Box::new(Destination(self.0.clone())))
        }
    }
    impl Session for Destination {
        fn view(&mut self, _: &ViewReq, cx: &ViewCx) -> Result<View, crate::Failure> {
            assert!(
                cx.settings.bool("destination", false),
                "Reopen uses destination settings on every view"
            );
            *self.0.lock().unwrap() += 1;
            Ok(View::Document(DocumentView {
                markdown: "destination".into(),
                origin: Origin::Converted {
                    from: "source.conv".into(),
                },
                resources: Default::default(),
                options: Default::default(),
                title: None,
                truncated: false,
            }))
        }
    }
    let calls = Arc::new(Mutex::new(0));
    let plugin = Arc::new(Destination(calls.clone()));
    let cx = AppCx {
        path: "source.conv".into(),
        reopen: Some(ReopenSource::new(move |_| {
            let mut settings = Settings::new();
            settings.set("destination", Value::Bool(true));
            Some(crate::reopen::ContentTarget {
                plugin: plugin.clone(),
                settings,
            })
        })),
        ..Default::default()
    };
    let keys = AppKeys::defaults();
    let mut runner = FileLoopState::new(&cx, &keys);
    let mut gutter = PreviewGutter::new(cx.path.clone(), PathBuf::new(), true, Default::default());
    let mut terminal = Terminal::new(TestBackend::new(32, 11)).unwrap();
    run_file_with_preview(
        &mut terminal,
        &mut Converter,
        &mut [AppEvent::Resize].into_iter(),
        &cx,
        &mut runner,
        None,
        Some(&mut gutter),
    )
    .unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
    assert!(
        gutter.reservation.is_none(),
        "Reopen discards the source reservation"
    );
    assert!(
        gutter.wake_in().is_none(),
        "converted documents start no file gutter worker"
    );
}

#[test]
fn preview_picture_timeouts_do_not_request_the_session_again() {
    use crate::app::Pictures;
    use crate::canvas_pictures::SvgDraw;
    use crate::view::{CanvasImage, CanvasView, CellRect, ImageData};
    struct HeldSvg(Mutex<mpsc::Receiver<()>>);
    impl SvgDraw for HeldSvg {
        fn untrusted(
            &self,
            _: &[u8],
            _: &std::path::Path,
            _: Option<&[PathBuf]>,
            _: u32,
            _: &dyn Fn() -> bool,
        ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
            unreachable!()
        }
        fn embedded(
            &self,
            _: &[u8],
            _: u32,
            _: &dyn Fn() -> bool,
        ) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
            self.0.lock().unwrap().recv().unwrap();
            Err(konoma_svg::svg::RasterError::Cancelled)
        }
    }
    struct Canvas(Arc<Mutex<usize>>);
    impl Session for Canvas {
        fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
            *self.0.lock().unwrap() += 1;
            let mut canvas = CanvasView::new(req.cols, req.rows);
            canvas.images.push(CanvasImage::new(
                1,
                CellRect::new(0, 0, 2, 2),
                ImageData::Svg {
                    data: Arc::from(
                        b"<svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'/>"
                            .as_slice(),
                    ),
                    base_dir: None,
                    trusted: true,
                },
            ));
            Ok(View::Canvas(canvas))
        }
    }
    struct Timeout;
    impl EventSource for Timeout {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            assert!(wait.is_some(), "picture deadline wakes the Preview");
            Next::Timeout
        }
    }
    let cx = AppCx {
        path: "picture".into(),
        ..Default::default()
    };
    let keys = AppKeys::defaults();
    let mut runner = FileLoopState::new(&cx, &keys);
    let mut gutter = PreviewGutter::new(cx.path.clone(), PathBuf::new(), true, Default::default());
    gutter.yield_on_timeout = true;
    let (release, released) = mpsc::channel();
    let mut pictures = Pictures::new(
        ratatui_image::picker::Picker::halfblocks(),
        "picture",
        &cx.path,
        800,
    )
    .with_svg(Arc::new(HeldSvg(Mutex::new(released))))
    .with_out(Box::new(std::io::sink()));
    let calls = Arc::new(Mutex::new(0));
    let mut canvas = Canvas(calls.clone());
    let mut terminal = Terminal::new(TestBackend::new(32, 11)).unwrap();
    for _ in 0..3 {
        assert_eq!(
            run_file_with_preview(
                &mut terminal,
                &mut canvas,
                &mut Timeout,
                &cx,
                &mut runner,
                Some(&mut pictures),
                Some(&mut gutter)
            )
            .unwrap(),
            crate::app::looping::FileLoopExit::Yield
        );
        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "picture timeout redraws without asking the session again"
        );
    }
    // Release the one outstanding decode, including when it starts after the event fence.
    let _ = release.send(());
}

#[test]
fn preview_view_timer_rearms_after_yield_without_input() {
    struct Timed(usize);
    impl Session for Timed {
        fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, crate::Failure> {
            self.0 += 1;
            let mut canvas = crate::view::CanvasView::new(req.cols, req.rows);
            canvas.redraw_after = Some(Duration::from_millis(if self.0 == 1 { 200 } else { 400 }));
            Ok(View::Canvas(canvas))
        }
    }
    struct Deadline(bool);
    impl EventSource for Deadline {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            assert_eq!(
                wait,
                Some(Duration::from_millis(if self.0 { 400 } else { 200 })),
                "a timed view arms a fresh deadline after yielding"
            );
            if self.0 {
                Next::End
            } else {
                Next::Timeout
            }
        }
    }
    let cx = AppCx::default();
    let keys = AppKeys::defaults();
    let mut runner = FileLoopState::new(&cx, &keys);
    let mut gutter = PreviewGutter::new(PathBuf::new(), PathBuf::new(), true, Default::default());
    gutter.yield_on_timeout = true;
    let mut terminal = Terminal::new(TestBackend::new(32, 11)).unwrap();
    let mut timed = Timed(0);
    assert_eq!(
        run_file_with_preview(
            &mut terminal,
            &mut timed,
            &mut Deadline(false),
            &cx,
            &mut runner,
            None,
            Some(&mut gutter)
        )
        .unwrap(),
        crate::app::looping::FileLoopExit::Yield
    );
    assert_eq!(
        run_file_with_preview(
            &mut terminal,
            &mut timed,
            &mut Deadline(true),
            &cx,
            &mut runner,
            None,
            Some(&mut gutter)
        )
        .unwrap(),
        crate::app::looping::FileLoopExit::End
    );
    assert_eq!(timed.0, 2);
}
