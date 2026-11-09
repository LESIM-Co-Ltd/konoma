//! Full-screen reference data, captured through konoma's real key dispatcher and renderer.
use super::*;
use konoma_widgets::picture_overlay::record_inline_paints_for_test;
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};
use serde_json::{json, Value};
use worker::MediaDiffPictureDecoded;

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

fn computed(case: &Value, req: &MediaDiffRequest) -> MediaDiffComputed {
    let side = |old: bool| {
        let count = if old { 2 } else { 4 };
        if req.page > count {
            return worker::MediaDiffSideDecoded::PageMissing;
        }
        worker::MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            natural_px: if old { (40, 20) } else { (20, 40) },
            bytes: 8,
            page_count: Some(count),
            image: image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                2,
                1,
                image::Rgba([10, 20, 30, 255]),
            )),
            frames: None,
            svg: None,
            svg_base: None,
            cache_key: format!(
                "media-diff://{}/{}/{}x{}",
                if old { "o" } else { "n" },
                req.page,
                req.raster_px.0,
                req.raster_px.1
            )
            .into(),
        }))
    };
    let (old, new) = match case["sides"].as_str().unwrap() {
        "placeholders" => (
            worker::MediaDiffSideDecoded::Absent,
            worker::MediaDiffSideDecoded::Failed {
                reason: "invalid PDF".into(),
            },
        ),
        "missing" => (
            worker::MediaDiffSideDecoded::PageMissing,
            worker::MediaDiffSideDecoded::Absent,
        ),
        "summary" => {
            return MediaDiffComputed::Summary {
                base: MediaBase::Head,
                same_bytes: false,
                old_len: Some(8),
                new_len: Some(16),
            }
        }
        "unavailable" => return MediaDiffComputed::Unavailable,
        _ => (side(true), side(false)),
    };
    MediaDiffComputed::Ready {
        kind: MediaDiffKind::Pdf,
        base: MediaBase::Head,
        same_bytes: false,
        old,
        new,
    }
}

fn lines() -> Vec<crate::git::DiffLine> {
    (0..60)
        .map(|i| crate::git::DiffLine {
            kind: match i % 3 {
                0 => crate::git::DiffLineKind::Removed,
                1 => crate::git::DiffLineKind::Added,
                _ => crate::git::DiffLineKind::Context,
            },
            old_no: Some(i + 1),
            new_no: Some(i + 1),
            text: format!("row {i:02}: alpha beta gamma delta epsilon zeta eta theta iota kappa"),
        })
        .collect()
}

fn draw(t: &mut Terminal<TestBackend>, app: &mut App) {
    t.draw(|f| crate::ui::render(f, app)).unwrap();
    if app.take_md_overlay_moved() {
        t.clear().unwrap();
        t.draw(|f| crate::ui::render(f, app)).unwrap();
    }
}

fn stage(f: &Value, case: &Value, count: usize) -> Value {
    let dir = crate::test_support::unique_tmp("media_screen_host");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    let name = if case["surface"] == "Media" {
        "picture.pdf"
    } else {
        "doc.md"
    };
    let path = root.join(name);
    std::fs::write(&path, f["new"].as_str().unwrap()).unwrap();
    let mut cfg = crate::config::Config::default();
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.busy_indicator = false;
    cfg.ui.diff_view = if case["surface"] == "Source" {
        "source"
    } else {
        "rendered"
    }
    .into();
    cfg.ui.git_gutter = case["gutter"].as_bool().unwrap();
    cfg.git.diff = case["line"].as_str().unwrap().into();
    cfg.git.media_diff = case["layout"].as_str().unwrap().into();
    for key in ["R", "x", "e", "o", "g", "!", "F"] {
        cfg.keys
            .surfaces
            .entry("preview_git_diff".into())
            .or_default()
            .insert(key.into(), "noop".into());
    }
    let mut app = App::new(root.clone(), cfg).unwrap();
    let case_owned = case.clone();
    app.media_diff = MediaDiffWorker::with_compute(Arc::new(move |r| computed(&case_owned, r)));
    let old_pre = f["old"].as_str().unwrap().to_owned();
    let new_pre = f["new"].as_str().unwrap().to_owned();
    let (_, _, ops) = crate::preview::markdown::diff_align(&old_pre, &new_pre);
    app.md_diff =
        konoma_plugin::host::project::md_diff::MdDiffWorker::with_compute(Arc::new(move |_| {
            MdDiffOutcome::Ready {
                old_pre: old_pre.clone(),
                new_pre: new_pre.clone(),
                ops: ops.clone(),
                any_change: true,
                front_matter_only: false,
            }
        }));
    app.open_git_diff(&path);
    app.tab.came_from_git_view = true;
    app.git_status
        .insert(path.clone(), crate::git::FileStatus::Modified);
    app.git_status_for = Some(root);
    app.git_status_dirty = false;
    if case["surface"] == "Source" {
        app.diff_cache = Some(DiffCache {
            path: path.clone(),
            lines: lines(),
        });
    }
    app.picker = case["picker"]
        .as_bool()
        .unwrap()
        .then(ratatui_image::picker::Picker::halfblocks);
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_md_encoder(tx);
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    let (_, requests) = record_inline_paints_for_test(|| {
        draw(&mut terminal, &mut app);
        for key in &case["keys"].as_array().unwrap()[..count] {
            let code = match key.as_str().unwrap() {
                "PageDown" => crossterm::event::KeyCode::PageDown,
                "PageUp" => crossterm::event::KeyCode::PageUp,
                s => crossterm::event::KeyCode::Char(s.chars().next().unwrap()),
            };
            assert!(!crate::handle_key(
                &mut app,
                crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
            )
            .unwrap());
            draw(&mut terminal, &mut app);
        }
    });
    let mut paints = std::collections::BTreeMap::new();
    for (key, r) in requests {
        paints.insert(format!("{key}/{:?}",r.target),json!({"key":key,"target":[r.target.x,r.target.y,r.target.width,r.target.height],"cols":r.cols,"rows":r.rows,"offset":r.row_offset,"visible":r.visible_rows}));
    }
    let encodes: Vec<_> = rx.try_iter().map(encoded_request).collect();
    json!({"cells":cells(terminal.backend().buffer()),"paints":paints.into_values().collect::<Vec<_>>(),"encodes":encodes,"page":app.diff_media_page()})
}

#[test]
fn media_diff_screen_bridge_checks_host_frames() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/diff_media_screen.json");
    let mut f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let recording = std::env::var_os("KONOMA_RECORD_MEDIA_SCREEN").is_some();
    for index in 0..f["cases"].as_array().unwrap().len() {
        let case = &f["cases"][index];
        let stages: Vec<_> = (0..=case["keys"].as_array().unwrap().len())
            .map(|n| stage(&f, case, n))
            .collect();
        if recording {
            f["cases"][index]["stages"] = json!(stages);
        } else {
            assert_eq!(json!(stages), case["stages"], "{}", case["name"]);
        }
    }
    if recording {
        std::fs::write(path, serde_json::to_string_pretty(&f).unwrap() + "\n").unwrap();
    }
}
