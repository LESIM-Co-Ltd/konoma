//! Independent full-screen literals captured through konoma's real input and render paths.
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use konoma_widgets::picture_overlay::{record_inline_paints_for_test, PicturePaintRequest};
use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};
use serde_json::{json, Value};

fn cells(buf: &Buffer) -> Value {
    assert_eq!(buf.area, Rect::new(0, 0, 54, 14));
    let mut runs: Vec<Value> = Vec::new();
    let mut nonempty = 0;
    let mut count = 0;
    for c in &buf.content {
        let value = json!([
            c.symbol(),
            format!("{:?}", c.fg),
            format!("{:?}", c.bg),
            c.modifier.bits()
        ]);
        if let Some(last) = runs.last_mut().filter(|r| r[1] == value) {
            last[0] = json!(last[0].as_u64().unwrap() + 1);
        } else {
            runs.push(json!([1, value]));
        }
        nonempty += usize::from(!c.symbol().trim().is_empty());
        count += 1;
    }
    assert_eq!(count, 756);
    assert!(nonempty > 0);
    json!({"rect":[0,0,54,14],"count":count,"runs":runs})
}

fn paints(requests: Vec<(String, PicturePaintRequest)>) -> Value {
    let mut latest = std::collections::BTreeMap::new();
    for (key, r) in requests {
        latest.insert(format!("{key}/{:?}",r.kind), json!({"key":key,"kind":format!("{:?}",r.kind),"target":[r.target.x,r.target.y,r.target.width,r.target.height],"cols":r.cols,"rows":r.rows,"offset":r.row_offset,"visible":r.visible_rows}));
    }
    json!(latest.into_values().collect::<Vec<_>>())
}

fn seed_pictures(app: &mut App, f: &Value) {
    use konoma_widgets::inline_images::{InlineImage, MdEncodeKey, MdImgEntry, MdProtoSlot};
    for src in [f["old"].as_str().unwrap(), f["new"].as_str().unwrap()] {
        for code in crate::preview::markdown::collect_mermaid_fences(src) {
            let key = crate::preview::markdown::mermaid_fence_url(&code);
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
            app.md_images.cache.insert(key.into(), entry);
        }
    }
}

fn draw(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    terminal
        .draw(|frame| crate::ui::render(frame, app))
        .unwrap();
    // The real loop clears moved kitty overlays before its second draw (src/lib.rs).
    if app.take_md_overlay_moved() {
        terminal.clear().unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, app))
            .unwrap();
    }
}

fn stage(f: &Value, case: &Value, count: usize) -> Value {
    let dir = konoma_test_support::unique_tmp("rendered_screen_host");
    let root = dir.join("reference");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let path = root.join("doc.md");
    std::fs::write(&path, f["new"].as_str().unwrap()).unwrap();
    let mut cfg = crate::config::Config::default();
    cfg.ui.lang = "en".into();
    cfg.ui.tabbar = "hidden".into();
    cfg.ui.diff_view = "rendered".into();
    cfg.ui.wrap = case["wrap"].as_bool().unwrap();
    cfg.git.diff = "unified".into();
    for key in ["R", "x", "e", "o", "g", "!", "F"] {
        cfg.keys
            .surfaces
            .entry("preview_git_diff".into())
            .or_default()
            .insert(key.into(), "noop".into());
    }
    let old_pre = f["old"].as_str().unwrap().to_owned();
    let new_pre = f["new"].as_str().unwrap().to_owned();
    let (_, _, ops) = crate::preview::markdown::diff_align(&old_pre, &new_pre);
    let outcome = MdDiffOutcome::Ready {
        old_pre,
        new_pre,
        ops,
        any_change: true,
        front_matter_only: false,
    };
    let mut app = App::new(root.clone(), cfg).unwrap();
    app.md_diff =
        konoma_plugin::host::project::md_diff::MdDiffWorker::with_compute(Arc::new(move |_| {
            outcome.clone()
        }));
    app.open_git_diff(&path);
    app.git_status
        .insert(path.clone(), crate::git::FileStatus::Modified);
    app.git_status_for = Some(root);
    app.git_status_dirty = false;
    if case["pictures"].as_bool().unwrap() {
        app.picker = Some(ratatui_image::picker::Picker::halfblocks());
        seed_pictures(&mut app, f);
    }
    let mut terminal = Terminal::new(TestBackend::new(54, 14)).unwrap();
    let (_, requests) = record_inline_paints_for_test(|| {
        draw(&mut terminal, &mut app);
        for key in &f["keys"].as_array().unwrap()[..count] {
            let code = match key.as_str().unwrap() {
                "PageDown" => KeyCode::PageDown,
                "Home" => KeyCode::Home,
                "End" => KeyCode::End,
                s => KeyCode::Char(s.chars().next().unwrap()),
            };
            assert!(!crate::handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap());
            draw(&mut terminal, &mut app);
        }
    });
    json!({"cells":cells(terminal.backend().buffer()),"paints":paints(requests),"scroll":app.tab.preview_scroll})
}

#[test]
fn rendered_diff_screen_host_bridge_checks_independent_literals() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots/diff_rendered_screen.json");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(f["page_count"], 1);
    for case in f["cases"].as_array().unwrap() {
        let stages: Vec<_> = (0..=f["keys"].as_array().unwrap().len())
            .map(|count| stage(&f, case, count))
            .collect();
        assert_eq!(
            json!(stages),
            case["stages"],
            "wrap={} pictures={}",
            case["wrap"],
            case["pictures"]
        );
    }
}
