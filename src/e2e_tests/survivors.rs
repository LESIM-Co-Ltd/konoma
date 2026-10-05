//! End-to-end tests added after a mutation audit of the spreadsheet preview (App / UI side). Each
//! pins a rule that a mutated build used to get away with; the comment on each says which.

use super::*;
use crate::i18n::Msg;

/// Rewrites `book` with `rows` as its only sheet, gives it a modification time `k` seconds past a
/// fixed point (the reload test is on the time, which must differ from the last read), tells the
/// app the file changed and applies the answer of the reload.
fn rewrite(s: &mut Sim, book: &std::path::Path, rows: &str, k: u64) {
    build_xlsx(book, &[("S", "visible", rows, "")]);
    let f = std::fs::OpenOptions::new().write(true).open(book).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_900_000_000 + k))
        .unwrap();
    s.app
        .refresh_fs_watched(false, std::slice::from_ref(&book.to_path_buf()));
    s.draw();
    drain_media_until_current(s);
}

fn apples(rows: &[u32]) -> String {
    let mut out = format!(r#"<row r="1">{}</row>"#, x_str("A1", "head"));
    for r in rows {
        out += &format!(
            r#"<row r="{r}">{}</row>"#,
            x_str(&format!("A{r}"), &format!("apple {r}"))
        );
    }
    out
}

/// `e` and the `?` row are about *files*: a directory that happens to be called `a.docx` is a
/// directory, and the row must not promise an Office app.
#[test]
fn e2e_survivor_a_directory_named_like_an_office_file_is_not_an_office_file() {
    let dir = sandbox("surv_dir_named_docx");
    for name in ["a.docx", "b.xlsx", "c.odp"] {
        std::fs::create_dir(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("inside.txt"), "x\n").unwrap();
    }
    let mut s = Sim::with_config(&canon(&dir), cfg_en());
    for name in ["a.docx", "b.xlsx", "c.odp"] {
        s.select(name);
        assert!(
            matches!(
                s.app.edit_help_label(Msg::EditExternalEnv),
                Some(Msg::EditExternalEnv)
            ),
            "{name}: the editor wording"
        );
    }
    s.key('?');
    s.dont_see("open in an Office app");
}

/// A workbook whose sheets are all hidden has nothing to draw: it is not a table preview (the
/// keys, footer and help are the plain text ones), whatever the message on the screen says.
#[test]
fn e2e_survivor_a_workbook_with_every_sheet_hidden_is_not_a_table_preview() {
    let dir = sandbox("surv_all_hidden");
    build_xlsx(
        &dir.join("h.xlsx"),
        &[("A", "hidden", "", ""), ("B", "veryHidden", "", "")],
    );
    let s = open_sheet_file(&dir, "h.xlsx", cfg_en());
    s.see("every sheet in this workbook is hidden");
    assert!(!s.app.is_table_preview());
    assert!(!s.app.is_sheet_preview());
    assert!(!s.app.sheet_can_switch());
    // One visible sheet and it is a table preview (the control).
    build_xlsx(
        &dir.join("v.xlsx"),
        &[
            ("A", "hidden", "", ""),
            (
                "B",
                "visible",
                &format!(r#"<row r="1">{}</row>"#, x_str("A1", "v")),
                "",
            ),
        ],
    );
    let s = open_sheet_file(&dir, "v.xlsx", cfg_en());
    assert!(s.app.is_table_preview());
}

/// A search confirmed while the sheet was loading waits for it (`search_pending`). When the load
/// *fails*, the wait is over: a later, successful reload runs the search where it stands and does
/// not jump to its first hit as if it had just been confirmed.
#[test]
fn e2e_survivor_a_failed_load_ends_the_wait_of_a_pending_search() {
    let dir = sandbox("surv_fail_then_ok");
    let book = canon(&dir).join("f.xlsx");
    std::fs::write(&book, b"this is not a zip at all").unwrap();
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("f.xlsx");
    s.enter();
    assert!(s.app.is_sheet_loading());
    s.key('/');
    s.keys("apple");
    s.enter(); // waits for a sheet that never comes
    s.drain_media();
    assert!(
        s.app.sheet_error().is_some(),
        "the load failed\n{}",
        s.screen()
    );
    // The file is repaired: the reload finds the hit far from the cursor.
    rewrite(&mut s, &book, &apples(&[9]), 1);
    assert!(s.app.table_cell_is_hit(8, 0), "the hit is found");
    assert_eq!(
        s.app.table_cursor(),
        (0, 0),
        "no jump: the search was not waiting any more\n{}",
        s.screen()
    );
    assert_eq!(s.app.search_status(), Some((1, 1)));
}

/// After a reload the hit that was current stays current while it is still a hit — by *index into
/// the new list*, not by position in the old one: with three hits and the first current (and
/// after an unchanged reload) it is still the first; with the second current, the second. When
/// the current hit is gone, the next hit after it takes over; when nothing is after it, the last.
#[test]
fn e2e_survivor_a_reload_keeps_the_current_hit_or_moves_to_the_next_one() {
    let dir = sandbox("surv_reload_hits");
    let book = canon(&dir).join("r.xlsx");
    build_xlsx(&book, &[("S", "visible", &apples(&[2, 4, 6]), "")]);
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("r.xlsx");
    s.enter();
    s.drain_media();
    s.key('/');
    s.keys("apple");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 3)));
    assert_eq!(s.app.table_cursor(), (1, 0));

    // The same cells again: the first hit is still the current one, the cursor stays.
    rewrite(&mut s, &book, &apples(&[2, 4, 6]), 1);
    assert_eq!(
        s.app.search_status(),
        Some((1, 3)),
        "first hit, unchanged reload"
    );
    assert_eq!(s.app.table_cursor(), (1, 0));

    // On the second hit, the same again.
    s.key('n');
    assert_eq!(s.app.search_status(), Some((2, 3)));
    assert_eq!(s.app.table_cursor(), (3, 0));
    rewrite(&mut s, &book, &apples(&[2, 4, 6]), 2);
    assert_eq!(
        s.app.search_status(),
        Some((2, 3)),
        "second hit, unchanged reload"
    );
    assert_eq!(s.app.table_cursor(), (3, 0));

    // The current hit (row 4) is gone: the next one (row 6) takes over, now the second of two.
    rewrite(&mut s, &book, &apples(&[2, 6]), 3);
    assert_eq!(
        s.app.search_status(),
        Some((2, 2)),
        "the next hit is current"
    );
    assert!(s.app.table_cell_is_hit(5, 0));
    assert_eq!(s.app.table_cursor(), (3, 0), "the cursor does not move");

    // The current hit (row 6) is gone and nothing is after it: the last hit that is left.
    rewrite(&mut s, &book, &apples(&[2, 4]), 4);
    assert_eq!(
        s.app.search_status(),
        Some((2, 2)),
        "the last hit is current"
    );
    assert!(s.app.table_cell_is_hit(3, 0));
}

