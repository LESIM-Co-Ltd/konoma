//! The embedded kinds (a ```` ```mermaid ```` fence, a formula) and the standalone `.mmd` file go
//! through the plugin registry: switching the plugin off changes what is drawn, a plugin standing
//! in for the real one is what draws, and the old `[ui]` names and the new `[plugins.*]` names
//! both reach the plugin (the new one wins).

use std::sync::{Arc, Mutex};

use image::GenericImageView;
use konoma_plugin::view::{ImageData, ImageView, Layout, View, ViewReq};
use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session, ViewCx};

use super::embed_plugin::EMBED_STAND_IN;
use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

const FENCE_DOC: &str = "# title\n\n```mermaid\ngraph LR\n  A --> B\n```\n\ntail\n";
const MATH_DOC: &str = "# title\n\ntext $E=mc^2$ and\n\n$$\\frac{a}{b}$$\n\ntail\n";
const BOTH_DOC: &str =
    "# title\n\n```mermaid\ngraph LR\n  A --> B\n```\n\ntext $E=mc^2$ and\n\n$$\\frac{a}{b}$$\n";

fn picker() -> ratatui_image::picker::Picker {
    ratatui_image::picker::Picker::halfblocks()
}

fn off() -> toml::Table {
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    t
}

fn table(kv: &[(&str, toml::Value)]) -> toml::Table {
    let mut t = toml::Table::new();
    for (k, v) in kv {
        t.insert((*k).to_string(), v.clone());
    }
    t
}

