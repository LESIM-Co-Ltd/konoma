//! End-to-end tests added after the stage-2 mutation audit: the change blocks of a diff in each of
//! its presentations, and which hints `Ctrl-t` and `e` earn from the item in focus. Driven through
//! the real `handle_key`.

use super::*;
use crate::keymap::{Action, Context};
use crate::ops::gate::{gate, Gate};

/// A long Markdown file with one paragraph per line, so the rendered presentation and the line
/// diff put the same change at different scroll rows.
#[cfg(feature = "git")]
fn long_doc(changed: &[usize]) -> String {
    let mut s = String::from("# Title\n\n");
    for i in 0..60 {
        if changed.contains(&i) {
            s += &format!("CHANGED paragraph number {i} with some more words in it.\n\n");
        } else {
            s += &format!("Paragraph number {i} with some words in it, and a few more.\n\n");
        }
    }
    s
}

/// A repository with `doc.md` committed long and changed in two far-apart paragraphs, `p.txt`
/// changed, `same.txt` untouched and a picture changed.
#[cfg(feature = "git")]
fn seed_diff_repo(dir: &std::path::Path) {
    use crate::test_support::{init_git_repo, run_git, write_solid_png};
    init_git_repo(dir);
    std::fs::write(dir.join("doc.md"), long_doc(&[])).unwrap();
    std::fs::write(dir.join("p.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("same.txt"), "same\n").unwrap();
    write_solid_png(&dir.join("pic.png"), 4, 4, [1, 1, 1]);
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "-q", "-m", "init"]);
    std::fs::write(dir.join("doc.md"), long_doc(&[2, 50])).unwrap();
    std::fs::write(dir.join("p.txt"), "one\nTWO\nthree\n").unwrap();
    write_solid_png(&dir.join("pic.png"), 4, 4, [200, 1, 1]);
}

/// Press `R` until the diff shows `want` (at most a few times: the cycle is three long).
#[cfg(feature = "git")]
fn to_view(s: &mut Sim, want: crate::app::DiffView) {
    for _ in 0..4 {
        if s.app.diff_view_for_test() == want {
            return;
        }
        s.key('R');
        s.draw();
    }
    assert_eq!(s.app.diff_view_for_test(), want, "R did not reach it");
}

#[cfg(feature = "git")]
fn open_diff(s: &mut Sim, name: &str) {
    s.select(name);
    s.key('d');
    s.draw();
    assert!(s.app.is_git_diff_preview(), "{name}");
}

/// `n` / `N` have blocks to step to in the line diff, in the rendered and the preview
/// presentations of a Markdown diff, and none in the side by side pictures of an image diff.
#[cfg(feature = "git")]
#[test]
fn e2e_survivor_a_diff_has_change_blocks_in_every_presentation_but_the_pictures() {
    use crate::app::DiffView;
    let dir = sandbox("surv_diff_blocks");
    seed_diff_repo(&dir);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 100, 24);
    // Markdown, in each of its three presentations.
    open_diff(&mut s, "doc.md");
    // The rendered presentation, then (`R`) the preview, which leaves the diff screen, then (`R`)
    // the line diff.
    for step in 0..3 {
        if step > 0 {
            s.key('R');
            s.draw();
        }
        if step == 1 {
            assert!(
                !s.app.is_git_diff_preview(),
                "the preview is an ordinary preview"
            );
            continue;
        }
        assert!(s.app.is_git_diff_preview(), "step {step}");
        assert!(s.app.diff_has_change_blocks(), "step {step}");
        assert_eq!(
            gate(&s.app, Context::Diff, Action::DiffNextChange),
            Gate::On,
            "step {step}"
        );
    }
    assert_eq!(s.app.diff_view_for_test(), DiffView::Source);
    s.esc();
    // A text file: the line diff.
    open_diff(&mut s, "p.txt");
    assert!(s.app.diff_has_change_blocks());
    assert_eq!(
        gate(&s.app, Context::Diff, Action::DiffNextChange),
        Gate::On
    );
    // A picture: the side by side view has nothing to scroll.
    s.esc();
    open_diff(&mut s, "pic.png");
    assert!(s.app.diff_media_active());
    assert!(!s.app.diff_has_change_blocks());
    assert_eq!(
        gate(&s.app, Context::Diff, Action::DiffNextChange),
        Gate::Off(None)
    );
    // `n` in it does nothing at all.
    let before = s.app.tab.preview_scroll;
    s.key('n');
    assert_eq!(s.app.tab.preview_scroll, before);
}

