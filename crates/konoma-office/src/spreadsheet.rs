//! The `spreadsheet` plugin: a workbook shown one sheet at a time.
//!
//! **Pages.** The sheets are the plugin's separate pages. `open` only checks that it was handed a
//! file; the first `view` reads the workbook (the list of visible sheets and the cells of the page
//! asked for), and every later `view` reads the workbook again for its page. Only the sheet on
//! screen has cells: a [`SheetGrid`] holds one sheet, the host drops the grid of the sheet it
//! leaves, and the big sheets of a workbook are never held side by side. The page count and the
//! sheet names are reported by [`Session::layout`] after each read, which is how the host learns
//! them (and why `open` can stay cheap: the pages are unknown until a sheet has been read).
//!
//! **Reading.** The reader stops at its first cancel check once the host moves on (every row of a
//! sheet is a check), and the host runs the jobs of this plugin one at a time with the newest
//! waiting one only (the `office` work group), so a held `J` cannot start a read per key repeat.

use std::path::PathBuf;
use std::sync::Arc;

use konoma_office_core::numfmt::builtin_format_code;
use konoma_office_core::{
    load_workbook_sheet_cancellable, Cancel as ReadCancel, CellType, LoadOptions, Locale,
    NumFmtRef, OfficeError, Sheet, Workbook,
};
use konoma_plugin::view::{CellDetail, Grid, Layout, PageLayout, TableView, View, ViewReq};
use konoma_plugin::{t, Failure, Input, Manifest, OpenCx, Plugin, Session, ViewCx};

use crate::texts::{self, cell, ty};

/// Spreadsheet column letters for a 0-based column: `0 -> A`, `25 -> Z`, `26 -> AA`.
fn column_letters(col: usize) -> String {
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
fn cell_address(row: usize, col: usize) -> String {
    format!("{}{}", column_letters(col), row + 1)
}

/// One sheet of a workbook, as the host's table widget reads it. The workbook's other sheets are
/// only names here.
pub struct SheetGrid {
    wb: Workbook,
    /// The sheet whose cells are loaded.
    idx: usize,
    lang: String,
    locale: Locale,
}

impl SheetGrid {
    fn sheet(&self) -> Option<&Sheet> {
        self.wb.sheets.get(self.idx)
    }

    /// The words for the type of `cell` in the user's language.
    fn type_word(&self, ty: CellType) -> String {
        match ty {
            CellType::Number => ty::number(),
            CellType::Text => ty::text(),
            CellType::Bool => ty::boolean(),
            CellType::Error => ty::error(),
            CellType::DateTime => ty::date_time(),
        }
        .get(&self.lang)
        .to_string()
    }
}

impl Grid for SheetGrid {
    fn nrows(&self) -> usize {
        self.sheet().map_or(0, |s| s.nrows)
    }

    fn ncols(&self) -> usize {
        self.sheet().map_or(0, |s| s.ncols)
    }

    /// A sheet has no header record: its header is the column letters.
    fn header(&self, col: usize) -> Option<String> {
        Some(column_letters(col))
    }

    fn cell(&self, row: usize, col: usize) -> String {
        self.cell_str(row, col).unwrap_or_default().to_string()
    }

    /// The text Excel shows (the number format applied). Borrowed from the sheet, so the host
    /// reads every cell of a search without copying any.
    fn cell_str(&self, row: usize, col: usize) -> Option<&str> {
        Some(self.sheet().map_or("", |s| s.display(row, col)))
    }

    /// Up to the last cell that shows something: a stored cell whose format hides its value adds
    /// nothing, so a row copies without trailing tabs.
    fn row_len(&self, row: usize) -> usize {
        self.sheet().map_or(0, |s| {
            s.row_cells(row)
                .iter()
                .rev()
                .find(|(_, c)| !c.display().is_empty())
                .map_or(0, |(col, _)| *col as usize + 1)
        })
    }

    fn truncated(&self) -> bool {
        self.sheet()
            .is_some_and(|s| s.rows_truncated || s.cols_truncated)
    }

    /// A sheet is sparse (up to 16,384 columns): only the cells that exist, in column order.
    fn visit_row(&self, row: usize, f: &mut dyn FnMut(usize, &str)) {
        let Some(sheet) = self.sheet() else {
            return;
        };
        for (col, cell) in sheet.row_cells(row) {
            let shown = cell.display();
            if !shown.is_empty() {
                f(*col as usize, shown);
            }
        }
    }

    /// The address, what Excel shows and everything behind it: the raw value, the type, the
    /// formula (when the cell has one) and the number format (when it is not General).
    fn detail(&self, row: usize, col: usize) -> Option<CellDetail> {
        let sheet = self.sheet()?;
        let mut lines = vec![
            (cell::address(), cell_address(row, col)),
            (cell::displayed(), sheet.display(row, col).to_string()),
        ];
        if let Some(c) = sheet.cell(row, col) {
            lines.push((cell::raw(), c.raw_text()));
            lines.push((cell::type_label(), self.type_word(c.cell_type())));
            if let Some(f) = sheet.formula(row, col) {
                lines.push((cell::formula(), format!("={f}")));
            }
            let code = match self.wb.format_of(c) {
                NumFmtRef::General => None,
                NumFmtRef::Custom(code) => Some(code.to_string()),
                NumFmtRef::Builtin(n) => Some(
                    builtin_format_code(u32::from(*n), self.locale)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("#{n}")),
                ),
            };
            if let Some(code) = code.filter(|c| c != "General") {
                lines.push((cell::format(), code));
            }
        } else {
            // No stored cell: still say it is empty (the type line is the one place that does).
            lines.push((cell::type_label(), ty::empty().get(&self.lang).to_string()));
        }
        Some(CellDetail { lines })
    }
}

