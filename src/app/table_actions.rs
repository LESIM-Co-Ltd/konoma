use super::search::SEARCH_MATCH_CAP;
use super::*;
use crate::preview::office::{CellType, NumFmtRef};
use crate::preview::table::{cell_address, Grid, TableData};

/// What the table renderer shows right now, built from borrowed *fields* (not `&self`) so callers
/// can keep reading the grid while writing a different field such as `tab.table_cur_row`.
/// A loaded workbook wins (it is only ever set while a spreadsheet preview is active); otherwise
/// the CSV/TSV/archive table. `None` while loading, after a failed load, or with no visible sheet.
fn grid_of<'a>(
    table: &'a Option<TableData>,
    workbook: &'a Option<Box<crate::preview::office::Workbook>>,
    sheet_idx: usize,
) -> Option<Grid<'a>> {
    if let Some(wb) = workbook {
        // Only the sheet on screen has cells: while another one is being read (or failed to
        // read) there is no grid, whatever was loaded before.
        return wb
            .sheets
            .get(sheet_idx)
            .filter(|s| s.loaded)
            .map(Grid::Sheet);
    }
    table.as_ref().map(Grid::Csv)
}

/// Frees `value` on a thread of its own (one, shared, so holding a key does not start a thread
/// per press). The UI thread must not spend tens of milliseconds freeing a large workbook; if the
/// thread cannot be had the value is freed here, as it would have been.
pub(super) fn discard_in_background<T: Send + 'static>(value: T) {
    use std::sync::{mpsc, Mutex, OnceLock};
    type Junk = Box<dyn Send>;
    static REAPER: OnceLock<Option<Mutex<mpsc::Sender<Junk>>>> = OnceLock::new();
    let reaper = REAPER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Junk>();
        std::thread::Builder::new()
            .name("konoma-reaper".into())
            .spawn(move || {
                // Dropping each item as it arrives is the whole job.
                for junk in rx {
                    drop(junk);
                }
            })
            .ok()
            .map(|_| Mutex::new(tx))
    });
    if let Some(tx) = reaper {
        // A send that fails hands the value back inside the error, which is freed right here.
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(Box::new(value));
            return;
        }
    }
    drop(value);
}

/// A table lighter than this (see [`row_weight`]) is searched on the calling thread: starting
/// threads costs a few hundred microseconds, which only a bigger table earns back.
const PARALLEL_SEARCH_MIN_WEIGHT: usize = 2_000;
/// Bytes of displayed text that weigh as much as one cell: a cell is about this long on average,
/// so a few cells with megabytes of text each weigh like the millions of short ones they cost.
const BYTES_PER_WEIGHT: usize = 32;

/// The cost of searching a row of `cells` cells holding `bytes` bytes of displayed text.
fn row_weight(cells: usize, bytes: usize) -> usize {
    cells.saturating_add(bytes / BYTES_PER_WEIGHT)
}
/// Most threads a table search uses.
const MAX_SEARCH_THREADS: usize = 8;

/// The cells of rows `0..nrows` that `scan_row` reports (it appends the matches of one row, in
/// column order), in reading order, at most [`SEARCH_MATCH_CAP`] of them, and whether there were
/// more. A big table (millions of cells, or a few with megabytes of text each) is split into runs
/// of rows of about the same weight and searched on several threads at once: the scan is on the
/// UI thread and has to stay within a frame, and a search is nothing but independent reads.
/// `weigh` is the [`row_weight`] of a row. The result is the one a single thread gives.
fn scan_rows(
    nrows: usize,
    weigh: impl Fn(usize) -> usize,
    scan_row: impl Fn(usize, &mut Vec<(u64, usize, usize)>) + Sync,
) -> (Vec<(u64, usize, usize)>, bool) {
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    scan_rows_with(nrows, weigh, scan_row, threads)
}

fn scan_rows_with(
    nrows: usize,
    weigh: impl Fn(usize) -> usize,
    scan_row: impl Fn(usize, &mut Vec<(u64, usize, usize)>) + Sync,
    threads: usize,
) -> (Vec<(u64, usize, usize)>, bool) {
    let weights: Vec<usize> = (0..nrows).map(&weigh).collect();
    let total: usize = weights.iter().fold(0, |a, w| a.saturating_add(*w));
    let threads = threads.min(MAX_SEARCH_THREADS).min(nrows);
    let run = |rows: std::ops::Range<usize>| {
        let mut out = Vec::new();
        for r in rows {
            scan_row(r, &mut out);
            // One past the cap: that extra hit is how a cut-off search is told from one with
            // exactly the cap.
            if out.len() > SEARCH_MATCH_CAP {
                break;
            }
        }
        out
    };
    let mut out = if threads < 2 || total < PARALLEL_SEARCH_MIN_WEIGHT {
        run(0..nrows)
    } else {
        // Runs of rows of about `total / threads` weight each, in order.
        let per = total.div_ceil(threads);
        let mut runs = Vec::with_capacity(threads);
        let (mut start, mut weight) = (0, 0usize);
        for (r, w) in weights.iter().enumerate() {
            weight = weight.saturating_add(*w);
            if weight >= per && runs.len() + 1 < threads {
                runs.push(start..r + 1);
                (start, weight) = (r + 1, 0);
            }
        }
        runs.push(start..nrows);
        std::thread::scope(|scope| {
            let handles: Vec<_> = runs
                .into_iter()
                .map(|rows| {
                    let run = &run;
                    scope.spawn(move || run(rows))
                })
                .collect();
            // Joined in order, which is reading order.
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        })
    };
    let truncated = out.len() > SEARCH_MATCH_CAP;
    out.truncate(SEARCH_MATCH_CAP);
    (out, truncated)
}

/// A search query for table cells: case-insensitive "contains", without allocating per cell.
/// (`cell.to_lowercase().contains(..)` allocated a string for every cell: 60-100 ms on a
/// maximum-size sheet, on the UI thread.)
///
/// **The query and the cell text go through the same folding, character by character**
/// ([`fold`]). (An earlier version lower-cased the query as a whole string and the text one
/// character at a time: the two disagree on a word-final `Σ`, which `str::to_lowercase` turns into
/// `ς` from its context and `char::to_lowercase` into `σ`, so `ΟΔΟΣ` found neither `ΟΔΟΣ` nor
/// `οδος`.)
struct Needle {
    /// The query folded, as characters.
    chars: Vec<char>,
    /// The query as the bytes a **byte search** compares (ASCII letters lower-cased, everything
    /// else as UTF-8), when such a search finds exactly what the folding would. That holds when
    /// no character of the query other than an ASCII letter is the result of folding some other
    /// character: ASCII letters are compared case-insensitively on the bytes, a CJK or other
    /// uncased character only equals itself, and UTF-8 is self-synchronising, so a byte match is
    /// a match of whole characters. `None` for a query with a cased non-ASCII letter (`é`,
    /// `Σ`, `ω`...), which needs the character-by-character comparison.
    bytes: Option<Vec<u8>>,
    /// The byte search is only exact for text without [`FOLDS_TO_ASCII`] when the query has one of
    /// the ASCII letters they fold to.
    has_folded_to_ascii_letter: bool,
    /// Where a match of the folded text can start (see [`Start`]).
    starts: Vec<Start>,
    /// The leading byte of every start character: the scan of a text stops only at these.
    lead_bytes: [bool; 256],
}

/// The characters outside ASCII whose folding contains an ASCII letter: the Kelvin sign `K`
/// (`k`) and `İ` (`i` and a combining dot). Text with one of them can match an ASCII query in
/// a way a byte search does not see; a pinned test scans all of Unicode for the complete list.
const FOLDS_TO_ASCII: [char; 2] = ['\u{212A}', '\u{130}'];
/// The ASCII letters those fold to.
const FOLDED_TO_ASCII: [u8; 2] = *b"ki";

/// The case folding of search: full Unicode lower-casing of one character, and `ς` (final sigma)
/// the same as `σ`, so `Σ`, `σ` and `ς` are one letter whatever the position in the word.
fn fold(c: char) -> impl Iterator<Item = char> {
    c.to_lowercase().map(|l| if l == 'ς' { 'σ' } else { l })
}

/// Every character that is the result of folding a *different* character (sorted). Characters
/// above U+1FFFF have no case, so the scan stops there (a test checks the whole range).
fn fold_results() -> &'static [char] {
    static RESULTS: std::sync::OnceLock<Vec<char>> = std::sync::OnceLock::new();
    RESULTS.get_or_init(|| fold_results_up_to(0x1FFFF))
}