/// An app in a fresh directory holding `name` with `body`, an image backend attached, the file
/// opened and its decoration built twice (the first pass finds the synchronous renders loading).
fn app_showing(
    tag: &str,
    name: &str,
    body: &str,
    cfg: Config,
) -> (App, crate::test_support::TmpDir, PathBuf) {
    let dir = unique_tmp(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(name);
    std::fs::write(&file, body).unwrap();
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.picker = Some(picker());
    app.enter_preview(&file);
    app.ensure_md_cache(80);
    app.ensure_md_cache(80);
    (app, dir, file)
}

fn cache_text(app: &App) -> String {
    app.md_cache
        .as_ref()
        .map(|c| {
            c.lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn has_fence(app: &App) -> bool {
    app.md_images()
        .iter()
        .any(|p| crate::preview::markdown::is_mermaid_fence_url(&p.url))
}

fn has_math(app: &App) -> bool {
    app.md_images()
        .iter()
        .any(|p| crate::preview::markdown::is_math_url(&p.url))
}

// ---- (1) the embeds go through the registry ------------------------------------------------------

#[test]
fn by_default_a_fence_and_a_formula_are_pictures() {
    let (app, _dir, _) = app_showing("konoma_embed_default", "d.md", BOTH_DOC, Config::default());
    assert!(app.mermaid_image_mode() && app.math_image_mode());
    assert!(has_fence(&app), "the fence is a picture");
    assert!(has_math(&app), "the formulas are pictures");
}

/// What the document looks like with `mode = "text"` (the reference for "cannot be drawn").
fn text_of_mode_text() -> String {
    let mut cfg = Config::default();
    cfg.plugins.insert(
        "mermaid".into(),
        table(&[("mode", toml::Value::String("text".into()))]),
    );
    let (app, _dir, _) = app_showing("konoma_embed_mode_text_ref", "d.md", FENCE_DOC, cfg);
    cache_text(&app)
}

#[test]
fn a_switched_off_mermaid_plugin_leaves_a_fence_as_the_text_diagram() {
    let mut cfg = Config::default();
    cfg.plugins.insert("mermaid".into(), off());
    let (app, _dir, _) = app_showing("konoma_embed_mermaid_off", "d.md", FENCE_DOC, cfg);
    assert!(!app.mermaid_image_mode());
    assert!(!has_fence(&app), "no picture is asked for");
    assert!(
        app.md_images
            .cache
            .keys()
            .all(|k| !crate::preview::markdown::is_mermaid_fence_url(&k.to_string_lossy())),
        "nothing is rendered for it either"
    );
    // The same look as a diagram that cannot be drawn (and as `mode = "text"`): the fence stays a
    // code block with its source.
    let text = cache_text(&app);
    assert!(
        text.contains("graph LR") && text.contains("A --> B"),
        "{text}"
    );
    assert_eq!(text, text_of_mode_text(), "same as mode = text");
    // Switching only the *mermaid* plugin off leaves the formulas alone.
    assert!(app.math_image_mode());
}

#[test]
fn a_switched_off_math_plugin_leaves_a_formula_as_its_source() {
    let mut cfg = Config::default();
    cfg.plugins.insert("math".into(), off());
    let (app, _dir, _) = app_showing("konoma_embed_math_off", "d.md", MATH_DOC, cfg);
    assert!(!app.math_image_mode());
    assert!(!has_math(&app));
    let text = cache_text(&app);
    assert!(text.contains("E=mc^2"), "the source stays as text: {text}");
    assert!(app.mermaid_image_mode());
}

#[test]
fn rendering_a_fence_or_formula_with_no_plugin_is_a_failed_entry_not_a_panic() {
    let mut cfg = Config::default();
    cfg.plugins.insert("mermaid".into(), off());
    cfg.plugins.insert("math".into(), off());
    let dir = unique_tmp("konoma_embed_direct_off");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.picker = Some(picker());
    assert!(app.ensure_mermaid_fence_render("graph LR\n A-->B".into()));
    assert!(app.ensure_math_render("x^2".into(), false));
    assert!(!app.md_images.cache.is_empty());
    assert!(app.md_images.cache.values().all(|e| e.failed));
}

#[test]
fn a_switched_off_mermaid_plugin_means_a_mmd_file_is_plain_text() {
    let mut cfg = Config::default();
    cfg.plugins.insert("mermaid".into(), off());
    let dir = unique_tmp("konoma_embed_mmd_off");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("d.mmd");
    std::fs::write(&f, "graph LR\n  A --> B\n").unwrap();
    assert!(matches!(cfg.resolve_preview(&f), PreviewKind::Text(_)));
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.picker = Some(picker());
    app.enter_preview(&f);
    assert!(app.image_src.is_none() && !app.is_image_preview());
}

// ---- the plugin that answers is the one that draws -----------------------------------------------

const RED_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="#ff0000"/></svg>"##;

/// What the stand-in was asked: (kind, source, display) per `open`.
type Asked = Arc<Mutex<Vec<(String, String, bool)>>>;

/// A stand-in for the plugin that takes the embedded kinds: records what it was asked and always
/// answers with a red rectangle.
struct Red {
    asked: Asked,
    trusted: bool,
    fail: bool,
}

struct RedSession {
    trusted: bool,
    fail: bool,
}

impl Plugin for Red {
    fn manifest(&self) -> Manifest {
        Manifest::new("red", "0")
    }
    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if let Input::Embed {
            kind,
            source,
            display,
        } = input
        {
            self.asked.lock().unwrap().push((kind, source, display));
        }
        Ok(Box::new(RedSession {
            trusted: self.trusted,
            fail: self.fail,
        }))
    }
}

impl Session for RedSession {
    fn layout(&self) -> Layout {
        Layout::single()
    }
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        if self.fail {
            return Err(Failure::unsupported(konoma_plugin::Tr::new("no")));
        }
        Ok(View::Image(ImageView::new(ImageData::Svg {
            data: Arc::from(RED_SVG.as_bytes()),
            base_dir: None,
            trusted: self.trusted,
        })))
    }
}

/// Puts a stand-in in place for this thread and takes it out again, panic or not.
struct StandIn;

impl StandIn {
    fn put(p: Red) -> StandIn {
        EMBED_STAND_IN.with(|s| *s.borrow_mut() = Some(Arc::new(p)));
        StandIn
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        EMBED_STAND_IN.with(|s| *s.borrow_mut() = None);
    }
}

fn red(trusted: bool, fail: bool) -> (StandIn, Asked) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let g = StandIn::put(Red {
        asked: asked.clone(),
        trusted,
        fail,
    });
    (g, asked)
}

fn entry_pixel(app: &App, url_pred: impl Fn(&str) -> bool) -> Option<[u8; 4]> {
    let (_, e) = app
        .md_images
        .cache
        .iter()
        .find(|(k, _)| url_pred(&k.to_string_lossy()))?;
    let img = e.decoded.as_ref()?;
    let (w, h) = img.dimensions();
    Some(img.get_pixel(w / 2, h / 2).0)
}

#[test]
fn the_plugin_the_registry_names_draws_the_fence_and_the_formulas() {
    let (_g, asked) = red(true, false);
    let (app, _dir, _) = app_showing("konoma_embed_standin", "d.md", BOTH_DOC, Config::default());
    let asked = asked.lock().unwrap().clone();
    // The fence body, the inline formula and the block formula were all handed over as embeds.
    assert!(
        asked
            .iter()
            .any(|(k, s, d)| k == "fence:mermaid" && s.contains("A --> B") && !*d),
        "{asked:?}"
    );
    assert!(
        asked
            .iter()
            .any(|(k, s, d)| k == "math" && s == "E=mc^2" && !*d),
        "{asked:?}"
    );
    assert!(
        asked
            .iter()
            .any(|(k, s, d)| k == "math" && s == "\\frac{a}{b}" && *d),
        "{asked:?}"
    );
    // And what is on screen is the plugin's picture, not konoma's own diagram.
    let px = entry_pixel(&app, crate::preview::markdown::is_mermaid_fence_url).expect("a raster");
    assert_eq!(&px[..3], &[255, 0, 0], "{px:?}");
    let px = entry_pixel(&app, crate::preview::markdown::is_math_url).expect("a raster");
    assert_eq!(&px[..3], &[255, 0, 0], "{px:?}");
}

#[test]
fn a_picture_the_plugin_does_not_vouch_for_is_refused() {
    // The host keeps the SVG and rasterizes it again in this process (sharp zoom, rebuilding an
    // evicted raster): an untrusted one must not take that road.
    let (_g, asked) = red(false, false);
    let (app, _dir, _) = app_showing(
        "konoma_embed_untrusted",
        "d.md",
        FENCE_DOC,
        Config::default(),
    );
    assert!(!asked.lock().unwrap().is_empty(), "the plugin was asked");
    assert!(entry_pixel(&app, crate::preview::markdown::is_mermaid_fence_url).is_none());
    assert!(
        app.md_images.cache.values().all(|e| e.failed),
        "the fence is shown the way an undrawable one is"
    );
}

#[test]
fn a_plugin_that_cannot_draw_the_fence_degrades_it_to_text() {
    let (_g, _asked) = red(true, true);
    let (app, _dir, _) = app_showing("konoma_embed_fail", "d.md", FENCE_DOC, Config::default());
    assert!(app.md_images.cache.values().all(|e| e.failed));
    // A fence that was asked for and could not be drawn shows the Unicode text diagram (a fence
    // nobody is asked about, with the plugin off or in text mode, stays a code block).
    let text = cache_text(&app);
    assert!(text.contains('┌') && text.contains('│'), "{text}");
}

#[test]
fn the_full_screen_fence_takes_its_picture_from_the_plugin_too() {
    let (_g, asked) = red(true, false);
    let (mut app, _dir, _) = app_showing(
        "konoma_embed_fullscreen",
        "d.md",
        FENCE_DOC,
        Config::default(),
    );
    asked.lock().unwrap().clear();
    let idx = app
        .md_items
        .iter()
        .position(|it| matches!(it.kind, MdItemKind::MermaidFence { .. }))
        .expect("the fence is a Tab item");
    app.tab.focused_item = Some(idx);
    app.md_activate_focused().unwrap();
    assert!(matches!(
        app.tab.preview_kind,
        Some(PreviewKind::MermaidFence(0))
    ));
    assert!(
        asked
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _, _)| k == "fence:mermaid"),
        "the plugin was asked for the full-screen picture"
    );
    let img = app.image_src.as_ref().expect("a raster");
    let (w, h) = img.dimensions();
    assert_eq!(&img.get_pixel(w / 2, h / 2).0[..3], &[255, 0, 0]);
    assert!(app.vector_svg.is_some(), "the SVG is kept for sharp zoom");
}

