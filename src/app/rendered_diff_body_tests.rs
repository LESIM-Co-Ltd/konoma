//! Literal bridge through the host's actual decorated-diff branch.
use super::*;
use crate::config::Config;

#[path = "rendered_diff_body_fixture.rs"]
mod fixture;

fn host_record(case: &str) -> serde_json::Value {
    let (dir, outcome) = fixture::outcome(case);
    use konoma_plugin::document_pictures::{
        clear_test_cache_root, get_test_cache_root, set_test_cache_root,
    };
    struct RestoreCache(Option<PathBuf>);
    impl Drop for RestoreCache {
        fn drop(&mut self) {
            match self.0.take() {
                Some(path) => set_test_cache_root(path),
                None => clear_test_cache_root(),
            }
        }
    }
    let _cache = RestoreCache(get_test_cache_root());
    set_test_cache_root(dir.join("remote-cache"));
    let path = dir.join("doc.md");
    let mut cfg = Config::default();
    cfg.ui.lang = "en".into();
    cfg.ui.md_details = "closed".into();
    let mut app = App::new(dir.to_path_buf(), cfg).unwrap();
    app.tab.preview_path = Some(path.clone());
    app.tab.preview_kind = Some(PreviewKind::GitDiff(path.clone()));
    app.tab.diff_view = DiffView::Rendered;
    app.picker = Some(ratatui_image::picker::Picker::halfblocks());
    app.details_open = [(0, false), (1, true)].into();
    let (old_pre, new_pre, _) = fixture::ready(&outcome);
    let mut keys = Vec::new();
    for src in [old_pre, new_pre] {
        for code in crate::preview::markdown::collect_mermaid_fences(src) {
            keys.push((crate::preview::markdown::mermaid_fence_url(&code), (77, 38)));
        }
        for (latex, display) in crate::preview::markdown::collect_math_exprs(src) {
            keys.push((
                crate::preview::markdown::math_url(&latex, display),
                (80, 40),
            ));
        }
    }
    for (key, dims) in keys {
        app.md_images.cache.insert(
            PathBuf::from(key),
            MdImgEntry {
                layout_px: Some(dims),
                ..Default::default()
            },
        );
    }
    let source = DecoratedSource::Diff { old_pre, new_pre };
    let align = App::compute_gutter_align(&source, Some(&outcome));
    let decorated = app.build_decorated(&path, fixture::WIDTH, &source, &align);
    let lines = decorated.lines;
    assert_eq!(app.doc_options(), fixture::options());
    fixture::record(
        &lines,
        &decorated.images,
        &decorated.diff_marks,
        &decorated.remote_urls,
        &decorated.mermaid_fences,
        &decorated.math_exprs,
        decorated.src_lines,
        konoma_plugin::document_render::wrap_row_prefix(&lines, fixture::WIDTH),
    )
}

#[test]
fn rendered_diff_body_host_bridge_checks_independent_literals() {
    for case in ["resources", "details-cap"] {
        fixture::assert_reference(case, host_record(case));
    }
}