fn fold_results_up_to(last: u32) -> Vec<char> {
    let mut out: Vec<char> = (0..=last)
        .filter_map(char::from_u32)
        .flat_map(|c| {
            let folded: Vec<char> = fold(c).collect();
            if folded == [c] {
                Vec::new()
            } else {
                folded
            }
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A text longer than this many bytes is searched for each start character with the substring
/// search of std, a shorter one with a byte loop (see `Needle::found_in_by_chars`).
const LONG_TEXT: usize = 256;

/// A character a match of the folded text can start at: one whose folding *begins* with the
/// query's first folded character. A match starts at one of these and nowhere else, so the
/// character search looks only at their occurrences instead of folding at every position of
/// every cell.
struct Start {
    ch: char,
    /// How many query characters its folding already matches (more than one when it folds to
    /// several, as `İ` does).
    matched: usize,
    /// The bytes that can begin the next character of a text for the match to go on (the leading
    /// byte of every character that folds to start with the query's next character); `None` when
    /// this character alone is the whole query.
    next_lead: Option<Box<[bool; 256]>>,
}

/// The characters that fold to something other than themselves, as `(first folded character,
/// the character)`, sorted. About 1,400 entries; built once. Characters above U+1FFFF have no
/// case, so the scan stops there, as for [`fold_results`].
fn fold_sources() -> &'static [(char, char)] {
    static SOURCES: std::sync::OnceLock<Vec<(char, char)>> = std::sync::OnceLock::new();
    SOURCES.get_or_init(|| {
        let mut out: Vec<(char, char)> = (0..=0x1FFFFu32)
            .filter_map(char::from_u32)
            .filter_map(|c| match fold(c).next() {
                Some(f) if f != c => Some((f, c)),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    })
}

/// The characters `c` with `fold(c)` beginning with `first`.
fn chars_folding_first_to(first: char) -> Vec<char> {
    let all = fold_sources();
    let at = all.partition_point(|&(f, _)| f < first);
    let mut out: Vec<char> = all[at..]
        .iter()
        .take_while(|&&(f, _)| f == first)
        .map(|&(_, c)| c)
        .collect();
    if fold(first).next() == Some(first) {
        out.push(first);
    }
    out
}

impl Needle {
    fn new(q: &str) -> Needle {
        let chars: Vec<char> = q.chars().flat_map(fold).collect();
        let bytes = (chars
            .iter()
            .all(|c| c.is_ascii() || fold_results().binary_search(c).is_err()))
        .then(|| {
            chars
                .iter()
                .collect::<String>()
                .into_bytes()
                .into_iter()
                .map(|b| b.to_ascii_lowercase())
                .collect::<Vec<u8>>()
        });
        let has_folded_to_ascii_letter = chars
            .iter()
            .any(|&c| c.is_ascii() && FOLDED_TO_ASCII.contains(&(c as u8)));
        let mut starts = Vec::new();
        let mut lead_bytes = [false; 256];
        if let Some(&first) = chars.first() {
            for ch in chars_folding_first_to(first) {
                let folded: Vec<char> = fold(ch).collect();
                let matched = folded.len().min(chars.len());
                // A character whose folding disagrees with the query past its first character
                // (`İ` for a query `ix`) cannot start a match.
                if folded[..matched] != chars[..matched] {
                    continue;
                }
                let next_lead = chars.get(matched).map(|&next| {
                    let mut lead = Box::new([false; 256]);
                    for c in chars_folding_first_to(next) {
                        lead[c.to_string().as_bytes()[0] as usize] = true;
                    }
                    lead
                });
                lead_bytes[ch.to_string().as_bytes()[0] as usize] = true;
                starts.push(Start {
                    ch,
                    matched,
                    next_lead,
                });
            }
        }
        Needle {
            chars,
            bytes,
            has_folded_to_ascii_letter,
            starts,
            lead_bytes,
        }
    }

    /// Whether `hay` contains the query, ignoring case. Most text is searched as bytes (no
    /// decoding, no allocation): ASCII text, and any text for a query whose byte match is exact
    /// (`bytes`) unless the text holds a character that folds to an ASCII letter the query has.
    /// Otherwise characters are folded one at a time (full Unicode lower-casing, so `É` matches
    /// `é` and the Kelvin sign matches `k`).
    fn found_in(&self, hay: &str) -> bool {
        if self.chars.is_empty() {
            return true;
        }
        match self.bytes.as_deref() {
            Some(n) => {
                // A byte match is a match of the folded text, whatever else the text holds.
                if bytes_contain(hay.as_bytes(), n) {
                    return true;
                }
                // A miss is only a miss when no character of the text folds to a letter the
                // query has (the Kelvin sign for `k`, `İ` for `i`).
                self.has_folded_to_ascii_letter
                    && !hay.is_ascii()
                    && has_folds_to_ascii(hay)
                    && self.found_in_by_chars(hay)
            }
            // A query with a cased non-ASCII letter cannot match ASCII-only text.
            None if hay.is_ascii() => false,
            None => self.found_in_by_chars(hay),
        }
    }

    /// Whether the folded text contains the folded query: it starts at a character whose folding
    /// begins with the query's first character, so only those are tried. (The reference, folding
    /// at every position, is `found_in_at_every_position` in the tests; this must agree with it.)
    fn found_in_by_chars(&self, hay: &str) -> bool {
        // A long text is scanned for each start character with std's substring search (a word
        // at a time, several times faster than a byte loop); a short one, the common cell, with
        // one pass of a byte loop, which has no per-search set-up to pay for.
        if hay.len() > LONG_TEXT {
            return self.starts.iter().any(|st| {
                hay.match_indices(st.ch)
                    .any(|(i, _)| self.continues(hay, st, i + st.ch.len_utf8()))
            });
        }
        let bytes = hay.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            // A leading byte of a start character (a byte of this kind is never in the middle
            // of a character, so `i` is a character boundary).
            if !self.lead_bytes[usize::from(b)] {
                continue;
            }
            let Some(c) = hay[i..].chars().next() else {
                continue;
            };
            if self
                .starts
                .iter()
                .filter(|st| st.ch == c)
                .any(|st| self.continues(hay, st, i + c.len_utf8()))
            {
                return true;
            }
        }
        false
    }

    /// Whether a match that began with the start character `st`, ending at byte `after` of
    /// `hay`, goes on to the end of the query.
    fn continues(&self, hay: &str, st: &Start, after: usize) -> bool {
        // The next byte must be one that can begin the next character of the query.
        if let Some(lead) = &st.next_lead {
            match hay.as_bytes().get(after) {
                Some(&n) if lead[usize::from(n)] => {}
                _ => return false,
            }
        }
        self.rest_matches(&hay[after..], st.matched)
    }

    /// Whether the folded characters of `s` continue the query from its `k`th character (ASCII
    /// folds without the Unicode tables, which are a binary search per character).
    fn rest_matches(&self, s: &str, mut k: usize) -> bool {
        let want = &self.chars;
        for c in s.chars() {
            if k >= want.len() {
                return true;
            }
            if c.is_ascii() {
                if want[k] != c.to_ascii_lowercase() {
                    return false;
                }
                k += 1;
            } else {
                for f in fold(c) {
                    if k >= want.len() {
                        return true;
                    }
                    if want[k] != f {
                        return false;
                    }
                    k += 1;
                }
            }
        }
        k >= want.len()
    }
}

/// Whether the text has a character of [`FOLDS_TO_ASCII`] (two substring searches, which std
/// does with a fast byte scan; only made for a text the byte search did not find the query in).
fn has_folds_to_ascii(hay: &str) -> bool {
    FOLDS_TO_ASCII.iter().any(|&c| hay.contains(c))
}

/// Whether `h` contains `n`, comparing ASCII letters without regard to case (`n` has them in
/// lower case) and everything else byte for byte.
fn bytes_contain(h: &[u8], n: &[u8]) -> bool {
    if h.len() < n.len() {
        return false;
    }
    let (lo, up) = (n[0], n[0].to_ascii_uppercase());
    let last = h.len() - n.len();
    let mut i = 0;
    while i <= last {
        match h[i..=last].iter().position(|&b| b == lo || b == up) {
            None => return false,
            Some(p) => {
                i += p;
                if h[i..i + n.len()].eq_ignore_ascii_case(n) {
                    return true;
                }
                i += 1;
            }
        }
    }
    false
}

impl App {
    // ---- CSV/TSV table preview ------------------------------------------

    /// Parse the current Table/Archive-kind preview into `table_data` (None on failure = raw-text
    /// fallback / can-not-preview). Does not touch the cursor/scroll (callers reset or restore
    /// those as appropriate).
    pub(super) fn load_table(&mut self) {
        self.table_data = None;
        match self.tab.preview_kind.clone() {
            Some(PreviewKind::Table { path, delimiter }) => {
                if let Ok(t) = crate::preview::table::parse(&path, delimiter) {
                    self.table_data = Some(t);
                }
            }
            // Listing an archive relies on third-party crates (zip/tar). Even an unlikely panic is
            // swallowed by catch_silent, and a failure becomes None (→ safely degrades to [can not preview]).
            Some(PreviewKind::Archive { path, kind }) => {
                self.table_data = crate::preview::markdown::catch_silent(|| {
                    crate::preview::archive::list(&path, kind)
                })
                .and_then(Result::ok);
            }
            _ => {}
        }
    }

    /// Clamp the cell cursor into the table's bounds (after a reload/restore that may have shrunk it).
    pub(super) fn clamp_table_cursor(&mut self) {
        // A spreadsheet that is still loading (or failed) has nothing to clamp against yet: keep the
        // saved sheet number and cursor so they apply when the workbook lands.
        if matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && self.grid().is_none()
        {
            return;
        }
        if let Some(wb) = &self.workbook {
            self.tab.sheet_idx = self.tab.sheet_idx.min(wb.sheets.len().saturating_sub(1));
        }
        let dims = self.grid().map(|g| (g.nrows(), g.ncols()));
        match dims {
            Some((nr, nc)) if nr > 0 && nc > 0 => {
                self.tab.table_cur_row = self.tab.table_cur_row.min(nr - 1);
                self.tab.table_cur_col = self.tab.table_cur_col.min(nc - 1);
            }
            _ => {
                self.tab.table_cur_row = 0;
                self.tab.table_cur_col = 0;
            }
        }
    }

    /// Whether a CSV/TSV/archive table preview is active **and parsed** (routes the PreviewTable
    /// surface / renderer). A Table/Archive kind whose parse failed returns false → the preview
    /// degrades to raw text (CSV/TSV) or a can-not-preview-style hint (archive).
    ///
    /// A spreadsheet counts as soon as its workbook is open with at least one visible sheet, even
    /// while the cells of the sheet just switched to are still being read: the surface (keys,
    /// footer, help) stays the table's, so `J`/`K` keep working. The renderer draws the spinner
    /// (`is_sheet_loading`) or the reason the sheet could not be read until the grid is there.
    pub fn is_table_preview(&self) -> bool {
        match self.tab.preview_kind {
            Some(PreviewKind::Table { .. } | PreviewKind::Archive { .. }) => self.grid().is_some(),
            Some(PreviewKind::Spreadsheet(_)) => {
                self.workbook.as_ref().is_some_and(|w| !w.sheets.is_empty())
            }
            _ => false,
        }
    }

    /// Sets what the preview shows. **The one place a preview kind is assigned**: the parsed
    /// spreadsheet is App-level state that can hold hundreds of MB, so whenever the preview is
    /// not a spreadsheet (a git diff, the tree, a diagram, another file) it is released here.
    /// This keeps the invariant "`workbook` is `Some` only while the kind is `Spreadsheet`"
    /// (`App::workbook_matches_preview`) true after every operation, instead of each caller
    /// remembering to reset it.
    pub(super) fn set_preview_kind(&mut self, kind: Option<PreviewKind>) {
        if !matches!(kind, Some(PreviewKind::Spreadsheet(_))) {
            self.set_workbook(None);
            self.workbook_error = None;
        }
        // Likewise the converted Word document: App-level, only while the kind is `Document`.
        if !matches!(kind, Some(PreviewKind::Document(_))) {
            self.set_document(None);
            self.document_error = None;
        }
        self.tab.preview_kind = kind;
    }

    /// Replaces the open workbook. **The one place a workbook is dropped**: freeing a large one
    /// (millions of cells and strings) takes about 40 ms, which on the UI thread is a visible
    /// stall at every `q`, sheet or tab switch, so the old one goes to [`discard_in_background`].
    pub(super) fn set_workbook(&mut self, wb: Option<Box<crate::preview::office::Workbook>>) {
        if let Some(old) = std::mem::replace(&mut self.workbook, wb) {
            discard_in_background(old);
        }
    }

    /// The invariant [`App::set_preview_kind`] keeps: a loaded workbook (or its error) exists only
    /// while the preview is a spreadsheet.
    #[cfg(test)]
    pub(crate) fn workbook_matches_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            || (self.workbook.is_none() && self.workbook_error.is_none())
    }

    /// The field-separator byte of the active table (`,` by default). A spreadsheet never gets
    /// here: its copy is built from the cells (tab-joined) in `table_copy_text`.
    fn table_delimiter(&self) -> u8 {
        match self.tab.preview_kind {
            Some(PreviewKind::Table { delimiter, .. }) => delimiter,
            _ => b',',
        }
    }

    /// The parsed CSV/TSV/archive table (None for a spreadsheet or when not a table preview).
    /// Prefer [`App::grid`] in code that should work for both.
    #[cfg(test)]
    pub fn table_data(&self) -> Option<&crate::preview::table::TableData> {
        self.table_data.as_ref()
    }

    /// What the table renderer shows: the CSV/TSV/archive table, or the current sheet of the loaded
    /// spreadsheet. None while a workbook is still loading / failed to load / has no visible sheet.
    pub fn grid(&self) -> Option<Grid<'_>> {
        grid_of(&self.table_data, &self.workbook, self.tab.sheet_idx)
    }

    /// Whether a spreadsheet preview is on screen (a sheet is showing).
    #[cfg(test)]
    pub fn is_sheet_preview(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && matches!(self.grid(), Some(Grid::Sheet(_)))
    }

    /// True while a spreadsheet's worker has not delivered the sheet to show: the "loading" screen
    /// (opening the file, or moving to another sheet). A *re*load keeps the sheet that is on
    /// screen instead.
    pub fn is_sheet_loading(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && self.grid().is_none()
            && self.workbook_error.is_none()
            && self.media_loading
    }

    /// Test-only: the loaded workbook, if any.
    #[cfg(test)]
    pub fn workbook_for_test(&self) -> Option<&crate::preview::office::Workbook> {
        self.workbook.as_deref()
    }

    /// A spreadsheet's worker has not delivered yet (the file is being opened, another sheet is
    /// being read, or the sheet on screen is being re-read after an outside edit).
    pub(super) fn sheet_load_in_flight(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_))) && self.media_loading
    }

    /// How many sheets of the open workbook hold cells (at most one: the sheet on screen); `None`
    /// when no workbook is open.
    #[cfg(test)]
    pub(crate) fn loaded_sheet_count(&self) -> Option<usize> {
        self.workbook
            .as_ref()
            .map(|w| w.sheets.iter().filter(|s| s.loaded).count())
    }

    /// Why the spreadsheet could not be shown (None while loading, or on success).
    pub fn sheet_error(&self) -> Option<&crate::preview::office::OfficeError> {
        self.workbook_error.as_ref()
    }

    /// Whether the loaded spreadsheet has more than one visible sheet — the single predicate behind
    /// `J`/`K`, the footer hint and the help row (a hint shows only when its key acts).
    /// It does not depend on whether the sheet on screen has been read yet: the keys act the same
    /// while a sheet is loading.
    pub fn sheet_can_switch(&self) -> bool {
        matches!(self.tab.preview_kind, Some(PreviewKind::Spreadsheet(_)))
            && self.workbook.as_ref().is_some_and(|w| w.sheets.len() > 1)
    }

    /// `(name, 1-based index, sheet count, hidden sheet count)` of the sheet on screen.
    pub fn sheet_info(&self) -> Option<(&str, usize, usize, usize)> {
        let wb = self.workbook.as_ref()?;
        let sheet = wb.sheets.get(self.tab.sheet_idx)?;
        Some((
            sheet.name.as_str(),
            self.tab.sheet_idx + 1,
            wb.sheets.len(),
            wb.hidden_sheets,
        ))
    }

    /// `J`: the next sheet (stops at the last one, like the PDF page keys). The cursor and scroll
    /// go back to A1 — a sheet is a different grid.
    pub fn sheet_next(&mut self) {
        self.sheet_goto(self.tab.sheet_idx.saturating_add(1));
    }

    /// `K`: the previous sheet (stops at the first one).
    pub fn sheet_prev(&mut self) {
        self.sheet_goto(self.tab.sheet_idx.saturating_sub(1));
    }

    /// Moves to sheet `idx`. Only one sheet's cells are held at a time (memory stays that of the
    /// largest sheet shown), so this drops the cells of the sheet being left and starts the worker
    /// that reads the one being entered; the table shows the loading screen until it arrives
    /// (`apply_payload` then re-runs an active search on the new sheet). A result of a worker
    /// started earlier is dropped by the media generation, so quick `J J J` ends on the last sheet.
    fn sheet_goto(&mut self, idx: usize) {
        if !self.sheet_can_switch() {
            return;
        }
        let Some(wb) = self.workbook.as_mut() else {
            return;
        };
        let idx = idx.min(wb.sheets.len() - 1);
        if idx == self.tab.sheet_idx {
            return;
        }
        // The cells of the sheet being left are freed on another thread (about 40 ms for a
        // large one).
        discard_in_background(wb.unload_cells());
        self.workbook_error = None;
        self.tab.sheet_idx = idx;
        self.tab.table_cur_row = 0;
        self.tab.table_cur_col = 0;
        self.tab.table_top_row = 0;
        self.tab.table_left_col = 0;
        self.table_cell_open = false;
        // The old sheet's match cells mean nothing here.
        self.tab.search_matches.clear();
        self.tab.search_idx = 0;
        if let Some(PreviewKind::Spreadsheet(path)) = self.tab.preview_kind.clone() {
            self.start_media_load(&PreviewKind::Spreadsheet(path.clone()), &path);
        }
    }

    /// The cell cursor as (data-row, column), both 0-based.
    pub fn table_cursor(&self) -> (usize, usize) {
        (self.tab.table_cur_row, self.tab.table_cur_col)
    }

    /// The current (top data row, left column) scroll offsets.
    pub fn table_scroll(&self) -> (usize, usize) {
        (self.tab.table_top_row, self.tab.table_left_col)
    }

    /// Renderer feedback: store the scroll offsets it settled on (to keep the cursor visible) plus the
    /// visible data-row count (used as the PageUp/Down step). Mirrors how `preview_scroll`/`preview_viewport`
    /// are clamped/recorded at render time.
    pub fn set_table_view(&mut self, top_row: usize, left_col: usize, viewport_rows: u16) {
        self.tab.table_top_row = top_row;
        self.tab.table_left_col = left_col;
        self.table_viewport_rows = viewport_rows;
    }

    /// Move the cell cursor by (drow, dcol), clamped to the table. The renderer scrolls to follow.
    pub fn table_cursor_move(&mut self, drow: i32, dcol: i32) {
        let Some(g) = self.grid() else {
            return;
        };
        let (nr, nc) = (g.nrows(), g.ncols());
        if nr == 0 || nc == 0 {
            return;
        }
        let r = (self.tab.table_cur_row as i64 + drow as i64).clamp(0, nr as i64 - 1);
        let c = (self.tab.table_cur_col as i64 + dcol as i64).clamp(0, nc as i64 - 1);
        self.tab.table_cur_row = r as usize;
        self.tab.table_cur_col = c as usize;
    }

    /// Jump to the first (`bottom=false`) or last (`bottom=true`) data row.
    pub fn table_row_to(&mut self, bottom: bool) {
        let Some(nr) = self.grid().map(|g| g.nrows()) else {
            return;
        };
        self.tab.table_cur_row = if bottom { nr.saturating_sub(1) } else { 0 };
    }

    /// Jump to the first (`end=false`) or last (`end=true`) column.
    pub fn table_col_to(&mut self, end: bool) {
        let Some(nc) = self.grid().map(|g| g.ncols()) else {
            return;
        };
        self.tab.table_cur_col = if end { nc.saturating_sub(1) } else { 0 };
    }

    /// Move the cursor down/up by whole pages (`dir` = +1 / -1). The page size is the last render's visible rows.
    pub fn table_page(&mut self, dir: i32) {
        let page = self.table_viewport_rows.max(1) as i32;
        self.table_cursor_move(dir * page, 0);
    }

    /// Move the cursor down/up by half a page (`dir` = +1 / -1).
    pub fn table_half_page(&mut self, dir: i32) {
        let half = (self.table_viewport_rows / 2).max(1) as i32;
        self.table_cursor_move(dir * half, 0);
    }

    /// Build the text a table copy would place on the clipboard (None when there is no table).
    /// Cell = the current cell's value; Row = the current row's cells joined by the delimiter;
    /// Column = the column's header + every cell value, one per line.
    pub(super) fn table_copy_text(&self, kind: TableCopyKind) -> Option<String> {
        let g = self.grid()?;
        let (r, c) = (self.tab.table_cur_row, self.tab.table_cur_col);
        // A sheet copies the text Excel shows (never the raw value, never the row-number gutter or
        // the column letters — those are not data).
        let t = match g {
            Grid::Sheet(sheet) => {
                return Some(match kind {
                    TableCopyKind::Cell => sheet.display(r, c).to_string(),
                    // Tab-separated, up to the last non-empty cell (no trailing tabs).
                    TableCopyKind::Row => match sheet
                        .row_cells(r)
                        .iter()
                        .rev()
                        .find(|(_, cell)| !cell.display().is_empty())
                    {
                        Some(&(last, _)) => (0..=last as usize)
                            .map(|c| sheet.display(r, c))
                            .collect::<Vec<_>>()
                            .join("\t"),
                        None => String::new(),
                    },
                    TableCopyKind::Column => (0..sheet.nrows)
                        .map(|r| sheet.display(r, c))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            }
            Grid::Csv(t) => t,
        };
        let sep = (self.table_delimiter() as char).to_string();
        Some(match kind {
            TableCopyKind::Cell => {
                if t.nrows() == 0 {
                    t.header(c).to_string()
                } else {
                    t.cell(r, c).to_string()
                }
            }
            TableCopyKind::Row => {
                if t.nrows() == 0 {
                    t.headers.join(&sep)
                } else {
                    t.rows.get(r).map(|row| row.join(&sep)).unwrap_or_default()
                }
            }
            TableCopyKind::Column => {
                let mut vals = vec![t.header(c).to_string()];
                vals.extend(
                    t.rows
                        .iter()
                        .map(|row| row.get(c).cloned().unwrap_or_default()),
                );
                vals.join("\n")
            }
        })
    }

    /// Copy the current cell / row / column to the clipboard and flash the result.
    pub fn table_copy(&mut self, kind: TableCopyKind) {
        let Some(text) = self.table_copy_text(kind) else {
            self.flash = Some(tr(self.lang, crate::i18n::Msg::NoCopyTarget).into());
            return;
        };
        self.set_clipboard_flash(&text);
    }

    /// Case-insensitive cell scan for a table preview, in reading order (row-major).
    /// Only data cells are searched: the cell cursor addresses data rows, so a header-only hit
    /// would have nowhere to jump to.
    pub(super) fn table_search_scan(&mut self, q: &str) {
        self.tab.search_matches.clear();
        self.tab.search_truncated = false;
        let needle = Needle::new(q);
        let Some(g) = grid_of(&self.table_data, &self.workbook, self.tab.sheet_idx) else {
            return;
        };
        let (matches, truncated) = match g {
            // A sheet is sparse (up to 16k columns): walk only the cells that exist, in reading
            // order, and match the displayed text.
            Grid::Sheet(sheet) => scan_rows(
                sheet.nrows,
                |r| {
                    let cells = sheet.row_cells(r);
                    row_weight(
                        cells.len(),
                        cells.iter().map(|(_, c)| c.display().len()).sum(),
                    )
                },
                |r, out| {
                    for (c, cell) in sheet.row_cells(r) {
                        let shown = cell.display();
                        if !shown.is_empty() && needle.found_in(shown) {
                            out.push((0, r, *c as usize));
                        }
                    }
                },
            ),
            Grid::Csv(t) => scan_rows(
                t.nrows(),
                |r| row_weight(t.ncols, (0..t.ncols).map(|c| t.cell(r, c).len()).sum()),
                |r, out| {
                    for c in 0..t.ncols {
                        if needle.found_in(t.cell(r, c)) {
                            out.push((0, r, c));
                        }
                    }
                },
            ),
        };
        self.tab.search_matches = matches;
        self.tab.search_truncated = truncated;
    }

    /// Whether this data cell matched the active search (renderer lookup). The matches of a table
    /// are recorded in reading order (row-major), so this is a binary search of `search_matches`
    /// itself: no second copy of a (possibly multi-million) result set to build, and nothing that
    /// could outlive a tab switch (`search_matches` is per tab).
    pub fn table_cell_is_hit(&self, row: usize, col: usize) -> bool {
        matches!(
            self.tab.preview_kind,
            Some(
                PreviewKind::Table { .. }
                    | PreviewKind::Archive { .. }
                    | PreviewKind::Spreadsheet(_)
            )
        ) && self
            .tab
            .search_matches
            .binary_search(&(0, row, col))
            .is_ok()
    }

    // ---- Table cell full-text popup (`Enter` in a table preview) --------------
    // konoma's table grid truncates any cell that doesn't fit the column width with `…`, so while
    // `y→c` can copy it to the clipboard, it can't be read on screen (the copy target is always the
    // raw cell value = no truncation). To fill this gap: an App-global (not per-tab) toggle
    // overlay, the same shape as Outline/Info.

    /// `Enter`: open/close the full-cell popup (mirrors `toggle_outline`/`toggle_info`). The real
    /// trigger is the fixed `Enter` key (`handle_enter`/`handle_esc` in main.rs); `Action::ToggleTableCell`
    /// exists so `q` inside the popup can also close it through the ordinary keymap. Flashes and
    /// stays closed when the table has no columns (an empty file — nothing to show).
    pub fn toggle_table_cell_view(&mut self) {
        if self.table_cell_open {
            self.table_cell_open = false;
            return;
        }
        match self.grid() {
            Some(g) if g.ncols() > 0 => {}
            _ => {
                self.flash = Some(tr(self.lang, crate::i18n::Msg::TableCellEmpty).into());
                return;
            }
        }
        self.table_cell_scroll = 0;
        self.table_cell_open = true;
    }

    /// Whether the full-cell popup is showing.
    pub fn is_table_cell_open(&self) -> bool {
        self.table_cell_open
    }

    /// The full-cell popup's content (the cursor cell, read live so it reflects any reload/move
    /// while open). None when there is no table, or it has no columns (nothing to show).
    pub fn table_cell_view(&self) -> Option<TableCellView> {
        let g = self.grid()?;
        if g.ncols() == 0 {
            return None;
        }
        let (r, c) = (self.tab.table_cur_row, self.tab.table_cur_col);
        let text = match g {
            // A spreadsheet cell: the address, what Excel shows, and everything behind it.
            Grid::Sheet(sheet) => self.sheet_cell_detail(sheet, r, c),
            Grid::Csv(t) if t.nrows() == 0 => String::new(), // a header-only file: the content is empty.
            Grid::Csv(t) => t.cell(r, c).to_string(),
        };
        Some(TableCellView {
            // For a sheet the "header" is the cell address (`B3`), the natural name of the cell.
            header: match g {
                Grid::Sheet(_) => cell_address(r, c),
                Grid::Csv(_) => g.header(c).to_string(),
            },
            row: r + 1,
            col: c + 1,
            nrows: g.nrows(),
            ncols: g.ncols(),
            text,
        })
    }

    /// The spreadsheet cell popup body: address, displayed text, raw value, type, formula (when the
    /// cell has one) and number-format code (when it is not General). One `label: value` per line.
    fn sheet_cell_detail(
        &self,
        sheet: &crate::preview::office::Sheet,
        r: usize,
        c: usize,
    ) -> String {
        use crate::i18n::Msg;
        let lang = self.lang;
        let mut lines = vec![
            format!(
                "{}: {}",
                tr(lang, Msg::SheetCellAddress),
                cell_address(r, c)
            ),
            format!(
                "{}: {}",
                tr(lang, Msg::SheetCellDisplayed),
                sheet.display(r, c)
            ),
        ];
        if let Some(cell) = sheet.cell(r, c) {
            lines.push(format!(
                "{}: {}",
                tr(lang, Msg::SheetCellRaw),
                cell.raw_text()
            ));
            let ty = match cell.cell_type() {
                CellType::Number => Msg::SheetTypeNumber,
                CellType::Text => Msg::SheetTypeText,
                CellType::Bool => Msg::SheetTypeBool,
                CellType::Error => Msg::SheetTypeError,
                CellType::DateTime => Msg::SheetTypeDateTime,
            };
            lines.push(format!(
                "{}: {}",
                tr(lang, Msg::SheetCellType),
                tr(lang, ty)
            ));
            if let Some(f) = sheet.formula(r, c) {
                lines.push(format!("{}: ={f}", tr(lang, Msg::SheetCellFormula)));
            }
            let locale = match lang {
                crate::i18n::Lang::Jp => crate::preview::office::Locale::Ja,
                crate::i18n::Lang::En => crate::preview::office::Locale::En,
            };
            let code = self
                .workbook
                .as_ref()
                .and_then(|wb| match wb.format_of(cell) {
                    NumFmtRef::General => None,
                    NumFmtRef::Custom(code) => Some(code.to_string()),
                    NumFmtRef::Builtin(n) => Some(
                        crate::preview::office::numfmt::builtin_format_code(u32::from(*n), locale)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("#{n}")),
                    ),
                });
            if let Some(code) = code.filter(|c| c != "General") {
                lines.push(format!("{}: {code}", tr(lang, Msg::SheetCellFormat)));
            }
        } else {
            // No stored cell: still say it is empty (the type line is the one place that does).
            lines.push(format!(
                "{}: {}",
                tr(lang, Msg::SheetCellType),
                tr(lang, Msg::SheetTypeEmpty)
            ));
        }
        lines.join("\n")
    }

    /// The popup's current vertical scroll offset (wrapped-row units).
    pub fn table_cell_scroll(&self) -> u16 {
        self.table_cell_scroll
    }

    /// Renderer feedback: the clamped scroll and the popup's visible row count (used as the
    /// PageUp/Down step). Mirrors `set_table_view`.
    pub fn set_table_cell_view(&mut self, scroll: u16, viewport: u16) {
        self.table_cell_scroll = scroll;
        self.table_cell_viewport = viewport;
    }

    /// Scroll the popup by `delta` wrapped rows (the upper bound is clamped at render time,
    /// mirroring `preview_scroll`).
    pub fn table_cell_scroll_by(&mut self, delta: i32) {
        let v = self.table_cell_scroll as i32 + delta;
        self.table_cell_scroll = v.max(0) as u16;
    }

    /// To the top (`bottom=false`) or bottom (`bottom=true`, clamped at render time via `u16::MAX`) of the popup.
    pub fn table_cell_scroll_to(&mut self, bottom: bool) {
        self.table_cell_scroll = if bottom { u16::MAX } else { 0 };
    }

    /// Page the popup up/down (`dir` = -1/+1). The page size is the last render's visible rows.
    pub fn table_cell_page(&mut self, dir: i32) {
        let page = self.table_cell_viewport.saturating_sub(1).max(1) as i32;
        self.table_cell_scroll_by(dir * page);
    }
}