/// The locale that decides what a locale-dependent built-in format looks like.
fn locale_of(lang: &str) -> Locale {
    if lang == "ja" {
        Locale::Ja
    } else {
        Locale::En
    }
}

/// The table view of the loaded sheet `idx` of `wb`, in the language `lang`: the sheet's grid and
/// the title note `Sales (1/3) +1 hidden` (the name, the position among the visible sheets, and
/// how many sheets are hidden).
pub fn table_view(wb: Workbook, idx: usize, lang: &str, rainbow: bool) -> TableView {
    let count = wb.sheets.len();
    let name = wb
        .sheets
        .get(idx)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let hidden = if wb.hidden_sheets > 0 {
        format!(" +{} {}", wb.hidden_sheets, texts::hidden_word(lang))
    } else {
        String::new()
    };
    let title = format!("{name} ({}/{count}){hidden}", idx + 1);
    TableView {
        grid: Arc::new(SheetGrid {
            wb,
            idx,
            lang: lang.to_string(),
            locale: locale_of(lang),
        }),
        title: Some(title),
        rainbow,
        addressed: true,
    }
}

/// An opened workbook file. It keeps what the last read learned about the sheets and nothing else.
struct SpreadsheetSession {
    path: PathBuf,
    /// The visible sheets' names, in order (empty until a read has listed them).
    names: Vec<String>,
}

