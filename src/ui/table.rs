//! CSV/TSV table preview rendering.
//!
//! Draws an aligned grid with a fixed header row, rainbow (per-column) colors, and a highlighted
//! cell cursor. Column widths are measured from the header plus the currently visible rows (like
//! csvlens), so opening is instant even for large files. Vertical/horizontal scrolling is done by
//! adjusting the scroll offsets to keep the cursor visible, then writing them back to `App`.

use ratatui::layout::Rect;
use ratatui::Frame;

use konoma_widgets::table::{render_table, CellPopup, TableOptions, TableViewState};

use crate::app::{App, TableCellView};
use crate::i18n::tr;
use crate::preview::table::Grid;

pub fn render(frame: &mut Frame, app: &mut App, area: Rect) {
    let (cur_row, cur_col) = app.table_cursor();
    let (top, left) = app.table_scroll();
    let mut state = TableViewState {
        cursor_row: cur_row,
        cursor_col: cur_col,
        top,
        left,
        viewport_rows: 0,
    };
    let Some(t) = app.grid() else {
        return; // The caller (preview::render) has already confirmed is_table_preview. Defensive no-op.
    };
    let addressed = app.table_addressed();
    let opts = TableOptions {
        // Decided once per frame: it reads the plugin's view (or the csv plugin's setting for a sheet).
        rainbow: app.table_rainbow(),
        title: build_title(app, &t, cur_row, cur_col, addressed),
        empty_label: tr(app.lang, crate::i18n::Msg::SheetEmptyWord).to_string(),
        gutter: addressed,
        is_hit: &|r, c| app.table_cell_is_hit(r, c),
    };
    let laid_out = render_table(frame.buffer_mut(), area, &t, &mut state, &opts);
    // The borrow of the grid ends here -> App can now be updated mutably. A draw that laid
    // nothing out (no columns, or a collapsed area) must not overwrite the last real view.
    if laid_out {
        app.set_table_view(state.top, state.left, state.viewport_rows);
    }
}

/// `Enter` in a table preview: a centered popup showing the **cursor cell's untruncated text**
/// (wrapped, scrollable with j/k/g/G/PageUp/PageDown). Reads the cell live from `App` each frame
/// (so an external edit reloading the table, or the popup staying open across an unrelated redraw,
/// always shows the current value) and writes the clamped scroll back, mirroring the
/// render/`set_table_view` convention of the grid.
pub fn render_cell_popup(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(view): Option<TableCellView> = app.table_cell_view() else {
        return; // The caller (ui::render) has already confirmed is_table_cell_open. Defensive no-op.
    };
    let title = konoma_widgets::table::cell_popup_title(
        tr(app.lang, crate::i18n::Msg::TableCellTitle),
        &view.header,
        (view.row, view.col),
        (view.nrows, view.ncols),
    );
    let footer = crate::ops::footer::tokens(app, crate::keymap::Context::TableCell).join("  ");
    let (scroll, viewport) = konoma_widgets::table::render_cell_popup(
        frame.buffer_mut(),
        area,
        CellPopup {
            title,
            footer,
            text: view.text,
            scroll: app.table_cell_scroll(),
        },
    );
    app.set_table_cell_view(scroll, viewport);
}