/// The full-cell popup's content: the cursor cell's header/position/untruncated text.
/// `row`/`col` are 1-based (display convention, matching the table grid's own title).
pub struct TableCellView {
    pub header: String,
    pub row: usize,
    pub col: usize,
    pub nrows: usize,
    pub ncols: usize,
    pub text: String,
}

#[cfg(test)]
mod needle_tests {
    use super::{fold, fold_results, fold_results_up_to, Needle, FOLDED_TO_ASCII, FOLDS_TO_ASCII};

    /// The reference of the character search: fold the text at every character position.
    fn found_in_at_every_position(n: &Needle, hay: &str) -> bool {
        hay.char_indices().any(|(i, _)| {
            let mut folded = hay[i..].chars().flat_map(fold);
            n.chars.iter().all(|&c| folded.next() == Some(c))
        })
    }

    /// What the search used to do (allocating per cell).
    fn old(hay: &str, q: &str) -> bool {
        hay.to_lowercase().contains(&q.to_lowercase())
    }

    #[test]
    fn needle_matches_exactly_what_to_lowercase_contains_did() {
        // (Sigma is the one place the new folding is deliberately not the old one: see
        // `sigma_is_one_letter_in_any_position`, which has its own corpus.)
        let hays = [
            "",
            "a",
            "Hello World",
            "HELLO",
            "hello",
            "x-Ray 123",
            "ÉCOLE",
            "école",
            "Straße",
            "STRASSE",
            "Kelvin \u{212A}",
            "İstanbul",
            "日本語テキスト",
            "ＡＢＣ ａｂｃ",
            "Ωmega ω",
            "mixed ÀÉÎ and abc",
            "tab\tnew\nline",
            "😀 smile",
            "ǅ title",
            // Precomposed vs decomposed é, and an İ followed by a combining dot: no normalization
            // either way, exactly as before.
            "caf\u{e9}",
            "cafe\u{301}",
            "\u{130}\u{307}x",
            "Ǳǲǳ",
            "ﬁne ﬃ",
        ];
        let needles = [
            "",
            "a",
            "A",
            "hello",
            "WORLD",
            "o w",
            "ray",
            "123",
            "é",
            "É",
            "ecole",
            "école",
            "ß",
            "ss",
            "k",
            "K",
            "i",
            "i\u{307}",
            "istanbul",
            "語",
            "テキ",
            "ａｂｃ",
            "ＡＢＣ",
            "ω",
            "Ω",
            "àéî",
            "AND",
            "\t",
            "\n",
            "😀",
            "ǆ",
            "Ǆ",
            "zzzz",
            "hello world and more",
            "e\u{301}",
            "caf\u{e9}",
            "fi",
            "ﬁ",
            "ǳ",
        ];
        for h in hays {
            for q in needles {
                assert_eq!(
                    Needle::new(q).found_in(h),
                    old(h, q),
                    "hay {h:?} needle {q:?}"
                );
            }
        }
    }

