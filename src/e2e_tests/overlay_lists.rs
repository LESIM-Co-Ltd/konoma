//! `l` / `h` and `/` on the overlay lists (plugin stage 2 / B4, design §10-3-3 layer 1): on a list
//! `l` opens (`Enter`'s operation) and `h` goes back (`q`), and `/` filters the rows. The overlay
//! lists are the heading outline (and the page list, which is the same overlay), the tab list and
//! the bookmark list. The bookmark list keeps every plain letter as a bookmark name, so `h` and
//! `l` jump there as they always did and only `/` is added. Driven through the real `handle_key`.

use super::*;
use crate::app::list_filter::ListId;
use crate::keymap::{Action, Context, KeyPress, LeaderId, Resolution, Surface};

const OUTLINE_BODY: &str =
    "# Alpha Heading\n\nintro\n\n## Beta Heading\n\nmiddle\n\n## Gamma Heading\n\nend\n";

// ---------------------------------------------------------------------------------------------
// `l` = open, `h` = back
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_overlay_outline_l_goes_to_the_heading_and_h_goes_back() {
    let mut body = String::from("# Top Heading\n\nintro\n\n");
    for i in 0..40 {
        body.push_str(&format!("filler line {i}\n\n"));
    }
    body.push_str("## Deep Section\n\ndeep body text\n");
    let (mut s, _dir) = md_preview(Config::default(), "ov_outline_l", &body);
    s.key('O');
    assert!(s.app.is_outline());
    assert_eq!(s.app.key_context(), Some(Context::Outline));
    // `h` is `q`: it closes the list and the preview stays where it was.
    s.key('h');
    assert!(!s.app.is_outline(), "h closes the outline");
    assert_eq!(s.app.tab.preview_scroll, 0);
    assert_eq!(s.app.tab.mode, Mode::Preview, "h only closed the list");
    // `l` is Enter: it goes to the selected heading and closes the list.
    s.key('O');
    s.key('j');
    s.key('l');
    assert!(!s.app.is_outline(), "l closes the outline");
    assert!(s.app.tab.preview_scroll > 0, "l jumped to the deep heading");
    s.see("Deep Section");
    // The same two operations as the fixed key and `q`.
    assert_eq!(s.app.last_dispatched, Some(Action::OutlineJump));
}

#[test]
fn e2e_overlay_page_list_l_goes_to_the_page_and_h_goes_back() {
    let Some(pdf) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let dir = sandbox("ov_pages_l");
    std::fs::copy(&pdf, dir.join("doc.pdf")).unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("doc.pdf");
    s.enter();
    s.drain_media();
    assert_eq!(s.app.page_indicator(), Some((1, 3)));
    s.key('O');
    assert!(s.app.is_outline());
    s.key('h');
    assert!(!s.app.is_outline(), "h closes the page list");
    assert_eq!(s.app.page_indicator(), Some((1, 3)), "h moved nowhere");
    s.key('O');
    s.key('j');
    s.key('l');
    assert!(!s.app.is_outline(), "l closes the page list");
    s.drain_media();
    assert_eq!(s.app.page_indicator(), Some((2, 3)), "l went to page 2");
}

#[test]
fn e2e_overlay_tab_list_l_switches_and_h_goes_back() {
    let dir = sandbox("ov_tabs_l");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    s.key('t');
    assert_eq!(s.app.active_tab_index(), 1);
    s.key('T');
    assert!(s.app.is_tab_list());
    assert_eq!(s.app.key_context(), Some(Context::Tabs));
    // `h` is `q`: the list closes and the tab stays.
    s.key('h');
    assert!(!s.app.is_tab_list(), "h closes the tab list");
    assert_eq!(s.app.active_tab_index(), 1);
    // `l` is Enter: switch to the selected tab.
    s.key('T');
    s.key('k');
    assert_eq!(s.app.tab_list_sel(), 0);
    s.key('l');
    assert!(!s.app.is_tab_list(), "l closes the tab list");
    assert_eq!(s.app.active_tab_index(), 0, "l switched to the first tab");
    assert_eq!(s.app.last_dispatched, Some(Action::TabListActivate));
}

