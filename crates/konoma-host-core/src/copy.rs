//! What `y` copies, as pure functions: the path strings (`CopyKind`), the table strings
//! (`TableCopyKind`) and the wording of the notice that follows. No app state and no clipboard
//! access: the caller passes the target path, the base directory and the cells as values, so the
//! host and the plugin SDK's small host build the same text.

use std::borrow::Cow;
use std::path::Path;

use crate::help_msg::CoreMsg;
use crate::keymap::{CopyKind, TableCopyKind};
use crate::lang::Lang;
use crate::paths::{rel_from, rel_to_open};

/// The string `kind` copies for `path`. `open_dir` is the base of the relative forms (the
/// directory konoma was opened in, or the anchor).
pub fn path_text(path: &Path, open_dir: &Path, kind: CopyKind) -> String {
    match kind {
        CopyKind::Name => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        CopyKind::Full => path.display().to_string(),
        CopyKind::Parent => path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        // Exactly the basis of the top-left title display (`format_path`'s Relative).
        CopyKind::Relative => rel_to_open(open_dir, path),
        CopyKind::AtRef => at_ref_text(open_dir, path),
    }
}

/// `@path` reference for AI agents (Claude Code's `@file` context syntax). Under `open_dir` the path is
/// strictly relative to it (`src/app.rs` -- no leading dir name, unlike `rel_to_open`); outside it is
/// `..`-relative; if neither works, absolute. Prefixed with `@`.
pub fn at_ref_text(open_dir: &Path, path: &Path) -> String {
    let rel = match path.strip_prefix(open_dir) {
        Ok(r) if !r.as_os_str().is_empty() => r.display().to_string(),
        _ => match rel_from(open_dir, path) {
            Some(r) if !r.as_os_str().is_empty() => r.display().to_string(),
            _ => path.display().to_string(),
        },
    };
    format!("@{rel}")
}

/// The cells a table copy reads. "Rows" are data rows: a CSV's header record is `header`.
pub trait CopyGrid {
    /// Number of data rows.
    fn nrows(&self) -> usize;
    /// Number of columns.
    fn ncols(&self) -> usize;
    /// The header text of `col`.
    fn header(&self, col: usize) -> Cow<'_, str>;
    /// How many cells data row `row` has (a ragged row is shorter than `ncols`; a sheet row ends
    /// at its last cell that shows something).
    fn row_len(&self, row: usize) -> usize;
    /// The text shown in a data cell (`""` when empty or out of range).
    fn cell(&self, row: usize, col: usize) -> Cow<'_, str>;
}

/// The string a table copy places on the clipboard. `cur` is the cursor `(row, col)`; `addressed`
/// is a spreadsheet (copies the text shown, tab-joined, never a header or the row-number gutter);
/// `sep` joins a row of a delimited table (`,` or the tab).
/// Cell = the cursor cell; Row = the row's cells joined; Column = header + every cell, one per line.
pub fn table_text(
    t: &dyn CopyGrid,
    kind: TableCopyKind,
    (r, c): (usize, usize),
    addressed: bool,
    sep: &str,
) -> String {
    if addressed {
        return match kind {
            TableCopyKind::Cell => t.cell(r, c).to_string(),
            // Tab-separated, up to the last cell that shows something (no trailing tabs).
            TableCopyKind::Row => (0..t.row_len(r))
                .map(|c| t.cell(r, c))
                .collect::<Vec<_>>()
                .join("\t"),
            TableCopyKind::Column => (0..t.nrows())
                .map(|r| t.cell(r, c))
                .collect::<Vec<_>>()
                .join("\n"),
        };
    }
    // A table with no data rows (a header-only file) copies its header; a ragged row copies
    // the cells it has, no padding.
    match kind {
        TableCopyKind::Cell => {
            if t.nrows() == 0 {
                t.header(c).to_string()
            } else {
                t.cell(r, c).to_string()
            }
        }
        TableCopyKind::Row => {
            if t.nrows() == 0 {
                (0..t.ncols())
                    .map(|c| t.header(c))
                    .collect::<Vec<_>>()
                    .join(sep)
            } else {
                (0..t.row_len(r))
                    .map(|c| t.cell(r, c))
                    .collect::<Vec<_>>()
                    .join(sep)
            }
        }
        TableCopyKind::Column => {
            let mut vals = vec![t.header(c).to_string()];
            vals.extend((0..t.nrows()).map(|r| t.cell(r, c).into_owned()));
            vals.join("\n")
        }
    }
}

