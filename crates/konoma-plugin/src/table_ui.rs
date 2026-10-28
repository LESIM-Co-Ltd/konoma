//! What a host shows of a plugin's [`Grid`]: the konoma host and the standalone app both read the
//! cells, the full-cell popup text and the table source through these, so a table looks and reads
//! the same in both.

use std::borrow::Cow;

use konoma_widgets::table::{cell_address, TableSource};

use crate::view::Grid;

/// The header text of `col` (empty when the table has none; a sheet's are the column letters).
pub fn header_of(g: &dyn Grid, col: usize) -> Cow<'_, str> {
    Cow::Owned(g.header(col).unwrap_or_default())
}

/// The text shown in a data cell (`""` for an empty or out-of-range cell). A grid that keeps its
/// cells as text is borrowed, so reading every cell copies nothing.
pub fn cell_of(g: &dyn Grid, row: usize, col: usize) -> Cow<'_, str> {
    match g.cell_str(row, col) {
        Some(s) => Cow::Borrowed(s),
        None => Cow::Owned(g.cell(row, col)),
    }
}

/// A plugin's grid as the table widget's source.
pub struct GridSource<'a>(pub &'a dyn Grid);

impl TableSource for GridSource<'_> {
    fn nrows(&self) -> usize {
        self.0.nrows()
    }
    fn ncols(&self) -> usize {
        self.0.ncols()
    }
    fn header(&self, col: usize) -> Cow<'_, str> {
        header_of(self.0, col)
    }
    fn cell(&self, row: usize, col: usize) -> Cow<'_, str> {
        cell_of(self.0, row, col)
    }
}

/// The full-cell popup's content: the cursor cell's header, position and untruncated text.
/// `row` and `col` are 1-based (the display convention of the table's own title).
pub struct TableCellView {
    pub header: String,
    pub row: usize,
    pub col: usize,
    pub nrows: usize,
    pub ncols: usize,
    pub text: String,
}

