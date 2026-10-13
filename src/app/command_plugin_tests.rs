//! The built-in `command` plugin (`plugins/command.rs`) and the host's side of it: a delegated
//! command opened on a worker, its text and picture results, its failure text, and the detached
//! launch that happens once and never again (not by a tab switch, an outside change to the file,
//! `r` or a second `view`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;

use konoma_plugin::view::{ImageData, TextSource, View};
use konoma_plugin::{Input, OpenCx, Plugin, ViewCx, ViewReq};

use super::*;
use crate::plugins::CommandPlugin;
use crate::test_support::unique_tmp;

fn open_view(
    plugin: &CommandPlugin,
    file: &Path,
) -> (
    Box<dyn konoma_plugin::Session>,
    Result<View, konoma_plugin::Failure>,
) {
    let mut session = plugin
        .open(
            Input::File {
                path: file.to_path_buf(),
                hint: None,
            },
            &OpenCx::bare(),
        )
        .expect("the command plugin opens a file");
    let view = session.view(&ViewReq::default(), &ViewCx::bare());
    (session, view)
}

fn sandbox(name: &str) -> (crate::test_support::TmpDir, PathBuf) {
    let dir = unique_tmp(name);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.custom");
    std::fs::write(&file, "irrelevant").unwrap();
    (dir, file)
}

fn app_with_rule(dir: &Path, rule: crate::config::Rule) -> App {
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    app.cfg.preview.rules = vec![rule];
    app
}

fn rule(command: &str) -> crate::config::Rule {
    crate::config::Rule {
        glob: Some("*.custom".into()),
        command: Some(command.into()),
        ..Default::default()
    }
}

// ---- the plugin on its own ----------------------------------------------------------------------

/// A text result is a view whose source is the file the output was written to: the host reads it
/// in windows like any text.
#[cfg(unix)]
#[test]
fn a_text_command_answers_with_a_text_view_over_its_output_file() {
    let (_dir, file) = sandbox("konoma_cmdplugin_text");
    let (_session, view) = open_view(&CommandPlugin::new("cat {path}", None, false), &file);
    match view.expect("the command ran") {
        View::Text(t) => match t.source {
            TextSource::File(p) => assert_eq!(std::fs::read_to_string(p).unwrap(), "irrelevant"),
            other => panic!("a file source was expected, not {other:?}"),
        },
        other => panic!("a text view was expected, not {other:?}"),
    }
}

/// `render_as = "image"`: the produced file is decoded (under the image limits) and handed over as
/// pixels; the temp file is gone.
#[cfg(unix)]
#[test]
fn an_image_command_answers_with_the_decoded_pixels() {
    let (_dir, file) = sandbox("konoma_cmdplugin_image");
    // The "command" copies the previewed file, which is itself a PNG, to `{out}`.
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(5, 3, image::Rgb([9, 9, 200])))
        .save_with_format(&file, image::ImageFormat::Png)
        .unwrap();
    let plugin = CommandPlugin::new("cp {path} {out}", Some("image"), false);
    let (_session, view) = open_view(&plugin, &file);
    match view.expect("the command ran") {
        View::Image(iv) => match iv.data {
            ImageData::Rgba {
                width,
                height,
                pixels,
            } => {
                assert_eq!((width, height), (5, 3));
                assert_eq!(pixels.len(), 5 * 3 * 4);
            }
            other => panic!("pixels were expected, not {other:?}"),
        },
        other => panic!("an image view was expected, not {other:?}"),
    }
}

/// A command that cannot run is a failure whose English message is the reason the
/// `[can not preview: <ext>]` screen shows.
#[cfg(unix)]
#[test]
fn a_command_that_cannot_run_fails_with_its_reason() {
    let (_dir, file) = sandbox("konoma_cmdplugin_fail");
    let plugin = CommandPlugin::new("konoma-no-such-program-xyz {path}", None, false);
    let (_session, view) = open_view(&plugin, &file);
    let f = view.expect_err("the program does not exist");
    assert!(f.message.en().contains("konoma-no-such-program-xyz"), "{f}");
    assert_eq!(f.kind, konoma_plugin::FailureKind::External);
}

