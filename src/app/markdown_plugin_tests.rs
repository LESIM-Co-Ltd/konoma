//! The `markdown` plugin on the host's plugin road: a Markdown file and a converted Word document
//! are both shown by the host's document widget from the plugin's `DocumentView`, drawn with the
//! plugin's options, and `R` is a request for the plugin's other representation. The plugin's own
//! behavior is tested in `crates/konoma-markdown`; the keys over real files in
//! `e2e_tests/{word,deck_view}.rs` and the many Markdown tests that never changed.

use std::sync::atomic::{AtomicUsize, Ordering};

use konoma_plugin::view::{DocOptions, DocumentView, Layout};
use konoma_plugin::{
    Bind, Failure, FailureKind, Input, Manifest, OpenCx, Origin, Plugin, Resources, Session,
    ViewCx, ViewReq,
};

use super::markdown_plugin::CONTENT_STAND_IN;
use super::*;
use crate::preview::office::docx::Document;
use crate::test_support::unique_tmp;

fn dir_with(name: &str, files: &[(&str, &[u8])]) -> (crate::test_support::TmpDir, PathBuf) {
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    for (n, bytes) in files {
        std::fs::write(dir.join(n), bytes).unwrap();
    }
    let root = dir.canonicalize().unwrap();
    (dir, root)
}

fn cfg_from(toml_text: &str) -> Config {
    toml::from_str::<Config>(toml_text).unwrap()
}

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

fn long_md(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n\n")).collect()
}

/// An `App` showing the Markdown file `name` (through a tree `Enter`).
fn app_on(cfg: Config, name: &str, text: &str) -> (App, PathBuf, crate::test_support::TmpDir) {
    let (dir, root) = dir_with("konoma_md_plugin", &[(name, text.as_bytes())]);
    let mut app = App::new(root.clone(), cfg).unwrap();
    let path = root.join(name);
    app.enter_preview(&path);
    app.tab.preview_viewport = 20;
    (app, path, dir)
}

// ---- the road a Markdown file takes ----------------------------------------------------------

#[test]
fn a_markdown_file_is_a_document_the_markdown_plugin_returned() {
    let (mut app, path, _dir) = app_on(Config::default(), "a.md", "# Title\n\nbody\n");
    assert!(app.tab.preview_kind.as_ref().unwrap().is_markdown());
    assert_eq!(app.tab.preview_path.as_deref(), Some(path.as_path()));
    assert!(app.holds_document() && app.document_ready());
    assert!(
        !app.is_document(),
        "a Markdown file is not a converted document"
    );
    assert_eq!(app.document_markdown_for_test(), Some("# Title\n\nbody"));
    let show = app.plugin.as_ref().unwrap();
    assert_eq!(show.id, "markdown");
    let reprs: Vec<&str> = show.layout.reprs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(reprs, ["rendered", "source"]);
    // Drawn from the plugin's text (the file is not read by the widget).
    let text = plain(&app.decorated_lines(60));
    assert!(text.contains("Title") && text.contains("body"), "{text}");
}

#[test]
fn the_widget_draws_the_text_the_plugin_handed_over_not_the_file_on_disk() {
    let (mut app, path, _dir) = app_on(Config::default(), "a.md", "first\n");
    std::fs::write(&path, "second\n").unwrap();
    app.md_cache = None;
    let text = plain(&app.decorated_lines(60));
    assert!(text.contains("first") && !text.contains("second"), "{text}");
    // A reload asks the plugin again and the new text lands.
    app.reload_preview();
    let text = plain(&app.decorated_lines(60));
    assert!(text.contains("second") && !text.contains("first"), "{text}");
}

#[test]
fn a_file_cut_at_the_display_cap_carries_the_notice_the_loader_always_added() {
    // Past the line cap, and past the byte cap: the plugin's text plus the notice is exactly what
    // the host's own loader (`preview::text::load`) has always produced.
    let many_lines: String = (0..5100).map(|i| format!("l{i}\n")).collect();
    let many_bytes = format!("{}\n", "x".repeat(99)).repeat(12_000);
    let exact = "y".repeat(1024 * 1024);
    let crlf = "a\r\nb\r\n\r\nc\r\n".to_string();
    let empty = String::new();
    for (label, text) in [
        ("lines", many_lines),
        ("bytes", many_bytes),
        ("exact", exact),
        ("crlf", crlf),
        ("empty", empty),
    ] {
        let (mut app, path, _dir) = app_on(Config::default(), "c.md", &text);
        let want =
            super::md_render::decorated_file_text(&crate::preview::text::load(&path).unwrap());
        assert_eq!(app.document_source_text(), want, "{label}");
        let _ = app.md_layout(60);
    }
}