// ---- (2) old and new setting names ---------------------------------------------------------------

fn svg_of_fence(app: &App) -> String {
    let (_, e) = app
        .md_images
        .cache
        .iter()
        .find(|(k, _)| crate::preview::markdown::is_mermaid_fence_url(&k.to_string_lossy()))
        .expect("the fence rendered");
    String::from_utf8(e.svg.as_ref().expect("its SVG").to_vec()).unwrap()
}

fn svg_of_math(app: &App) -> String {
    let (_, e) = app
        .md_images
        .cache
        .iter()
        .find(|(k, _)| crate::preview::markdown::is_math_url(&k.to_string_lossy()))
        .expect("the formula rendered");
    String::from_utf8(e.svg.as_ref().expect("its SVG").to_vec()).unwrap()
}

#[test]
fn the_old_mermaid_name_and_the_new_one_both_reach_the_plugin_and_the_new_one_wins() {
    let theme_svg = |cfg: Config, tag: &str| {
        let (app, _d, _) = app_showing(tag, "d.md", FENCE_DOC, cfg);
        svg_of_fence(&app)
    };
    let dark = theme_svg(Config::default(), "konoma_embed_theme_default");
    let mut old = Config::default();
    old.ui.mermaid_theme = "light".into();
    let old_svg = theme_svg(old, "konoma_embed_theme_old");
    let mut new = Config::default();
    new.plugins.insert(
        "mermaid".into(),
        table(&[("theme", toml::Value::String("light".into()))]),
    );
    let new_svg = theme_svg(new, "konoma_embed_theme_new");
    let mut both = Config::default();
    both.ui.mermaid_theme = "forest".into();
    both.plugins.insert(
        "mermaid".into(),
        table(&[("theme", toml::Value::String("light".into()))]),
    );
    let both_svg = theme_svg(both, "konoma_embed_theme_both");
    assert_ne!(dark, old_svg, "the old name changes the picture");
    assert_eq!(old_svg, new_svg, "the new name gives the same picture");
    assert_eq!(new_svg, both_svg, "the new name wins over the old one");
}