/// The bookmark list is the one list that keeps `h` and `l` out of layer 1: every plain letter is
/// the name of a bookmark and jumps there, `h` and `l` included.
#[test]
fn e2e_overlay_bookmark_list_keeps_h_and_l_as_bookmark_names() {
    let dir = sandbox("ov_bm_hl");
    seed_files(&dir);
    let root = canon(&dir);
    // Bookmarks named `h` (a file) and `l` (a directory).
    let mut s = Sim::new(&root);
    s.select("notes.txt");
    s.key('m');
    s.key('h');
    s.select("src");
    s.key('m');
    s.key('l');
    // Neither letter is bound in the list, so the lookup falls through to the bookmark jump.
    let m = &s.app.keymaps;
    for c in ['h', 'l'] {
        assert_eq!(
            m.resolve(Context::Bookmarks, None, KeyPress::ch(c)),
            Resolution::Unbound,
            "{c} is a bookmark name here, not back / open"
        );
    }
    s.key('\'');
    assert!(s.app.is_bookmark_list());
    s.key('h');
    assert!(!s.app.is_bookmark_list(), "h jumped to its bookmark");
    assert_eq!(
        s.app.tab.mode,
        Mode::Preview,
        "h opened the bookmarked file"
    );
    assert!(s
        .app
        .tab
        .preview_path
        .as_ref()
        .is_some_and(|p| p.ends_with("notes.txt")));
    s.key('q');
    s.key('\'');
    s.key('l');
    assert!(!s.app.is_bookmark_list(), "l jumped to its bookmark");
    assert!(s.app.tab.root.ends_with("src"), "l went into the directory");
}

// ---------------------------------------------------------------------------------------------
// `/` filters the rows
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_overlay_outline_slash_filters_the_headings() {
    let (mut s, _dir) = md_preview(Config::default(), "ov_outline_f", OUTLINE_BODY);
    s.key('O');
    s.see("Alpha Heading");
    s.see("Beta Heading");
    s.see("Gamma Heading");
    // `/` starts the query: keys are text now, not commands.
    s.key('/');
    assert_eq!(s.app.surface(), Surface::ListFilter);
    assert_eq!(s.app.key_context(), None);
    assert_eq!(s.app.filtering_list(), Some(ListId::Outline));
    s.keys("gam");
    s.see("Outline /gam");
    // The rows of the list itself (the document behind it still has all its headings).
    s.see("• Gamma Heading");
    s.dont_see("• Alpha Heading");
    s.dont_see("• Beta Heading");
    assert_eq!(s.app.list_visible(ListId::Outline), Some(vec![2]));
    assert_eq!(
        s.app.outline_sel(),
        2,
        "the cursor moved to the row that is still shown"
    );
    // `Enter` keeps the query and goes back to moving.
    s.enter();
    assert_eq!(s.app.surface(), Surface::Outline);
    assert_eq!(s.app.list_filter_query(ListId::Outline), "gam");
    s.dont_see("• Alpha Heading");
    // Moving stays among the rows that are shown.
    s.key('j');
    assert_eq!(s.app.outline_sel(), 2);
    s.key('k');
    assert_eq!(s.app.outline_sel(), 2);
    // `Esc` clears the query first, a second one closes the list.
    s.esc();
    assert!(s.app.is_outline(), "the first Esc only cleared the query");
    assert_eq!(s.app.list_filter_query(ListId::Outline), "");
    s.see("• Alpha Heading");
    s.esc();
    assert!(!s.app.is_outline());
}

#[test]
fn e2e_overlay_outline_a_query_that_hides_everything_leaves_nothing_to_jump_to() {
    let (mut s, _dir) = md_preview(Config::default(), "ov_outline_none", OUTLINE_BODY);
    s.key('O');
    s.key('/');
    s.keys("zzzz");
    s.enter();
    assert_eq!(s.app.list_visible(ListId::Outline), Some(Vec::new()));
    let scroll = s.app.tab.preview_scroll;
    s.enter();
    assert!(s.app.is_outline(), "Enter has no row to go to");
    s.key('l');
    assert!(s.app.is_outline(), "nor does l");
    assert_eq!(s.app.tab.preview_scroll, scroll);
    // Backspace through the query shows the rows again.
    s.key('/');
    s.press(KeyCode::Backspace, KeyModifiers::NONE);
    s.press(KeyCode::Backspace, KeyModifiers::NONE);
    s.press(KeyCode::Backspace, KeyModifiers::NONE);
    s.press(KeyCode::Backspace, KeyModifiers::NONE);
    s.see("• Alpha Heading");
}