    /// `σ`, `ς` and `Σ` are one letter. The old search (`to_lowercase().contains`) found `ΟΔΟΣ`
    /// in `ΟΔΟΣ` and in `οδος` and so must this one; the new folding additionally finds `οδοσ` in
    /// `οδος` (a `σ` typed for a final `ς`), which the old search did not. Every other pair must
    /// agree with the old search.
    #[test]
    fn sigma_is_one_letter_in_any_position() {
        let hays = [
            "ΟΔΟΣ",
            "οδος",
            "οδοσ",
            "ΟΔΟΣ ΑΘΗΝΩΝ",
            "οδός",
            "Σίσυφος",
            "ΣΊΣΥΦΟΣ",
            "ΣΣ",
            "σς",
            "ΑΣ-ΒΣ",
            "Ωmega ΩΣ",
            "Σ",
            "ς",
            "x",
        ];
        let needles = [
            "Σ",
            "σ",
            "ς",
            "ΟΔΟΣ",
            "οδος",
            "οδοσ",
            "ΟΣ",
            "ος",
            "οσ",
            "Σίσυφος",
            "ΣΊΣΥΦΟΣ",
            "σίσυφοσ",
            "ΣΣ",
            "σς",
            "ΑΣ",
            "ΒΣ",
            "ΩΣ",
            "ΟΔΟΣ ΑΘΗΝΩΝ",
            "οδός",
        ];
        let mut only_new = 0;
        for h in hays {
            for q in needles {
                let new = Needle::new(q).found_in(h);
                let old = old(h, q);
                // Never loses a hit the old search had.
                assert!(new || !old, "hay {h:?} needle {q:?}: lost a hit");
                if new != old {
                    only_new += 1;
                    // A new hit is a sigma-blind hit and nothing else.
                    let blind = |s: &str| s.to_lowercase().replace('ς', "σ");
                    assert!(
                        blind(h).contains(&blind(q)),
                        "hay {h:?} needle {q:?}: a hit that is not sigma-blind"
                    );
                }
            }
        }
        assert!(only_new > 0, "the corpus does exercise the sigma folding");
        // The cases that were broken, spelled out.
        assert!(Needle::new("ΟΔΟΣ").found_in("ΟΔΟΣ"));
        assert!(Needle::new("οδος").found_in("ΟΔΟΣ"));
        assert!(Needle::new("ΟΔΟΣ").found_in("οδος"));
        assert!(Needle::new("σ").found_in("ΟΔΟΣ"));
        assert!(Needle::new("ς").found_in("ΟΔΟΣ"));
        assert!(!Needle::new("ΟΔΟΣ").found_in("ΟΔΟΝ"));
    }