#[test]
fn the_old_math_color_and_the_new_one_both_reach_the_plugin_and_the_new_one_wins() {
    let svg = |cfg: Config, tag: &str| {
        let (app, _d, _) = app_showing(tag, "d.md", MATH_DOC, cfg);
        svg_of_math(&app)
    };
    let mut old = Config::default();
    old.ui.math_color = "#00ff00".into();
    let old_svg = svg(old, "konoma_embed_color_old");
    assert!(
        old_svg.contains("#00ff00") || old_svg.contains("#0f0"),
        "{old_svg}"
    );
    let mut both = Config::default();
    both.ui.math_color = "#00ff00".into();
    both.plugins.insert(
        "math".into(),
        table(&[("color", toml::Value::String("#ff0000".into()))]),
    );
    let both_svg = svg(both, "konoma_embed_color_both");
    assert!(both_svg.contains("#ff0000"), "{both_svg}");
    assert!(!both_svg.contains("#00ff00"));
    // A color the rasterizer would paint black falls back to the default, from either name.
    let mut bad = Config::default();
    bad.plugins.insert(
        "math".into(),
        table(&[("color", toml::Value::String("wihte".into()))]),
    );
    assert!(svg(bad, "konoma_embed_color_bad").contains("#d0d0d0"));
}

#[test]
fn the_mode_and_the_rows_follow_the_new_name_then_the_old_one() {
    let mut c = Config::default();
    assert!(!crate::plugins::mermaid_mode_is_text(&c));
    assert!(!crate::plugins::math_mode_is_text(&c));
    c.ui.mermaid = "text".into();
    c.ui.math = "text".into();
    assert!(crate::plugins::mermaid_mode_is_text(&c));
    assert!(crate::plugins::math_mode_is_text(&c));
    // The new name wins in both directions.
    c.plugins.insert(
        "mermaid".into(),
        table(&[("mode", toml::Value::String("image".into()))]),
    );
    c.plugins.insert(
        "math".into(),
        table(&[("mode", toml::Value::String("image".into()))]),
    );
    assert!(!crate::plugins::mermaid_mode_is_text(&c));
    assert!(!crate::plugins::math_mode_is_text(&c));
    let mut c = Config::default();
    c.plugins.insert(
        "mermaid".into(),
        table(&[("mode", toml::Value::String("text".into()))]),
    );
    assert!(crate::plugins::mermaid_mode_is_text(&c));

    // Rows: the old name, the new one winning, and the invalid values going to the default 24.
    let rows = |old: u16, new: Option<i64>| {
        let mut c = Config::default();
        c.ui.mermaid_rows = old;
        if let Some(n) = new {
            c.plugins.insert(
                "mermaid".into(),
                table(&[("rows", toml::Value::Integer(n))]),
            );
        }
        crate::plugins::mermaid_rows(&c)
    };
    assert_eq!(rows(10, None), 10);
    assert_eq!(rows(10, Some(7)), 7);
    assert_eq!(rows(10, Some(0)), 24);
    assert_eq!(rows(10, Some(-3)), 24);
    assert_eq!(rows(10, Some(70_000)), 24);
    assert_eq!(rows(0, None), 24);
    // ...and the effective settings the plugin is handed say the same.
    let mut c = Config::default();
    c.ui.mermaid_rows = 10;
    let s = crate::plugins::effective_settings(&c, "mermaid");
    assert_eq!(s.int("rows", 0), 10);
    assert_eq!(s.str("theme", ""), "dark");
    assert_eq!(s.str("mode", ""), "image");
}