#[test]
fn e2e_overlay_outline_the_query_is_forgotten_when_the_list_opens_again() {
    let (mut s, _dir) = md_preview(Config::default(), "ov_outline_reset", OUTLINE_BODY);
    s.key('O');
    s.key('/');
    s.keys("beta");
    s.enter();
    s.key('q');
    assert!(!s.app.is_outline());
    s.key('O');
    assert_eq!(s.app.list_filter_query(ListId::Outline), "");
    s.see("• Alpha Heading");
    s.see("• Gamma Heading");
}

#[test]
fn e2e_overlay_outline_typing_filters_only_with_the_list_open() {
    // While the query is typed every character is text, not a command: `q`, `j`, `O`, `T`.
    let (mut s, _dir) = md_preview(Config::default(), "ov_outline_text", OUTLINE_BODY);
    s.key('O');
    s.key('/');
    s.keys("qjOT");
    assert!(s.app.is_outline(), "the letters were typed, not run");
    assert!(!s.app.is_tab_list());
    assert_eq!(s.app.list_filter_query(ListId::Outline), "qjOT");
    s.esc();
    assert_eq!(s.app.list_filter_query(ListId::Outline), "");
    assert!(s.app.is_outline(), "Esc ended the input; the list stays");
    assert_eq!(s.app.surface(), Surface::Outline);
}

#[test]
fn e2e_overlay_tab_list_slash_filters_the_tabs() {
    let dir = sandbox("ov_tabs_f");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    s.key('t');
    s.select("readme.md");
    s.enter();
    s.key('t');
    assert_eq!(s.app.tab_count(), 3);
    s.key('T');
    // The rows of the list itself carry the root after the name (the tab bar behind does not).
    s.see("notes.txt  —");
    s.see("readme.md  —");
    s.key('/');
    assert_eq!(s.app.surface(), Surface::ListFilter);
    assert_eq!(s.app.filtering_list(), Some(ListId::Tabs));
    s.keys("readme");
    s.see("/readme");
    assert_eq!(s.app.list_visible(ListId::Tabs), Some(vec![1]));
    assert_eq!(s.app.tab_list_sel(), 1);
    s.see("readme.md  —");
    s.dont_see("notes.txt  —");
    s.enter();
    assert_eq!(s.app.surface(), Surface::Tabs);
    // `l` switches to the one tab that is shown, whatever else is selected.
    s.key('j');
    assert_eq!(s.app.tab_list_sel(), 1, "j stays among the shown rows");
    s.key('l');
    assert!(!s.app.is_tab_list());
    assert_eq!(s.app.active_tab_index(), 1);
}

#[test]
fn e2e_overlay_tab_list_a_query_that_hides_the_tab_closes_and_switches_nothing() {
    let dir = sandbox("ov_tabs_none");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.enter();
    s.key('t');
    s.key('T');
    s.key('/');
    s.keys("nosuchtab");
    s.enter();
    assert_eq!(s.app.list_visible(ListId::Tabs), Some(Vec::new()));
    let before = (s.app.tab_count(), s.app.active_tab_index());
    s.enter();
    assert!(s.app.is_tab_list(), "no row to switch to");
    s.key('d');
    assert_eq!(
        (s.app.tab_count(), s.app.active_tab_index()),
        before,
        "d has no row to close"
    );
    s.esc();
    assert!(s.app.is_tab_list(), "Esc cleared the query");
    s.esc();
    assert!(!s.app.is_tab_list());
}

