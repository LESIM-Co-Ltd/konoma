use super::*;
use konoma_plugin::app::{DocEnv, DocPics, EventSource, InProcessSvg, Next, Pictures};
use konoma_plugin::canvas_pictures::EmbedRender;
use konoma_plugin::prelude::*;
use konoma_widgets::inline_images::{
    InlineImage, InlineImageCache, MdEncodeKey, MdImgEntry, MdProtoSlot,
};
use konoma_widgets::picture_overlay::{record_inline_paints_for_test, PicturePaintRequest};
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
use serde_json::{json, Value};

fn reference() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../snapshots/diff_rendered_screen.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn outcome(f: &Value) -> MdDiffOutcome {
    let old_pre = f["old"].as_str().unwrap().to_owned();
    let new_pre = f["new"].as_str().unwrap().to_owned();
    let (_, _, ops) = konoma_widgets::markdown::diff_align(&old_pre, &new_pre);
    MdDiffOutcome::Ready {
        old_pre,
        new_pre,
        ops,
        any_change: true,
        front_matter_only: false,
    }
}

fn config(wrap: bool) -> SharedConfig {
    let mut cfg = SharedConfig::parse(&format!(
        "[ui]\nlang='en'\ndiff_view='rendered'\nwrap={wrap}\n[git]\ndiff='unified'\n"
    ))
    .unwrap();
    for key in ["R", "x", "e", "o", "g", "!", "F"] {
        cfg.keys
            .surfaces
            .entry("preview_git_diff".into())
            .or_default()
            .insert(key.into(), "noop".into());
    }
    cfg
}

struct NoDraw;
impl Plugin for NoDraw {
    fn manifest(&self) -> Manifest {
        Manifest::new("fixture-mermaid", "1")
    }
    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        panic!("preseeded pictures never render a plugin")
    }
}

fn pictures(path: &Path, f: &Value) -> Pictures {
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut cache = InlineImageCache::default();
    cache.attach_picker(&picker);
    // Every possible band is ready. A wrong crop selects a different key and remains observable
    // in the actual overlay requests; no encoder, terminal or scheduler races participate.
    for source in [f["old"].as_str().unwrap(), f["new"].as_str().unwrap()] {
        for code in konoma_widgets::markdown::collect_mermaid_fences(source) {
            let key = konoma_widgets::markdown::mermaid_fence_url(&code);
            let pixels = image::RgbaImage::from_pixel(40, 160, image::Rgba([10, 20, 30, 255]));
            let mut entry = MdImgEntry {
                decoded: Some(Arc::new(image::DynamicImage::ImageRgba8(pixels.clone()))),
                layout_px: Some((40, 160)),
                ..Default::default()
            };
            for off in 0..8 {
                for visible in 1..=8 - off {
                    let encode = if off == 0 && visible == 8 {
                        MdEncodeKey::Full { cols: 4, rows: 8 }
                    } else {
                        MdEncodeKey::Clip {
                            cols: 4,
                            full_rows: 8,
                            row_off: off,
                            vis_rows: visible,
                        }
                    };
                    let image = InlineImage::Kitty(konoma_widgets::kitty::KittyImage::build(
                        &pixels, 4, visible, 42, false,
                    ));
                    let mut sent = Buffer::empty(Rect::new(0, 0, 4, visible));
                    image.render(sent.area, &mut sent);
                    let slot = MdProtoSlot::encoded(encode, image);
                    if off == 0 && visible == 8 {
                        entry.full.push(slot);
                    } else {
                        entry.clip.push(slot);
                    }
                }
            }
            assert_eq!(
                entry.layout_px,
                Some((
                    f["natural"][0].as_u64().unwrap() as u32,
                    f["natural"][1].as_u64().unwrap() as u32
                ))
            );
            assert_eq!(
                entry.decoded.as_ref().unwrap().to_rgba8().into_raw(),
                [10, 20, 30, 255].repeat(40 * 160)
            );
            cache.cache.insert(key.into(), entry);
        }
    }
    let (_tx, rx) = mpsc::channel();
    let mut doc =
        DocPics::new(&picker, "markdown", path, 800, Arc::new(InProcessSvg)).with_cache(cache, rx);
    doc.set_env(DocEnv {
        embed: Box::new(|kind, svg| {
            (kind == "fence:mermaid").then(|| {
                EmbedRender::new(Arc::new(NoDraw), Settings::new(), "en", false, svg.clone())
            })
        }),
        remote_images: false,
        fetcher: |_, _| panic!("no network"),
    });
    let mut p = Pictures::new(picker, "markdown", path, 800).with_out(Box::new(Vec::<u8>::new()));
    *p.doc_pics() = doc;
    p
}

fn cells(buf: &Buffer) -> Value {
    assert_eq!(buf.area, Rect::new(0, 0, 54, 14));
    let mut runs: Vec<Value> = Vec::new();
    let mut nonempty = 0;
    let mut compared = 0;
    for cell in &buf.content {
        let value = json!([
            cell.symbol(),
            format!("{:?}", cell.fg),
            format!("{:?}", cell.bg),
            cell.modifier.bits()
        ]);
        if let Some(last) = runs.last_mut().filter(|r| r[1] == value) {
            last[0] = json!(last[0].as_u64().unwrap() + 1);
        } else {
            runs.push(json!([1, value]));
        }
        nonempty += usize::from(!cell.symbol().trim().is_empty());
        compared += 1;
    }
    assert_eq!(compared, 756);
    assert!(nonempty > 0);
    json!({"rect":[0,0,54,14],"count":compared,"runs":runs})
}