/// The instance the registry holds is bound to no rule: an empty template is no command at all,
/// and the previewed file is never run as one.
#[cfg(unix)]
#[test]
fn the_registry_instance_is_bound_to_no_rule() {
    let (_dir, file) = sandbox("konoma_cmdplugin_unbound");
    let registry = crate::plugins::shared_registry(&[]);
    let plugin = registry
        .plugin("command")
        .expect("the command plugin is registered");
    let mut session = plugin
        .open(
            Input::File {
                path: file,
                hint: None,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let f = session
        .view(&ViewReq::default(), &ViewCx::bare())
        .expect_err("an empty template runs nothing");
    assert!(f.message.en().contains("empty command template"), "{f}");
}

/// A detached session starts its program once. A second `view` answers with the message again and
/// starts nothing.
#[cfg(unix)]
#[test]
fn a_detached_session_starts_the_program_once() {
    let (_dir, file) = sandbox("konoma_cmdplugin_detached_once");
    let plugin = CommandPlugin::new("true", None, true);
    let before = crate::preview::command::calls_for_test();
    let (mut session, first) = open_view(&plugin, &file);
    let first_text = match first.expect("launched") {
        View::Message(m) => m.lines.iter().map(|l| l.plain()).collect::<String>(),
        other => panic!("a message was expected, not {other:?}"),
    };
    assert!(
        first_text.contains("true"),
        "names the template: {first_text}"
    );
    let again = session.view(&ViewReq::default(), &ViewCx::bare());
    assert!(matches!(again, Ok(View::Message(_))));
    assert_eq!(
        crate::preview::command::calls_for_test() - before,
        1,
        "the second view launched nothing"
    );
}

/// The message follows the user's language.
#[cfg(unix)]
#[test]
fn the_detached_message_is_in_the_users_language() {
    let (_dir, file) = sandbox("konoma_cmdplugin_detached_lang");
    let plugin = CommandPlugin::new("true", None, true);
    let mut session = plugin
        .open(
            Input::File {
                path: file,
                hint: None,
            },
            &OpenCx::bare(),
        )
        .unwrap();
    let cx = ViewCx::new(konoma_plugin::Settings::empty(), "ja");
    match session.view(&ViewReq::default(), &cx).unwrap() {
        View::Message(m) => assert!(m.lines[0].plain().contains("外部プログラム")),
        other => panic!("a message was expected, not {other:?}"),
    }
}

// ---- through the host ----------------------------------------------------------------------------

/// A detached rule: entering the preview launches once and the plugin's message is what the
/// screen says. Redrawing, a tab switch, an outside change to the file, `r` and a plain reload
/// launch nothing more.
#[cfg(unix)]
#[test]
fn a_detached_command_is_not_started_again_by_reload_refresh_r_or_a_tab_switch() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let (dir, file) = sandbox("konoma_cmdplugin_detached_app");
    let mut app = app_with_rule(
        &dir,
        crate::config::Rule {
            detached: true,
            ..rule("true")
        },
    );
    let before = crate::preview::command::calls_for_test();
    app.enter_preview(&file);
    assert_eq!(crate::preview::command::calls_for_test() - before, 1);
    assert!(
        app.command_message().is_some_and(|m| m.contains("true")),
        "the plugin's message is on the show: {:?}",
        app.command_message()
    );
    assert!(app.command_error().is_none());
    assert!(!app.is_media_loading(), "the launch settled at once");

    let screen = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(70, 10)).unwrap();
        term.draw(|f| crate::ui::preview::render(f, app, f.area()))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    };
    assert!(screen(&mut app).contains("true"));

    // An outside change to the file (a newer mtime), then every way the host looks at it again.
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    app.refresh_fs(false).unwrap();
    app.reload_preview();
    app.refresh().unwrap();
    app.tab_new().unwrap();
    app.tab_cycle(-1);
    app.reload_preview();
    assert_eq!(
        crate::preview::command::calls_for_test() - before,
        1,
        "nothing but entering the preview launches a detached command"
    );
}