#[test]
fn e2e_overlay_bookmark_list_slash_filters_and_letters_are_input_while_typing() {
    let dir = sandbox("ov_bm_f");
    seed_files(&dir);
    let root = canon(&dir);
    let mut s = Sim::new(&root);
    s.select("notes.txt");
    s.key('m');
    s.key('a');
    s.select("src");
    s.key('m');
    s.key('b');
    s.key('\'');
    assert!(s.app.is_bookmark_list());
    s.see("a  konoma_e2e_ov_bm_f");
    s.see("b  konoma_e2e_ov_bm_f");
    s.key('/');
    assert_eq!(s.app.surface(), Surface::ListFilter);
    assert_eq!(s.app.filtering_list(), Some(ListId::Bookmarks));
    // `a` and `b` are bookmark names, but while the query is typed they are text.
    s.keys("note");
    assert!(s.app.is_bookmark_list(), "typing did not jump");
    assert_eq!(s.app.list_filter_query(ListId::Bookmarks), "note");
    assert_eq!(s.app.list_visible(ListId::Bookmarks), Some(vec![0]));
    s.see("a  konoma_e2e_ov_bm_f");
    s.dont_see("b  konoma_e2e_ov_bm_f");
    s.enter();
    // Back to moving: `Enter` jumps to the one bookmark that is shown.
    assert_eq!(s.app.surface(), Surface::Bookmarks);
    s.enter();
    assert!(!s.app.is_bookmark_list());
    assert!(s
        .app
        .tab
        .preview_path
        .as_ref()
        .is_some_and(|p| p.ends_with("notes.txt")));
}

#[test]
fn e2e_overlay_bookmark_list_a_query_that_hides_the_row_protects_it_from_delete_and_edit() {
    let dir = sandbox("ov_bm_hidden");
    seed_files(&dir);
    let mut s = Sim::new(&canon(&dir));
    s.select("notes.txt");
    s.key('m');
    s.key('a');
    s.key('\'');
    s.key('/');
    s.keys("zzz");
    s.enter();
    assert_eq!(s.app.list_visible(ListId::Bookmarks), Some(Vec::new()));
    // `Space d` deletes "the selected bookmark": it is not shown, so nothing is deleted.
    s.key(' ');
    s.key('d');
    assert!(
        s.app.bookmarks.get('a').is_some(),
        "the hidden bookmark survived"
    );
    s.enter();
    assert!(s.app.is_bookmark_list(), "and Enter has no row to go to");
}

// ---------------------------------------------------------------------------------------------
// `open_sort_menu` is another name for the `,` menu
// ---------------------------------------------------------------------------------------------

#[test]
fn e2e_open_sort_menu_opens_the_options_menu_of_the_context() {
    let dir = sandbox("ov_sort_alias");
    seed_files(&dir);
    let keys: &[(&str, &[(&str, &str)])] = &[
        ("tree", &[("s", "open_sort_menu")]),
        ("text", &[("s", "open_sort_menu")]),
        ("tabs", &[("s", "open_sort_menu")]),
    ];
    let mut s = Sim::with_keys(&canon(&dir), keys);
    // In the tree it is the `,` menu with the sort letters.
    s.key('s');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Options));
    s.see("Sort & display");
    s.esc();
    assert_eq!(s.app.pending_leader, None);
    // In a text preview it is that preview's `,` menu (no sort letters there).
    s.select("notes.txt");
    s.enter();
    s.key('s');
    assert_eq!(s.app.pending_leader, Some(LeaderId::Options));
    let it = crate::ops::menu::menu(
        &s.app,
        Context::Text,
        LeaderId::Options,
        crate::ops::menu::View::Open,
    )
    .unwrap();
    assert!(
        it.items
            .iter()
            .all(|i| !matches!(i.action, Action::SortSet(_))),
        "{it:?}"
    );
    s.esc();
    // Where the context has no `,` menu there is nothing to open.
    s.key('T');
    assert_eq!(s.app.key_context(), Some(Context::Tabs));
    s.key('s');
    assert_eq!(s.app.pending_leader, None, "the tab list has no `,` menu");
    assert!(s.app.is_tab_list(), "and the key did nothing else");
}
