use super::*;
use konoma_plugin::app::{DocPics, InProcessSvg};
use konoma_widgets::inline_images::{
    md_encode_worker, InlineImageCache, MdEncodeRequest, MdEncodeResult,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{path::Path, sync::Mutex, time::Instant};
const DEADLINE: Duration = Duration::from_secs(10);

pub(super) fn check() {
    let dir = konoma_test_support::unique_tmp("media_wake");
    let mut prepared = support::prepared(&dir);
    let (entered, entries) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let held = Mutex::new(held);
    let (tx, rx) = mpsc::channel();
    let (forward, replies) = mpsc::channel();
    let (published, ready) = mpsc::channel();
    let mut worker = MediaDiffWorker::with_compute(Arc::new(move |req| {
        entered.send(()).unwrap();
        held.lock().unwrap().recv().unwrap();
        computed(req)
    }));
    worker.attach(tx);
    std::thread::spawn(move || {
        while let Ok(r) = rx.recv() {
            if forward.send(r).is_err() || published.send(()).is_err() {
                break;
            }
        }
    });
    prepared.diff = Some(
        prepared
            .diff
            .take()
            .unwrap()
            .with_media_worker(worker, replies),
    );
    struct Events {
        step: usize,
        entered: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
        ready: mpsc::Receiver<()>,
    }
    impl EventSource for Events {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            let step = self.step;
            self.step += 1;
            match step {
                0 => {
                    self.entered.recv_timeout(DEADLINE).unwrap();
                    assert_eq!(
                        wait,
                        Some(Duration::from_millis(16)),
                        "media worker alone wakes without pictures"
                    );
                    Next::Event(AppEvent::Key(Key::char('r')))
                }
                1 => Next::Event(AppEvent::Key(Key::char('R'))),
                2 | 3 => {
                    assert_eq!(
                        wait,
                        Some(Duration::from_millis(16)),
                        "busy and queued media needs its own deadline"
                    );
                    if step == 3 {
                        self.entered
                            .recv_timeout(DEADLINE)
                            .expect("the queued request is dispatched after the canceled result");
                    }
                    self.release.send(()).unwrap();
                    self.ready.recv_timeout(DEADLINE).unwrap();
                    Next::Timeout
                }
                _ => Next::End,
            }
        }
    }
    let mut events = Events {
        step: 0,
        entered: entries,
        release,
        ready,
    };
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(54, 14)).unwrap();
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut prepared,
        &mut events,
        &Default::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        prepared.diff.as_ref().unwrap().wake_in(),
        None,
        "queued work drains even after switching to Source"
    );
    prepared
        .diff
        .as_mut()
        .unwrap()
        .handle_key(konoma_host_core::keymap::KeyPress::ch('R'))
        .unwrap();
    assert!(
        prepared
            .diff
            .as_ref()
            .unwrap()
            .presentation()
            .enabled(konoma_plugin::host::project::diff_presentation::DiffUiOp::Layout),
        "the result lands without another key"
    );

    // A supplied clock advances a GIF and its ordinary encoder, with no terminal or sleeps.
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut cache = InlineImageCache::default();
    cache.attach_picker(&picker);
    let (tx, requests) = mpsc::channel::<MdEncodeRequest>();
    cache.enc_tx = Some(tx);
    let (encoded, enc_rx) = mpsc::channel::<MdEncodeResult>();
    let (mut pics, mut state) = (
        DocPics::new(
            &picker,
            "fixture",
            Path::new("/fixture/side.svg"),
            128,
            Arc::new(InProcessSvg),
        )
        .with_cache(cache, enc_rx),
        prepared.diff.take().unwrap(),
    );
    state = state.with_media_worker(
        MediaDiffWorker::with_compute(Arc::new(|req| {
            let mut result = computed(req);
            if let MediaDiffComputed::Ready {
                ref mut old,
                ref mut new,
                ..
            } = result
            {
                for side in [old, new] {
                    if let MediaDiffSideDecoded::Picture(p) = side {
                        p.frames = Some(vec![
                            (p.image.clone(), Duration::from_millis(40)),
                            (
                                image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                                    2,
                                    1,
                                    image::Rgba([60, 70, 80, 255]),
                                )),
                                Duration::from_millis(40),
                            ),
                        ]);
                    }
                }
            }
            result
        })),
        mpsc::channel().1,
    );
    let area = Rect::new(0, 1, 54, 12);
    state.prepare_frame(area, Some(&mut pics));
    // Exercise the actual frame through another run: DocPics itself exposes only the overlay.
    let mut p = support::prepared(&dir);
    p.diff = Some(state);
    let mut pictures =
        konoma_plugin::app::Pictures::new(picker.clone(), "fixture", &dir.join("side.svg"), 128)
            .with_out(Box::new(std::io::sink()));
    *pictures.doc_pics() = pics;
    struct Encode {
        requests: mpsc::Receiver<MdEncodeRequest>,
        encoded: mpsc::Sender<MdEncodeResult>,
        step: usize,
        picker: ratatui_image::picker::Picker,
    }
    impl EventSource for Encode {
        fn next(&mut self, wait: Option<Duration>) -> Next {
            if self.step > 0 {
                return Next::End;
            }
            self.step += 1;
            assert_eq!(
                wait,
                Some(Duration::from_millis(10)),
                "the first GIF deadline beats the encode wait without input"
            );
            for i in 0..2 {
                let mut req = self.requests.recv_timeout(DEADLINE).unwrap();
                req.kitty = Some((42 + i, false));
                let (tx, rx) = mpsc::channel();
                tx.send(req).unwrap();
                drop(tx);
                md_encode_worker(self.picker.clone(), rx, self.encoded.clone());
            }
            Next::Timeout
        }
    }
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut p,
        &mut Encode {
            requests,
            encoded,
            step: 0,
            picker: picker.clone(),
        },
        &Default::default(),
        Some(pictures),
    )
    .unwrap();

    let reference: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/diff_media_screen.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let stage = &reference["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "matrix-Media-unified-false")
        .unwrap()["stages"][0];
    let request = &stage["encodes"][0];
    let placement = stage["paints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == request["key"])
        .unwrap();
    let target = &placement["target"];
    assert_eq!(
        terminal.backend().buffer()[(
            target[0].as_u64().unwrap() as u16,
            target[1].as_u64().unwrap() as u16
        )]
            .symbol(),
        request["kitty"].as_str().unwrap(),
        "the idle encode paints the exact fixed host kitty transfer at the shared target"
    );

    let mut pics = DocPics::new(
        &picker,
        "fixture",
        Path::new("/fixture/side.svg"),
        128,
        Arc::new(InProcessSvg),
    );
    let now = Instant::now();
    let mut result = computed(&MediaDiffRequest {
        gen: 0,
        path: "/fixture/side.svg".into(),
        root: "/fixture".into(),
        baseline: konoma_plugin::host::project::md_diff::DiffBaseline::Empty,
        page: 1,
        raster_px: (64, 64),
        classify: Arc::new(|_, _| konoma_host_core::project::DiffTarget::Picture),
        rasterize_svg: svg,
        cancel: Arc::default(),
    });
    if let MediaDiffComputed::Ready {
        old: MediaDiffSideDecoded::Picture(ref mut p),
        ..
    } = result
    {
        p.frames = Some(vec![
            (p.image.clone(), Duration::from_millis(40)),
            (p.image.clone(), Duration::from_millis(40)),
        ]);
    }
    pics.materialize_media_diff(result);
    let laid = konoma_plugin::app::Laid {
        lines: vec!["".into(); 4],
        row_prefix: vec![],
        max_line_cols: 4,
        images: vec![konoma_widgets::markdown::ImagePlacement {
            url: "media-diff://o/1/64x64".into(),
            alt: String::new(),
            line: 0,
            col: 0,
            cols: 4,
            rows: 4,
            fence_ord: None,
        }],
    };
    pics.draw(&mut Buffer::empty(area), area, Some((&laid, 0)));
    pics.poll_at(now);
    pics.take_relayout();
    pics.take_redraw();
    assert_eq!(
        pics.wake_in_at(Duration::from_millis(16), now + Duration::from_millis(35)),
        Some(Duration::from_millis(10)),
        "GIF deadline beats a longer worker/encode wait"
    );
    pics.poll_at(now + Duration::from_millis(40));
    assert!(
        pics.take_redraw(),
        "GIF frame completion redraws without input"
    );
}
fn svg(
    _: &[u8],
    _: &Path,
    _: u32,
    _: &dyn Fn() -> bool,
) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
    unreachable!()
}
