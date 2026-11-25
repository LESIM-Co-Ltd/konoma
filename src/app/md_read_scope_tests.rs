//! The read scope of a document a plugin the user added returned: its `![](path)` pictures are read
//! only inside the plugin's scope (the tab root, the folder of the shown file, `read_dirs`).

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;
use ratatui::text::Line;

fn plain(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn png(p: &Path) {
    image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]))
        .save(p)
        .unwrap();
}

/// root/docs/a.md showing `![](<img>)`; root/docs/in.png inside, elsewhere/out.png outside the
/// root. The app is rooted at root/docs, the plugin shown is `id`.
struct Fx {
    app: App,
    out: PathBuf,
    _dir: crate::test_support::TmpDir,
}

fn fx(id: &str, cfg: Config, img: impl Fn(&Path, &Path) -> String) -> Fx {
    let dir = unique_tmp("konoma_md_read_scope");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let (docs, elsewhere) = (base.join("docs"), base.join("elsewhere"));
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let (inside, out) = (docs.join("in.png"), elsewhere.join("out.png"));
    png(&inside);
    png(&out);
    let md = docs.join("a.md");
    std::fs::write(&md, format!("![alt]({})\n", img(&inside, &out))).unwrap();
    let mut app = App::new(docs.clone(), cfg).unwrap();
    app.enter_preview(&md);
    app.tab.preview_viewport = 20;
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    // Stand in for a document a plugin of this id returned.
    app.plugin.as_mut().expect("plugin shown").id = id.into();
    app.md_cache = None;
    Fx {
        app,
        out,
        _dir: dir,
    }
}

fn outside(_inside: &Path, out: &Path) -> String {
    out.to_string_lossy().into_owned()
}

#[test]
fn a_picture_outside_the_scope_of_a_user_plugin_is_refused_with_the_reason_and_a_flash() {
    let mut f = fx("demo", Config::default(), outside);
    let text = plain(&f.app.decorated_lines(100));
    assert!(text.contains("read_dirs"), "{text}");
    assert!(f
        .app
        .flash
        .as_deref()
        .is_some_and(|m| m.contains("demo") && m.contains("read_dirs")));
    // Nothing is decoded or served for it.
    let url = f.out.to_string_lossy().into_owned();
    f.app.ensure_md_image(&url, 10, 5, 0, 5);
    assert!(!f.app.md_images.cache.contains_key(&f.out), "not decoded");
    assert!(f.app.md_image_proto(&url, 10, 5, 0, 5).is_none());
    // The reason is said once: the same page built again does not flash again.
    f.app.flash = None;
    f.app.md_cache = None;
    let _ = f.app.decorated_lines(100);
    assert!(f.app.flash.is_none(), "flashed once per plugin");
}

#[test]
fn ensure_md_image_alone_refuses_and_flashes() {
    // The draw-time gate, without the page build having run before it.
    let mut f = fx("demo", Config::default(), outside);
    let url = f.out.to_string_lossy().into_owned();
    f.app.ensure_md_image(&url, 10, 5, 0, 5);
    assert!(!f.app.md_images.cache.contains_key(&f.out));
    assert!(f.app.flash.as_deref().is_some_and(|m| m.contains("demo")));
}

#[test]
fn read_dirs_in_the_config_lets_the_picture_through() {
    let cfg_text = |dir: &Path| {
        format!(
            "[plugins.demo]\nread_dirs = [{:?}]\n",
            dir.to_str().unwrap()
        )
    };
    let dir = unique_tmp("konoma_md_read_scope_cfg");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    let (docs, el) = (base.join("docs"), base.join("elsewhere"));
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::create_dir_all(&el).unwrap();
    let out = el.join("out.png");
    png(&out);
    let md = docs.join("a.md");
    std::fs::write(&md, format!("![alt]({})\n", out.display())).unwrap();
    let cfg: Config = toml::from_str(&cfg_text(&el)).unwrap();
    let mut app = App::new(docs, cfg).unwrap();
    app.enter_preview(&md);
    app.tab.preview_viewport = 20;
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.plugin.as_mut().unwrap().id = "demo".into();
    app.md_cache = None;
    let text = plain(&app.decorated_lines(100));
    assert!(!text.contains("read_dirs"), "{text}");
    assert!(app.flash.is_none());
    assert!(app.md_image_refused(&out.to_string_lossy(), &out).is_none());
}

