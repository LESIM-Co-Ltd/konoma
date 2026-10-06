//! CSV/TSV table preview: parse a delimited file into a header + rows grid.
//!
//! The grid is rendered with rainbow (per-column) colors and a movable cell cursor
//! (see `ui/table.rs`), and cells/rows/columns can be copied to the clipboard.
//! Parsing goes through the `csv` crate so quoted commas, embedded newlines, and
//! ragged (variable-column) rows are handled correctly instead of a naive split.

use std::borrow::Cow;
use std::path::Path;

use anyhow::{Context, Result};

use crate::preview::office::Sheet;

/// Cap on the number of data rows read for the preview. CSVs can be arbitrarily large;
/// we bound memory/parse time and mark the table `truncated` so the UI can say so.
/// 100k rows is far more than a person scrolls through in a preview.
pub const MAX_ROWS: usize = 100_000;

/// A parsed CSV/TSV table. The first record is treated as the header row.
#[derive(Debug, Clone, Default)]
pub struct TableData {
    /// Header cells (the first record). Empty when the file is empty.
    pub headers: Vec<String>,
    /// Data rows. Each row may be shorter or longer than `headers` (ragged files are padded on display).
    pub rows: Vec<Vec<String>>,
    /// Column count = max width across the header and every row (short rows are padded when drawn).
    pub ncols: usize,
    /// True when the file had more than `MAX_ROWS` data rows and reading stopped early.
    pub truncated: bool,
}

impl TableData {
    /// The header cell at `col` (empty string when out of range).
    pub fn header(&self, col: usize) -> &str {
        self.headers.get(col).map(String::as_str).unwrap_or("")
    }

    /// The data cell at (`row`, `col`) (empty string for ragged/out-of-range access).
    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Number of data rows (excludes the header).
    pub fn nrows(&self) -> usize {
        self.rows.len()
    }
}

