//! Tests added after a mutation audit of the spreadsheet preview: rules of `App` that no key
//! sequence can reach (they are about state the other paths keep from ever existing), so they set
//! that state directly.

use super::*;
use crate::config::Config;
use crate::test_support::unique_tmp;

fn app_with_media_channel() -> (
    App,
    crate::test_support::TmpDir,
    std::sync::mpsc::Receiver<MediaResult>,
) {
    let dir = unique_tmp("konoma_surv_wbq");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_loader(tx);
    (app, dir, rx)
}

/// An Office request waiting behind the running job of the work group is started when that job
/// ends — unless the preview moved on since it was asked (the media generation changed): then it
/// is dropped, and the group is simply free again. (Leaving a file clears the waiting request
/// itself; this is the second line of defence for any other path that bumps the generation.)
#[test]
fn a_waiting_workbook_request_of_an_old_generation_is_dropped_when_the_load_ends() {
    use konoma_plugin::view::ViewReq;
    use konoma_plugin::Input;
    use plugin_host::PluginJob;
    let (mut app, dir, _rx) = app_with_media_channel();
    let path = dir.join("never-there.docx");
    let ask = |app: &App| {
        let job = PluginJob::Open {
            plugin: Arc::new(konoma_office::DocumentPlugin),
            input: Input::File {
                path: path.clone(),
                hint: None,
            },
        };
        let mut work = app.plugin_work(
            konoma_office::DOCUMENT_ID.to_string(),
            path.clone(),
            job,
            ViewReq::default(),
        );
        work.gen = app.media_gen;
        work
    };
    let group = konoma_office::OFFICE_WORK_GROUP;

    // Stale: asked under a generation that has since moved on.
    let work = ask(&app);
    let slot = app.plugin_groups.entry(group.to_string()).or_default();
    slot.busy = true;
    slot.queued = Some(work);
    app.bump_media_gen();
    let started = app.plugin_group_dispatches;
    app.release_plugin_group(group);
    assert_eq!(
        app.plugin_group_dispatches, started,
        "the stale request was not started"
    );
    let slot = &app.plugin_groups[group];
    assert!(!slot.busy, "the group is free");
    assert!(slot.queued.is_none(), "and nothing waits any more");

    // Current: asked under the generation in force: started.
    let work = ask(&app);
    let slot = app.plugin_groups.entry(group.to_string()).or_default();
    slot.busy = true;
    slot.queued = Some(work);
    let started = app.plugin_group_dispatches;
    app.release_plugin_group(group);
    assert_eq!(
        app.plugin_group_dispatches,
        started + 1,
        "the current request was started"
    );
    assert!(app.plugin_groups[group].busy);
}

// ---- `R` for a plugin's own representations (`repr_cycle.rs`) -----------------------------------

