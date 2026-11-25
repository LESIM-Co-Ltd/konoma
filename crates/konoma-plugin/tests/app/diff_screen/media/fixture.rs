//! Full rectangles and picture requests against data captured by the host bridge.
use super::*;
use konoma_host_core::{
    project::{DiffLayout, DiffView},
    shared::SharedConfig,
};
use konoma_plugin::app::diff_screen::{
    DiffScreenState, FileDiffData, FileDiffReply, FileDiffRequest,
};
use konoma_plugin::app::project::{ProjectServiceInfo, ProjectServices, ProjectSettings};
use konoma_plugin::app::{DocPics, InProcessSvg, Pictures};
use konoma_plugin::host::project::{
    md_diff::{MdDiffOutcome, MdDiffWorker},
    MatchedProject,
};
use konoma_plugin::Failure;
use konoma_vcs_core::git::{ChangeEntry, DiffLine, DiffLineKind};
use konoma_widgets::inline_images::InlineImageCache;
use konoma_widgets::picture_overlay::record_inline_paints_for_test;
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};
use serde_json::{json, Value};
use std::path::Path;

fn lines() -> Vec<DiffLine> {
    (0..60)
        .map(|i| DiffLine {
            kind: match i % 3 {
                0 => DiffLineKind::Removed,
                1 => DiffLineKind::Added,
                _ => DiffLineKind::Context,
            },
            old_no: Some(i + 1),
            new_no: Some(i + 1),
            text: format!("row {i:02}: alpha beta gamma delta epsilon zeta eta theta iota kappa"),
        })
        .collect()
}
struct Services;
impl ProjectServices for Services {
    fn inspect(
        &self,
        _: &MatchedProject,
        _: &ProjectSettings,
    ) -> Result<ProjectServiceInfo, Failure> {
        unreachable!()
    }
    fn file_diff(&self, _: &ProjectServiceInfo, _: &Path) -> Vec<DiffLine> {
        unreachable!()
    }
    fn changed_files(&self, _: &ProjectServiceInfo) -> Vec<ChangeEntry> {
        unreachable!()
    }
    fn base_contents(&self, _: &ProjectServiceInfo, _: &Path) -> Option<Vec<u8>> {
        unreachable!()
    }
    fn spawn_file_diff(self: Arc<Self>, req: FileDiffRequest) -> Result<FileDiffReply, Failure> {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(FileDiffData {
            lines: if req.path.extension().is_some_and(|e| e == "md") {
                lines()
            } else {
                vec![]
            },
            paths: vec![req.path],
        }))
        .unwrap();
        Ok(rx)
    }
}
fn encoded_request(mut r: konoma_widgets::inline_images::MdEncodeRequest) -> Value {
    let mut value = json!({"key":r.path,"cols":r.cols,"rows":r.rows,"crop":r.crop,"rgba":r.image.to_rgba8().into_raw()});
    if r.cols == 4 && r.rows == 1 {
        r.kitty = Some((42, false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (done, replies) = std::sync::mpsc::channel();
        tx.send(r).unwrap();
        drop(tx);
        konoma_widgets::inline_images::md_encode_worker(
            ratatui_image::picker::Picker::halfblocks(),
            rx,
            done,
        );
        let image = replies.recv().unwrap().image.unwrap();
        let area = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(area);
        image.render(area, &mut buf);
        let transfer = buf[(0, 0)].symbol();
        assert!(
            transfer.starts_with("\x1b_Gq=2,i=42,"),
            "a fixed kitty transfer is drawn at its target"
        );
        value["kitty"] = json!(transfer);
    }
    value
}

fn cells(buf: &Buffer) -> Value {
    assert_eq!(buf.area, Rect::new(0, 0, 54, 14));
    assert_eq!(buf.content.len(), 756);
    assert!(buf.content.iter().any(|c| !c.symbol().trim().is_empty()));
    let mut runs: Vec<Value> = vec![];
    let mut palette: Vec<Value> = vec![];
    for c in &buf.content {
        let value = json!([
            c.symbol(),
            format!("{:?}", c.fg),
            format!("{:?}", c.bg),
            c.modifier.bits()
        ]);
        let index = palette.iter().position(|p| *p == value).unwrap_or_else(|| {
            palette.push(value);
            palette.len() - 1
        });
        if let Some(last) = runs.last_mut().filter(|r| r[1] == index) {
            last[0] = json!(last[0].as_u64().unwrap() + 1);
        } else {
            runs.push(json!([1, index]));
        }
    }
    json!({"rect":[0,0,54,14],"count":756,"palette":palette,"runs":runs})
}
fn stage(f: &Value, case: &Value, count: usize) -> Value {
    let dir = konoma_test_support::unique_tmp("media_screen_sdk");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    let mut prepared = support::prepared(&root);
    let name = if case["surface"] == "Media" {
        "picture.pdf"
    } else {
        "doc.md"
    };
    let path = root.join(name);
    std::fs::write(&path, f["new"].as_str().unwrap()).unwrap();
    let mut cfg = SharedConfig::parse(&format!(
        "[ui]\nlang='en'\nbusy_indicator=false\ndiff_view='{}'\n[git]\ndiff='{}'\nmedia_diff='{}'",
        if case["surface"] == "Source" {
            "source"
        } else {
            "rendered"
        },
        case["line"].as_str().unwrap(),
        case["layout"].as_str().unwrap()
    ))
    .unwrap();
    for key in ["R", "x", "e", "o", "g", "!", "F"] {
        cfg.keys
            .surfaces
            .entry("preview_git_diff".into())
            .or_default()
            .insert(key.into(), "noop".into());
    }
    let old = f["old"].as_str().unwrap().to_owned();
    let new = f["new"].as_str().unwrap().to_owned();
    let (_, _, ops) = konoma_widgets::markdown::diff_align(&old, &new);
    let markdown = MdDiffWorker::with_compute(Arc::new(move |_| MdDiffOutcome::Ready {
        old_pre: old.clone(),
        new_pre: new.clone(),
        ops: ops.clone(),
        any_change: true,
        front_matter_only: false,
    }));
    let mut request = prepared.diff.as_ref().unwrap().request.clone();
    request.path = path.clone();
    let state = DiffScreenState::open_with_markdown_worker(
        request,
        Arc::new(Services),
        &cfg,
        if case["line"] == "split" {
            DiffLayout::Split
        } else {
            DiffLayout::Unified
        },
        Some(if case["surface"] == "Source" {
            DiffView::Source
        } else {
            DiffView::Rendered
        }),
        markdown,
        mpsc::channel().1,
    )
    .unwrap();
    let case_owned = case.clone();
    let media = MediaDiffWorker::with_compute(Arc::new(move |req| {
        match case_owned["sides"].as_str().unwrap() {
            "placeholders" => MediaDiffComputed::Ready {
                kind: MediaDiffKind::Pdf,
                base: MediaBase::Head,
                same_bytes: false,
                old: MediaDiffSideDecoded::Absent,
                new: MediaDiffSideDecoded::Failed {
                    reason: "invalid PDF".into(),
                },
            },
            "missing" => MediaDiffComputed::Ready {
                kind: MediaDiffKind::Pdf,
                base: MediaBase::Head,
                same_bytes: false,
                old: MediaDiffSideDecoded::PageMissing,
                new: MediaDiffSideDecoded::Absent,
            },
            "summary" => MediaDiffComputed::Summary {
                base: MediaBase::Head,
                same_bytes: false,
                old_len: Some(8),
                new_len: Some(16),
            },
            "unavailable" => MediaDiffComputed::Unavailable,
            _ => computed(req),
        }
    }));
    prepared.diff = Some(state.with_media_worker(media, mpsc::channel().1));
    prepared.config = cfg;
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut cache = InlineImageCache::default();
    cache.attach_picker(&picker);
    let (tx, encodes) = mpsc::channel();
    cache.enc_tx = Some(tx);
    let pics = case["picker"].as_bool().unwrap().then(|| {
        let mut p = Pictures::new(picker.clone(), "fixture", &path, 128)
            .with_out(Box::new(std::io::sink()));
        *p.doc_pics() = DocPics::new(&picker, "fixture", &path, 128, Arc::new(InProcessSvg))
            .with_cache(cache, mpsc::channel().1);
        p
    });
    struct Keys(std::vec::IntoIter<Key>);
    impl EventSource for Keys {
        fn next(&mut self, _: Option<Duration>) -> Next {
            self.0
                .next()
                .map_or(Next::End, |k| Next::Event(AppEvent::Key(k)))
        }
    }
    let mut keys = Keys(
        case["keys"].as_array().unwrap()[..count]
            .iter()
            .map(|k| match k.as_str().unwrap() {
                "PageDown" => Key::new(KeyCode::PageDown),
                "PageUp" => Key::new(KeyCode::PageUp),
                s => Key::char(s.chars().next().unwrap()),
            })
            .collect::<Vec<_>>()
            .into_iter(),
    );
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    let (_, requests) = record_inline_paints_for_test(|| {
        konoma_plugin::app::project::run_project_with(
            &mut terminal,
            &mut prepared,
            &mut keys,
            &Default::default(),
            pics,
        )
        .unwrap()
    });
    let mut paints = std::collections::BTreeMap::new();
    for (key, r) in requests {
        paints.insert(format!("{key}/{:?}",r.target),json!({"key":key,"target":[r.target.x,r.target.y,r.target.width,r.target.height],"cols":r.cols,"rows":r.rows,"offset":r.row_offset,"visible":r.visible_rows}));
    }
    let encodes: Vec<_> = encodes.try_iter().map(encoded_request).collect();
    json!({"cells":cells(terminal.backend().buffer()),"paints":paints.into_values().collect::<Vec<_>>(),"encodes":encodes,"page":prepared.diff.as_ref().unwrap().media_page()})
}
pub(super) fn check_screens() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/diff_media_screen.json");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(f["rect"], json!([0, 0, 54, 14]));
    let mut matrix = 0;
    for case in f["cases"].as_array().unwrap() {
        if case["name"].as_str().unwrap().starts_with("matrix-") {
            matrix += 1;
        }
        for n in 0..=case["keys"].as_array().unwrap().len() {
            let actual = stage(&f, case, n);
            let expected = &case["stages"][n];
            assert_eq!(actual["cells"]["rect"], expected["cells"]["rect"]);
            assert_eq!(actual["cells"]["count"], expected["cells"]["count"]);
            let expand = |v: &Value| {
                v["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|r| {
                        std::iter::repeat_n(
                            v["palette"][r[1].as_u64().unwrap() as usize].clone(),
                            r[0].as_u64().unwrap() as usize,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let actual_cells = expand(&actual["cells"]);
            let expected_cells = expand(&expected["cells"]);
            assert_eq!(actual_cells.len(), 756);
            assert_eq!(expected_cells.len(), 756);
            for (i, (a, e)) in actual_cells.iter().zip(&expected_cells).enumerate() {
                assert_eq!(
                    a,
                    e,
                    "{} stage {n} cell ({},{}): symbol/fg/bg/modifier",
                    case["name"],
                    i % 54,
                    i / 54
                );
            }
            assert_eq!(
                actual["paints"], expected["paints"],
                "{} stage {n}: keys and target/crop reservations",
                case["name"]
            );
            assert_eq!(
                actual["encodes"], expected["encodes"],
                "{} stage {n}: encode pixels and sizes",
                case["name"]
            );
            assert_eq!(actual["page"], expected["page"], "shared page");
        }
    }
    assert_eq!(matrix, 12);
}

pub(super) fn check_real_pdf_pages() {
    fn pdf(pages: usize) -> Vec<u8> {
        let kids = (0..pages)
            .map(|i| format!("{} 0 R", 3 + i))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        ];
        for _ in 0..pages {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 20] /Contents {} 0 R >>",
                3 + pages
            ));
        }
        let stream = "0 0 1 rg 0 0 40 20 re f\n";
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}endstream",
            stream.len()
        ));
        let mut data = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(data.len());
            data.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
        }
        let start = data.len();
        data.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes(),
        );
        for offset in &offsets[1..] {
            data.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        data.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        data
    }
    let dir = konoma_test_support::unique_tmp("media_pdf_shared_page");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("picture.pdf");
    std::fs::write(&path, pdf(4)).unwrap();
    let mut worker = MediaDiffWorker::default();
    let (tx, rx) = mpsc::channel();
    worker.attach(tx);
    worker.dispatch(MediaDiffRequest {
        gen: 0,
        path,
        root: dir.to_path_buf(),
        baseline: konoma_plugin::host::project::md_diff::DiffBaseline::FollowSnapshot(pdf(2)),
        page: 3,
        raster_px: (64, 64),
        classify: Arc::new(|_, _| konoma_host_core::project::DiffTarget::Picture),
        rasterize_svg: unused_svg,
        cancel: Arc::default(),
    });
    let result = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let MediaDiffComputed::Ready {
        old: MediaDiffSideDecoded::PageMissing,
        new: MediaDiffSideDecoded::Picture(new),
        ..
    } = result.computed
    else {
        panic!("both sides decode the same page: old page 3 is missing, new page 3 is a picture")
    };
    assert_eq!(new.page_count, Some(4));
    assert_eq!(new.natural_px, (40, 20));
    assert!(
        new.cache_key.to_string_lossy().ends_with("/3/64x64"),
        "both sides receive the shared page"
    );
}
fn unused_svg(
    _: &[u8],
    _: &Path,
    _: u32,
    _: &dyn Fn() -> bool,
) -> Result<image::DynamicImage, konoma_svg::svg::RasterError> {
    unreachable!()
}
