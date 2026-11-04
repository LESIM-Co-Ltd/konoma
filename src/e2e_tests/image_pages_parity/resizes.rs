//! Scoped memoization of completed halfblocks renderers for an immutable PDF. Requests and
//! responses are opaque in ratatui-image; keep the completed viewer instead. Every operation
//! still lays out the real image and queues its resize before we look up that exact geometry.

use super::*;
use konoma_widgets::image_view::ImageViewer;
use std::collections::BTreeMap;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    file: std::path::PathBuf,
    page: (u32, u32),
    size: (u16, u16),
    crop: (u32, u32, u32, u32),
    pose: [u64; 3],
}

/// The channel and cache belong to one synchronous walk group, never to another test. No
/// request remains in flight when a viewer enters the cache, so its old generation is settled.
pub(super) struct Resizes {
    tx: tokio::sync::mpsc::UnboundedSender<ratatui_image::thread::ResizeRequest>,
    rx: tokio::sync::mpsc::UnboundedReceiver<ratatui_image::thread::ResizeRequest>,
    ready: BTreeMap<Key, ImageViewer>,
    pub(super) computed: usize,
    pub(super) reused: usize,
}

impl Default for Resizes {
    fn default() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            tx,
            rx,
            ready: BTreeMap::new(),
            computed: 0,
            reused: 0,
        }
    }
}

impl Resizes {
    pub(super) fn attach(&self, s: &mut Sim) {
        s.app
            .attach_image_backend(Picker::halfblocks(), self.tx.clone());
    }

    pub(super) fn settle(&mut self, s: &mut Sim, file: &std::path::Path) {
        let pose = s.app.tab.image_view;
        let key = Key {
            file: file.to_path_buf(),
            page: s.app.page_indicator().expect("a PDF page"),
            size: (s.term.size().unwrap().width, s.term.size().unwrap().height),
            crop: s
                .app
                .image_viewer
                .crop()
                .expect("the real image was laid out"),
            pose: [
                pose.zoom.to_bits(),
                pose.center.0.to_bits(),
                pose.center.1.to_bits(),
            ],
        };
        if let Some(viewer) = self.ready.remove(&key) {
            // Only the pure resize is skipped: the key, media load and layout have finished.
            while self.rx.try_recv().is_ok() {}
            assert_eq!(viewer.crop(), s.app.image_viewer.crop());
            assert_eq!(viewer.vis_frac(), s.app.image_viewer.vis_frac());
            s.app.image_viewer = viewer;
            self.reused += 1;
            s.draw();
        } else {
            while let Ok(req) = self.rx.try_recv() {
                self.computed += 1;
                s.app.apply_image_resize(req.resize_encode());
                s.draw();
            }
        }
        assert!(!s.app.image_viewer.resize_pending());
        // Motion uses the previous visible fraction. Retain that layout while lending the
        // completed renderer to the cache; the next draw builds its real protocol as usual.
        let mut layout = ImageViewer::new();
        layout.set_layout(key.crop, s.app.image_viewer.vis_frac());
        self.ready
            .insert(key, std::mem::replace(&mut s.app.image_viewer, layout));
    }
}

#[test]
fn cached_resizes_match_fresh_pixels_at_every_frame() {
    let dir = sandbox("cached-pdf-resizes");
    make_pdf(&dir, "three.pdf", 3);
    std::fs::write(
        dir.join("one.pdf"),
        konoma_test_support::build_pdf(&[(200, 300, b"0 1 1 rg 0 0 200 300 re f" as &[u8]); 3]),
    )
    .unwrap();
    let root = canon(&dir);
    let plugin = Arc::new(cached_pdf::CachedPdf::default());
    let _media = super::super::plugin_act_parity::MediaPluginGuard::set(plugin);
    let mut cache = Resizes::default();
    let mut checked = 0;
    for (file, h, spec) in [
        ("three.pdf", HOST_H, ""),
        ("one.pdf", HOST_H, ""),
        ("three.pdf", HOST_H - 2, ""),
        ("three.pdf", HOST_H - 2, "+ J h"),
        ("three.pdf", HOST_H, "+ + + + j"),
    ]
    .into_iter()
    .chain(SPECS.iter().map(|s| ("three.pdf", HOST_H, *s)))
    {
        let mut fresh = Vec::new();
        host_pdf_walk(&root, file, h, spec, true, |s, _| {
            fresh.push((
                s.term.backend().buffer().clone(),
                s.app.tab.image_view,
                s.app.image_viewer.crop(),
                s.app.image_viewer.vis_frac(),
            ))
        });
        host_pdf_walk_cached(&root, file, h, spec, &mut cache, |s, n| {
            assert_eq!(
                s.term.backend().buffer(),
                &fresh[n].0,
                "{file} {h} [{spec}] frame {n}"
            );
            assert_eq!(
                s.app.tab.image_view, fresh[n].1,
                "{file} {h} [{spec}] frame {n}"
            );
            assert_eq!(s.app.image_viewer.crop(), fresh[n].2);
            assert_eq!(s.app.image_viewer.vis_frac(), fresh[n].3);
            checked += 1;
        });
    }
    assert_eq!(checked, 65);
    assert!(cache.reused > 20);
}
