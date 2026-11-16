//! Leaving a project screen for the tree, in Japanese, as the real key sequence and the real
//! frames: the terminal must show exactly the tree frame that was drawn.

use super::vcs_changes::{drain, git_repo, plugin_config};
use super::*;

/// The cells the terminal shows after `p` and then `q`, against the tree frame that was drawn.
/// The project frame's top edge carries wide screen tabs; `Paragraph` leaves the `─` behind each
/// wide character in the buffer, so without the end-of-frame blanking the diff to the tree frame
/// (whose edge is unbroken) writes nothing over those cells and the terminal keeps a gap.
#[test]
fn e2e_p_then_q_in_japanese_leaves_the_tree_top_edge_unbroken() {
    use unicode_width::UnicodeWidthStr;
    let dir = git_repo("project_wide_gap_back_to_tree", true);
    let cfg = plugin_config("git", "project_wide_gap_back_to_tree", "ja");
    let (w, h) = (100, 24);
    let mut sim = Sim::with_config_sized(&dir, cfg, w, h);
    sim.app.tab.projects.wait_for_result();
    sim.app.poll_projects();
    let frame = |sim: &mut Sim| {
        let app = &mut sim.app;
        sim.term
            .draw(|f| ui::render(f, app))
            .unwrap()
            .buffer
            .clone()
    };
    let mut model = crate::test_support::TermModel::new(w, h);
    let tree_before = frame(&mut sim);
    model.apply(
        &ratatui::buffer::Buffer::empty(tree_before.area),
        &tree_before,
    );

    sim.key('p');
    drain(&mut sim);
    assert!(sim.app.project_active(), "{:?}", sim.app.flash);
    let project = frame(&mut sim);
    assert!(
        project.content.iter().any(|c| c.symbol().width() > 1),
        "the project screen draws a wide character"
    );
    model.apply(&tree_before, &project);

    sim.key('q');
    drain(&mut sim);
    assert!(!sim.app.project_active(), "q leaves the project screen");
    assert!(matches!(sim.app.tab.mode, Mode::Tree));
    let tree = frame(&mut sim);
    model.apply(&project, &tree);

    for y in 0..h {
        let mut expected = String::new();
        let mut x = 0;
        while x < w {
            let symbol = tree[(x, y)].symbol();
            expected.push_str(symbol);
            x += symbol.width().max(1) as u16;
        }
        assert_eq!(
            model.row(y),
            expected,
            "row {y}: the terminal differs from the tree frame it was drawn from (a gap behind a wide character)"
        );
    }
}
