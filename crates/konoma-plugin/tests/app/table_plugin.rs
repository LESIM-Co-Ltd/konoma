//! A table plugin for the app tests.
//!
//! - `*.grid`: comma-separated rows; the first line is the header (a csv).
//! - `*.book`: sheets separated by a line `===`, each sheet's rows comma-separated, no header:
//!   the cells are addressed (`A1`), the title note names the sheet (`Sheet2 (2/3)`) and the
//!   layout declares the sheets as pages. The session counts its `view` calls in `views`.
//!   A cell may hold `\n` written as the two characters `\n` (a multi-line cell).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use konoma_plugin::prelude::*;
use konoma_plugin::view::{CellDetail, Grid, PageLayout, TableView};

pub struct Rows {
    pub header: Option<Vec<String>>,
    pub rows: Vec<Vec<String>>,
    pub ncols: usize,
    pub addressed: bool,
}

impl Rows {
    fn parse(text: &str, header: bool, addressed: bool) -> Rows {
        let mut lines: Vec<Vec<String>> = text
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.split(',').map(|c| c.replace("\\n", "\n")).collect())
            .collect();
        let header = (header && !lines.is_empty()).then(|| lines.remove(0));
        let ncols = lines
            .iter()
            .map(Vec::len)
            .chain(header.iter().map(Vec::len))
            .max()
            .unwrap_or(0);
        Rows {
            header,
            rows: lines,
            ncols,
            addressed,
        }
    }
}

impl Grid for Rows {
    fn nrows(&self) -> usize {
        self.rows.len()
    }
    fn ncols(&self) -> usize {
        self.ncols
    }
    fn header(&self, col: usize) -> Option<String> {
        if self.addressed {
            return Some(konoma_widgets::table::column_letters(col));
        }
        self.header.as_ref().and_then(|h| h.get(col).cloned())
    }
    fn cell(&self, row: usize, col: usize) -> String {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
            .unwrap_or_default()
    }
    fn detail(&self, row: usize, col: usize) -> Option<CellDetail> {
        self.addressed.then(|| CellDetail {
            lines: vec![
                (t!(en: "value"), self.cell(row, col)),
                (t!(en: "kind"), "text".to_string()),
            ],
        })
    }
}

pub struct TablePlugin {
    /// How many times any session was asked for a view.
    pub views: Arc<AtomicUsize>,
}

impl TablePlugin {
    pub fn new() -> (TablePlugin, Arc<AtomicUsize>) {
        let views = Arc::new(AtomicUsize::new(0));
        (
            TablePlugin {
                views: views.clone(),
            },
            views,
        )
    }
}

impl Plugin for TablePlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("app-table-test", "0.0.1")
            .bind(Bind::files(["*.grid", "*.book"]))
            .shows(ViewKind::Table)
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(t!(en: "a file only")));
        };
        let text = std::fs::read_to_string(&path)?;
        let book = path.extension().is_some_and(|e| e == "book");
        let sheets: Vec<String> = if book {
            text.split("\n===\n").map(str::to_string).collect()
        } else {
            vec![text]
        };
        Ok(Box::new(TableSession {
            sheets,
            book,
            views: self.views.clone(),
        }))
    }
}

struct TableSession {
    sheets: Vec<String>,
    book: bool,
    views: Arc<AtomicUsize>,
}

impl Session for TableSession {
    fn layout(&self) -> Layout {
        let mut l = Layout::single();
        if self.book {
            l.pages = Some(PageLayout::Separate {
                count: Some(self.sheets.len()),
                titles: (1..=self.sheets.len())
                    .map(|i| format!("Sheet{i}"))
                    .collect(),
            });
        }
        l
    }

    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        self.views.fetch_add(1, Ordering::SeqCst);
        let page = req
            .page
            .unwrap_or(0)
            .min(self.sheets.len().saturating_sub(1));
        let grid = Rows::parse(&self.sheets[page], !self.book, self.book);
        Ok(View::Table(TableView {
            grid: Arc::new(grid),
            title: self
                .book
                .then(|| format!("Sheet{} ({}/{})", page + 1, page + 1, self.sheets.len())),
            rainbow: false,
            addressed: self.book,
        }))
    }
}
