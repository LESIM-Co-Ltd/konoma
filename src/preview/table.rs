//! What the table renderer shows: a plugin's table (CSV/TSV, an archive listing, the sheet of a
//! workbook) read through the same [`Grid`].
//!
//! The CSV/TSV and archive files are read by the `csv` and `archive` plugins
//! (`crates/konoma-table`), the workbooks by the `spreadsheet` plugin (`crates/konoma-office`);
//! this module only re-exports what the host's tests share, and holds the borrowed view over the
//! table a plugin returned.

use std::borrow::Cow;

use konoma_plugin::view::{CellDetail, Grid as PluginGrid};

/// What the host's tests read a CSV with: the parser, its row cap, the parsed table, and the
/// (thread-local) counter of full-file parses.
#[cfg(test)]
pub use konoma_table::table::{parse, TableData, MAX_ROWS, PARSE_CALLS, PARSE_LOG};
#[cfg(test)]
pub use konoma_table::table::{PauseGate, CANCEL_CHECK_ROWS};

/// A read-only view of the table the renderer is showing: a table a plugin returned (a parsed
/// CSV/TSV, an archive listing, one sheet of a workbook), read cell by cell through [`PluginGrid`].
/// It is borrowed, never converted (a sheet can hold 4M cells — copying it would double that).
///
/// "Rows" always means data rows: a CSV's header record is exposed through [`Grid::header`], and a
/// sheet has no header record (its header is the column letters).
#[derive(Clone, Copy)]
pub enum Grid<'a> {
    /// A table a plugin returned.
    Plugin(&'a dyn PluginGrid),
}

impl std::fmt::Debug for Grid<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Grid::Plugin(g) => write!(f, "Grid::Plugin({}x{})", g.nrows(), g.ncols()),
        }
    }
}

impl<'a> Grid<'a> {
    /// Number of data rows.
    pub fn nrows(&self) -> usize {
        match self {
            Grid::Plugin(g) => g.nrows(),
        }
    }

    /// Number of columns.
    pub fn ncols(&self) -> usize {
        match self {
            Grid::Plugin(g) => g.ncols(),
        }
    }

    /// The header text of `col`: the plugin table's header cell (empty when it has none; a
    /// sheet's are the column letters `A`, `B`, .. `AA`).
    pub fn header(&self, col: usize) -> Cow<'a, str> {
        match self {
            Grid::Plugin(g) => Cow::Owned(g.header(col).unwrap_or_default()),
        }
    }

    /// The text shown in a data cell (`""` for an empty/out-of-range cell). For a sheet this is the
    /// cell as Excel displays it (number format applied). A plugin table that keeps its cells as
    /// text is borrowed, so reading every cell (the search does) copies nothing.
    pub fn cell(&self, row: usize, col: usize) -> Cow<'a, str> {
        match self {
            Grid::Plugin(g) => match g.cell_str(row, col) {
                Some(s) => Cow::Borrowed(s),
                None => Cow::Owned(g.cell(row, col)),
            },
        }
    }

    /// How many cells data row `row` really has (a ragged CSV row is shorter than `ncols`; a sheet
    /// row ends at its last cell that shows something).
    pub fn row_len(&self, row: usize) -> usize {
        match self {
            Grid::Plugin(g) => g.row_len(row),
        }
    }

    /// True when the source was cut off at a limit (CSV row cap; sheet row/column cap).
    pub fn truncated(&self) -> bool {
        match self {
            Grid::Plugin(g) => g.truncated(),
        }
    }

    /// Calls `f(col, text)` for each cell of `row` that has something to search (see
    /// [`PluginGrid::visit_row`]).
    pub fn visit_row(&self, row: usize, f: &mut dyn FnMut(usize, &str)) {
        match self {
            Grid::Plugin(g) => g.visit_row(row, f),
        }
    }

    /// The labelled lines of the cell popup, if the table has them (a sheet's).
    pub fn detail(&self, row: usize, col: usize) -> Option<CellDetail> {
        match self {
            Grid::Plugin(g) => g.detail(row, col),
        }
    }
}

/// Spreadsheet column letters for a 0-based column: `0 -> A`, `25 -> Z`, `26 -> AA`, `16383 -> XFD`.
pub fn column_letters(col: usize) -> String {
    let mut n = col + 1;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// The A1-style address of a 0-based cell (`(2, 1) -> "B3"`).
pub fn cell_address(row: usize, col: usize) -> String {
    format!("{}{}", column_letters(col), row + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_letters_follow_the_spreadsheet_scheme() {
        let cases = [
            (0, "A"),
            (1, "B"),
            (25, "Z"),
            (26, "AA"),
            (27, "AB"),
            (51, "AZ"),
            (52, "BA"),
            (701, "ZZ"),
            (702, "AAA"),
            (16_383, "XFD"),
        ];
        for (col, want) in cases {
            assert_eq!(column_letters(col), want, "col {col}");
        }
    }

    #[test]
    fn cell_address_is_column_letters_then_one_based_row() {
        assert_eq!(cell_address(0, 0), "A1");
        assert_eq!(cell_address(2, 1), "B3");
        assert_eq!(cell_address(99, 26), "AA100");
    }

    #[test]
    fn grid_over_csv_exposes_header_and_data_rows() {
        let t = TableData {
            headers: vec!["h1".into(), "h2".into()],
            rows: vec![vec!["a".into(), "b".into()]],
            ncols: 2,
            truncated: true,
        };
        let g = Grid::Plugin(&t);
        assert_eq!((g.nrows(), g.ncols()), (1, 2));
        assert_eq!(g.header(1), "h2");
        assert_eq!(g.cell(0, 1), "b");
        assert_eq!(g.cell(5, 5), "");
        assert!(g.truncated());
    }

    #[test]
    fn grid_over_an_empty_sheet_is_empty_with_letter_headers() {
        let wb = crate::preview::office::Workbook {
            sheets: vec![crate::preview::office::Sheet::default()],
            ..Default::default()
        };
        let view = konoma_office::table_view(wb, 0, "en", true);
        let g = Grid::Plugin(&*view.grid);
        assert_eq!((g.nrows(), g.ncols()), (0, 0));
        assert_eq!(g.header(0), "A");
        assert_eq!(g.cell(0, 0), "");
        assert!(!g.truncated());
        assert!(view.addressed);
    }

    /// A plugin table that makes its cell text on demand (no borrowed string) reads the same.
    #[test]
    fn grid_over_a_table_that_builds_its_cells_reads_through_cell() {
        struct Made;
        impl PluginGrid for Made {
            fn nrows(&self) -> usize {
                2
            }
            fn ncols(&self) -> usize {
                1
            }
            fn header(&self, _col: usize) -> Option<String> {
                None
            }
            fn cell(&self, row: usize, _col: usize) -> String {
                format!("r{row}")
            }
        }
        let made = Made;
        let g = Grid::Plugin(&made);
        assert_eq!(g.cell(1, 0), "r1");
        assert_eq!(g.header(0), "", "a missing header reads as empty");
    }
}
