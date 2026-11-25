use super::*;

#[test]
fn rendered_diff_without_a_diff_alignment_preserves_the_host_fallback() {
    let dir = konoma_test_support::unique_tmp("rendered_alignment_fallback");
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
    let path = dir.join("doc.md");
    let old_pre = "old paragraph\n";
    let new_pre = "new paragraph\n";
    let source = DecoratedSource::Diff { old_pre, new_pre };
    // Neither non-Diff alignment is allowed to silently omit the changed body.
    for align in [GutterAlign::None, GutterAlign::File { ops: Vec::new() }] {
        let actual = app.build_decorated(&path, 30, &source, &align);
        assert_eq!(actual.pre_src, "new paragraph\n");
        assert!(actual
            .lines
            .iter()
            .any(|l| l.to_string().contains("old paragraph")));
        assert!(actual
            .lines
            .iter()
            .any(|l| l.to_string().contains("new paragraph")));
        assert!(
            !actual.diff_marks.is_empty(),
            "fallback must compute real change marks"
        );
        assert!(actual.images.is_empty());
        assert_eq!(actual.src_lines, 1);
    }
}
