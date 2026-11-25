//! Every overlay that clears its ground (dialogs, bookmark / tab / outline lists, the info
//! popup, the table cell popup, help) drawn over a background of wide characters, as a terminal
//! shows it. A wide character crossing an overlay's edge must not leave half of itself where the
//! border goes (see `konoma_widgets::overlay`). The overlay is drawn by its own render function
//! over wide characters that cover the screen, once with them starting on even columns and once on
//! odd ones, so one of the two always crosses the left edge and the other the right edge.

use super::*;
use konoma_test_support::assert_overlay_over_wide_background as check;

const W: u16 = 60;
const H: u16 = 20;

fn sim(name: &str) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    seed_files(&dir);
    let s = Sim::with_config_sized(&canon(&dir), cfg_en(), W, H);
    (s, dir)
}

#[test]
fn overlay_wide_the_input_dialog() {
    let (mut s, _d) = sim("ow_input");
    s.app.start_create();
    assert!(s.app.is_dialog() && s.app.dialog_is_input());
    check("input dialog", W, H, |f| {
        crate::ui::dialog::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_confirm_dialog() {
    let (mut s, _d) = sim("ow_confirm");
    s.select("notes.txt");
    s.app.start_delete();
    assert!(s.app.dialog_is_confirm());
    check("confirm dialog", W, H, |f| {
        crate::ui::dialog::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_batch_rename_preview() {
    let (mut s, _d) = sim("ow_preview");
    s.app.visual_select_scope(true);
    s.app.start_batch_rename();
    for c in "img_{n}".chars() {
        s.app.dialog_input_push(c);
    }
    s.app.dialog_submit().unwrap();
    assert!(s.app.dialog_is_preview());
    check("batch rename preview", W, H, |f| {
        crate::ui::dialog::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_bookmark_list() {
    let (mut s, _d) = sim("ow_bookmarks");
    s.select("notes.txt");
    s.key('m');
    s.key('a');
    s.key('\'');
    assert!(s.app.is_bookmark_list());
    check("bookmark list", W, H, |f| {
        crate::ui::bookmarks::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_tab_list() {
    let (mut s, _d) = sim("ow_tabs");
    s.select("notes.txt");
    s.key('t');
    s.key('T');
    assert!(s.app.is_tab_list());
    check("tab list", W, H, |f| {
        crate::ui::tab_list::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_outline() {
    let (mut s, _d) = md_preview(
        Config::default(),
        "ow_outline",
        "# Alpha 見出し\n\nintro\n\n## Beta\n\nmiddle\n",
    );
    s.key('O');
    assert!(s.app.is_outline());
    check("outline", W, H, |f| {
        crate::ui::outline::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_info_popup() {
    let (mut s, _d) = sim("ow_info");
    s.select("notes.txt");
    s.key('i');
    assert!(s.app.is_info());
    check("info popup", W, H, |f| {
        crate::ui::info::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_help_panel() {
    let (mut s, _d) = sim("ow_help");
    s.key('?');
    assert!(s.app.show_help);
    check("help panel", W, H, |f| {
        crate::ui::help::render(f, &s.app, f.area())
    });
}

#[test]
fn overlay_wide_the_table_cell_popup() {
    let dir = sandbox("ow_cell");
    std::fs::write(
        dir.join("t.csv"),
        "id,note\n1,a rather long value ワイド文字 that does not fit\n",
    )
    .unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), W, H);
    s.select("t.csv");
    s.enter();
    s.key('j');
    s.key('l');
    s.enter();
    assert!(s.app.is_table_cell_open());
    check("table cell popup", W, H, |f| {
        crate::ui::table::render_cell_popup(f, &mut s.app, f.area())
    });
}