#[test]
fn the_mermaid_rows_of_the_new_name_size_the_inline_diagram() {
    // The host's document layout reads the plugin's rows setting: a small cap shrinks the diagram.
    let rows_of = |cfg: Config, tag: &str| {
        let (app, _d, _) = app_showing(tag, "d.md", FENCE_DOC, cfg);
        app.md_images()
            .iter()
            .find(|p| crate::preview::markdown::is_mermaid_fence_url(&p.url))
            .map(|p| p.rows)
            .expect("a placement")
    };
    let mut small = Config::default();
    small.plugins.insert(
        "mermaid".into(),
        table(&[("rows", toml::Value::Integer(4))]),
    );
    let mut old_small = Config::default();
    old_small.ui.mermaid_rows = 4;
    let a = rows_of(small, "konoma_embed_rows_new");
    let b = rows_of(old_small, "konoma_embed_rows_old");
    assert!(a <= 4, "{a}");
    assert_eq!(a, b, "the new name sizes it exactly as the old one did");
}

// ---- (3) the standalone .mmd file --------------------------------------------------------------

const FLOW: &str = "graph LR\n  A[start] --> B[end]\n";

#[test]
fn a_mmd_file_in_image_mode_is_the_plugins_trusted_picture_with_sharp_zoom() {
    let (app, _d, _) = app_showing("konoma_embed_mmd_image", "d.mmd", FLOW, Config::default());
    assert!(app.image_src.is_some(), "rasterized");
    assert!(app.vector_svg.is_some(), "the SVG is kept for sharp zoom");
    assert!(app.is_image_preview());
    assert!(app.tab.preview_kind.as_ref().unwrap().is_mermaid_file());
    // It is the SVG konoma's renderer draws with the settings (here the defaults).
    let want = konoma_mermaid_core::mermaid_to_svg_flow(FLOW, "dark", "basis", "splines").unwrap();
    assert_eq!(app.vector_svg.as_ref().unwrap().as_slice(), want.as_bytes());
}

#[test]
fn a_mmd_file_in_text_mode_is_the_decorated_text_diagram_by_either_setting_name() {
    let mut old = Config::default();
    old.ui.mermaid = "text".into();
    let mut new = Config::default();
    new.plugins.insert(
        "mermaid".into(),
        table(&[("mode", toml::Value::String("text".into()))]),
    );
    let reference = crate::preview::markdown::render_mermaid_file(FLOW.trim_end(), 80);
    for (cfg, tag) in [
        (old, "konoma_embed_mmd_text_old"),
        (new, "konoma_embed_mmd_text_new"),
    ] {
        let (app, _d, _) = app_showing(tag, "d.mmd", FLOW, cfg);
        assert!(app.image_src.is_none() && !app.is_image_preview(), "{tag}");
        assert!(app.is_decorated_kind(), "R can switch to the raw source");
        assert_eq!(app.md_cache.as_ref().unwrap().lines, reference, "{tag}");
    }
}

#[test]
fn a_mmd_file_that_cannot_be_drawn_falls_to_the_same_text_as_before() {
    let src = "this is definitely not mermaid syntax\n";
    let (app, _d, _) = app_showing("konoma_embed_mmd_bad", "d.mmd", src, Config::default());
    assert!(
        app.image_src.is_none() && !app.is_image_preview(),
        "no picture, so the decorated path draws it"
    );
    let reference = crate::preview::markdown::render_mermaid_file(src.trim_end(), 80);
    assert_eq!(app.md_cache.as_ref().unwrap().lines, reference);
    assert!(!app.media_loading, "nothing is left loading");
}