/// Title with path + dimensions + cursor position (+ a truncation note when the file was capped).
/// A spreadsheet shows the sheet name and position, the hidden-sheet count (the note its plugin
/// gives) and the cursor's cell address instead of the row/column counters.
fn build_title(app: &App, t: &Grid<'_>, cur_row: usize, cur_col: usize, addressed: bool) -> String {
    let path = app
        .tab
        .preview_path
        .clone()
        .map(|p| app.format_path(&p))
        .unwrap_or_else(|| "table".to_string());
    let capped = t
        .truncated()
        .then(|| tr(app.lang, crate::i18n::Msg::SheetCappedWord));
    // A sheet names the cursor by its address after the plugin's note (the sheet's name and position).
    let sheet = addressed.then(|| app.table_title_note().unwrap_or_default());
    konoma_widgets::table::table_title(
        &path,
        (t.nrows(), t.ncols()),
        (cur_row, cur_col),
        sheet,
        capped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- FIXED CRASH: `render_cell_popup` used to panic on a small terminal ----------------
    //
    // `Ord::clamp` is `assert!(min <= max)` + branch — **not** `debug_assert!` — so it panicked
    // in release builds too. The popup width used to be `.clamp(20, area.width.saturating_sub(2))`:
    // once `area.width <= 21`, `saturating_sub(2) <= 19 < 20` and the assert fired. Same story for
    // height with the 6-row floor once `area.height <= 7`. `area` here is the *entire terminal*
    // (`ui/mod.rs` passes `frame.area()`), so simply resizing the terminal small enough while the
    // cell popup was open (CSV/TSV/archive → `Enter` → `Enter`) crashed the whole app.
    //
    // `render_cell_popup` now uses `.min(..).max(..)` (the same convention `ui/help.rs` and
    // `ui/dialog.rs` already used), which has no `min <= max` invariant to violate: the popup
    // rect can legitimately end up larger than the terminal, and `Clear`/`Block`/`Paragraph` all
    // intersect their draw area against the buffer's area before writing a single cell (verified
    // against the `ratatui-widgets` 0.3.2 source: `clear.rs`, `block.rs`, `paragraph.rs` each do
    // `area.intersection(buf.area)` first), so an oversized popup just gets silently clipped
    // instead of panicking. These tests assert that drawing at each size does not panic — the
    // previous, broken behavior is preserved only in git history.

    /// Build an App with a tiny CSV opened as a table preview, with the full-cell popup already
    /// open (mirrors `Sim`'s `select` + `enter` + `enter` in `e2e_tests.rs`, but self-contained
    /// here — that harness is private to another module). Drives the real `tree_activate`/
    /// `toggle_table_cell_view` App methods rather than poking private fields directly.
    fn app_with_open_cell_popup() -> (App, crate::test_support::TmpDir) {
        let dir = crate::test_support::unique_tmp("konoma_table_popup_panic_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t.csv"), "h1,h2\na,b\n").unwrap();
        let mut app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
        let idx = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("t.csv"))
            .expect("t.csv should be in the freshly-built tree");
        app.tab.selected = idx;
        app.tree_activate().unwrap();
        assert!(
            app.is_table_preview(),
            "setup precondition: t.csv should open as a table preview"
        );
        app.toggle_table_cell_view();
        assert!(
            app.is_table_cell_open(),
            "setup precondition: the full-cell popup should be open"
        );
        (app, dir)
    }

    /// Draw just the cell popup (the same call `ui::render` makes: `area` = the whole frame) into
    /// a `w`×`h` `TestBackend` terminal, returning a snapshot of the rendered buffer so callers
    /// can assert on its content — not just that drawing didn't panic. This is what used to
    /// trigger the crash on a small terminal (see the module note above).
    fn draw_cell_popup(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let completed = term
            .draw(|f| {
                let area = f.area();
                render_cell_popup(f, app, area);
            })
            .unwrap();
        completed.buffer.clone()
    }

    /// True if the buffer has at least one non-blank cell. Used to confirm that a tiny terminal
    /// still gets a *clipped fragment* of the popup (border corner/side) rather than either a
    /// panic or a silent blank screen: `Block`/`Paragraph`/`Clear` all intersect the popup's `Rect`
    /// against the buffer's actual area before writing a single cell (verified against the
    /// `ratatui-widgets` 0.3.2 source — `block.rs`, `paragraph.rs`, `clear.rs` each do
    /// `area.intersection(buf.area)` up front), so an oversized popup degrades to "draw whatever
    /// sliver fits" instead of corrupting the screen.
    fn has_visible_content(buf: &ratatui::buffer::Buffer) -> bool {
        buf.content.iter().any(|c| c.symbol() != " ")
    }

    #[test]
    fn cell_popup_survives_width_20() {
        // area.width.saturating_sub(2) = 18 < 20 → used to be clamp(20, 18), which panicked.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 20, 30);
    }

    #[test]
    fn cell_popup_survives_width_21_boundary() {
        // area.width.saturating_sub(2) = 19 < 20 → used to be clamp(20, 19), which panicked. This
        // is the exact boundary: one column narrower than the first size that survived even
        // before the fix.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 21, 30);
    }

    #[test]
    fn cell_popup_survives_width_22() {
        // area.width.saturating_sub(2) = 20 → used to be clamp(20, 20), which was valid even
        // before the fix (min == max is allowed). Non-regression control.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 22, 30);
    }

    #[test]
    fn cell_popup_survives_height_6() {
        // area.height.saturating_sub(2) = 4 < 6 → used to be clamp(6, 4), which panicked. Width is
        // 80, comfortably above the width floor, so only the height side is under test here.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 80, 6);
    }

    #[test]
    fn cell_popup_survives_height_7_boundary() {
        // area.height.saturating_sub(2) = 5 < 6 → used to be clamp(6, 5), which panicked.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 80, 7);
    }

    #[test]
    fn cell_popup_survives_height_8() {
        // area.height.saturating_sub(2) = 6 → used to be clamp(6, 6), which was valid even before
        // the fix. Non-regression control.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 80, 8);
    }

    #[test]
    fn cell_popup_survives_at_normal_terminal_size() {
        // Non-regression control at an ordinary terminal size (matches `Sim`'s default 90×26 in
        // `e2e_tests.rs`): must not panic, before or after the fix.
        let (mut app, _dir) = app_with_open_cell_popup();
        draw_cell_popup(&mut app, 90, 26);
    }

    // ---- Extreme terminal sizes: below the old crash floor on *both* axes at once, or with one
    // axis reduced to the smallest possible value (1). Each of these also asserts
    // `has_visible_content` — the point isn't only "doesn't panic" but "still draws a clipped,
    // legible fragment of the popup" (per the task: confirm the render isn't corrupted, not just
    // that it didn't crash). Before the fix, all four of these panicked (both the width and the
    // height clamp are violated at once, or at the very least the width one, since `saturating_sub`
    // never lets `area.width - 2` or `area.height - 2` climb back up to the 20/6 floor).

    #[test]
    fn cell_popup_survives_width_1_height_1() {
        let (mut app, _dir) = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 1, 1);
        assert_eq!(buf.area.width, 1);
        assert_eq!(buf.area.height, 1);
        assert!(
            has_visible_content(&buf),
            "a 1x1 terminal should still show a clipped fragment of the popup border (its \
             top-left corner), not a blank screen"
        );
    }

    #[test]
    fn cell_popup_survives_width_2_height_1() {
        let (mut app, _dir) = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 2, 1);
        assert_eq!(buf.area.width, 2);
        assert_eq!(buf.area.height, 1);
        assert!(has_visible_content(&buf));
    }

    #[test]
    fn cell_popup_survives_width_1_height_30() {
        let (mut app, _dir) = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 1, 30);
        assert_eq!(buf.area.width, 1);
        assert_eq!(buf.area.height, 30);
        assert!(
            has_visible_content(&buf),
            "a 1-column-wide terminal should still show a clipped sliver of the popup's left \
             border running down the single visible column, not a blank screen"
        );
    }

    #[test]
    fn cell_popup_survives_width_30_height_1() {
        let (mut app, _dir) = app_with_open_cell_popup();
        let buf = draw_cell_popup(&mut app, 30, 1);
        assert_eq!(buf.area.width, 30);
        assert_eq!(buf.area.height, 1);
        assert!(
            has_visible_content(&buf),
            "a 1-row-tall terminal should still show a clipped sliver of the popup's top border \
             running across the single visible row, not a blank screen"
        );
    }

    /// A draw into a collapsed area (no inner width/height) lays nothing out and must not reset
    /// the page size the last real draw recorded.
    #[test]
    fn collapsed_draw_keeps_the_last_page_size() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let dir = crate::test_support::unique_tmp("konoma_table_collapsed_draw");
        std::fs::create_dir_all(&dir).unwrap();
        let csv: String = std::iter::once("h".to_string())
            .chain((0..60).map(|i| format!("r{i}")))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("t.csv"), csv).unwrap();
        let mut app = App::new(dir.to_path_buf(), crate::config::Config::default()).unwrap();
        let idx = app
            .tab
            .entries
            .iter()
            .position(|e| e.path.ends_with("t.csv"))
            .unwrap();
        app.tab.selected = idx;
        app.tree_activate().unwrap();
        assert!(app.is_table_preview());
        let draw = |app: &mut App, w: u16, h: u16| {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| {
                let area = f.area();
                render(f, app, area);
            })
            .unwrap();
        };
        // 12 rows tall: inner 10, minus header and separator = page of 8.
        draw(&mut app, 40, 12);
        app.table_page(1);
        assert_eq!(app.table_cursor().0, 8, "setup: the page is 8 rows");
        draw(&mut app, 2, 12); // inner width 0
        app.table_page(1);
        assert_eq!(app.table_cursor().0, 16, "zero width kept the page size");
        draw(&mut app, 40, 2); // inner height 0
        app.table_page(1);
        assert_eq!(app.table_cursor().0, 24, "zero height kept the page size");
    }
}
