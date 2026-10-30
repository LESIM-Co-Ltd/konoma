//! What the copy menu (`y`) does: the text the shared rules make, the clipboard, the notice.

use std::borrow::Cow;
use std::path::Path;

use konoma_host_core::copy::{self, CopyGrid};
use konoma_host_core::keymap::CoreAction;

use super::draw::TableFrame;
use crate::table_ui;
use crate::view::Grid;
use konoma_host_core::lang::Lang;

/// The table of the frame on screen, read through the same cell and header rules the host uses.
struct AppGrid<'a>(&'a dyn Grid);

impl CopyGrid for AppGrid<'_> {
    fn nrows(&self) -> usize {
        self.0.nrows()
    }
    fn ncols(&self) -> usize {
        self.0.ncols()
    }
    fn header(&self, col: usize) -> Cow<'_, str> {
        table_ui::header_of(self.0, col)
    }
    fn row_len(&self, row: usize) -> usize {
        self.0.row_len(row)
    }
    fn cell(&self, row: usize, col: usize) -> Cow<'_, str> {
        table_ui::cell_of(self.0, row, col)
    }
}

/// Run a menu item and return the notice to show. `None`: the item does nothing here.
///
/// `hint` is the note of the binding that picked the plugin (it decides the delimiter of a
/// CSV/TSV row, as it did when the table was read).
/// `open_dir` is the directory the program was started in (the base of the relative title); a
/// file opened with no such directory is relative to its own.
pub(super) fn run(
    action: CoreAction,
    lang: Lang,
    path: &Path,
    hint: Option<&str>,
    open_dir: Option<&Path>,
    table: Option<&TableFrame>,
) -> Option<String> {
    match action {
        CoreAction::CopyPath(kind) => {
            if path.as_os_str().is_empty() {
                return Some(copy::no_target_notice(lang).into());
            }
            let base = open_dir.or_else(|| path.parent()).unwrap_or(Path::new(""));
            let text = copy::path_text(path, base, kind);
            Some(match konoma_host_core::clipboard::set_text(&text) {
                Ok(()) => copy::copied_notice(lang, &text),
                Err(e) => copy::copy_failed_notice(lang, &e),
            })
        }
        CoreAction::TableCopy(kind) => {
            let Some(t) = table else {
                return Some(copy::no_target_notice(lang).into());
            };
            // A spreadsheet never reads the delimiter: its row is tab-joined.
            let sep = if t.addressed {
                String::new()
            } else {
                (crate::table_sep::delimiter(hint, path) as char).to_string()
            };
            let text = copy::table_text(
                &AppGrid(&*t.grid),
                kind,
                (t.state.cursor_row, t.state.cursor_col),
                t.addressed,
                &sep,
            );
            Some(match konoma_host_core::clipboard::set_text(&text) {
                Ok(()) => copy::copied_preview_notice(lang, &text),
                Err(e) => copy::copy_failed_notice(lang, &e),
            })
        }
        _ => None,
    }
}