#[test]
fn the_plugins_text_diagram_equals_the_markdown_renderers_for_every_kind_of_source() {
    let dir = unique_tmp("konoma_embed_text_parity");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let sources = [
        FLOW,
        "sequenceDiagram\n  A->>B: hi\n  B-->>A: ok\n",
        "sequenceDiagram\n  participant 利用者\n  利用者->>システム: 開く\n",
        "pie title P\n  \"a\" : 3\n  \"b\" : 4\n",
        "graph LR\n  A[a long first box] --> B[a long second box] --> C[a long third box]\n",
        "this is definitely not mermaid syntax",
        "",
        "graph LR\n  A --> B\n\n\n",
    ];
    for src in sources {
        for width in [0u16, 20, 30, 80, 200] {
            assert_eq!(
                app.mermaid_text_lines(src, width),
                crate::preview::markdown::render_mermaid_file(src, width),
                "width {width}, source {src:?}"
            );
        }
    }
}

#[test]
fn the_text_diagram_follows_the_display_width_after_a_resize() {
    let src = "graph LR\n  A[a long first box] --> B[a long second box] --> C[a long third box]\n";
    let (mut app, _d, _) = app_showing("konoma_embed_mmd_resize", "d.mmd", src, Config::default());
    // Force text mode for the rest.
    app.picker = None;
    app.md_cache = None;
    app.ensure_md_cache(100);
    let wide = app.md_cache.as_ref().unwrap().lines.clone();
    app.ensure_md_cache(30);
    let narrow = app.md_cache.as_ref().unwrap().lines.clone();
    assert_ne!(wide, narrow, "a new width asks the plugin again");
    assert_eq!(
        narrow,
        crate::preview::markdown::render_mermaid_file(src.trim_end(), 30)
    );
}

#[test]
fn a_mmd_file_with_no_image_backend_is_text_without_a_plugin_picture_job() {
    let dir = unique_tmp("konoma_embed_mmd_nopicker");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("d.mmd");
    std::fs::write(&f, FLOW).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    assert!(app.picker.is_none());
    app.enter_preview(&f);
    app.ensure_md_cache(80);
    assert!(app.image_src.is_none());
    assert!(!app.media_loading);
    assert_eq!(
        app.md_cache.as_ref().unwrap().lines,
        crate::preview::markdown::render_mermaid_file(FLOW.trim_end(), 80)
    );
}

#[test]
fn the_plugin_is_asked_once_per_fence_not_again_when_the_decoration_is_rebuilt() {
    let (_g, asked) = red(true, false);
    let (mut app, _d, _) = app_showing("konoma_embed_once", "d.md", FENCE_DOC, Config::default());
    let n = asked.lock().unwrap().len();
    assert_eq!(n, 1, "{:?}", asked.lock().unwrap());
    app.md_cache = None;
    app.ensure_md_cache(80);
    app.ensure_md_cache(100);
    assert_eq!(asked.lock().unwrap().len(), n);
}

// ---- who vouches for the SVG an embedded kind keeps -----------------------------------------------

/// The standard plugins draw konoma's own SVG (in this process, drawn again so); a plugin the user
/// added is never believed, so its entry is marked and is drawn again by the child process only.
#[test]
fn the_entries_of_a_plugin_the_user_added_are_marked_untrusted_and_the_standard_ones_are_not() {
    let (app, _dir, _) = app_showing(
        "konoma_embed_trust_std",
        "d.md",
        FENCE_DOC,
        Config::default(),
    );
    let kept: Vec<_> = app
        .md_images
        .cache
        .values()
        .filter(|e| e.svg.is_some())
        .collect();
    assert!(!kept.is_empty(), "the standard plugin drew the fence");
    assert!(
        kept.iter().all(|e| !e.svg_trust.is_untrusted()),
        "konoma's own diagram"
    );

    let marker = crate::plugins::test_plugins::register(
        "embed_trust_user",
        std::sync::Arc::new(crate::plugins::test_views::EmbedSvg),
    );
    let (app, _dir, _) = app_showing(
        "konoma_embed_trust_user",
        "d.md",
        FENCE_DOC,
        crate::plugins::test_plugins::marker_config(&marker),
    );
    let kept: Vec<_> = app
        .md_images
        .cache
        .values()
        .filter(|e| e.svg.is_some())
        .collect();
    assert!(!kept.is_empty(), "the user's plugin drew the fence");
    assert!(
        kept.iter().all(|e| e.svg_trust.is_untrusted()),
        "never believed"
    );
}