/// The popup's content for the cell `(r, c)` (0-based), read live. `None` when the table has no
/// columns (nothing to show). `addressed` is a spreadsheet's: the popup is named by the cell's
/// address and shows the plugin's labelled lines (in `lang`) when it has them.
pub fn cell_view(
    g: &dyn Grid,
    addressed: bool,
    (r, c): (usize, usize),
    lang: &str,
) -> Option<TableCellView> {
    if g.ncols() == 0 {
        return None;
    }
    let text = if addressed {
        // A spreadsheet cell: the address, what Excel shows, and everything behind it (one
        // `label: value` per line).
        match g.detail(r, c) {
            Some(detail) => detail
                .lines
                .iter()
                .map(|(label, value)| format!("{}: {value}", label.get(lang)))
                .collect::<Vec<_>>()
                .join("\n"),
            None => g.cell(r, c),
        }
    } else if g.nrows() == 0 {
        String::new() // a header-only file: the content is empty.
    } else {
        g.cell(r, c)
    };
    Some(TableCellView {
        // For a sheet the "header" is the cell address (`B3`), the natural name of the cell.
        header: if addressed {
            cell_address(r, c)
        } else {
            header_of(g, c).into_owned()
        },
        row: r + 1,
        col: c + 1,
        nrows: g.nrows(),
        ncols: g.ncols(),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Tr;
    use crate::view::CellDetail;

    /// A grid that makes its cells on demand (`cell_str` is `None`), with optional details.
    struct Made {
        rows: Vec<Vec<&'static str>>,
        heads: Option<Vec<&'static str>>,
        detail: bool,
        borrowed: bool,
    }

    impl Grid for Made {
        fn nrows(&self) -> usize {
            self.rows.len()
        }
        fn ncols(&self) -> usize {
            self.heads
                .as_ref()
                .map_or_else(|| self.rows.first().map_or(0, Vec::len), Vec::len)
        }
        fn header(&self, col: usize) -> Option<String> {
            self.heads.as_ref()?.get(col).map(|s| s.to_string())
        }
        fn cell(&self, row: usize, col: usize) -> String {
            self.rows
                .get(row)
                .and_then(|r| r.get(col))
                .map(|s| s.to_string())
                .unwrap_or_default()
        }
        fn cell_str(&self, row: usize, col: usize) -> Option<&str> {
            if !self.borrowed {
                return None;
            }
            self.rows.get(row)?.get(col).copied()
        }
        fn detail(&self, row: usize, col: usize) -> Option<CellDetail> {
            self.detail.then(|| CellDetail {
                lines: vec![
                    (Tr::new("Type").ja("型"), "text".into()),
                    (Tr::new("Value"), self.cell(row, col)),
                ],
            })
        }
    }

    fn grid(borrowed: bool, detail: bool) -> Made {
        Made {
            rows: vec![vec!["a", "b"], vec!["c", "d"], vec!["e"]],
            heads: Some(vec!["h1", "h2"]),
            detail,
            borrowed,
        }
    }

    #[test]
    fn header_of_reads_the_header_and_is_empty_without_one() {
        let g = grid(false, false);
        assert_eq!(header_of(&g, 1), "h2");
        assert_eq!(header_of(&g, 9), "", "out of range");
        let g = Made {
            heads: None,
            ..grid(false, false)
        };
        assert_eq!(header_of(&g, 0), "");
    }

    #[test]
    fn cell_of_borrows_a_text_grid_and_makes_the_rest() {
        let g = grid(true, false);
        assert!(matches!(cell_of(&g, 0, 1), Cow::Borrowed("b")));
        // A ragged short row has no `cell_str`: it reads as an empty made cell.
        assert_eq!(cell_of(&g, 2, 1), "");
        let g = grid(false, false);
        assert!(matches!(cell_of(&g, 0, 1), Cow::Owned(_)));
        assert_eq!(cell_of(&g, 0, 1), "b");
        assert_eq!(cell_of(&g, 7, 7), "", "out of range");
    }

    #[test]
    fn the_grid_source_is_the_grid() {
        for borrowed in [true, false] {
            let g = grid(borrowed, false);
            let s = GridSource(&g);
            assert_eq!((s.nrows(), s.ncols()), (3, 2));
            assert_eq!(s.header(0), "h1");
            assert_eq!(s.cell(1, 0), "c");
            assert_eq!(s.cell(2, 1), "");
        }
    }

    #[test]
    fn a_plain_table_cell_view_names_the_column_and_shows_the_cell() {
        let g = grid(false, true);
        let v = cell_view(&g, false, (1, 1), "en").unwrap();
        assert_eq!(v.header, "h2");
        assert_eq!((v.row, v.col, v.nrows, v.ncols), (2, 2, 3, 2));
        assert_eq!(v.text, "d", "the details are for a sheet only");
    }

    #[test]
    fn an_addressed_cell_view_is_named_by_the_address_and_lists_the_details() {
        let g = grid(false, true);
        let v = cell_view(&g, true, (2, 1), "en").unwrap();
        assert_eq!(v.header, "B3");
        assert_eq!(v.text, "Type: text\nValue: ");
        let v = cell_view(&g, true, (0, 0), "ja").unwrap();
        assert_eq!(v.text, "型: text\nValue: a", "a missing language falls back to English");
        // No detail from the plugin: the cell itself.
        let g = grid(false, false);
        let v = cell_view(&g, true, (0, 1), "en").unwrap();
        assert_eq!((v.header.as_str(), v.text.as_str()), ("B1", "b"));
    }

    #[test]
    fn a_header_only_table_has_an_empty_popup_and_no_columns_has_none() {
        let g = Made {
            rows: vec![],
            heads: Some(vec!["x", "y"]),
            detail: false,
            borrowed: false,
        };
        let v = cell_view(&g, false, (0, 1), "en").unwrap();
        assert_eq!((v.header.as_str(), v.text.as_str()), ("y", ""));
        assert_eq!((v.row, v.col, v.nrows, v.ncols), (1, 2, 0, 2));
        let none = Made {
            rows: vec![],
            heads: None,
            detail: false,
            borrowed: false,
        };
        assert!(cell_view(&none, false, (0, 0), "en").is_none());
        assert!(cell_view(&none, true, (0, 0), "en").is_none());
    }
}