/// The notice after a path copy: the whole copied text.
pub fn copied_notice(lang: Lang, text: &str) -> String {
    format!("{}{text}", CoreMsg::CopiedPrefix.tr(lang))
}

/// The notice after a copy of arbitrary text: the first line, cut at 60 characters, with `…`
/// when anything was cut or more lines follow, so a multi-line
/// copy does not overflow the footer.
pub fn copied_preview_notice(lang: Lang, text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    let mut preview: String = first.chars().take(60).collect();
    if first.chars().count() > 60 || text.lines().nth(1).is_some() {
        preview.push('…');
    }
    format!("{}{preview}", CoreMsg::CopiedPrefix.tr(lang))
}

/// The notice when the clipboard write failed.
pub fn copy_failed_notice(lang: Lang, err: &dyn std::fmt::Display) -> String {
    format!("{}{err}", CoreMsg::CopyFailed.tr(lang))
}

/// The notice when there is nothing to copy.
pub fn no_target_notice(lang: Lang) -> &'static str {
    CoreMsg::NoCopyTarget.tr(lang)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn five_kinds_under_the_open_dir() {
        let open = p("/w/proj");
        let f = p("/w/proj/src/app.rs");
        assert_eq!(path_text(&f, &open, CopyKind::Name), "app.rs");
        assert_eq!(path_text(&f, &open, CopyKind::Full), "/w/proj/src/app.rs");
        assert_eq!(path_text(&f, &open, CopyKind::Parent), "/w/proj/src");
        // Relative leads with the open directory's name; AtRef does not.
        assert_eq!(path_text(&f, &open, CopyKind::Relative), "proj/src/app.rs");
        assert_eq!(path_text(&f, &open, CopyKind::AtRef), "@src/app.rs");
    }

    #[test]
    fn relative_and_at_ref_outside_the_open_dir() {
        let open = p("/w/A");
        let f = p("/w/B/x.md");
        assert_eq!(path_text(&f, &open, CopyKind::Relative), "../B/x.md");
        assert_eq!(path_text(&f, &open, CopyKind::AtRef), "@../B/x.md");
        // An ancestor of the open dir.
        assert_eq!(at_ref_text(&open, &p("/w")), "@..");
    }

    #[test]
    fn root_level_file_and_the_open_dir_itself() {
        let open = p("/w/proj");
        assert_eq!(
            path_text(&p("/w/proj/a.txt"), &open, CopyKind::Relative),
            "proj/a.txt"
        );
        assert_eq!(
            path_text(&p("/w/proj/a.txt"), &open, CopyKind::AtRef),
            "@a.txt"
        );
        assert_eq!(
            path_text(&p("/w/proj/a.txt"), &open, CopyKind::Parent),
            "/w/proj"
        );
        // The open dir itself: nothing relative remains, so AtRef falls back to the absolute path.
        assert_eq!(at_ref_text(&open, &open), "@/w/proj");
    }

    #[test]
    fn directory_target_copies_like_a_file() {
        let open = p("/w/proj");
        let d = p("/w/proj/docs");
        assert_eq!(path_text(&d, &open, CopyKind::Name), "docs");
        assert_eq!(path_text(&d, &open, CopyKind::Parent), "/w/proj");
        assert_eq!(path_text(&d, &open, CopyKind::Relative), "proj/docs");
        assert_eq!(path_text(&d, &open, CopyKind::AtRef), "@docs");
        // The filesystem root has no name and no parent.
        assert_eq!(path_text(&p("/"), &open, CopyKind::Name), "");
        assert_eq!(path_text(&p("/"), &open, CopyKind::Parent), "");
    }

    #[test]
    fn full_width_paths_are_kept_as_is() {
        let open = p("/作業/案件");
        let f = p("/作業/案件/資料/メモ 一覧.txt");
        assert_eq!(path_text(&f, &open, CopyKind::Name), "メモ 一覧.txt");
        assert_eq!(path_text(&f, &open, CopyKind::Parent), "/作業/案件/資料");
        assert_eq!(
            path_text(&f, &open, CopyKind::Relative),
            "案件/資料/メモ 一覧.txt"
        );
        assert_eq!(path_text(&f, &open, CopyKind::AtRef), "@資料/メモ 一覧.txt");
    }

    /// Rows of cells; `None` header row means "no header text".
    struct G {
        headers: Vec<&'static str>,
        rows: Vec<Vec<&'static str>>,
    }
    impl CopyGrid for G {
        fn nrows(&self) -> usize {
            self.rows.len()
        }
        fn ncols(&self) -> usize {
            self.headers.len()
        }
        fn header(&self, col: usize) -> Cow<'_, str> {
            Cow::Borrowed(self.headers.get(col).copied().unwrap_or(""))
        }
        fn row_len(&self, row: usize) -> usize {
            self.rows.get(row).map_or(0, Vec::len)
        }
        fn cell(&self, row: usize, col: usize) -> Cow<'_, str> {
            Cow::Borrowed(
                self.rows
                    .get(row)
                    .and_then(|r| r.get(col))
                    .copied()
                    .unwrap_or(""),
            )
        }
    }

    fn grid() -> G {
        G {
            headers: vec!["name", "qty", "note"],
            rows: vec![vec!["apple", "", "赤い"], vec!["kiwi", "7"]],
        }
    }

    #[test]
    fn delimited_table_cell_row_column() {
        let g = grid();
        let t = |k, cur, sep| table_text(&g, k, cur, false, sep);
        assert_eq!(t(TableCopyKind::Cell, (0, 2), ","), "赤い");
        assert_eq!(t(TableCopyKind::Cell, (0, 1), ","), "");
        assert_eq!(t(TableCopyKind::Row, (0, 0), ","), "apple,,赤い");
        assert_eq!(t(TableCopyKind::Row, (0, 0), "\t"), "apple\t\t赤い");
        // A ragged row copies the cells it has, with no padding.
        assert_eq!(t(TableCopyKind::Row, (1, 0), ","), "kiwi,7");
        // Column = header + every cell, one per line; a missing cell is empty.
        assert_eq!(t(TableCopyKind::Column, (0, 1), ","), "qty\n\n7");
        assert_eq!(t(TableCopyKind::Column, (0, 2), ","), "note\n赤い\n");
    }

    #[test]
    fn header_only_table_copies_its_header() {
        let g = G {
            headers: vec!["a", "b"],
            rows: vec![],
        };
        let t = |k, sep| table_text(&g, k, (0, 1), false, sep);
        assert_eq!(t(TableCopyKind::Cell, ","), "b");
        assert_eq!(t(TableCopyKind::Row, ","), "a,b");
        assert_eq!(t(TableCopyKind::Row, "\t"), "a\tb");
        assert_eq!(t(TableCopyKind::Column, ","), "b");
    }

    #[test]
    fn addressed_sheet_is_tab_joined_and_ignores_the_separator() {
        // A sheet's row ends at its last cell that shows something (`row_len`).
        let g = G {
            headers: vec!["A", "B", "C"],
            rows: vec![vec!["Rate", "", "25%"], vec!["", "x"]],
        };
        let t = |k, cur| table_text(&g, k, cur, true, ",");
        assert_eq!(t(TableCopyKind::Cell, (0, 2)), "25%");
        assert_eq!(t(TableCopyKind::Row, (0, 0)), "Rate\t\t25%");
        assert_eq!(t(TableCopyKind::Row, (1, 0)), "\tx");
        // No header in a sheet's column copy.
        assert_eq!(t(TableCopyKind::Column, (0, 0)), "Rate\n");
    }

    #[test]
    fn notices_use_the_shared_wording_in_both_languages() {
        assert_eq!(copied_notice(Lang::En, "@a"), "copied: @a");
        assert_eq!(copied_notice(Lang::Jp, "@a"), "コピー: @a");
        assert_eq!(copy_failed_notice(Lang::En, &"boom"), "copy failed: boom");
        assert_eq!(copy_failed_notice(Lang::Jp, &"boom"), "コピー失敗: boom");
        assert_eq!(no_target_notice(Lang::En), "no copy target");
        assert_eq!(no_target_notice(Lang::Jp), "コピー対象がありません");
    }

    #[test]
    fn preview_notice_keeps_the_first_line_and_cuts_at_sixty() {
        assert_eq!(copied_preview_notice(Lang::En, "one"), "copied: one");
        assert_eq!(copied_preview_notice(Lang::En, "one\ntwo"), "copied: one…");
        let long = "あ".repeat(61);
        assert_eq!(
            copied_preview_notice(Lang::En, &long),
            format!("copied: {}…", "あ".repeat(60))
        );
        let exact = "x".repeat(60);
        assert_eq!(
            copied_preview_notice(Lang::En, &exact),
            format!("copied: {exact}")
        );
        assert_eq!(copied_preview_notice(Lang::En, ""), "copied: ");
    }
}
