//! HEAD's first matching glob does not ask for content; format suffixes do not need it either.
use super::*;
use crate::config::Rule;
use konoma_plugin::host::file_preview::test_hooks::count_path_io;

#[test]
fn extension_decided_previews_and_selected_plugins_do_no_path_io() {
    let dir = crate::test_support::unique_tmp("preview_suffix_io");
    std::fs::create_dir_all(&dir).unwrap();
    // HEAD: user rules precede the image MIME default; default SVG precedes that MIME default.
    // plugin_id_of keeps HEAD's availability gate; the shared decision adds no metadata or open.
    for (name, builtin, target) in [
        (
            "image.png",
            Some("image"),
            konoma_host_core::project::DiffTarget::Picture,
        ),
        (
            "document.pdf",
            Some("pdf"),
            konoma_host_core::project::DiffTarget::Picture,
        ),
        (
            "drawing.svg",
            None,
            konoma_host_core::project::DiffTarget::Svg,
        ),
    ] {
        let rules: Vec<_> = builtin
            .map(|id| Rule {
                glob: Some("*".into()),
                builtin: Some(id.into()),
                ..Default::default()
            })
            .into_iter()
            .collect();
        let path = dir.join(name);
        std::fs::write(&path, b"unused").unwrap();
        let (id, io) = count_path_io(|| plugin_id_of(&rules, &[], &path));
        assert!(id.is_some(), "{name}: a glob selects the plugin");
        assert_eq!(
            io,
            Default::default(),
            "{name}: selected plugin performs no path I/O"
        );
        let (resolved, io) = count_path_io(|| resolve_file(&rules, &[], true, &path, None));
        assert_eq!(
            resolved.diff_target, target,
            "{name}: suffix decides diff target"
        );
        assert_eq!(
            io,
            Default::default(),
            "{name}: resolved preview performs no path I/O"
        );
    }
}

#[test]
fn selecting_a_plugin_does_not_classify_its_diff_format() {
    let dir = crate::test_support::unique_tmp("selected_plugin_no_diff_io");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("no-format-suffix");
    std::fs::write(&path, b"unused").unwrap();
    // HEAD plugin_id_of returns the registry selection, without asking what the preview can draw.
    let rules = [Rule {
        glob: Some("*".into()),
        builtin: Some("image".into()),
        ..Default::default()
    }];
    let (id, io) = count_path_io(|| plugin_id_of(&rules, &[], &path));
    assert_eq!(id.as_deref(), Some("image"));
    assert_eq!(
        io,
        Default::default(),
        "selected plugin must not sniff a diff format"
    );
}

#[test]
fn mime_text_and_diff_share_one_prefix_and_baseline_bytes_need_no_path_io() {
    use konoma_plugin::host::file_preview::test_hooks::PathIo;
    let dir = crate::test_support::unique_tmp("shared_preview_prefix");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unknown");
    std::fs::write(&path, "ordinary text").unwrap();
    let (resolved, io) = count_path_io(|| resolve_file(&[], &[], true, &path, None));
    assert_eq!(
        resolved.plan,
        konoma_plugin::host::file_preview::FilePreviewPlan::Text
    );
    assert_eq!(
        io,
        PathIo {
            metadata: 1,
            opens: 1,
            reads: 1
        }
    );
    let (resolved, io) = count_path_io(|| {
        resolve_file(&[], &[], true, &dir.join("deleted"), Some(b"ordinary text"))
    });
    assert_eq!(
        resolved.plan,
        konoma_plugin::host::file_preview::FilePreviewPlan::Text
    );
    assert_eq!(
        io,
        Default::default(),
        "baseline bytes replace all path reads"
    );
}