/// A command that fails shows its reason where it always did (`command_error`), from the plugin's
/// failure.
#[cfg(unix)]
#[test]
fn a_failed_command_sets_the_reason_for_the_can_not_preview_screen() {
    let (dir, file) = sandbox("konoma_cmdplugin_fail_app");
    let mut app = app_with_rule(&dir, rule("konoma-no-such-program-xyz {path}"));
    app.enter_preview(&file);
    let why = app.command_error().expect("the reason is kept");
    assert!(why.contains("konoma-no-such-program-xyz"), "{why}");
    assert!(app.plugin_failure().is_some(), "the show is a failed one");
    assert!(!app.is_media_loading());
}

/// With a worker attached, a text command runs off the UI thread: the preview is "loading" until
/// the result lands, and then the output is the windowed text, titled by the original file.
#[cfg(unix)]
#[test]
fn a_text_command_runs_on_a_worker_and_lands_as_windowed_text() {
    let (dir, file) = sandbox("konoma_cmdplugin_worker");
    let mut app = app_with_rule(&dir, rule("echo from-the-worker"));
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    app.enter_preview(&file);
    assert!(
        app.is_media_loading(),
        "the command is running off this thread"
    );
    assert!(!app.is_windowed(), "nothing to read until it has answered");
    let res = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker answers");
    assert!(app.apply_media(res));
    assert!(!app.is_media_loading());
    assert!(app.is_windowed());
    let out = app.tab.command_out.clone().expect("the output file");
    assert!(std::fs::read_to_string(out)
        .unwrap()
        .contains("from-the-worker"));
    assert!(app.command_error().is_none());
}

/// A result that arrives after the preview moved on is dropped, and nothing of it is kept.
#[cfg(unix)]
#[test]
fn a_command_result_for_a_preview_left_behind_is_dropped() {
    let (dir, file) = sandbox("konoma_cmdplugin_stale");
    let other = dir.join("plain.txt");
    std::fs::write(&other, "text").unwrap();
    let mut app = app_with_rule(&dir, rule("echo stale-output"));
    let (tx, rx) = channel();
    app.attach_media_loader(tx);
    app.enter_preview(&file);
    app.enter_preview(&other); // moves on before the command has been applied
    while let Ok(res) = rx.recv_timeout(Duration::from_millis(500)) {
        app.apply_media(res);
    }
    assert!(
        app.tab.command_out.is_none(),
        "the old output was not adopted"
    );
    assert!(app.command_error().is_none());
}

/// An image-mode command, through the host: with a graphics backend the produced picture lands
/// as the full-screen image (the same pixels as before), and the temp file is consumed.
#[cfg(unix)]
#[test]
fn an_image_command_lands_as_the_full_screen_picture() {
    let (dir, file) = sandbox("konoma_cmdplugin_image_app");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 4, image::Rgb([200, 9, 9])))
        .save_with_format(&file, image::ImageFormat::Png)
        .unwrap();
    let mut app = app_with_rule(
        &dir,
        crate::config::Rule {
            render_as: Some("image".into()),
            ..rule("cp {path} {out}")
        },
    );
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.enter_preview(&file);
    assert!(app.is_image_preview(), "the picture is on screen");
    let (w, h, rgba) = app.shown_picture_rgba().expect("a still picture");
    assert_eq!((w, h), (6, 4));
    assert_eq!(&rgba[..4], &[200, 9, 9, 255]);
    assert!(app.command_error().is_none());
}

/// Without a graphics backend an image-mode command is not started at all (as before): the
/// render side falls back, no program runs.
#[cfg(unix)]
#[test]
fn an_image_command_without_a_graphics_backend_starts_nothing() {
    let (dir, file) = sandbox("konoma_cmdplugin_image_nopicker");
    let mut app = app_with_rule(
        &dir,
        crate::config::Rule {
            render_as: Some("image".into()),
            ..rule("cp {path} {out}")
        },
    );
    assert!(app.picker.is_none());
    let before = crate::preview::command::calls_for_test();
    app.enter_preview(&file);
    assert_eq!(crate::preview::command::calls_for_test(), before);
    assert!(!app.is_image_preview());
}