impl Session for SpreadsheetSession {
    fn layout(&self) -> Layout {
        if self.names.is_empty() {
            return Layout::single();
        }
        Layout {
            pages: Some(PageLayout::Separate {
                count: Some(self.names.len()),
                titles: self.names.clone(),
            }),
            ..Layout::default()
        }
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        let page = req.page.unwrap_or(0);
        let opts = LoadOptions {
            locale: locale_of(cx.lang),
            ..LoadOptions::default()
        };
        let cancel = {
            let host = cx.cancel.clone();
            ReadCancel::new(move || host.is_cancelled())
        };
        // The readers sit on third-party parsers: a panic on a pathological file becomes a
        // "corrupt" reason instead of ending the job.
        let loaded = konoma_base::panic::catch_silent(|| {
            load_workbook_sheet_cancellable(&self.path, &opts, page, Some(cancel))
        })
        .unwrap_or_else(|| Err(OfficeError::Corrupt("reader panicked".into())));
        if cx.cancel.is_cancelled() {
            // What was read is partial and the host is not waiting for it.
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        let wb = match loaded {
            Ok(wb) => wb,
            Err(e) => {
                // The file does not open: there are no sheets to switch between.
                self.names.clear();
                return Err(texts::failure_of(&e));
            }
        };
        self.names = wb.sheets.iter().map(|s| s.name.clone()).collect();
        if wb.sheets.is_empty() {
            return Err(Failure::unsupported(texts::no_visible_sheets()));
        }
        // The workbook opened but the sheet asked for could not be read: the other sheets stay
        // reachable (the names are kept above).
        if let Some(e) = &wb.sheet_error {
            return Err(texts::failure_of(e));
        }
        let Some(idx) = wb.loaded_index() else {
            return Err(texts::failure_of(&OfficeError::Corrupt(
                "no sheet was read".into(),
            )));
        };
        Ok(View::Table(table_view(
            wb,
            idx,
            cx.lang,
            cx.settings.bool("rainbow", true),
        )))
    }
}

/// The spreadsheet plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpreadsheetPlugin;

impl Plugin for SpreadsheetPlugin {
    fn manifest(&self) -> Manifest {
        crate::spreadsheet_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(t!(
                en: "this plugin opens files only",
                ja: "このプラグインはファイルだけを開きます",
            )));
        };
        if cx.cancel.is_cancelled() {
            return Err(Failure::cancelled(t!(en: "cancelled", ja: "中止しました")));
        }
        Ok(Box::new(SpreadsheetSession {
            path,
            names: Vec::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_plugin::testing::{check_manifest, open_file};
    use konoma_plugin::{Cancel, FailureKind, Settings, Value};
    use konoma_test_support::{build_xlsx, build_xlsx_with_styles, unique_tmp};

    const STYLES_XML: &str = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode=";;;"/></numFmts><cellXfs count="4"><xf numFmtId="0"/><xf numFmtId="14"/><xf numFmtId="59"/><xf numFmtId="164"/></cellXfs></styleSheet>"#;

    fn x_str(r: &str, text: &str) -> String {
        format!(r#"<c r="{r}" t="inlineStr"><is><t>{text}</t></is></c>"#)
    }

    fn one_row(text: &str) -> String {
        format!(r#"<row r="1">{}</row>"#, x_str("A1", text))
    }

    /// A view of page `page` (counted from 0) as a table, with the layout it left behind.
    fn read(
        session: &mut dyn Session,
        page: usize,
        lang: &str,
        settings: &Settings,
    ) -> Result<TableView, Failure> {
        let req = ViewReq {
            page: Some(page),
            ..ViewReq::default()
        };
        match session.view(&req, &ViewCx::new(settings, lang))? {
            View::Table(t) => Ok(t),
            other => panic!("expected a table: {other:?}"),
        }
    }

    fn titles_of(session: &dyn Session) -> Option<(Option<usize>, Vec<String>)> {
        match session.layout().pages {
            Some(PageLayout::Separate { count, titles }) => Some((count, titles)),
            _ => None,
        }
    }

    #[test]
    fn the_manifest_is_clean_binds_the_workbook_globs_and_joins_the_office_group() {
        assert!(check_manifest(&SpreadsheetPlugin).is_empty());
        let m = SpreadsheetPlugin.manifest();
        assert_eq!(m.id, crate::SPREADSHEET_ID);
        assert_eq!(m.work_group.as_deref(), Some(crate::OFFICE_WORK_GROUP));
        assert_eq!(m.api, konoma_plugin::ApiVersion::CURRENT);
        assert_eq!(m.verbs.len(), 2);
        assert!(m.settings.iter().any(|s| s.name == "rainbow"));
    }

    #[test]
    fn the_sheets_are_the_pages_and_are_known_after_the_first_read() {
        let dir = unique_tmp("konoma_office_pages");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.xlsx");
        build_xlsx(
            &p,
            &[
                ("Sales", "visible", &one_row("first"), ""),
                ("Secret", "hidden", &one_row("hidden"), ""),
                ("Costs", "visible", &one_row("third"), ""),
            ],
        );
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        // `open` only checks the input: the pages are not known until a sheet has been read.
        assert_eq!(s.layout(), Layout::single());
        let settings = Settings::new();
        let t = read(&mut *s, 0, "en", &settings).unwrap();
        assert_eq!(
            titles_of(&*s),
            Some((Some(2), vec!["Sales".into(), "Costs".into()]))
        );
        assert_eq!(t.title.as_deref(), Some("Sales (1/2) +1 hidden"));
        assert!(t.addressed && t.rainbow);
        assert_eq!(t.grid.cell(0, 0), "first");
        // The second visible sheet; only its cells are held.
        let t = read(&mut *s, 1, "ja", &settings).unwrap();
        assert_eq!(t.title.as_deref(), Some("Costs (2/2) +1 非表示"));
        assert_eq!(t.grid.cell(0, 0), "third");
        // A page past the end is the last one.
        let t = read(&mut *s, 9, "en", &settings).unwrap();
        assert_eq!(t.title.as_deref(), Some("Costs (2/2) +1 hidden"));
    }

    #[test]
    fn a_title_without_hidden_sheets_has_no_hidden_note() {
        let dir = unique_tmp("konoma_office_title");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("one.xlsx");
        build_xlsx(&p, &[("Only", "visible", &one_row("x"), "")]);
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let t = read(&mut *s, 0, "en", &Settings::new()).unwrap();
        assert_eq!(t.title.as_deref(), Some("Only (1/1)"));
    }

    #[test]
    fn the_rainbow_setting_reaches_the_view() {
        let dir = unique_tmp("konoma_office_rainbow");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("r.xlsx");
        build_xlsx(&p, &[("S", "visible", &one_row("x"), "")]);
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let off = Settings::new().with("rainbow", Value::Bool(false));
        assert!(!read(&mut *s, 0, "en", &off).unwrap().rainbow);
        assert!(read(&mut *s, 0, "en", &Settings::new()).unwrap().rainbow);
    }

    #[test]
    fn the_grid_reads_what_excel_shows_and_copies_up_to_the_last_shown_cell() {
        let dir = unique_tmp("konoma_office_grid");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.xlsx");
        // Row 1: "x", "y" and a number whose format (`;;;`) shows nothing; row 3: one far cell.
        build_xlsx_with_styles(
            &p,
            &[(
                "S",
                "visible",
                &format!(
                    r#"<row r="1">{}{}<c r="C1" s="3"><v>5</v></c></row><row r="3">{}</row>"#,
                    x_str("A1", "x"),
                    x_str("B1", "y"),
                    x_str("E3", "far"),
                ),
                "",
            )],
            Some(STYLES_XML),
        );
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let g = read(&mut *s, 0, "en", &Settings::new()).unwrap().grid;
        assert_eq!((g.nrows(), g.ncols()), (3, 5));
        assert_eq!(g.header(0).as_deref(), Some("A"));
        assert_eq!(g.header(4).as_deref(), Some("E"));
        assert_eq!(g.cell(0, 1), "y");
        assert_eq!(g.cell_str(0, 1), Some("y"));
        assert_eq!(g.cell(1, 1), "", "an empty cell");
        assert_eq!(g.cell(9, 9), "", "out of range");
        // The hidden C1 adds nothing to the row; a row of nothing is nothing.
        assert_eq!(g.row_len(0), 2);
        assert_eq!(g.row_len(1), 0);
        assert_eq!(g.row_len(2), 5);
        assert!(!g.truncated());
        // The search visits only the cells that show something, in column order.
        let mut seen = Vec::new();
        g.visit_row(0, &mut |c, t| seen.push((c, t.to_string())));
        assert_eq!(seen, [(0, "x".to_string()), (1, "y".to_string())]);
        let mut none = 0;
        g.visit_row(1, &mut |_, _| none += 1);
        g.visit_row(99, &mut |_, _| none += 1);
        assert_eq!(none, 0);
    }

    #[test]
    fn the_cell_detail_names_the_address_value_type_and_format_in_the_users_language() {
        let dir = unique_tmp("konoma_office_detail");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("d.xlsx");
        build_xlsx_with_styles(
            &p,
            &[(
                "S",
                "visible",
                &format!(
                    r#"<row r="1"><c r="A1" t="b"><v>1</v></c></row><row r="2"><c r="A2" s="1"><v>45296</v></c></row><row r="3"><c r="A3" s="2"><v>5</v></c>{}</row>"#,
                    x_str("B3", "plain")
                ),
                "",
            )],
            Some(STYLES_XML),
        );
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let line = |g: &Arc<dyn Grid>, r: usize, c: usize, lang: &str| -> Vec<String> {
            g.detail(r, c)
                .unwrap()
                .lines
                .iter()
                .map(|(l, v)| format!("{}: {v}", l.get(lang)))
                .collect()
        };
        let en = read(&mut *s, 0, "en", &Settings::new()).unwrap().grid;
        // A boolean in General format: no format line.
        let a1 = line(&en, 0, 0, "en");
        assert_eq!(
            a1,
            [
                "Cell: A1",
                "Displayed: TRUE",
                "Raw value: TRUE",
                "Type: boolean"
            ]
        );
        // A date: the built-in format 14 is month-first in English.
        let a2 = line(&en, 1, 0, "en");
        assert!(a2.contains(&"Displayed: 1/5/2024".to_string()), "{a2:?}");
        assert!(a2.contains(&"Raw value: 45296".to_string()), "{a2:?}");
        assert!(a2.contains(&"Type: date / time".to_string()), "{a2:?}");
        assert!(
            a2.contains(&"Number format: m/d/yyyy".to_string()),
            "{a2:?}"
        );
        // A built-in id with no code of its own is shown as `#59`.
        assert!(line(&en, 2, 0, "en").contains(&"Number format: #59".to_string()));
        // An empty cell still says what it is.
        assert_eq!(
            line(&en, 0, 3, "en"),
            ["Cell: D1", "Displayed: ", "Type: empty"]
        );
        // Japanese: the words and the year-first date.
        let ja = read(&mut *s, 0, "ja", &Settings::new()).unwrap().grid;
        let a2 = line(&ja, 1, 0, "ja");
        assert!(a2.contains(&"セル: A2".to_string()), "{a2:?}");
        assert!(a2.contains(&"表示: 2024/1/5".to_string()), "{a2:?}");
        assert!(a2.contains(&"型: 日付・時刻".to_string()), "{a2:?}");
        assert!(a2.contains(&"表示書式: yyyy/m/d".to_string()), "{a2:?}");
    }

    #[test]
    fn a_workbook_that_does_not_open_says_why_and_has_no_pages() {
        let dir = unique_tmp("konoma_office_fail");
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.xlsx");
        std::fs::write(&junk, b"this is not a workbook at all").unwrap();
        let mut s = open_file(&SpreadsheetPlugin, &junk).unwrap();
        let f = read(&mut *s, 0, "en", &Settings::new()).err().unwrap();
        assert_eq!(f.kind, FailureKind::Corrupt);
        assert!(f.message.en().contains("damaged"), "{}", f.message.en());
        assert!(f.message.get("ja").contains("壊れている"));
        assert_eq!(s.layout(), Layout::single());
        // A name that is no workbook format at all.
        let other = dir.join("a.txt");
        std::fs::write(&other, b"x").unwrap();
        let mut s = open_file(&SpreadsheetPlugin, &other).unwrap();
        let f = read(&mut *s, 0, "en", &Settings::new()).err().unwrap();
        assert_eq!(f.kind, FailureKind::Unsupported);
        assert!(f.message.en().contains("not a spreadsheet format"));
        // A missing file is not a crash either.
        let mut s = open_file(&SpreadsheetPlugin, &dir.join("nope.xlsx")).unwrap();
        assert!(read(&mut *s, 0, "en", &Settings::new()).is_err());
    }

    #[test]
    fn a_workbook_with_every_sheet_hidden_has_nothing_to_show() {
        let dir = unique_tmp("konoma_office_hidden");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("h.xlsx");
        build_xlsx(&p, &[("A", "hidden", "", ""), ("B", "veryHidden", "", "")]);
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let f = read(&mut *s, 0, "en", &Settings::new()).err().unwrap();
        assert!(f
            .message
            .en()
            .contains("every sheet in this workbook is hidden"));
        assert!(f.message.get("ja").contains("すべてのシートが非表示"));
        assert_eq!(s.layout(), Layout::single(), "no sheet to switch to");
    }

    #[test]
    fn a_damaged_sheet_fails_alone_and_the_other_sheets_stay_reachable() {
        let dir = unique_tmp("konoma_office_damaged");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("d.xlsx");
        build_xlsx(
            &p,
            &[
                ("Good", "visible", &one_row("fine"), ""),
                // A mismatched end tag: the sheet part is not well-formed XML.
                (
                    "Bad",
                    "visible",
                    "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row></nope>",
                    "",
                ),
                ("Last", "visible", &one_row("tail"), ""),
            ],
        );
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let settings = Settings::new();
        assert!(read(&mut *s, 0, "en", &settings).is_ok());
        let f = read(&mut *s, 1, "en", &settings).err().unwrap();
        assert!(f.message.en().contains("damaged"));
        // The failure keeps the pages: the host can still turn to the next sheet.
        assert_eq!(titles_of(&*s).map(|t| t.0), Some(Some(3)));
        let t = read(&mut *s, 2, "en", &settings).unwrap();
        assert_eq!(t.grid.cell(0, 0), "tail");
    }

    #[test]
    fn a_cancelled_read_gives_up_and_a_cancelled_open_does_not_start() {
        let dir = unique_tmp("konoma_office_cancel");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.xlsx");
        build_xlsx(&p, &[("S", "visible", &one_row("x"), "")]);
        let mut s = open_file(&SpreadsheetPlugin, &p).unwrap();
        let settings = Settings::new();
        let cx = ViewCx::new(&settings, "en").with_cancel(Cancel::from_fn(|| true));
        let f = s.view(&ViewReq::default(), &cx).err().unwrap();
        assert_eq!(f.kind, FailureKind::Cancelled);
        let cx = OpenCx::new(&settings, "en").with_cancel(Cancel::from_fn(|| true));
        let err = SpreadsheetPlugin
            .open(
                Input::File {
                    path: p,
                    hint: None,
                },
                &cx,
            )
            .err()
            .unwrap();
        assert_eq!(err.kind, FailureKind::Cancelled);
    }

    #[test]
    fn only_a_file_input_opens() {
        let err = SpreadsheetPlugin
            .open(
                Input::Embed {
                    kind: "x".into(),
                    source: String::new(),
                    display: false,
                },
                &OpenCx::bare(),
            )
            .err()
            .unwrap();
        assert_eq!(err.kind, FailureKind::Unsupported);
    }

    #[test]
    fn column_letters_follow_the_spreadsheet_scheme() {
        for (col, want) in [
            (0, "A"),
            (25, "Z"),
            (26, "AA"),
            (51, "AZ"),
            (701, "ZZ"),
            (702, "AAA"),
            (16_383, "XFD"),
        ] {
            assert_eq!(column_letters(col), want, "col {col}");
        }
        assert_eq!(cell_address(2, 1), "B3");
    }
}