#[test]
fn the_plugins_limits_are_the_loaders_limits() {
    // Two definitions of the same numbers (the plugin cannot depend on the host): pinned together.
    assert_eq!(konoma_markdown::MAX_BYTES, crate::preview::text::MAX_BYTES);
    let lines: String = (0..konoma_markdown::MAX_LINES + 1)
        .map(|i| format!("{i}\n"))
        .collect();
    let (dir, root) = dir_with("konoma_md_limits", &[("a.md", lines.as_bytes())]);
    let loaded = crate::preview::text::load(&root.join("a.md")).unwrap();
    let (text, cut) = konoma_markdown::read_limited(&root.join("a.md")).unwrap();
    assert_eq!(text, loaded.lines.join("\n"));
    assert_eq!(cut, loaded.truncated);
    assert_eq!(loaded.lines.len(), konoma_markdown::MAX_LINES);
    drop(dir);
}

#[test]
fn a_file_that_cannot_be_read_shows_the_reason_not_an_empty_document() {
    let (mut app, path, _dir) = app_on(Config::default(), "a.md", "x\n");
    std::fs::remove_file(&path).unwrap();
    app.reload_preview();
    assert!(app.document_text_missing() && app.document.is_none());
    let failure = app.document_error().expect("the plugin's reason");
    assert_eq!(failure.kind, FailureKind::Io);
    assert!(failure.message.en().contains("could not read"), "{failure}");
}

// ---- the options the document is drawn with --------------------------------------------------

fn alerts_md() -> &'static str {
    "> [!NOTE]\n> a note\n"
}

fn drawn_with(cfg: Config) -> String {
    let (mut app, _p, _d) = app_on(cfg, "a.md", alerts_md());
    plain(&app.decorated_lines(60))
}

#[test]
fn alerts_follow_the_new_name_or_the_old_one_and_the_new_one_wins() {
    // Default: drawn as a callout, the marker is gone.
    assert!(!drawn_with(Config::default()).contains("[!NOTE]"));
    // The old name alone.
    assert!(drawn_with(cfg_from("[ui]\nmd_alerts = false\n")).contains("[!NOTE]"));
    // The new name alone.
    assert!(drawn_with(cfg_from("[plugins.markdown]\nalerts = false\n")).contains("[!NOTE]"));
    // Both: the plugin's table wins, either way round.
    assert!(!drawn_with(cfg_from(
        "[ui]\nmd_alerts = false\n[plugins.markdown]\nalerts = true\n"
    ))
    .contains("[!NOTE]"));
    assert!(drawn_with(cfg_from(
        "[ui]\nmd_alerts = true\n[plugins.markdown]\nalerts = false\n"
    ))
    .contains("[!NOTE]"));
}

#[test]
fn the_documents_options_are_the_effective_settings() {
    let cfg = cfg_from(
        "[ui]\nmd_details = \"closed\"\nmd_table_align = \"right\"\n\
         md_task_states = [\" \", \"/\", \"x\"]\n\
         [plugins.markdown]\ndetails = \"open\"\n",
    );
    let (app, _p, _d) = app_on(cfg, "a.md", "x\n");
    let o = &app.document.as_ref().unwrap().options;
    assert_eq!(o.details, "open", "the new name wins");
    assert_eq!(o.table_align, "right", "the old name is read");
    assert_eq!(o.task_states, [' ', '/', 'x']);
    assert!(o.tasks_writable && o.front_matter);
}