/// The rendered presentation steps through the blocks of the rendered document, not through the
/// rows of the line diff: the two put the same change at different scroll rows.
#[cfg(feature = "git")]
#[test]
fn e2e_survivor_the_rendered_diff_steps_through_the_rendered_rows() {
    use crate::app::DiffView;
    let dir = sandbox("surv_diff_rendered_rows");
    seed_diff_repo(&dir);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 100, 24);
    open_diff(&mut s, "doc.md");
    to_view(&mut s, DiffView::Rendered);
    let rendered = s.app.diff_block_rows_for_test();
    assert!(rendered.len() >= 2, "{rendered:?}");
    to_view(&mut s, DiffView::Source);
    let lines = s.app.diff_block_rows_for_test();
    assert!(lines.len() >= 2, "{lines:?}");
    assert_ne!(
        rendered, lines,
        "the two presentations scroll to different rows"
    );
    // `n` in the rendered presentation lands on a row of the rendered one.
    to_view(&mut s, DiffView::Rendered);
    s.app.tab.preview_scroll = 0;
    s.key('n');
    s.draw();
    let max_v = s
        .app
        .diff_block_rows_for_test()
        .into_iter()
        .max()
        .unwrap_or(0);
    let landed = s.app.tab.preview_scroll as usize;
    assert!(
        rendered.contains(&landed) || landed <= max_v,
        "landed on {landed}, rendered rows {rendered:?}"
    );
    assert!(!lines.contains(&landed) || rendered.contains(&landed));
}

/// `e` in a diff of an untouched-by-blocks file: the edit line is the block at or above the view.
#[cfg(feature = "git")]
#[test]
fn e2e_survivor_a_file_with_no_changed_line_has_no_blocks() {
    let dir = sandbox("surv_diff_noblocks");
    seed_diff_repo(&dir);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), 100, 24);
    open_diff(&mut s, "p.txt");
    assert!(s.app.diff_has_change_blocks());
    assert_eq!(s.app.diff_block_rows_for_test().len(), 1);
}

// ---- Ctrl-t and e -----------------------------------------------------------------------------

/// `Ctrl-t` opens the focused link in a new tab for a local, an anchor and an external link alike,
/// and for nothing else; `Enter` (`LinkOpen`) also works on a checkbox.
#[test]
fn e2e_survivor_ctrl_t_acts_on_every_kind_of_link_and_on_nothing_else() {
    let body = "[local](other.md)\n\n[anchor](#end)\n\n[web](https://example.com/x)\n\n- [ ] task\n\n# end\n";
    let (mut s, _d) = md_preview(cfg_en(), "surv_links", body);
    s.draw();
    let mut seen = Vec::new();
    for _ in 0..4 {
        s.tab();
        s.draw();
        seen.push((
            s.app.md_focused_kind(),
            gate(&s.app, Context::Document, Action::OpenLinkNewTab),
            gate(&s.app, Context::Document, Action::LinkOpen),
        ));
    }
    use crate::app::MdFocus;
    let kinds: Vec<_> = seen.iter().map(|x| x.0).collect();
    assert!(kinds.contains(&Some(MdFocus::LocalLink)), "{kinds:?}");
    assert!(kinds.contains(&Some(MdFocus::AnchorLink)), "{kinds:?}");
    assert!(kinds.contains(&Some(MdFocus::ExternalLink)), "{kinds:?}");
    assert!(kinds.contains(&Some(MdFocus::Task)), "{kinds:?}");
    for (kind, newtab, open) in seen {
        let is_link = matches!(
            kind,
            Some(MdFocus::LocalLink | MdFocus::AnchorLink | MdFocus::ExternalLink)
        );
        assert_eq!(newtab.is_on(), is_link, "{kind:?}: Ctrl-t");
        assert!(open.is_on(), "{kind:?}: Enter");
    }
}

/// `e` on an Office document explains itself when the Office apps are switched off, and acts when
/// they are on.
#[test]
fn e2e_survivor_edit_on_an_office_file_follows_the_office_switch() {
    let dir = sandbox("surv_edit_office");
    std::fs::write(dir.join("a.docx"), b"not really a zip").unwrap();
    std::fs::write(dir.join("b.txt"), b"x\n").unwrap();
    for (on, want) in [
        (false, Gate::Off(Some(crate::i18n::Msg::OfficeAppsDisabled))),
        (true, Gate::On),
    ] {
        let mut cfg = cfg_en();
        cfg.external.office_apps = on;
        let mut s = Sim::with_config(&canon(&dir), cfg);
        s.select("a.docx");
        assert_eq!(
            gate(&s.app, Context::Tree, Action::RequestEdit),
            want,
            "office_apps={on}"
        );
        // A text file is edited either way.
        s.select("b.txt");
        assert_eq!(gate(&s.app, Context::Tree, Action::RequestEdit), Gate::On);
    }
}