#[test]
fn a_picture_inside_the_scope_a_standard_plugin_and_a_remote_url_are_not_refused() {
    let f = fx("demo", Config::default(), |i, _| {
        i.to_string_lossy().into_owned()
    });
    let inside = f
        .out
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs/in.png");
    assert!(f
        .app
        .md_image_refused(&inside.to_string_lossy(), &inside)
        .is_none());
    // A remote URL is never a file read.
    assert!(f
        .app
        .md_image_refused("https://example.com/a.png", &f.out)
        .is_none());
    // The Markdown plugin (standard) is not limited, whatever the file.
    let mut g = fx("markdown", Config::default(), outside);
    let text = plain(&g.app.decorated_lines(100));
    assert!(!text.contains("read_dirs"), "{text}");
    assert!(g.app.flash.is_none());
    let url = g.out.to_string_lossy().into_owned();
    assert!(g.app.md_image_refused(&url, &g.out).is_none());
    // No plugin shown at all: unrestricted.
    g.app.plugin = None;
    assert!(g.app.md_image_refused(&url, &g.out).is_none());
}

#[test]
fn a_link_out_of_the_scope_is_judged_by_its_real_path() {
    let mut f = fx("demo", Config::default(), |i, _| {
        i.to_string_lossy().into_owned()
    });
    let docs = f.out.parent().unwrap().parent().unwrap().join("docs");
    let link = docs.join("link.png");
    std::os::unix::fs::symlink(&f.out, &link).unwrap();
    assert_eq!(
        f.app
            .md_image_refused(&link.to_string_lossy(), &link)
            .as_deref(),
        Some("demo")
    );
    f.app.flash = None;
}

#[test]
fn a_picture_already_encoded_is_not_drawn_once_the_scope_refuses_it() {
    // Land an encoded picture while the shown plugin is a standard one (no limit), then let the
    // same file be asked for by a user plugin whose scope does not hold it.
    let mut f = fx("markdown", Config::default(), outside);
    let url = f.out.to_string_lossy().into_owned();
    let (enc_tx, enc_rx) = std::sync::mpsc::channel();
    f.app.attach_md_encoder(enc_tx);
    let (jobs, job_rx) = std::sync::mpsc::channel();
    let (res_tx, results) = std::sync::mpsc::channel();
    let picker = ratatui_image::picker::Picker::halfblocks();
    std::thread::spawn(move || md_encode_worker(picker, job_rx, res_tx));
    f.app.md_images.cache.insert(
        f.out.clone(),
        MdImgEntry {
            decoded: Some(Arc::new(image::DynamicImage::ImageRgb8(
                image::RgbImage::from_pixel(80, 40, image::Rgb([200, 0, 0])),
            ))),
            layout_px: Some((80, 40)),
            ..Default::default()
        },
    );
    f.app.ensure_md_image(&url, 10, 5, 0, 5);
    let req = enc_rx.try_recv().expect("an encode is asked for");
    jobs.send(req).unwrap();
    let done = results
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the worker answers");
    assert!(f.app.apply_md_encode(done));
    assert!(
        f.app.md_image_proto(&url, 10, 5, 0, 5).is_some(),
        "drawable without a limit"
    );
    f.app.plugin.as_mut().unwrap().id = "demo".into();
    assert!(
        f.app.md_image_proto(&url, 10, 5, 0, 5).is_none(),
        "refused by the scope even though the cache holds it"
    );
}

/// The pixel in the middle of the picture decoded for `path` by the background decoder of a
/// document whose shown plugin is `id`, for an in-scope SVG that names the file `out` by an
/// absolute href.
fn svg_in_a_document_centre_alpha(id: &str) -> u8 {
    let mut f = fx(id, Config::default(), |inside, out| {
        let red = out.with_file_name("red.png");
        image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]))
            .save(&red)
            .unwrap();
        let svg = inside.with_file_name("evil.svg");
        std::fs::write(
            &svg,
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image xlink:href="{}" width="20" height="20"/></svg>"#,
                red.display()
            ),
        )
        .unwrap();
        svg.to_string_lossy().into_owned()
    });
    let (tx, rx) = std::sync::mpsc::channel();
    f.app.attach_md_image_loader(tx);
    let svg = f
        .out
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs/evil.svg");
    assert!(f.app.spawn_md_decode(svg));
    let res = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the decoder answers");
    let img = res.image.expect("the SVG is drawn").to_rgba8();
    img.get_pixel(img.width() / 2, img.height() / 2)[3]
}

#[test]
fn a_markdown_svg_of_a_user_plugin_cannot_read_a_file_outside_through_its_href() {
    // Scenario (c): `![](in-scope.svg)` where the SVG names a file outside the scope.
    assert_eq!(svg_in_a_document_centre_alpha("demo"), 0);
    // The same document of a standard plugin is not limited (the check is about the scope).
    assert_eq!(svg_in_a_document_centre_alpha("markdown"), 255);
}