    /// The facts the byte search is built on, checked against all of Unicode (not just the range
    /// the lazily built table scans): which characters fold to ASCII, and that nothing above
    /// U+1FFFF folds to anything.
    #[test]
    fn the_folding_facts_the_byte_search_relies_on_hold_for_all_of_unicode() {
        let all = fold_results_up_to(0x10FFFF);
        assert_eq!(all, fold_results(), "no character above U+1FFFF has a case");
        let mut sources = Vec::new();
        let mut targets = std::collections::BTreeSet::new();
        for c in (0x80..=0x10FFFFu32).filter_map(char::from_u32) {
            for f in fold(c) {
                if f.is_ascii() {
                    sources.push(c);
                    targets.insert(f as u8);
                }
            }
        }
        sources.sort_unstable();
        let mut want = FOLDS_TO_ASCII.to_vec();
        want.sort_unstable();
        assert_eq!(sources, want, "the characters that fold to an ASCII letter");
        let mut want_t = FOLDED_TO_ASCII.to_vec();
        want_t.sort_unstable();
        assert_eq!(targets.into_iter().collect::<Vec<_>>(), want_t);
        // Folding is idempotent on its own results (the byte search compares folded text).
        for &r in &all {
            assert_eq!(fold(r).collect::<Vec<_>>(), [r], "{r:?}");
        }
    }