#[test]
fn the_settings_mapping_agrees_with_the_ui_getters_it_replaces() {
    use crate::preview::markdown::BlockAlign;
    let cases = [
        "[ui]\n",
        "[ui]\nmd_task_states = [\" \", \"/\", \"x\"]\n",
        "[ui]\nmd_task_states = [\" \", \"xx\"]\n",
        "[ui]\nmd_task_states = [\"x\"]\n",
        "[ui]\nmd_task_states = []\n",
        "[ui]\nmd_task_states = [\"○\", \"●\"]\n",
        "[ui]\nmd_table_align = \"center\"\nmd_image_align = \"right\"\n",
        "[ui]\nmd_table_align = \"nonsense\"\nmd_image_align = \"\"\n",
    ];
    for toml_text in cases {
        let cfg = cfg_from(toml_text);
        let opts = DocOptions::from_settings(&crate::plugins::effective_settings(
            &cfg,
            konoma_markdown::MARKDOWN_ID,
        ));
        assert_eq!(
            opts.task_states,
            cfg.ui.md_task_state_chars(),
            "{toml_text}"
        );
        assert_eq!(
            super::markdown_plugin::block_aligns(&opts),
            cfg.ui.md_block_aligns(),
            "{toml_text}"
        );
    }
    // And the defaults are the old defaults.
    let d = DocOptions::default();
    let ui = crate::config::UiConfig::default();
    assert_eq!(d.task_states, ui.md_task_state_chars());
    assert_eq!(d.table_align, ui.md_table_align);
    assert_eq!(d.image_align, ui.md_image_align);
    assert_eq!(d.details, ui.md_details);
    assert_eq!(
        super::markdown_plugin::block_aligns(&d).table,
        BlockAlign::Left
    );
}

#[test]
fn a_markdown_plugin_switched_off_leaves_plain_text() {
    let cfg = cfg_from("[plugins.markdown]\nenabled = false\n");
    let (dir, root) = dir_with("konoma_md_off", &[("a.md", b"# not a heading\n")]);
    let mut app = App::new(root.clone(), cfg).unwrap();
    app.enter_preview(&root.join("a.md"));
    assert!(matches!(app.tab.preview_kind, Some(PreviewKind::Text(_))));
    assert!(app.is_windowed(), "plain text is read in windows");
    assert!(!app.holds_document() && app.document.is_none());
    drop(dir);
}

// ---- R: the other representation, asked of the plugin ----------------------------------------

#[test]
fn r_shows_the_file_itself_and_back_and_each_switch_starts_at_the_top() {
    let (mut app, path, _dir) = app_on(Config::default(), "a.md", &long_md(80));
    let _ = app.md_layout(60);
    app.tab.preview_scroll = 7;
    app.tab.focused_item = None;
    assert!(!app.is_md_raw() && !app.is_windowed());

    app.toggle_md_raw();
    assert_eq!(app.tab.repr.as_deref(), Some("source"));
    assert!(app.is_md_raw() && app.is_raw_source() && app.is_windowed());
    assert_eq!(app.windowed_src(), Some(path.as_path()), "the file itself");
    assert_eq!(app.tab.command_out, None, "no temp file for a file");
    assert_eq!(app.tab.preview_scroll, 0);
    assert_eq!(app.tab.preview_top_line, 0);
    assert_eq!(app.tab.preview_cursor_line, 0);
    assert!(
        app.document_ready(),
        "the document is kept for the way back"
    );

    // Move in the source, then back to the rendered document.
    app.tab.preview_top_line = 9;
    app.tab.preview_cursor_line = 9;
    app.toggle_md_raw();
    assert_eq!(app.tab.repr, None);
    assert!(!app.is_md_raw() && !app.is_windowed());
    assert_eq!(app.tab.preview_scroll, 0, "back at the top, as before");
    assert_eq!(app.tab.preview_top_line, 0);
    assert_eq!(app.tab.preview_cursor_line, 0);
    assert!(app.document_ready() && app.md_cache.is_none());
    // The way back shows the same document again.
    let text = plain(&app.decorated_lines(60));
    assert!(text.contains("line 0"), "{text}");
}

#[test]
fn r_is_ignored_while_a_job_holds_the_session_and_nothing_changes() {
    let (mut app, _p, _d) = app_on(Config::default(), "a.md", "x\n");
    let held = app.plugin.as_mut().unwrap().session.take();
    app.toggle_md_raw();
    assert_eq!(app.tab.repr, None, "the choice was not recorded");
    assert!(!app.is_md_raw() && !app.is_windowed());
    app.plugin.as_mut().unwrap().session = held;
    app.toggle_md_raw();
    assert!(app.is_md_raw() && app.is_windowed());
}