#[test]
fn the_scope_is_built_once_and_not_at_every_redraw() {
    use crate::plugins::read_scope::SCOPE_BUILDS;
    let mut f = fx("demo", Config::default(), outside);
    let url = f.out.to_string_lossy().into_owned();
    let _ = f.app.decorated_lines(100);
    f.app.ensure_md_image(&url, 10, 5, 0, 5);
    let before = SCOPE_BUILDS.with(|c| c.get());
    for _ in 0..20 {
        f.app.ensure_md_image(&url, 10, 5, 0, 5);
        assert!(f.app.md_image_proto(&url, 10, 5, 0, 5).is_none());
        assert!(f.app.md_image_refused(&url, &f.out).is_some());
        f.app.md_cache = None;
        let _ = f.app.decorated_lines(100);
    }
    assert_eq!(
        SCOPE_BUILDS.with(|c| c.get()),
        before,
        "redraws reuse the scope"
    );
    // A different tab root is a different scope, built anew (once).
    f.app.tab.root = f.out.parent().unwrap().to_path_buf();
    assert!(f.app.md_image_refused(&url, &f.out).is_none());
    assert_eq!(SCOPE_BUILDS.with(|c| c.get()), before + 1);
}

#[test]
fn a_missing_picture_of_a_user_plugin_is_not_a_scope_refusal() {
    let mut f = fx("demo", Config::default(), |i, _| {
        i.with_file_name("gone.png").to_string_lossy().into_owned()
    });
    let text = plain(&f.app.decorated_lines(100));
    assert!(!text.contains("read_dirs"), "{text}");
    assert!(f.app.flash.is_none(), "{:?}", f.app.flash);
    let gone = f
        .out
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs/gone.png");
    assert!(f
        .app
        .md_image_refused(&gone.to_string_lossy(), &gone)
        .is_none());
    // A path under a file (cannot be entered) is not one either; one outside that exists is.
    let under = f.out.join("x.png");
    assert!(f
        .app
        .md_image_refused(&under.to_string_lossy(), &under)
        .is_none());
    assert!(f
        .app
        .md_image_refused(&f.out.to_string_lossy(), &f.out)
        .is_some());
}

#[test]
fn konoma_own_markdown_does_no_scope_work_and_one_resolve_per_picture_per_frame() {
    // konoma's own previews are unrestricted: drawing them must cost what it always did (one
    // resolve of the picture, no read scope built, no extra stat) however often it is redrawn.
    let dir = unique_tmp("konoma_md_read_scope_own");
    let base = dir.canonicalize().unwrap_or(dir.to_path_buf());
    std::fs::create_dir_all(&base).unwrap();
    png(&base.join("in.png"));
    let md = base.join("a.md");
    std::fs::write(&md, "![alt](in.png)\n").unwrap();
    let mut app = App::new(base.clone(), Config::default()).unwrap();
    app.enter_preview(&md);
    app.tab.preview_viewport = 20;
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    assert!(
        app.plugin.as_ref().is_some_and(|p| {
            crate::plugins::read_scope::is_host_owned(
                &crate::plugins::shared_registry(&app.cfg.plugin_disabled()),
                &p.id,
            )
        }),
        "the Markdown preview is konoma's own"
    );
    let builds = crate::plugins::read_scope::SCOPE_BUILDS.with(|c| c.get());
    let before = MD_IMAGE_RESOLVES.with(|c| c.get());
    for _ in 0..5 {
        app.ensure_md_image("in.png", 10, 5, 0, 5);
    }
    assert_eq!(
        MD_IMAGE_RESOLVES.with(|c| c.get()) - before,
        5,
        "one resolve (one stat) per call, none for a scope"
    );
    assert_eq!(
        crate::plugins::read_scope::SCOPE_BUILDS.with(|c| c.get()),
        builds,
        "no scope built"
    );
    assert!(app.read_scope_memo.borrow().is_none(), "nothing memoised");
}

#[test]
fn the_decode_of_a_user_plugins_picture_reads_the_real_path_that_was_judged() {
    let f = fx("demo", Config::default(), outside);
    let docs = f.app.tab.root.clone();
    let link = docs.join("link.png");
    std::os::unix::fs::symlink(docs.join("in.png"), &link).unwrap();
    let real = docs.join("in.png").canonicalize().unwrap();
    assert_ne!(link, real);
    assert_eq!(
        f.app.md_decode_read_path(&link),
        Some(real),
        "the judged file is the file read"
    );
    // An unresolvable path is handed on as it is (the missing-picture treatment shows it).
    let gone = docs.join("gone.png");
    assert_eq!(f.app.md_decode_read_path(&gone), Some(gone));
    // A file outside the scope has no path to read at all (never the spelling that was asked).
    assert_eq!(f.app.md_decode_read_path(&f.out), None);
    // A standard plugin's document is read as it is spelled.
    let std_app = fx("markdown", Config::default(), outside);
    assert_eq!(std_app.app.md_decode_read_path(&link), Some(link.clone()));
    assert_eq!(
        std_app.app.md_decode_read_path(&std_app.out),
        Some(std_app.out.clone())
    );
}
