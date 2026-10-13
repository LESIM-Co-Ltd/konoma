//! The `code` plugin on the host: what the plugin does (color the lines it is asked about) and
//! what stays the host's (the window, the caret, the search, the cache). Real files and the real
//! plugin, run through `enter_preview` with no worker attached (the plugin's work runs on the
//! spot), plus stand-in stylers to observe how the host asks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use konoma_plugin::view::{LineStyler, TextSource, TextView, View, ViewReq};
use konoma_plugin::{Line as PLine, Span as PSpan};

use super::plugin_host::{Picture, PluginPhase, PluginResult};
use super::*;
use crate::test_support::unique_tmp;

const RUST: &str = "fn main() {\n    let x = 42;\n}\n";

fn sandbox(name: &str, file: &str, body: &str) -> (crate::test_support::TmpDir, PathBuf) {
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, body).unwrap();
    (dir, path)
}

fn cfg_of(toml_text: &str) -> Config {
    toml::from_str(toml_text).expect("a valid config")
}

/// An app on `path`, opened the way a keystroke opens it. The grammar of `path` is warmed first
/// so the first frame is colored, not the loading state.
fn open(dir: &Path, path: &Path, cfg: Config) -> App {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        crate::preview::code::warm_file(ext, path);
    }
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.enter_preview(path);
    app
}

fn has_color(lines: &[Line<'static>]) -> bool {
    lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| matches!(s.style.fg, Some(ratatui::style::Color::Rgb(..))))
}

fn text_of(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// A styler that counts the lines it is given.
struct Counting {
    lines: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl LineStyler for Counting {
    fn style_block(&mut self, _first: usize, text: &str) -> Vec<PLine> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.lines
            .fetch_add(text.split('\n').count(), Ordering::SeqCst);
        text.split('\n')
            .map(|l| PLine::from(PSpan::raw(l.to_string())))
            .collect()
    }
}

fn counting_view(lines: &Arc<AtomicUsize>, calls: &Arc<AtomicUsize>) -> View {
    View::Text(TextView {
        source: TextSource::Lines(Vec::new()),
        styler: Some(Box::new(Counting {
            lines: Arc::clone(lines),
            calls: Arc::clone(calls),
        })),
        file_lines: true,
        syntax_name: None,
    })
}

// ---- the plugin shows the file ----------------------------------------------------------------

#[test]
fn a_code_file_is_shown_by_the_code_plugin_and_the_view_is_text() {
    let (dir, path) = sandbox("konoma_text_plugin_kind", "a.rs", RUST);
    let app = open(&dir, &path, Config::default());
    assert!(app.tab.preview_kind.as_ref().unwrap().is_code_plugin());
    assert!(app.is_windowed(), "the host reads the file in windows");
    let show = app.plugin.as_ref().expect("the plugin is open");
    assert_eq!(show.id, "code");
    assert!(matches!(show.phase, PluginPhase::Ready));
    match show.view.as_ref() {
        Some(View::Text(t)) => {
            assert!(t.styler.is_some());
            assert!(t.file_lines);
            assert!(matches!(&t.source, TextSource::File(p) if *p == path));
        }
        other => panic!("not a text view: {other:?}"),
    }
}

#[test]
fn the_window_is_what_the_core_highlighter_gives_for_the_same_lines() {
    let (dir, path) = sandbox("konoma_text_plugin_same", "a.rs", RUST);
    let mut app = open(&dir, &path, Config::default());
    let want = crate::preview::code::highlight(
        "fn main() {\n    let x = 42;\n}",
        &path,
        &app.cfg.ui.theme.code_theme,
    );
    let got = app.windowed_lines(10, 80);
    assert_eq!(got.len(), want.len());
    // The caret sits on the first line (the host paints it); the rest is the plugin's colors,
    // through the plugin's types and back, span for span.
    for (g, w) in got.iter().zip(&want).skip(1) {
        assert_eq!(g, w);
    }
    assert_eq!(text_of(&got[0]), text_of(&want[0]));
}

#[test]
fn the_styler_is_asked_for_the_visible_window_only() {
    let body: String = (0..50_000).map(|i| format!("let v{i} = {i};\n")).collect();
    let (dir, path) = sandbox("konoma_text_plugin_window", "big.rs", &body);
    let mut app = open(&dir, &path, Config::default());
    let (lines, calls) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    app.plugin.as_mut().unwrap().view = Some(counting_view(&lines, &calls));
    app.win_cache = None;
    let got = app.windowed_lines(30, 80);
    assert_eq!(got.len(), 30);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one block per window");
    assert_eq!(lines.load(Ordering::SeqCst), 30, "30 lines, not 50,000");
    // The same window again is the host's cache: the plugin is not asked.
    let _ = app.windowed_lines(30, 80);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Scrolling asks again, for the new window only.
    app.preview_scroll(1);
    let _ = app.windowed_lines(30, 80);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(lines.load(Ordering::SeqCst), 60);
}

#[test]
fn the_host_decorations_still_apply_on_top_of_the_plugin_lines() {
    let (dir, path) = sandbox("konoma_text_plugin_decor", "a.rs", "a\tb\nc\n");
    let mut cfg = Config::default();
    cfg.ui.line_numbers = true;
    let mut app = open(&dir, &path, cfg);
    let got = app.windowed_lines(10, 80);
    assert!(
        text_of(&got[0]).contains("1") && text_of(&got[0]).contains("a→"),
        "the line number and the expanded tab are the host's: {:?}",
        text_of(&got[0])
    );
}

// ---- switching it off -------------------------------------------------------------------------

#[test]
fn disabling_the_plugin_makes_a_code_file_plain_text_colored_by_the_text_rule() {
    let (dir, path) = sandbox("konoma_text_plugin_off", "a.rs", RUST);
    let mut app = open(&dir, &path, cfg_of("[plugins.code]\nenabled = false\n"));
    assert!(matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))));
    assert!(app.plugin.is_none(), "no plugin is opened for plain text");
    assert!(app.is_windowed());
    assert!(
        !app.is_highlight_pending(),
        "text never waits for a warm-up"
    );
    // A file whose name gives a grammar is still colored: the Text rule, by the same styler.
    assert!(has_color(&app.windowed_lines(10, 80)));
}