mod repr {
    use super::*;
    use crate::plugins::test_plugins;
    use crate::plugins::test_views::{Multi, Shape};
    use konoma_plugin::view::{Layout, View, ViewReq};
    use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session, ViewCx};
    use std::sync::Arc;

    /// What a [`Split`] plugin says its default representation is.
    #[derive(Clone, Copy)]
    struct Defaults {
        with_pixels: &'static str,
        without_pixels: Option<&'static str>,
    }

    /// A [`Multi`] plugin whose session reports the given defaults.
    struct Split {
        inner: Multi,
        defaults: Option<Defaults>,
    }

    struct SplitSession {
        inner: Box<dyn Session>,
        defaults: Option<Defaults>,
    }

    impl Plugin for Split {
        fn manifest(&self) -> Manifest {
            self.inner.manifest()
        }
        fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(SplitSession {
                inner: self.inner.open(input, cx)?,
                defaults: self.defaults,
            }))
        }
    }

    impl Session for SplitSession {
        fn layout(&self) -> Layout {
            let mut l = self.inner.layout();
            if let Some(d) = self.defaults {
                l.default_repr = Some(d.with_pixels.into());
                l.default_repr_without_pixels = d.without_pixels.map(String::from);
            }
            l
        }
        fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
            self.inner.view(req, cx)
        }
    }

    /// An app on a file of a [`Split`] plugin of `shape` (three representations, `alpha`, `beta`,
    /// `gamma`), opened with a media worker channel so that pictures land through `settle`.
    fn open(
        name: &str,
        file: &str,
        shape: Shape,
        reprs: usize,
        defaults: Option<Defaults>,
        pixels: bool,
    ) -> (
        App,
        crate::test_support::TmpDir,
        std::sync::mpsc::Receiver<MediaResult>,
    ) {
        let dir = unique_tmp(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), b"x").unwrap();
        let ext: &'static str = Box::leak(
            file.rsplit('.')
                .next()
                .unwrap()
                .to_string()
                .into_boxed_str(),
        );
        let id: &'static str = Box::leak(format!("x-{name}").into_boxed_str());
        let marker = test_plugins::register(
            name,
            Arc::new(Split {
                inner: Multi {
                    id,
                    ext,
                    shape,
                    reprs,
                    pages: 1,
                },
                defaults,
            }),
        );
        let mut cfg = test_plugins::marker_config(&marker);
        cfg.ui.lang = "en".into();
        let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_media_loader(tx);
        if pixels {
            let mut picker = ratatui_image::picker::Picker::halfblocks();
            picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
            let (rtx, _rrx) = tokio::sync::mpsc::unbounded_channel();
            app.attach_image_backend(picker, rtx);
        }
        app.enter_preview(&dir.join(file));
        settle(&mut app, &rx);
        (app, dir, rx)
    }

    /// Apply what the worker answered until the plugin's show is ready.
    #[track_caller]
    fn settle(app: &mut App, rx: &std::sync::mpsc::Receiver<MediaResult>) {
        for _ in 0..20 {
            if app
                .plugin
                .as_ref()
                .is_some_and(|p| matches!(p.phase, plugin_host::PluginPhase::Ready))
            {
                return;
            }
            let res = rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the worker answers");
            app.apply_media(res);
        }
        panic!("the plugin never became ready");
    }

    fn next_label(app: &App) -> Option<String> {
        app.repr_step().map(|s| s.label)
    }

    /// A document that declares no representation and has answered is not the host's
    /// `rendered`/`source` pair: it has no `R`. One that has not answered yet is assumed to be.
    #[test]
    fn a_ready_document_declaring_nothing_has_no_r_and_a_loading_one_is_assumed_standard() {
        let (mut app, _dir, _rx) = open("f8b_repr_zero", "a.xdoc", Shape::Doc, 0, None, false);
        assert!(app.holds_document());
        assert!(!app.host_source_view());
        assert!(next_label(&app).is_none());
        assert!(!app.cycle_repr(), "R acts on nothing");
        assert!(!app.is_md_raw());
        // Before the first answer the layout is not known: the standard shape is assumed.
        app.plugin.as_mut().unwrap().phase = plugin_host::PluginPhase::Loading;
        assert!(app.host_source_view());
    }

    /// The representation shown first depends on what the terminal can draw: the plugin's default
    /// with pixels, its default for a terminal without them else (falling back to the first when
    /// it declares none).
    #[test]
    fn the_first_representation_follows_what_the_terminal_draws() {
        let split = Some(Defaults {
            with_pixels: "alpha",
            without_pixels: Some("beta"),
        });
        // Pixels: alpha is shown, R goes to beta.
        let (app, _d, _r) = open("f8b_repr_px", "a.xtxt", Shape::Text, 3, split, true);
        assert!(app.terminal_draws_pixels());
        assert_eq!(next_label(&app).as_deref(), Some("Beta"));
        // No pixels: beta is shown, R goes to gamma.
        let (app, _d, _r) = open("f8b_repr_nopx", "a.xtxt", Shape::Text, 3, split, false);
        assert!(!app.terminal_draws_pixels());
        assert_eq!(next_label(&app).as_deref(), Some("Gamma"));
        // The other way round, so that neither field is read for both.
        let swapped = Some(Defaults {
            with_pixels: "gamma",
            without_pixels: Some("alpha"),
        });
        let (app, _d, _r) = open("f8b_repr_px2", "a.xtxt", Shape::Text, 3, swapped, true);
        assert_eq!(next_label(&app).as_deref(), Some("Alpha"));
        let (app, _d, _r) = open("f8b_repr_nopx2", "a.xtxt", Shape::Text, 3, swapped, false);
        assert_eq!(next_label(&app).as_deref(), Some("Beta"));
        // No default of its own for a terminal without pixels: the general default is used.
        let single = Some(Defaults {
            with_pixels: "beta",
            without_pixels: None,
        });
        let (app, _d, _r) = open("f8b_repr_nopx3", "a.xtxt", Shape::Text, 3, single, false);
        assert_eq!(next_label(&app).as_deref(), Some("Gamma"));
        // The tab's own choice wins over both.
        let (mut app, _d, _r) = open("f8b_repr_choice", "a.xtxt", Shape::Text, 3, split, false);
        app.tab.repr = Some("alpha".into());
        assert_eq!(next_label(&app).as_deref(), Some("Beta"));
    }

    /// When the session cannot be asked (a job holds it), a document's `R` does nothing and the
    /// choice it made is taken back (its answer lands on the spot, so it needs the session now).
    #[test]
    fn a_document_cycle_that_could_not_ask_the_session_leaves_the_choice_as_it_was() {
        let name = "f8b_repr_back_doc";
        let (mut app, _d, rx) = open(name, "a.xdoc", Shape::Doc, 3, None, false);
        assert_eq!(app.tab.repr, None, "{name}");
        // A first cycle works: the choice is `beta`. (This stand-in is not one of the host's own
        // plugins, so its answer comes from a worker, not on the spot.)
        assert!(app.cycle_repr(), "{name}");
        assert_eq!(app.tab.repr.as_deref(), Some("beta"), "{name}");
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker answers");
        app.apply_media(res);
        // A job holds the session now.
        let held = app.plugin.as_mut().unwrap().session.take();
        assert!(held.is_some());
        assert!(!app.cycle_repr(), "{name}: nothing to ask");
        assert_eq!(app.tab.repr.as_deref(), Some("beta"), "{name}: taken back");
        // And from no choice at all, the choice is back to none.
        app.plugin.as_mut().unwrap().session = held;
        app.tab.repr = None;
        let held = app.plugin.as_mut().unwrap().session.take();
        assert!(!app.cycle_repr(), "{name}");
        assert_eq!(app.tab.repr, None, "{name}: back to none");
        app.plugin.as_mut().unwrap().session = held;
    }

    /// Any other display asks on a worker: with a job holding the session the choice stands, the
    /// wish is remembered, and it is asked (once, with the newest choice) when the session returns.
    #[test]
    fn another_displays_cycle_waits_for_the_session_instead_of_being_lost() {
        let (mut app, _d, rx) = open("f8b_repr_wait_txt", "a.xtxt", Shape::Text, 3, None, false);
        let held = app.plugin.as_mut().unwrap().session.take();
        assert!(held.is_some());
        assert!(app.cycle_repr(), "R acts: the wish is remembered");
        assert_eq!(app.tab.repr.as_deref(), Some("beta"));
        assert!(app.plugin.as_ref().unwrap().flow.wants_view);
        // The session returns (a job answered): the view is asked for, with `beta`.
        app.plugin.as_mut().unwrap().session = held;
        app.plugin_session_returned();
        assert!(!app.plugin.as_ref().unwrap().flow.wants_view);
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker is asked");
        assert!(app.apply_media(res), "the answer lands");
    }

    /// Another representation starts the view over: scroll, caret, selection and the windowed
    /// reader of the one left do not carry over (a document and the other displays).
    #[test]
    fn a_new_representation_starts_the_view_over() {
        for (name, file, shape) in [
            ("f8b_repr_reset_doc", "a.xdoc", Shape::Doc),
            ("f8b_repr_reset_txt", "a.xtxt", Shape::Text),
        ] {
            let (mut app, _d, _r) = open(name, file, shape, 3, None, false);
            app.tab.preview_byte_top = 7;
            app.tab.preview_top_line = 6;
            app.tab.preview_scroll = 5;
            app.tab.preview_hscroll = 4;
            app.tab.preview_cursor_line = 3;
            app.tab.preview_cursor_col = 2;
            app.preview_visual_anchor = Some((1, 1));
            app.preview_visual_linewise = true;
            if matches!(shape, Shape::Doc) {
                app.ensure_md_cache(80);
                assert!(app.md_cache.is_some(), "{name}: the document is laid out");
            }
            assert!(app.cycle_repr(), "{name}");
            assert!(
                app.md_cache.is_none(),
                "{name}: the layout of the old one is dropped"
            );
            assert_eq!(app.tab.preview_byte_top, 0, "{name}");
            assert_eq!(app.tab.preview_top_line, 0, "{name}");
            assert_eq!(app.tab.preview_scroll, 0, "{name}");
            assert_eq!(app.tab.preview_hscroll, 0, "{name}");
            assert_eq!(app.tab.preview_cursor_line, 0, "{name}");
            assert_eq!(app.tab.preview_cursor_col, 0, "{name}");
            assert_eq!(app.preview_visual_anchor, None, "{name}");
            assert!(!app.preview_visual_linewise, "{name}");
        }
    }

    /// A picture in another representation is shown whole again: zoom, centre and crop start over.
    #[test]
    fn a_new_representation_of_a_picture_is_shown_whole_again() {
        let (mut app, _d, _r) = open("f8b_repr_reset_img", "a.ximg", Shape::Image, 3, None, true);
        assert!(app.shows_media_plugin());
        app.tab.image_zoom = 3.0;
        app.tab.image_center = (0.2, 0.9);
        app.image_crop = Some((1, 2, 3, 4));
        assert!(app.cycle_repr());
        assert_eq!(app.tab.image_zoom, 1.0);
        assert_eq!(app.tab.image_center, (0.5, 0.5));
        assert_eq!(app.image_crop, None);
        assert_eq!(app.tab.repr.as_deref(), Some("beta"));
    }
}