/// Moving to another sheet leaves nothing of the old sheet behind while the new one is read: the
/// hits (the old sheet's cell positions mean nothing there, and `n` must not jump to them), and
/// the full-cell popup (it shows the cell under the cursor, which has just moved to A1).
#[test]
fn e2e_survivor_switching_sheets_drops_the_old_hits_and_closes_the_cell_popup() {
    let dir = sandbox("surv_sheet_switch");
    let book = canon(&dir).join("w.xlsx");
    build_xlsx(
        &book,
        &[
            ("One", "visible", &apples(&[3, 6]), ""),
            (
                "Two",
                "visible",
                &format!(r#"<row r="1">{}</row>"#, x_str("A1", "plain")),
                "",
            ),
        ],
    );
    let mut s = Sim::with_config(&canon(&dir), cfg_en()).with_media();
    s.select("w.xlsx");
    s.enter();
    s.drain_media();
    s.key('/');
    s.keys("apple");
    s.enter();
    assert_eq!(s.app.search_status(), Some((1, 2)));
    assert_eq!(s.app.table_cursor(), (2, 0));
    s.enter();
    assert!(s.app.is_table_cell_open(), "the popup is open on the hit");

    s.app.sheet_next(); // the keys of the popup layer would not reach this: call it directly
    assert!(s.app.is_sheet_loading());
    assert!(
        !s.app.is_table_cell_open(),
        "the popup belonged to the old cell"
    );
    assert_eq!(
        s.app.search_status(),
        None,
        "no hits of the old sheet while the new one loads"
    );
    assert_eq!(s.app.table_cursor(), (0, 0));
    s.key('n');
    assert_eq!(
        s.app.table_cursor(),
        (0, 0),
        "n has nothing to go to before the sheet is there"
    );
    drain_media_until_current(&mut s);
    s.see("Two (2/2)");
    assert_eq!(
        s.app.search_status(),
        None,
        "no \"apple\" on the second sheet"
    );
}