#[test]
fn a_disabled_plugin_leaves_unnamed_text_uncolored() {
    let (dir, path) = sandbox("konoma_text_plugin_plain", "notes.zzzplain", "fn x() {}\n");
    let mut app = open(&dir, &path, Config::default());
    assert!(matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))));
    assert!(!has_color(&app.windowed_lines(10, 80)));
}

#[test]
fn syntax_highlight_off_colors_nothing_through_either_name() {
    for toml_text in [
        "[plugins.code]\nsyntax_highlight = false\n",
        "[ui]\nsyntax_highlight = false\n",
    ] {
        let (dir, path) = sandbox("konoma_text_plugin_nohl", "a.rs", RUST);
        let mut app = open(&dir, &path, cfg_of(toml_text));
        assert!(app.tab.preview_kind.as_ref().unwrap().is_code_plugin());
        let show = app.plugin.as_ref().unwrap();
        assert!(
            matches!(&show.view, Some(View::Text(t)) if t.styler.is_none()),
            "{toml_text}: the plugin returns no styler"
        );
        assert!(!app.is_highlight_pending(), "{toml_text}");
        assert!(
            app.take_warm_job().is_none(),
            "{toml_text}: nothing to warm"
        );
        assert!(!has_color(&app.windowed_lines(10, 80)), "{toml_text}");
    }
}

#[test]
fn the_plugin_name_wins_over_the_old_ui_name() {
    let (dir, path) = sandbox("konoma_text_plugin_wins", "a.rs", RUST);
    let mut app = open(
        &dir,
        &path,
        cfg_of("[ui]\nsyntax_highlight = false\n[plugins.code]\nsyntax_highlight = true\n"),
    );
    assert!(has_color(&app.windowed_lines(10, 80)));
}

// ---- named files and the other text ---------------------------------------------------------

#[test]
fn files_without_an_extension_are_colored_from_their_name() {
    for (name, body) in [
        (".bashrc", "export A=1\nif [ -f x ]; then echo hi; fi\n"),
        ("Makefile", "all:\n\techo hi\n"),
    ] {
        let (dir, path) = sandbox("konoma_text_plugin_named", name, body);
        let mut app = open(&dir, &path, Config::default());
        assert!(
            matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))),
            "{name} is text, not a code-plugin file"
        );
        assert!(has_color(&app.windowed_lines(10, 80)), "{name}");
    }
}

#[test]
fn text_is_colored_by_the_very_same_styler_as_a_code_file() {
    // The same content under a name the plugin owns and a name only the Text rule knows.
    let (dir, rs) = sandbox("konoma_text_plugin_twin", "a.rs", RUST);
    let mut code_app = open(&dir, &rs, Config::default());
    let code = code_app.windowed_lines(10, 80);
    let mut off = open(&dir, &rs, cfg_of("[plugins.code]\nenabled = false\n"));
    let text = off.windowed_lines(10, 80);
    assert_eq!(code, text, "one styler behind both");
}

// ---- the loading state ------------------------------------------------------------------------