// ---- the stamp of a file (`FileStamp`) ----------------------------------------------------------

/// Every part of a file's stamp is the file's own: the modification time, the size, the device and
/// inode, and the status-change time. A stamp that left one out would call two different files one.
#[test]
fn a_file_stamp_carries_every_identity_of_the_file() {
    use std::os::unix::fs::MetadataExt;
    let dir = unique_tmp("konoma_surv_stamp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.txt");
    std::fs::write(&path, "hello").unwrap();
    let m = std::fs::metadata(&path).unwrap();
    let s = file_stamp(&path).expect("a stamp");
    assert_eq!(s.modified, m.modified().unwrap());
    assert_eq!(s.len, 5);
    assert_eq!(s.dev, m.dev());
    assert_eq!(s.ino, m.ino());
    assert_eq!(s.ctime, (m.ctime(), m.ctime_nsec()));
    assert_ne!(s.ino, 0);
    assert!(file_stamp(&dir.join("not-there")).is_none());
}

/// A file replaced by another with the same size and the same modification time (a rename over
/// it) is another file: its stamp differs, in the inode as well as in the change time.
#[test]
fn a_file_replaced_by_a_rename_has_another_stamp() {
    let dir = unique_tmp("konoma_surv_stamp_rename");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.txt");
    let other = dir.join("b.txt");
    std::fs::write(&path, "same!").unwrap();
    std::fs::write(&other, "same!").unwrap();
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for p in [&path, &other] {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }
    let before = file_stamp(&path).unwrap();
    assert_eq!(before.modified, file_stamp(&other).unwrap().modified);
    std::fs::rename(&other, &path).unwrap();
    let after = file_stamp(&path).unwrap();
    assert_eq!(after.len, before.len);
    assert_eq!(after.modified, before.modified);
    assert_eq!(after.dev, before.dev, "the same device");
    assert_ne!(after.ino, before.ino);
    assert_ne!(after, before);
}