    #[test]
    fn which_queries_get_a_byte_search() {
        assert!(Needle::new("abc").bytes.is_some());
        assert!(Needle::new("ABC 123").bytes.is_some());
        assert!(Needle::new("日本語").bytes.is_some(), "uncased text");
        assert!(Needle::new("Excel表").bytes.is_some(), "ASCII and uncased");
        assert!(Needle::new("😀").bytes.is_some());
        assert!(Needle::new("é").bytes.is_none(), "a cased non-ASCII letter");
        assert!(Needle::new("É").bytes.is_none());
        assert!(Needle::new("σ").bytes.is_none());
        assert!(Needle::new("ς").bytes.is_none());
        assert!(
            Needle::new("ａ").bytes.is_none(),
            "fullwidth letters are cased"
        );
        assert!(
            Needle::new("\u{307}").bytes.is_none(),
            "a combining dot is a folding result (of İ)"
        );
        assert_eq!(
            Needle::new("日本k").bytes.as_deref(),
            Some("日本k".as_bytes())
        );
        assert_eq!(Needle::new("AbC").bytes.as_deref(), Some(b"abc".as_slice()));
    }

    /// Every fast path against the character-by-character reference, over a corpus built from the
    /// awkward characters (Kelvin sign, `İ`, a combining dot, sigmas, CJK, fullwidth, emoji) and
    /// a deterministic scramble of them.
    #[test]
    fn the_byte_search_agrees_with_the_folding_on_a_corpus() {
        let alphabet: Vec<&str> = vec![
            "a", "A", "k", "K", "i", "I", "x", "1", " ", "\u{212A}", "\u{130}", "\u{307}", "ı",
            "σ", "ς", "Σ", "é", "É", "e", "日", "本", "語", "ａ", "Ａ", "😀", "ß", "ǅ", "ǆ", "Ǆ",
            "ﬁ", "\t",
        ];
        // Deterministic xorshift: no `rand` dependency.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut make = |max: u64| -> String {
            let n = next() % max + 1;
            (0..n)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                .collect()
        };
        let hays: Vec<String> = (0..400).map(|_| make(10)).collect();
        let needles: Vec<String> = (0..300).map(|_| make(3)).collect();
        let mut hits = 0;
        let mut byte_queries = 0;
        for q in &needles {
            let n = Needle::new(q);
            byte_queries += usize::from(n.bytes.is_some());
            for h in &hays {
                let want = found_in_at_every_position(&n, h);
                assert_eq!(n.found_in(h), want, "hay {h:?} needle {q:?}");
                assert_eq!(n.found_in_by_chars(h), want, "hay {h:?} needle {q:?}");
                hits += usize::from(want);
            }
        }
        assert!(hits > 500, "the corpus finds things ({hits})");
        assert!(
            byte_queries > 50,
            "and exercises the byte search ({byte_queries})"
        );
        // The cases the byte search must hand to the folding, spelled out.
        assert!(Needle::new("k").found_in("\u{212A}elvin"));
        assert!(Needle::new("日本k").found_in("日本\u{212A}"));
        assert!(Needle::new("i").found_in("日本\u{130}"));
        // `İ` folds to `i` and a combining dot: `i` alone is found in it, `istanbul` is not.
        assert!(Needle::new("i\u{307}stanbul").found_in("\u{130}stanbul 日本"));
        assert!(!Needle::new("istanbul").found_in("\u{130}stanbul 日本"));
        assert!(!Needle::new("日本").found_in("日 本"));
        assert!(Needle::new("excel表").found_in("EXCEL表 2024"));
    }

    #[test]
    fn needle_handles_edges() {
        let n = Needle::new("ab");
        assert!(!n.found_in("a"));
        assert!(n.found_in("ab"));
        assert!(n.found_in("xxAB"));
        assert!(n.found_in("ABxx"));
        assert!(!n.found_in("aXb"));
        // A non-ASCII query never matches ASCII text; the empty query matches everything.
        assert!(!Needle::new("é").found_in("e"));
        assert!(Needle::new("").found_in("anything"));
        assert!(Needle::new("").found_in(""));
    }

    /// The character search tries only the characters whose folding begins with the query's first
    /// character. For every character of Unicode that has a case, a query of its folding is found
    /// in text holding that character (in any of its forms), exactly as the reference says.
    #[test]
    fn the_first_character_filter_loses_no_form_of_any_cased_character() {
        let mut checked = 0;
        for c in (0..=0x1FFFFu32).filter_map(char::from_u32) {
            let folded: String = fold(c).collect();
            if folded.chars().eq([c]) {
                continue;
            }
            let n = Needle::new(&folded);
            let hay = format!("x{c}y");
            assert!(n.found_in_by_chars(&hay), "{c:?} -> {folded:?}");
            assert_eq!(
                n.found_in_by_chars(&hay),
                found_in_at_every_position(&n, &hay),
                "{c:?}"
            );
            // A query of the character itself finds it in the folded text as well.
            let m = Needle::new(&c.to_string());
            let hay2 = format!("x{folded}y");
            assert_eq!(
                m.found_in(&hay2),
                found_in_at_every_position(&m, &hay2),
                "{c:?}"
            );
            // And a miss stays a miss.
            assert!(!n.found_in_by_chars("\u{1}\u{2}"), "{c:?}");
            checked += 1;
        }
        assert!(
            checked > 1000,
            "the scan covers the cased characters ({checked})"
        );
    }

    /// A query with a cased non-ASCII letter over a long text of non-ASCII cells: the shapes the
    /// per-position folding was slow for. Not a timing test (that is measured in release); it
    /// pins the result on a text where nearly every character is a candidate.
    #[test]
    fn the_first_character_filter_finds_a_match_after_many_false_starts() {
        let hay = format!("{}ÉCOLE", "éé é".repeat(5000));
        assert!(Needle::new("école").found_in(&hay));
        assert!(Needle::new("ÉCOLE").found_in(&hay));
        assert!(!Needle::new("écolx").found_in(&hay));
        let hay = format!("{}Σ", "οδο".repeat(5000));
        assert!(Needle::new("ς").found_in(&hay));
        assert!(!Needle::new("ΩΩ").found_in(&hay));
    }
}