#[test]
fn until_the_plugin_answers_the_window_is_plain_and_not_cached() {
    let (dir, path) = sandbox("konoma_text_plugin_pending", "a.rs", RUST);
    let mut app = open(&dir, &path, Config::default());
    // Put the show back to "opening".
    {
        let show = app.plugin.as_mut().unwrap();
        show.phase = PluginPhase::Loading;
        show.view = None;
        show.session = None;
    }
    app.win_cache = None;
    assert!(!app.is_plugin_loading(), "no Loading... screen for text");
    let got = app.windowed_lines(10, 80);
    assert_eq!(text_of(&got[0]), "fn main() {", "the file is on screen");
    assert!(!has_color(&got), "plain until the plugin answers");
    assert!(app.win_cache.is_none(), "and not cached");
}

#[test]
fn a_result_landing_colors_the_window_and_leaves_the_text_search_alone() {
    let (dir, path) = sandbox("konoma_text_plugin_land", "a.rs", RUST);
    let mut app = open(&dir, &path, Config::default());
    app.tab.preview_search = Some("fn".into());
    app.tab.search_matches = vec![(0, 0, 0)];
    app.tab.search_idx = 0;
    app.tab.search_pending = true;
    let lines = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let gen = app.media_gen;
    app.media_loading = true;
    let landed = app.land_plugin_result(PluginResult {
        gen,
        id: "code".into(),
        session: None,
        layout: None,
        view: Some(counting_view(&lines, &calls)),
        outcome: None,
        failure: None,
        req: ViewReq::default(),
        picture: Picture::None,
        document: None,
        group: None,
        enabled: Vec::new(),
        made_text: None,
    });
    assert!(landed);
    assert!(!app.is_media_loading());
    assert_eq!(app.tab.search_matches, vec![(0, 0, 0)], "the match is kept");
    assert!(app.tab.search_pending, "the pending jump is not consumed");
    // The new styler is the one asked.
    app.win_cache = None;
    let _ = app.windowed_lines(10, 80);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_styler_that_is_ready_ends_the_loading_state() {
    struct Ready;
    impl LineStyler for Ready {
        fn style_block(&mut self, _: usize, text: &str) -> Vec<PLine> {
            text.split('\n').map(PLine::from).collect()
        }
    }
    let (dir, path) = sandbox("konoma_text_plugin_ready", "a.zzzcold", "x\n");
    // A rule makes the unknown extension a code file; its grammar is cold.
    let cfg = cfg_of("[[preview.rules]]\nglob = \"*.zzzcold\"\nbuiltin = \"code\"\n");
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.enter_preview(&path);
    assert!(app.tab.preview_kind.as_ref().unwrap().is_code_plugin());
    assert!(app.is_highlight_pending(), "cold at first");
    // A plugin that says it is ready (the default of `ready`) clears the state when it lands.
    app.plugin.as_mut().unwrap().view = Some(View::Text(TextView {
        source: TextSource::Lines(Vec::new()),
        styler: Some(Box::new(Ready)),
        file_lines: true,
        syntax_name: None,
    }));
    app.text_view_landed();
    assert!(!app.is_highlight_pending());
}

#[test]
fn a_cold_grammar_keeps_the_pending_state_the_old_code_had() {
    let (dir, path) = sandbox("konoma_text_plugin_cold", "a.zzzcold2", "x\n");
    let cfg = cfg_of("[[preview.rules]]\nglob = \"*.zzzcold2\"\nbuiltin = \"code\"\n");
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.enter_preview(&path);
    assert!(app.is_highlight_pending());
    // The real styler is not ready while the grammar is cold: landing does not end it.
    app.text_view_landed();
    assert!(app.is_highlight_pending());
    let job = app.take_warm_job().expect("the warm-up is handed out once");
    assert_eq!(job.0, "zzzcold2");
    assert!(app.take_warm_job().is_none());
    assert!(app.busy_jobs().contains(&crate::i18n::Msg::BusyHighlight));
    app.clear_highlight_pending();
    assert!(!app.is_highlight_pending());
}

// ---- reading again ----------------------------------------------------------------------------

#[test]
fn an_outside_edit_reopens_the_window_without_opening_the_plugin_again() {
    let (dir, path) = sandbox("konoma_text_plugin_reload", "a.rs", RUST);
    let mut app = open(&dir, &path, Config::default());
    let gen = app.media_gen;
    std::fs::write(&path, "// changed\nfn main() {}\n").unwrap();
    app.reload_preview();
    assert_eq!(app.media_gen, gen, "no new plugin job");
    let got = app.windowed_lines(10, 80);
    assert_eq!(text_of(&got[0]), "// changed");
    assert!(has_color(&got));
}

#[test]
fn leaving_the_preview_drops_the_show() {
    let (dir, path) = sandbox("konoma_text_plugin_leave", "a.rs", RUST);
    let mut app = open(&dir, &path, Config::default());
    assert!(app.plugin.is_some());
    app.back_to_tree();
    assert!(app.plugin.is_none());
}

#[test]
fn moving_to_another_code_file_opens_its_own_show() {
    let (dir, a) = sandbox("konoma_text_plugin_two", "a.rs", RUST);
    let b = dir.join("b.py");
    std::fs::write(&b, "def f():\n    pass\n").unwrap();
    crate::preview::code::warm_file("py", &b);
    let mut app = open(&dir, &a, Config::default());
    app.enter_preview(&b);
    let show = app.plugin.as_ref().unwrap();
    assert_eq!(show.path, b);
    let got = app.windowed_lines(10, 80);
    assert_eq!(text_of(&got[0]), "def f():");
    assert!(has_color(&got));
}

// ---- with a worker ----------------------------------------------------------------------------

#[test]
fn with_a_worker_the_file_is_on_screen_at_once_and_colored_once_the_plugin_has_answered() {
    let (dir, path) = sandbox("konoma_text_plugin_worker", "a.rs", RUST);
    crate::preview::code::warm_file("rs", &path);
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    app.enter_preview(&path);
    // Whether the answer made the first frame or not, the text is there and there is no
    // Loading... screen.
    assert!(!app.is_plugin_loading());
    let first = app.windowed_lines(10, 80);
    assert_eq!(text_of(&first[0]), "fn main() {");
    if matches!(
        app.plugin.as_ref().map(|p| &p.phase),
        Some(PluginPhase::Loading)
    ) {
        assert!(!has_color(&first), "plain until the plugin answers");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker always answers");
        assert!(app.apply_media(res), "the screen changes when it lands");
    }
    assert!(has_color(&app.windowed_lines(10, 80)));
    assert!(!app.is_media_loading());
}

#[test]
fn the_plugin_answering_is_not_shown_as_a_media_job() {
    let (dir, path) = sandbox("konoma_text_plugin_busy", "a.rs", RUST);
    let mut app = open(&dir, &path, Config::default());
    app.media_loading = true;
    assert!(
        !app.busy_jobs().contains(&crate::i18n::Msg::BusyMedia),
        "no media label for a text file"
    );
}

// ---- the text a plugin makes is written to its file on the worker, not on the UI thread -----------

/// An app on a hex dump (a plugin that makes its text) opened without a worker.
fn hexdump_app(name: &str) -> (App, crate::test_support::TmpDir) {
    use crate::plugins::test_plugins::{marker_config, register, Hexdump, Variant};
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.bin");
    std::fs::write(&file, (0u8..20).collect::<Vec<_>>()).unwrap();
    let marker = register(name, Arc::new(Hexdump(Variant::default())));
    let mut app = App::new(dir.to_path_buf(), marker_config(&marker)).unwrap();
    app.enter_preview(&file);
    (app, dir)
}

/// The temp file of a made text is written by the worker before the result is sent: the UI thread
/// only takes the path. Landing a result that carries no file writes none (and says so), whatever
/// the view holds; landing one that carries a file windows exactly that file.
#[test]
fn the_ui_thread_only_takes_the_path_of_the_text_the_worker_wrote() {
    use super::plugin_host::MadeText;
    let (mut app, _d) = hexdump_app("konoma_text_plugin_made_file");
    let own = app.tab.command_out.clone().expect("the windowed file");
    assert!(
        std::fs::read_to_string(&own)
            .unwrap()
            .starts_with("00 01 02 03"),
        "the file the worker wrote"
    );
    // A landing without a worker-written file does not write one itself.
    app.clear_command_out();
    assert!(!own.exists(), "the old file was removed");
    app.made_text_landed(None);
    assert!(app.tab.command_out.is_none(), "the UI thread wrote no file");
    assert!(app.flash.is_some(), "and it says so");
    // With a file, it is that file.
    let made = MadeText::write("hello\nworld").unwrap();
    let path = made.path_for_test().unwrap().to_path_buf();
    app.made_text_landed(Some(Ok(made)));
    assert_eq!(app.tab.command_out.as_deref(), Some(path.as_path()));
    app.clear_command_out();
    assert!(!path.exists(), "the tab removes the file it owns");
}

/// A result that is dropped unused (the preview moved on) takes its temp file with it.
#[test]
fn a_stale_result_leaves_no_temp_file_behind() {
    use super::plugin_host::MadeText;
    let (mut app, _d) = hexdump_app("konoma_text_plugin_stale_file");
    let made = MadeText::write("never shown").unwrap();
    let path = made.path_for_test().unwrap().to_path_buf();
    assert!(path.exists());
    let mut res = PluginResult::failed(
        app.media_gen.wrapping_sub(1),
        "hexdump".into(),
        ViewReq::default(),
        konoma_plugin::Failure::external(konoma_plugin::Tr::new("stale")),
    );
    res.made_text = Some(Ok(made));
    assert!(!app.install_plugin_result(res), "a stale result is dropped");
    // The drop happens off the UI thread: give it a moment.
    for _ in 0..300 {
        if !path.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!path.exists(), "its temp file is gone");
}