// ---- the open plugin's actions (`plugin_acts.rs`) ----------------------------------------------

mod open_plugin_acts {
    use super::*;
    use crate::keymap::{Action, Context, KeyPress};
    use crate::plugins::test_plugins::{marker_config, register};
    use crate::plugins::PluginAct;
    use konoma_plugin::prelude::*;
    use konoma_plugin::view::ViewReq;
    use konoma_plugin::Act;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct Declares;
    struct Calm;

    impl Plugin for Declares {
        fn manifest(&self) -> Manifest {
            Manifest::new("x-declares", "1")
                .bind(Bind::files(["*.dcl"]))
                .shows(ViewKind::Text)
                .action(Act::new("anywhere").key("w").label("a").help("a help"))
                .action(
                    Act::new("in-table")
                        .context("table")
                        .label("t")
                        .help("t help"),
                )
                .action(
                    Act::new("in-image")
                        .context("image")
                        .label("i")
                        .help("i help"),
                )
                .action(
                    Act::new("in-doc")
                        .context("document")
                        .label("d")
                        .help("d help"),
                )
                .action(
                    Act::new("in-text")
                        .context("text")
                        .label("x")
                        .help("x help"),
                )
        }
        fn open(&self, _: konoma_plugin::Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(Calm))
        }
    }

    impl Session for Calm {
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::text(["x"]))
        }
    }

    fn open(name: &str) -> (App, PathBuf, crate::test_support::TmpDir) {
        let dir = unique_tmp(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.dcl");
        std::fs::write(&file, "x").unwrap();
        let marker = register(name, Arc::new(Declares));
        let mut app = App::new(dir.to_path_buf(), marker_config(&marker)).unwrap();
        app.enter_preview(&file);
        (app, file, dir)
    }

    fn ids(app: &App, ctx: Context) -> Vec<&'static str> {
        app.plugin_acts_in(ctx)
            .iter()
            .map(|b| b.act.act())
            .collect()
    }

    /// The plugin's actions live in the contexts that show a plugin's view and in no other, each
    /// action in the context it declared (and an action with none in all of them).
    #[test]
    fn actions_belong_to_the_view_contexts_they_were_declared_for() {
        let (app, _f, _d) = open("f8b_acts_ctx");
        assert_eq!(ids(&app, Context::Text), ["anywhere", "in-text"]);
        assert_eq!(ids(&app, Context::Table), ["anywhere", "in-table"]);
        assert_eq!(ids(&app, Context::Image), ["anywhere", "in-image"]);
        assert_eq!(ids(&app, Context::Document), ["anywhere", "in-doc"]);
        for ctx in [
            Context::Tree,
            Context::TreeVisual,
            Context::TextVisual,
            Context::TableCell,
            Context::Bookmarks,
            Context::Tabs,
            Context::Outline,
            Context::Info,
            Context::Help,
        ] {
            assert!(ids(&app, ctx).is_empty(), "{ctx:?}");
        }
        // The key finds the action in its own context only.
        assert_eq!(
            app.plugin_action_for_key(Context::Text, KeyPress::ch('w')),
            Some(Action::Plugin(PluginAct::new("x-declares", "anywhere")))
        );
        assert_eq!(
            app.plugin_action_for_key(Context::Tree, KeyPress::ch('w')),
            None
        );
        assert!(app
            .plugin_binding_in(Context::Table, PluginAct::new("x-declares", "in-table"))
            .is_some());
        assert!(app
            .plugin_binding_in(Context::Text, PluginAct::new("x-declares", "in-table"))
            .is_none());
    }

    /// The actions are those of the plugin on screen, for the file on screen: the show of another
    /// file, or of another plugin, is not the one that acts.
    #[test]
    fn only_the_show_of_this_plugin_and_this_file_acts() {
        let (mut app, file, dir) = open("f8b_acts_show");
        assert_eq!(ids(&app, Context::Text).len(), 2);
        let kind = |id: &str, path: PathBuf| PreviewKind::Plugin {
            path,
            id: id.into(),
            hint: None,
        };
        // The preview is another file of the same plugin: its show has not landed yet.
        app.tab.preview_kind = Some(kind("x-declares", dir.join("other.dcl")));
        assert!(ids(&app, Context::Text).is_empty(), "another file");
        // The preview is another plugin's, on the same file.
        app.tab.preview_kind = Some(kind("csv", file.clone()));
        assert!(ids(&app, Context::Text).is_empty(), "another plugin");
        app.tab.preview_kind = Some(kind("x-declares", file));
        assert_eq!(ids(&app, Context::Text).len(), 2);
    }

    /// Whether an action works is the answer of the session of **that plugin**'s show.
    #[test]
    fn an_action_of_another_plugin_is_not_enabled_by_this_ones_show() {
        let (mut app, _f, _d) = open("f8b_acts_enabled");
        let mine = PluginAct::new("x-declares", "anywhere");
        let other = PluginAct::new("someone-else", "anywhere");
        app.plugin
            .as_mut()
            .unwrap()
            .enabled
            .insert("anywhere".into(), true);
        assert!(app.plugin_act_enabled(mine));
        assert!(!app.plugin_act_enabled(other), "not this show's plugin");
    }
}