/// A read-only view of whatever the table renderer is showing: a parsed CSV/TSV/archive listing
/// ([`TableData`]) or one sheet of a workbook ([`Sheet`]). Both are borrowed, never converted into
/// each other (a sheet can hold 4M cells — copying it into `TableData` would double that).
///
/// "Rows" always means data rows: a CSV's header record is exposed through [`Grid::header`], and a
/// sheet has no header record (its header is the column letters, see [`column_letters`]).
#[derive(Debug, Clone, Copy)]
pub enum Grid<'a> {
    /// CSV/TSV or an archive listing.
    Csv(&'a TableData),
    /// One sheet of a spreadsheet.
    Sheet(&'a Sheet),
}

impl<'a> Grid<'a> {
    /// Number of data rows.
    pub fn nrows(&self) -> usize {
        match self {
            Grid::Csv(t) => t.nrows(),
            Grid::Sheet(s) => s.nrows,
        }
    }

    /// Number of columns.
    pub fn ncols(&self) -> usize {
        match self {
            Grid::Csv(t) => t.ncols,
            Grid::Sheet(s) => s.ncols,
        }
    }

    /// The header text of `col`: the CSV header cell, or the spreadsheet column letters (`A`, `B`, .. `AA`).
    pub fn header(&self, col: usize) -> Cow<'a, str> {
        match self {
            Grid::Csv(t) => Cow::Borrowed(t.header(col)),
            Grid::Sheet(_) => Cow::Owned(column_letters(col)),
        }
    }

    /// The text shown in a data cell (`""` for an empty/out-of-range cell). For a sheet this is the
    /// cell as Excel displays it (number format applied).
    pub fn cell(&self, row: usize, col: usize) -> &'a str {
        match self {
            Grid::Csv(t) => t.cell(row, col),
            Grid::Sheet(s) => s.display(row, col),
        }
    }

    /// True when the source was cut off at a limit (CSV row cap; sheet row/column cap).
    pub fn truncated(&self) -> bool {
        match self {
            Grid::Csv(t) => t.truncated,
            Grid::Sheet(s) => s.rows_truncated || s.cols_truncated,
        }
    }

    /// Whether this is a spreadsheet sheet (row-number gutter, cell addresses, no header record).
    pub fn is_sheet(&self) -> bool {
        matches!(self, Grid::Sheet(_))
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

// Test-only counter of full-file parses, **thread-local** so tests running in parallel don't see each
// other's work (a process-wide counter proved flaky for exactly that reason).
#[cfg(test)]
thread_local! {
    pub static PARSE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Parse a CSV/TSV file with the given delimiter byte (`b','` for CSV, `b'\t'` for TSV).
///
/// Reads byte records and lossily decodes to UTF-8, so a stray non-UTF-8 byte degrades to `�`
/// rather than failing the whole preview (principle #3). The first record becomes the header.
pub fn parse(path: &Path, delimiter: u8) -> Result<TableData> {
    #[cfg(test)]
    PARSE_CALLS.with(|c| c.set(c.get() + 1));
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true) // allow a variable column count (ragged rows); short rows are padded on display.
        .has_headers(false) // we handle the "first row is the header" convention ourselves.
        .from_path(path)
        .with_context(|| format!("open csv/tsv: {}", path.display()))?;

    let decode = |rec: &csv::ByteRecord| -> Vec<String> {
        rec.iter()
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect()
    };

    let mut rec = csv::ByteRecord::new();
    // header = the first record. An empty file yields an empty table.
    if !rdr
        .read_byte_record(&mut rec)
        .context("read csv/tsv header")?
    {
        return Ok(TableData::default());
    }
    let headers = decode(&rec);
    let mut ncols = headers.len();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut truncated = false;

    while rdr.read_byte_record(&mut rec).context("read csv/tsv row")? {
        if rows.len() >= MAX_ROWS {
            truncated = true;
            break;
        }
        let row = decode(&rec);
        ncols = ncols.max(row.len());
        rows.push(row);
    }

    Ok(TableData {
        headers,
        rows,
        ncols,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

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
        let g = Grid::Csv(&t);
        assert_eq!((g.nrows(), g.ncols()), (1, 2));
        assert_eq!(g.header(1), "h2");
        assert_eq!(g.cell(0, 1), "b");
        assert_eq!(g.cell(5, 5), "");
        assert!(g.truncated());
        assert!(!g.is_sheet());
    }

    #[test]
    fn grid_over_an_empty_sheet_is_empty_with_letter_headers() {
        let s = Sheet::default();
        let g = Grid::Sheet(&s);
        assert_eq!((g.nrows(), g.ncols()), (0, 0));
        assert_eq!(g.header(0), "A");
        assert_eq!(g.cell(0, 0), "");
        assert!(!g.truncated());
        assert!(g.is_sheet());
    }
    use std::io::Write;

    /// Returns the sandbox guard alongside the file path — the directory must outlive the
    /// `parse(&p, ...)` call every caller makes right after, not just this function's own return.
    fn write_temp(name: &str, content: &str) -> (crate::test_support::TmpDir, std::path::PathBuf) {
        let dir = unique_tmp("konoma_table_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (dir, p)
    }

    #[test]
    fn parses_headers_and_rows() {
        let (_dir, p) = write_temp("basic.csv", "a,b,c\n1,2,3\n4,5,6\n");
        let t = parse(&p, b',').unwrap();
        assert_eq!(t.headers, vec!["a", "b", "c"]);
        assert_eq!(t.nrows(), 2);
        assert_eq!(t.ncols, 3);
        assert_eq!(t.cell(0, 0), "1");
        assert_eq!(t.cell(1, 2), "6");
        assert_eq!(t.header(1), "b");
        assert!(!t.truncated);
    }

    #[test]
    fn quoted_comma_stays_one_cell() {
        // A comma inside quotes stays one cell (a naive split would break on it).
        let (_dir, p) = write_temp("quoted.csv", "name,note\n\"Doe, John\",hi\n");
        let t = parse(&p, b',').unwrap();
        assert_eq!(t.cell(0, 0), "Doe, John");
        assert_eq!(t.cell(0, 1), "hi");
    }

    #[test]
    fn tab_delimiter_for_tsv() {
        let (_dir, p) = write_temp("basic.tsv", "x\ty\n10\t20\n");
        let t = parse(&p, b'\t').unwrap();
        assert_eq!(t.headers, vec!["x", "y"]);
        assert_eq!(t.cell(0, 1), "20");
    }

    #[test]
    fn ragged_rows_report_max_columns() {
        // A varying column count per row doesn't crash; ncols = the max. Short rows get empty cells.
        let (_dir, p) = write_temp("ragged.csv", "a,b,c\n1\n4,5,6,7\n");
        let t = parse(&p, b',').unwrap();
        assert_eq!(t.ncols, 4);
        assert_eq!(t.cell(0, 2), ""); // row 0 has only 1 cell
        assert_eq!(t.cell(1, 3), "7");
    }

    #[test]
    fn empty_file_is_empty_table() {
        let (_dir, p) = write_temp("empty.csv", "");
        let t = parse(&p, b',').unwrap();
        assert!(t.headers.is_empty());
        assert_eq!(t.nrows(), 0);
        assert_eq!(t.ncols, 0);
    }
}