fn paints(requests: Vec<(String, PicturePaintRequest)>) -> Value {
    let mut latest = std::collections::BTreeMap::new();
    for (key, r) in requests {
        latest.insert(format!("{key}/{:?}",r.kind), json!({"key":key,"kind":format!("{:?}",r.kind),"target":[r.target.x,r.target.y,r.target.width,r.target.height],"cols":r.cols,"rows":r.rows,"offset":r.row_offset,"visible":r.visible_rows}));
    }
    json!(latest.into_values().collect::<Vec<_>>())
}

struct Keys(std::collections::VecDeque<Key>);
impl EventSource for Keys {
    fn next(&mut self, _: Option<Duration>) -> Next {
        self.0
            .pop_front()
            .map(|k| Next::Event(konoma_plugin::app::AppEvent::Key(k)))
            .unwrap_or(Next::End)
    }
}

pub fn check_screens() {
    let f = reference();
    assert_eq!(f["rgba"], json!([10, 20, 30, 255]));
    assert_eq!(f["rect"], json!([0, 0, 54, 14]));
    assert_eq!(f["page_count"], 1);
    let mut cropped = false;
    for case in f["cases"].as_array().unwrap() {
        for count in 0..=f["keys"].as_array().unwrap().len() {
            let dir = konoma_test_support::unique_tmp("rendered_cells");
            let root = dir.join("reference");
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("doc.md");
            std::fs::write(&path, f["new"].as_str().unwrap()).unwrap();
            let outcome = outcome(&f);
            let (worker, rx, ready) = fenced_worker(move |_| outcome.clone());
            let cfg = config(case["wrap"].as_bool().unwrap());
            let mut state = DiffScreenState::open_with_markdown_worker(
                request(&root, "doc.md"),
                Arc::new(Services),
                &cfg,
                DiffLayout::Unified,
                Some(DiffView::Rendered),
                worker,
                rx,
            )
            .unwrap();
            state.poll().unwrap();
            ready.recv_timeout(DEADLINE).unwrap();
            let mut p = prepared(&root, state, cfg);
            let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
            let mut keys = Keys(
                f["keys"].as_array().unwrap()[..count]
                    .iter()
                    .map(|k| {
                        Key::new(match k.as_str().unwrap() {
                            "PageDown" => KeyCode::PageDown,
                            "Home" => KeyCode::Home,
                            "End" => KeyCode::End,
                            s => KeyCode::Char(s.chars().next().unwrap()),
                        })
                    })
                    .collect(),
            );
            let pics = case["pictures"]
                .as_bool()
                .unwrap()
                .then(|| pictures(&path, &f));
            let (_, requests) = record_inline_paints_for_test(|| {
                konoma_plugin::app::project::run_project_with(
                    &mut terminal,
                    &mut p,
                    &mut keys,
                    &Default::default(),
                    pics,
                )
                .unwrap()
            });
            cropped |= requests.iter().any(|(_, r)| r.row_offset > 0);
            let actual = json!({"cells":cells(terminal.backend().buffer()),"paints":paints(requests),"scroll":p.diff.as_ref().unwrap().scroll});
            assert_stage(&actual, &case["stages"][count], count);
        }
    }
    assert!(cropped, "keys exercise a real cropped picture band");
    // The standalone can execute only these two views. A remapped or disabled R stays inert.
    let dir = konoma_test_support::unique_tmp("rendered_cycle");
    let result = outcome(&f);
    let (worker, rx, ready) = fenced_worker(move |_| result.clone());
    let mut state = DiffScreenState::open_with_markdown_worker(
        request(&dir, "doc.md"),
        Arc::new(Services),
        &SharedConfig::default(),
        DiffLayout::Unified,
        Some(DiffView::Source),
        worker,
        rx,
    )
    .unwrap();
    state
        .handle_key(konoma_host_core::keymap::KeyPress::ch('R'))
        .unwrap();
    assert_eq!(state.choice, DiffView::Rendered);
    ready.recv_timeout(DEADLINE).unwrap();
    state.prepare_frame(Rect::new(0, 1, 54, 12), None);
    let mut p = prepared(&dir, state, SharedConfig::parse("[ui]\nlang='en'").unwrap());
    let mut terminal = Terminal::new(TestBackend::new(180, 14)).unwrap();
    konoma_plugin::app::project::run_project_with(
        &mut terminal,
        &mut p,
        &mut Keys(Default::default()),
        &Default::default(),
        None,
    )
    .unwrap();
    let footer = terminal.backend().buffer().content[180 * 13..]
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(
        footer.contains("R:source"),
        "R advertises its executable next view: {footer}"
    );
    assert!(
        !footer.contains("preview"),
        "unimplemented Preview has no hint"
    );
    let state = p.diff.as_mut().unwrap();
    state
        .handle_key(konoma_host_core::keymap::KeyPress::ch('R'))
        .unwrap();
    assert_eq!(
        (state.choice, state.effective),
        (DiffView::Source, DiffView::Source)
    );
}

fn assert_stage(actual: &Value, expected: &Value, count: usize) {
    assert_eq!(actual["scroll"], expected["scroll"], "key count {count}");
    assert_eq!(
        actual["paints"], expected["paints"],
        "overlay placements and crop, key count {count}"
    );
    assert_eq!(actual["cells"]["rect"], expected["cells"]["rect"]);
    assert_eq!(actual["cells"]["count"], expected["cells"]["count"]);
    let expand = |v: &Value| -> Vec<Value> {
        v["cells"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| std::iter::repeat_n(r[1].clone(), r[0].as_u64().unwrap() as usize))
            .collect()
    };
    let a = expand(actual);
    let b = expand(expected);
    assert_eq!((a.len(), b.len()), (756, 756));
    let mut compared = 0;
    for (i, (actual, expected)) in a.iter().zip(&b).enumerate() {
        assert_eq!(actual, expected, "key count {count}, cell {i}");
        compared += 1;
    }
    assert_eq!(compared, 756);
}