// ---- follow: which files get their preview instead of a diff ------------------------------------

/// A video (a picture plugin's file that cannot be laid side by side) is followed with its preview
/// when it holds binary data, and with the diff when it is gone: a deleted file has nothing to
/// preview, and a file that cannot be read is not called binary.
#[test]
fn following_a_deleted_video_or_picture_opens_the_diff_not_the_preview() {
    let dir = unique_tmp("konoma_surv_follow_binary");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    // The reader of "a video": missing is not, a video's content is, text is not.
    assert!(!media_diff::is_video_file(&dir.join("gone.mp4")));
    let video = dir.join("clip.mp4");
    std::fs::write(&video, b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom\0\0").unwrap();
    assert!(media_diff::is_video_file(&video));
    let text = dir.join("notes.mp4");
    std::fs::write(&text, "not binary at all\n").unwrap();
    assert!(!media_diff::is_video_file(&text));
    // And what follow does with them.
    assert!(
        app.follow_previews_instead_of_diff(&video),
        "a video on disk"
    );
    assert!(
        !app.follow_previews_instead_of_diff(&dir.join("gone.mp4")),
        "a video that was deleted"
    );
    assert!(!app.follow_previews_instead_of_diff(&text));
    // A picture that can be shown side by side is not previewed instead.
    let png = dir.join("p.png");
    std::fs::write(&png, [0x89u8, b'P', b'N', b'G', 0, 0, 0, 0]).unwrap();
    assert!(!app.follow_previews_instead_of_diff(&png));
    assert!(!app.follow_previews_instead_of_diff(&dir.join("gone.png")));
}

/// Why a deleted video is followed with the diff, as in v0.31.0: the standard rule takes a video
/// by its content (`mime = "video/*"`), and a file that is gone has no content to sniff, so it is
/// not a video (nor a picture plugin's file at all) by the time follow asks. v0.31.0 asked
/// `matches!(resolve_preview(path), PreviewKind::Video(_))`, which is false for the same reason.
#[test]
fn a_deleted_video_is_not_resolved_as_a_video_so_follow_opens_the_diff() {
    let dir = unique_tmp("konoma_surv_follow_deleted_video_kind");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let gone = dir.join("gone.mp4");
    let kind = app.cfg.resolve_preview(&gone);
    assert!(
        !kind.is_plugin(konoma_media::VIDEO_ID) && !kind.profile().image,
        "a file with no content to sniff is not taken by the video rule"
    );
    assert!(!app.follow_previews_instead_of_diff(&gone));
    // The same file with its content on disk is a video, and is previewed.
    std::fs::write(&gone, b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom\0\0").unwrap();
    let kind = app.cfg.resolve_preview(&gone);
    assert!(kind.is_plugin(konoma_media::VIDEO_ID));
    assert!(app.follow_previews_instead_of_diff(&gone));
}

/// Follow previews a file whose **content** is a video, as v0.31.0's `mime = "video/*"` rule did,
/// not one that merely has a NUL in its head: a video format with no NUL in its first bytes is
/// previewed, and text or binary data of no known type gets the diff.
#[test]
fn follow_previews_by_the_content_being_a_video() {
    let dir = unique_tmp("konoma_surv_follow_video_content");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    // Videos whose first 512 bytes hold no NUL at all (Matroska/WebM, AVI).
    let mut webm = b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01\x42\xf7\x81\x01\x42\xf2\x81\x04\x42\xf3\x81\x08\x42\x82\x84webm".to_vec();
    webm.resize(600, 0x55);
    let mut avi = b"RIFF\x24\x10\x20\x01AVI LIST".to_vec();
    avi.resize(600, 0x55);
    for (name, bytes) in [("a.webm", webm), ("b.avi", avi)] {
        let f = dir.join(name);
        assert!(!bytes[..512].contains(&0));
        std::fs::write(&f, bytes).unwrap();
        assert!(media_diff::is_video_file(&f), "{name}");
        assert!(app.follow_previews_instead_of_diff(&f), "{name}");
    }
    // Not a video: binary data of no known type (NULs, no signature), and text.
    let blob = dir.join("blob.mp4");
    std::fs::write(&blob, [0u8, 1, 2, 3, 0, 0, 9, 9]).unwrap();
    assert!(!media_diff::is_video_file(&blob));
    assert!(!app.follow_previews_instead_of_diff(&blob));
    let text = dir.join("t.mp4");
    std::fs::write(&text, "plain text\n").unwrap();
    assert!(!app.follow_previews_instead_of_diff(&text));
    // A deleted video: nothing to judge, the diff (as in v0.31.0).
    assert!(!app.follow_previews_instead_of_diff(&dir.join("gone.webm")));
}