#[cfg(test)]
mod release_and_cap_tests {
    use super::*;
    use crate::test_support::unique_tmp;

    /// Records which thread it was dropped on.
    struct Probe(std::sync::mpsc::Sender<std::thread::ThreadId>);

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.0.send(std::thread::current().id());
        }
    }

    #[test]
    fn a_discarded_value_is_dropped_on_another_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        discard_in_background(Probe(tx));
        let on = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the value is dropped");
        assert_ne!(on, std::thread::current().id());
    }

    #[test]
    fn many_discards_are_all_freed_by_the_one_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..200 {
            discard_in_background(Probe(tx.clone()));
        }
        drop(tx);
        let ids: std::collections::HashSet<_> = rx.iter().take(200).collect();
        assert_eq!(ids.len(), 1, "one reaper thread, not a thread per value");
        assert!(!ids.contains(&std::thread::current().id()));
    }

    #[test]
    fn a_search_records_at_most_the_cap_and_the_first_in_reading_order() {
        let dir = unique_tmp("konoma_table_search_cap");
        std::fs::create_dir_all(&dir).unwrap();
        let mut csv = String::from("h\n");
        for i in 0..(SEARCH_MATCH_CAP + 1_000) {
            csv.push_str(&format!("hit {i}\n"));
        }
        std::fs::write(dir.join("t.csv"), csv).unwrap();
        let root = dir.canonicalize().unwrap();
        let mut app = App::new(root.clone(), Config::default()).unwrap();
        let path = root.join("t.csv");
        app.tab.preview_kind = Some(app.cfg.resolve_preview(&path));
        app.tab.preview_path = Some(path);
        app.tab.mode = Mode::Preview;
        app.load_table();
        assert_eq!(
            app.table_data().map(|t| t.nrows()),
            Some(SEARCH_MATCH_CAP + 1_000)
        );

        app.start_search();
        for c in "HIT".chars() {
            app.search_input_push(c);
        }
        app.search_commit();
        assert_eq!(app.tab.search_matches.len(), SEARCH_MATCH_CAP);
        assert_eq!(
            app.tab.search_matches.last(),
            Some(&(0, SEARCH_MATCH_CAP - 1, 0)),
            "the first hits in reading order"
        );
        assert_eq!(app.search_status(), Some((1, SEARCH_MATCH_CAP)));
        // The footer says the count was cut: "at least", not "exactly".
        assert!(app.search_capped());
        let hints = crate::ui::preview::footer_hints(&app);
        assert!(hints.iter().any(|h| h == "n/N:match[1/5000+]"), "{hints:?}");
        // A search with fewer hits than the cap is not cut.
        app.start_search();
        for c in "hit 4999".chars() {
            app.search_input_push(c);
        }
        app.search_commit();
        assert_eq!(app.tab.search_matches, vec![(0, 4999, 0)]);
        assert!(!app.search_capped());
    }

    #[test]
    fn a_table_search_with_exactly_the_cap_is_not_marked_cut() {
        for (extra, cut) in [(0, false), (1, true)] {
            let dir = unique_tmp("konoma_table_search_exact");
            std::fs::create_dir_all(&dir).unwrap();
            let mut csv = String::from("h\n");
            for i in 0..(SEARCH_MATCH_CAP + extra) {
                csv.push_str(&format!("hit {i}\n"));
            }
            csv.push_str("other\n");
            std::fs::write(dir.join("t.csv"), csv).unwrap();
            let root = dir.canonicalize().unwrap();
            let mut app = App::new(root.clone(), Config::default()).unwrap();
            let path = root.join("t.csv");
            app.tab.preview_kind = Some(app.cfg.resolve_preview(&path));
            app.tab.preview_path = Some(path);
            app.tab.mode = Mode::Preview;
            app.load_table();
            app.start_search();
            for c in "hit".chars() {
                app.search_input_push(c);
            }
            app.search_commit();
            assert_eq!(app.tab.search_matches.len(), SEARCH_MATCH_CAP);
            assert_eq!(app.search_capped(), cut, "{extra} over the cap");
            let hints = crate::ui::preview::footer_hints(&app);
            let want = if cut {
                "n/N:match[1/5000+]"
            } else {
                "n/N:match[1/5000]"
            };
            assert!(hints.iter().any(|h| h == want), "{hints:?}");
        }
    }

    #[test]
    fn a_windowed_search_with_exactly_the_cap_is_not_marked_cut() {
        for (hits, cut) in [(SEARCH_MATCH_CAP, false), (SEARCH_MATCH_CAP + 1, true)] {
            let dir = unique_tmp("konoma_windowed_search_exact");
            std::fs::create_dir_all(&dir).unwrap();
            let mut text = String::new();
            for i in 0..hits {
                text.push_str(&format!("needle {i}\n"));
            }
            std::fs::write(dir.join("a.rs"), text).unwrap();
            let mut app = App::new(dir.canonicalize().unwrap(), Config::default()).unwrap();
            app.tab.selected = app.tab.entries.iter().position(|e| !e.is_dir).unwrap();
            app.tree_activate().unwrap();
            assert!(app.is_windowed());
            app.start_search();
            for c in "needle".chars() {
                app.search_input_push(c);
            }
            app.search_commit();
            assert_eq!(app.tab.search_matches.len(), SEARCH_MATCH_CAP);
            assert_eq!(app.search_capped(), cut, "{hits} hits");
        }
    }

    /// The one place a workbook is dropped is `set_workbook` (and the sheet being left goes to
    /// `discard_in_background`): a plain assignment would free it on the UI thread again.
    #[test]
    fn no_code_assigns_the_workbook_or_drops_unloaded_sheets_on_the_ui_thread() {
        let table_actions = include_str!("table_actions.rs");
        let table_actions = table_actions
            .split("mod release_and_cap_tests")
            .next()
            .unwrap();
        for (name, src) in [
            ("app.rs", include_str!("../app.rs")),
            ("media_load.rs", include_str!("media_load.rs")),
            ("tab_lifecycle.rs", include_str!("tab_lifecycle.rs")),
            ("table_actions.rs", table_actions),
        ] {
            // (the tests module of app.rs is a separate file)
            for (n, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                assert!(
                    !code.contains("self.workbook = "),
                    "{name}:{}: assign through set_workbook: {line}",
                    n + 1
                );
                assert!(
                    !code.contains(".unload_cells();"),
                    "{name}:{}: hand the freed sheets to discard_in_background: {line}",
                    n + 1
                );
            }
        }
    }
}

