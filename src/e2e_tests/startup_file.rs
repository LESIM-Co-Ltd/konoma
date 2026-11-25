//! `konoma <file>`: the file's directory is the tree, the file is selected and previewed at once,
//! and `q` goes back to the tree. With saved tabs (`[ui] restore_tabs`) the tabs come back first
//! and the file opens in the tab that is on screen.

use super::*;

/// The preview is on screen right after start, the tree behind it has the file selected, and `q`
/// leaves for that tree (the ordinary preview, not a mode of its own).
#[test]
fn e2e_konoma_file_starts_in_the_preview_and_q_goes_back_to_the_tree() {
    let dir = sandbox("startup_file_basic");
    std::fs::write(dir.join("a.txt"), "alpha body\n").unwrap();
    std::fs::write(dir.join("b.txt"), "beta body\n").unwrap();
    let mut s = Sim::new(&dir);
    s.app.open_startup_file(&dir.join("b.txt"));
    s.draw();
    assert!(matches!(s.app.tab.mode, Mode::Preview));
    s.see("beta body");
    s.key('q');
    assert!(matches!(s.app.tab.mode, Mode::Tree));
    assert!(
        s.app.tab.entries[s.app.tab.selected]
            .path
            .ends_with("b.txt"),
        "the tree has the file selected"
    );
    s.see("a.txt");
}

/// A Markdown file starts as the rendered document, like one opened with Enter.
#[test]
fn e2e_konoma_file_opens_markdown_as_the_rendered_document() {
    let dir = sandbox("startup_file_md");
    std::fs::write(dir.join("doc.md"), "# Heading One\n\nsome **bold** words\n").unwrap();
    let mut s = Sim::new(&dir);
    s.app.open_startup_file(&dir.join("doc.md"));
    s.draw();
    assert!(matches!(s.app.tab.mode, Mode::Preview));
    s.see("Heading One");
    assert!(
        !s.screen().contains("# Heading One"),
        "rendered, not the source:\n{}",
        s.screen()
    );
}

/// A file below the root is revealed in the tree (its folders open) and previewed.
#[test]
fn e2e_konoma_file_below_the_root_reveals_it_in_the_tree() {
    let dir = sandbox("startup_file_deep");
    std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
    std::fs::write(dir.join("sub/deeper/c.txt"), "gamma body\n").unwrap();
    let mut s = Sim::new(&dir);
    s.app.open_startup_file(&dir.join("sub/deeper/c.txt"));
    s.draw();
    s.see("gamma body");
    s.key('q');
    assert!(s.app.tab.entries[s.app.tab.selected]
        .path
        .ends_with("c.txt"));
}

/// What the footer says at start (a config error, a warning) survives: opening the file is not a
/// jump the user made, so it does not replace the startup message with "→ path".
#[test]
fn e2e_konoma_file_keeps_the_startup_message() {
    let dir = sandbox("startup_file_flash");
    std::fs::write(dir.join("a.txt"), "alpha body\n").unwrap();
    let mut s = Sim::new(&dir);
    s.app.flash = Some("config error (using defaults): demo".into());
    s.app.open_startup_file(&dir.join("a.txt"));
    assert_eq!(
        s.app.flash.as_deref(),
        Some("config error (using defaults): demo")
    );
}

/// With a saved session the tabs are restored first, and the file opens in the tab that is on
/// screen: the tab count and the active tab do not change.
#[test]
fn e2e_konoma_file_with_a_saved_session_opens_in_the_current_tab() {
    let dir = sandbox("startup_file_session");
    std::fs::write(dir.join("a.txt"), "alpha body\n").unwrap();
    std::fs::write(dir.join("b.txt"), "beta body\n").unwrap();
    std::fs::write(dir.join("c.txt"), "gamma body\n").unwrap();
    let base = unique_tmp("konoma_e2e_startup_file_session_base");
    let _ = std::fs::remove_dir_all(&base);

    // Last time: a.txt previewed in tab 1, a second tab (tree, cursor on b.txt).
    let mut s = Sim::new(&dir);
    s.app
        .attach_session_store(crate::session::SessionStore::with_base(
            base.to_path_buf(),
            &dir,
        ));
    s.select("a.txt");
    s.press(KeyCode::Enter, KeyModifiers::NONE);
    s.key('t');
    s.select("b.txt");
    s.app.save_session();
    drop(s);

    // This time: `konoma c.txt` (the start directory is the file's directory).
    let mut s2 = Sim::new(&dir);
    s2.app
        .attach_session_store(crate::session::SessionStore::with_base(
            base.to_path_buf(),
            &dir,
        ));
    s2.app.restore_session();
    assert_eq!(s2.app.tab_count(), 2, "the saved tabs are back");
    assert_eq!(s2.app.active_tab_index(), 1);
    s2.app.open_startup_file(&dir.join("c.txt"));
    s2.draw();
    assert_eq!(s2.app.tab_count(), 2, "no tab was added");
    assert_eq!(
        s2.app.active_tab_index(),
        1,
        "the tab on screen took the file"
    );
    assert!(matches!(s2.app.tab.mode, Mode::Preview));
    s2.see("gamma body");
    // The other tab is as it was left: a.txt's preview.
    s2.key('q');
    s2.key('[');
    assert!(matches!(s2.app.tab.mode, Mode::Preview));
    s2.see("alpha body");
}