#[test]
fn a_converted_document_s_source_is_a_temp_file_the_host_writes_and_removes() {
    let dir = unique_tmp("konoma_md_word_source");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    app.show_document_for_test(
        dir.join("w.docx"),
        Document {
            markdown: "# Converted\n\ntext\n".into(),
            ..Document::default()
        },
    );
    assert!(app.is_document() && app.document_ready());
    // The plugin the show keeps from here on is the markdown plugin's session.
    let reprs: Vec<String> = app
        .plugin
        .as_ref()
        .unwrap()
        .layout
        .reprs
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(reprs, ["rendered", "source"]);
    assert!(!app.document.as_ref().unwrap().options.tasks_writable);
    assert!(!app.document.as_ref().unwrap().options.front_matter);

    app.toggle_md_raw();
    assert!(app.is_md_raw() && app.is_windowed());
    let tmp = app.document_raw_file_for_test().expect("a temp file");
    assert_eq!(
        std::fs::read_to_string(&tmp).unwrap(),
        "# Converted\n\ntext\n",
        "the converted text, as it was"
    );
    assert_ne!(Some(tmp.as_path()), app.tab.preview_path.as_deref());
    app.toggle_md_raw();
    assert!(!app.is_md_raw() && !app.is_windowed());
    assert!(
        !tmp.exists(),
        "leaving the source view removes the temp file"
    );
    assert!(app.document_ready());
}

#[test]
fn the_source_view_is_asked_again_after_a_reload_and_a_tab_comes_back_in_it() {
    let (mut app, path, _dir) = app_on(Config::default(), "a.md", &long_md(40));
    app.toggle_md_raw();
    assert!(app.is_md_raw() && app.is_windowed());
    // A tab switch away and back reopens the plugin; the source view is the one chosen.
    app.tab_new().unwrap();
    app.tab_goto(0);
    assert_eq!(app.tab.preview_path.as_deref(), Some(path.as_path()));
    assert!(app.is_md_raw() && app.is_windowed(), "still the source");
    assert!(
        app.document_ready(),
        "and the document is there to return to"
    );
    app.toggle_md_raw();
    assert!(!app.is_md_raw() && !app.is_windowed());
    assert!(plain(&app.decorated_lines(60)).contains("line 0"));
}

#[test]
fn a_new_file_starts_in_the_rendered_document_whatever_the_last_one_chose() {
    let (dir, root) = dir_with(
        "konoma_md_two",
        &[
            ("a.md", b"# A\n"),
            ("b.md", b"# B\n"),
            ("c.mmd", b"graph TD; A-->B\n"),
        ],
    );
    let mut app = App::new(root.clone(), Config::default()).unwrap();
    app.enter_preview(&root.join("a.md"));
    app.toggle_md_raw();
    assert!(app.is_md_raw());
    app.enter_preview(&root.join("b.md"));
    assert!(!app.is_md_raw() && !app.is_windowed() && app.document_ready());
    // A standalone diagram file has a source view of its own (no plugin round trip), and it
    // does not leak into a Markdown file opened next.
    app.enter_preview(&root.join("c.mmd"));
    app.toggle_md_raw();
    assert!(app.is_md_raw() && app.is_raw_source());
    assert_eq!(
        app.plugin_view_req().repr,
        None,
        "the picture plugin never hears of it"
    );
    app.enter_preview(&root.join("a.md"));
    assert!(!app.is_md_raw() && app.document_ready());
    drop(dir);
}

// ---- Word through the plugin that takes the content type -------------------------------------

/// A stand-in plugin that takes `markdown` in place of the standard one.
struct Replacement {
    opened: Arc<AtomicUsize>,
}

struct ReplacementSession;

impl Session for ReplacementSession {
    fn layout(&self) -> Layout {
        Layout::single()
    }

    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<konoma_plugin::View, Failure> {
        Ok(konoma_plugin::View::Document(DocumentView {
            markdown: "# Drawn by the replacement\n".into(),
            origin: Origin::Converted {
                from: PathBuf::from("x"),
            },
            resources: Resources::new(),
            options: DocOptions {
                front_matter: false,
                tasks_writable: false,
                alerts: false,
                ..DocOptions::default()
            },
            title: None,
            truncated: false,
        }))
    }
}