#[cfg(test)]
mod search_agreement_tests {
    use super::{fold, Needle};

    /// The reference: fold the text at every character position.
    fn at_every_position(n: &Needle, hay: &str) -> bool {
        hay.char_indices().any(|(i, _)| {
            let mut folded = hay[i..].chars().flat_map(fold);
            n.chars.iter().all(|&c| folded.next() == Some(c))
        })
    }

    /// Random short texts and queries over the characters that make the character search
    /// interesting (cased non-ASCII letters, the ones that fold to several characters or to ASCII,
    /// the three sigmas, combining marks), several seeds, matches in about half the pairs.
    #[test]
    fn the_character_search_agrees_with_folding_at_every_position() {
        let alphabet: Vec<char> =
            "aAbBkKiIxé É è ê ω Ω σ ς Σ ǅ ǆ Ǆ ß \u{212A}\u{130}\u{307}\u{3a3} 日 ａ Ａ 1 -"
                .chars()
                .collect();
        let mut agree = 0usize;
        let mut hits = 0usize;
        for seed in 1..=6u64 {
            let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0xD6E8_FEB8_6659_FD93);
            let mut next = move || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            };
            let mut make = |max: u64| -> String {
                let n = next() % max + 1;
                (0..n)
                    .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
                    .collect()
            };
            let hays: Vec<String> = (0..300).map(|_| make(14)).collect();
            let needles: Vec<String> = (0..150).map(|_| make(4)).collect();
            for q in &needles {
                let n = Needle::new(q);
                for h in &hays {
                    let want = at_every_position(&n, h);
                    assert_eq!(n.found_in_by_chars(h), want, "hay {h:?} needle {q:?}");
                    assert_eq!(n.found_in(h), want, "hay {h:?} needle {q:?}");
                    // The same text made long (the other branch of the search): padding that
                    // is in no query, in front and behind.
                    let long = format!("{pad}{h}{pad}", pad = "\u{b7}".repeat(super::LONG_TEXT));
                    assert_eq!(
                        n.found_in_by_chars(&long),
                        want,
                        "long hay {h:?} needle {q:?}"
                    );
                    assert_eq!(n.found_in(&long), want, "long hay {h:?} needle {q:?}");
                    agree += 1;
                    hits += usize::from(want);
                }
            }
        }
        assert!(agree > 250_000);
        assert!(hits > 10_000, "the corpus finds things ({hits})");
    }
}

#[cfg(test)]
mod parallel_search_tests {
    use super::*;

    /// Rows of uneven weight: a row `r` has `r % 7` cells (some none), and a cell matches when
    /// `(r * 31 + c) % 5 == 0`.
    fn scan(threads: usize, nrows: usize) -> Vec<(u64, usize, usize)> {
        scan_flag(threads, nrows).0
    }

    fn scan_flag(threads: usize, nrows: usize) -> (Vec<(u64, usize, usize)>, bool) {
        scan_rows_with(
            nrows,
            |r| r % 7,
            |r, out| {
                for c in 0..r % 7 {
                    if (r * 31 + c) % 5 == 0 {
                        out.push((0, r, c));
                    }
                }
            },
            threads,
        )
    }

    #[test]
    fn several_threads_give_exactly_what_one_thread_gives() {
        for nrows in [0, 1, 2, 7, 100, 1_000, 5_000, 20_000] {
            let one = scan(1, nrows);
            for threads in [2, 3, 4, 8, 64] {
                assert_eq!(
                    scan(threads, nrows),
                    one,
                    "{nrows} rows on {threads} threads"
                );
            }
            assert!(one.windows(2).all(|w| w[0] < w[1]), "reading order");
        }
        // The big case really is the parallel path (past the cell threshold) and is cut at the
        // cap, in reading order.
        let big = scan(8, 20_000);
        assert_eq!(big.len(), SEARCH_MATCH_CAP);
        assert_eq!(big, scan(1, 20_000));
    }

    #[test]
    fn one_row_holding_most_of_the_cells_still_searches_everything() {
        // 3 rows, the middle one with all the cells: runs cannot be balanced, nothing is lost.
        let out = scan_rows_with(
            3,
            |r| if r == 1 { 10_000 } else { 1 },
            |r, out| {
                let n = if r == 1 { 10_000 } else { 1 };
                for c in 0..n {
                    if c % 1_000 == 0 {
                        out.push((0, r, c));
                    }
                }
            },
            8,
        )
        .0;
        let want: Vec<_> = (0..3)
            .flat_map(|r| {
                let n = if r == 1 { 10_000 } else { 1 };
                (0..n).filter(|c| c % 1_000 == 0).map(move |c| (0u64, r, c))
            })
            .collect();
        assert_eq!(out, want);
    }

    #[test]
    fn a_small_table_does_not_start_threads() {
        // Under the threshold the work runs on this thread: the closure may observe it.
        let me = std::thread::current().id();
        let out = scan_rows_with(
            100,
            |_| 1,
            |r, out| {
                assert_eq!(std::thread::current().id(), me);
                out.push((0, r, 0));
            },
            8,
        )
        .0;
        assert_eq!(out.len(), 100);
    }

    /// One hit per row: `hits` rows match, so the scan finds exactly `hits` cells.
    fn exact_hits(threads: usize, hits: usize, weight: usize) -> (usize, bool) {
        let (v, cut) = scan_rows_with(
            20_000,
            |_| weight,
            |r, out| {
                if r < hits {
                    out.push((0, r, 0));
                }
            },
            threads,
        );
        (v.len(), cut)
    }

    #[test]
    fn exactly_the_cap_is_not_cut_but_one_more_is() {
        for threads in [1, 2, 8] {
            for weight in [1, 1_000] {
                assert_eq!(
                    exact_hits(threads, SEARCH_MATCH_CAP - 1, weight),
                    (SEARCH_MATCH_CAP - 1, false)
                );
                assert_eq!(
                    exact_hits(threads, SEARCH_MATCH_CAP, weight),
                    (SEARCH_MATCH_CAP, false),
                    "{threads} threads"
                );
                assert_eq!(
                    exact_hits(threads, SEARCH_MATCH_CAP + 1, weight),
                    (SEARCH_MATCH_CAP, true),
                    "{threads} threads"
                );
                assert_eq!(
                    exact_hits(threads, SEARCH_MATCH_CAP + 3_000, weight),
                    (SEARCH_MATCH_CAP, true)
                );
            }
        }
    }

    #[test]
    fn a_few_heavy_rows_are_searched_on_several_threads() {
        // 100 rows of one cell holding a megabyte each: far under the cell threshold, far over
        // the weight one.
        let me = std::thread::current().id();
        let off_thread = std::sync::atomic::AtomicBool::new(false);
        let (out, _) = scan_rows_with(
            100,
            |_| row_weight(1, 1 << 20),
            |r, out| {
                if std::thread::current().id() != me {
                    off_thread.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                out.push((0, r, 0));
            },
            8,
        );
        assert_eq!(out.len(), 100);
        assert!(off_thread.load(std::sync::atomic::Ordering::Relaxed));
        assert!(row_weight(1, 0) < PARALLEL_SEARCH_MIN_WEIGHT);
    }
}
