//! Main-loop configuration: decisions arrive through the media mailbox, never by join.
use super::*;
#[cfg(feature = "git")]
use crate::config::Rule;
use konoma_plugin::host::file_preview::test_hooks::count_path_io;
use std::time::Duration;

#[cfg(feature = "git")]
#[test]
fn ui_suffix_decisions_are_received_without_path_io_or_synchronous_resolution() {
    for (name, builtin, target) in [
        (
            "image.png",
            "image",
            konoma_host_core::project::DiffTarget::Picture,
        ),
        (
            "document.pdf",
            "pdf",
            konoma_host_core::project::DiffTarget::Picture,
        ),
        (
            "drawing.svg",
            "svg",
            konoma_host_core::project::DiffTarget::Svg,
        ),
    ] {
        let dir = crate::test_support::unique_tmp("ui_preview_suffix");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"unused").unwrap();
        let mut config = Config::default();
        config.preview.rules = vec![Rule {
            glob: Some("*".into()),
            builtin: Some(builtin.into()),
            ..Default::default()
        }];
        let mut app = App::new(dir.to_path_buf(), config).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_preview_resolution_worker(tx);
        app.tab.preview_kind = Some(PreviewKind::GitDiff(path.clone()));
        let (((_, lookups), calls), io) = count_path_io(|| {
            crate::test_support::count_resolve_preview_calls(|| {
                crate::test_support::count_plugin_id_reads(|| {
                    app.refresh_diff_target_kind_cache();
                    assert!(
                        app.diff_target_kind_cache.is_none(),
                        "{name}: await worker reply"
                    );
                    for _ in 0..3 {
                        assert_eq!(
                            app.verb_plugin_id(&path),
                            None,
                            "{name}: selected plugin is pending"
                        );
                        let _ = app.diff_representations(&path);
                        let _ = app.diff_media_capable(&path);
                    }
                })
            })
        });
        assert_eq!(
            lookups, 0,
            "{name}: no synchronous selected-plugin lookup on the UI thread"
        );
        assert_eq!(calls, 0, "{name}: no synchronous resolver on the UI thread");
        assert_eq!(
            io,
            Default::default(),
            "{name}: no path I/O on the UI thread"
        );
        // Both publications are awaited explicitly, irrespective of their arrival order.
        for _ in 0..2 {
            let reply = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let (_, io) = count_path_io(|| app.apply_media(reply));
            assert_eq!(
                io,
                Default::default(),
                "{name}: adopting the reply does not resolve again"
            );
        }
        assert_eq!(app.verb_plugin_id(&path).as_deref(), Some(builtin));
        let (_, resolved, exists) = app.diff_target_kind_cache.as_ref().unwrap();
        assert!(*exists);
        assert_eq!(resolved.diff_target, target);
    }
}

#[test]
fn a_deferred_edit_keeps_its_original_file_after_tree_navigation() {
    let dir = crate::test_support::unique_tmp("deferred_preview_verb");
    std::fs::create_dir_all(&dir).unwrap();
    let first = dir.join("a.md");
    let second = dir.join("b.md");
    std::fs::write(&first, "# first").unwrap();
    std::fs::write(&second, "# second").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_preview_resolution_worker(tx);
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == first)
        .unwrap();
    let (_, io) = count_path_io(|| app.request_edit());
    assert_eq!(io, Default::default());
    assert!(
        app.pending_edit.is_none(),
        "a not-yet-resolved plugin must not launch a fallback editor"
    );
    app.tab.selected = app
        .tab
        .entries
        .iter()
        .position(|e| e.path == second)
        .unwrap();
    app.verb_plugin_id(&second);
    for _ in 0..2 {
        app.apply_media(rx.recv_timeout(Duration::from_secs(10)).unwrap());
    }
    assert_eq!(
        app.pending_edit.as_ref().map(|(path, _)| path),
        Some(&first)
    );
}

#[cfg(feature = "git")]
#[test]
fn a_stale_preview_decision_cannot_replace_a_new_diff_target() {
    let dir = crate::test_support::unique_tmp("stale_diff_preview_plan");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_preview_resolution_worker(tx);
    let first = dir.join("old.md");
    let second = dir.join("new.rs");
    app.tab.preview_kind = Some(PreviewKind::GitDiff(first));
    app.refresh_diff_target_kind_cache();
    app.tab.preview_kind = Some(PreviewKind::GitDiff(second.clone()));
    app.refresh_diff_target_kind_cache();
    for _ in 0..2 {
        app.apply_media(rx.recv_timeout(Duration::from_secs(10)).unwrap());
    }
    let (path, resolved, _) = app.diff_target_kind_cache.as_ref().unwrap();
    assert_eq!(path, &second);
    assert_eq!(
        resolved.diff_target,
        konoma_host_core::project::DiffTarget::Text
    );
}

#[test]
fn an_ordinary_preview_receives_its_plan_without_ui_resolution() {
    let dir = crate::test_support::unique_tmp("ordinary_preview_plan");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plain");
    std::fs::write(&path, "plain text").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_preview_resolution_worker(tx);
    let ((_, calls), io) = count_path_io(|| {
        crate::test_support::count_resolve_preview_calls(|| app.enter_preview(&path))
    });
    assert_eq!(calls, 0);
    assert_eq!(io, Default::default());
    assert!(app.media_loading);
    app.apply_media(rx.recv_timeout(Duration::from_secs(10)).unwrap());
    assert!(matches!(&app.tab.preview_kind, Some(PreviewKind::Text(p)) if p == &path));
    assert!(!app.media_loading);
}
