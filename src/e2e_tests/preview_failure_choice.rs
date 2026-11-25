use super::*;

#[test]
fn e2e_diff_view_failed_preview_open_preserves_preview_choice() {
    let dir = sandbox("failed_preview_choice");
    init_git_repo(&dir);
    let path = dir.join("doc.md");
    std::fs::write(&path, "# Old\n").unwrap();
    run_git(&dir, &["add", "doc.md"]);
    run_git(&dir, &["commit", "-qm", "baseline"]);
    std::fs::write(&path, "# Changed\n").unwrap();
    let mut cfg = cfg_en();
    cfg.ui.diff_view = "rendered".into();
    let mut sim = Sim::with_config(&canon(&dir), cfg);
    sim.select("doc.md");
    sim.key('d');
    assert_eq!(sim.app.diff_view_for_test(), crate::app::DiffView::Rendered);
    // Keep the opening diff's resolved capabilities, then fail the real file opener.
    std::fs::remove_file(&path).unwrap();
    sim.key('R');
    assert!(
        matches!(
            sim.app.tab.preview_kind,
            Some(crate::preview::PreviewKind::CanNotPreview { .. })
        ),
        "the real Preview open failed"
    );
    assert!(sim.app.preview_from_diff_for_test());
    assert_eq!(
        sim.app.diff_view_choice_for_test(),
        crate::app::DiffView::Preview,
        "failed Preview open preserves the user's choice"
    );
}