impl Plugin for Replacement {
    fn manifest(&self) -> Manifest {
        Manifest::new("replacement", "0.0.1").bind(Bind::content("markdown"))
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        assert!(
            matches!(input, Input::Content { ref content_type, .. } if content_type == "markdown"),
            "{input:?}"
        );
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ReplacementSession))
    }
}

fn word_doc(markdown: &str) -> Document {
    Document {
        markdown: markdown.into(),
        ..Document::default()
    }
}

#[test]
fn a_plugin_that_takes_markdown_in_place_of_the_standard_one_changes_how_word_looks() {
    let opened = Arc::new(AtomicUsize::new(0));
    CONTENT_STAND_IN.with(|s| {
        *s.borrow_mut() = Some(Arc::new(Replacement {
            opened: opened.clone(),
        }));
    });
    // The standard Markdown plugin is switched off: the replacement alone takes the type.
    let cfg = cfg_from("[plugins.markdown]\nenabled = false\n");
    let dir = unique_tmp("konoma_md_replaced");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.show_document_for_test(dir.join("w.docx"), word_doc("# What Word converted\n"));
    CONTENT_STAND_IN.with(|s| *s.borrow_mut() = None);

    assert_eq!(opened.load(Ordering::SeqCst), 1);
    assert!(app.document_ready());
    assert_eq!(
        app.document_markdown_for_test(),
        Some("# Drawn by the replacement\n"),
        "the document is the replacement's answer"
    );
    assert!(
        !app.document.as_ref().unwrap().options.alerts,
        "and its options"
    );
    let text = plain(&app.decorated_lines(60));
    assert!(text.contains("Drawn by the replacement"), "{text}");
    assert!(!text.contains("What Word converted"));
}

#[test]
fn with_no_plugin_for_markdown_a_word_document_says_so_instead_of_showing_nothing() {
    let cfg = cfg_from("[plugins.markdown]\nenabled = false\n");
    let dir = unique_tmp("konoma_md_none");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.canonicalize().unwrap(), cfg).unwrap();
    app.show_document_for_test(dir.join("w.docx"), word_doc("# Hello\n"));
    assert!(!app.document_ready() && app.document.is_none());
    let f = app.document_error().expect("a reason");
    assert_eq!(f.kind, FailureKind::Unsupported);
    assert!(f.message.en().contains("no plugin"), "{f}");
    assert!(app.document_text_missing());
}

#[test]
fn a_converted_document_has_no_focusable_checkbox_but_a_file_has() {
    let task = "- [ ] a task\n";
    let dir = unique_tmp("konoma_md_tasks");
    std::fs::create_dir_all(&dir).unwrap();
    let mut word = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    word.show_document_for_test(dir.join("w.docx"), word_doc(task));
    word.md_layout(60);
    assert!(!word.doc_tasks_writable());
    assert!(!word.md_has_tasks(), "a glyph in Word text is only text");

    let (mut file, _p, _d) = app_on(Config::default(), "t.md", task);
    file.md_layout(60);
    assert!(file.doc_tasks_writable());
    assert!(file.md_has_tasks());
}

#[test]
fn a_documents_front_matter_follows_the_option_for_a_file_and_never_for_a_conversion() {
    let src = "---\ntitle: T\n---\n\nbody\n";
    let (mut with, _p, _d) = app_on(Config::default(), "f.md", src);
    let shown = plain(&with.decorated_lines(60));
    assert!(!shown.contains("---"), "front matter is a block: {shown}");
    let (mut without, _p, _d2) = app_on(cfg_from("[ui]\nmd_frontmatter = false\n"), "f.md", src);
    assert!(plain(&without.decorated_lines(60)).contains("title: T"));
    // A converted document is drawn as it is, whatever the setting says.
    let dir = unique_tmp("konoma_md_fm_word");
    std::fs::create_dir_all(&dir).unwrap();
    let mut word = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
    word.show_document_for_test(dir.join("w.docx"), word_doc(src));
    assert!(plain(&word.decorated_lines(60)).contains("title: T"));
}
