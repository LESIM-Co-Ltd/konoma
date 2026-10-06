//! Spreadsheet preview (xlsx / xlsm / xltx / xltm / xlsb / xls / ods): the reading side.
//!
//! xlsx / xlsm / xltx / xltm are read by konoma itself in one pass (`xlsx.rs`: values, number
//! formats, formulas, merged ranges). For xlsb / xls / ods the values, sheet list, visibility and
//! formulas come from `calamine`, and the number format of each cell (which `calamine` does not
//! expose) is read by konoma's own per-format "format pass" (`fmt_*.rs`). Every file passes
//! through [`container`] first: size limits, decompression accounting and encryption detection
//! happen *before* any reader runs.
//!
//! This module has no UI, `App` or i18n dependency: [`OfficeError`] carries no user-facing text
//! (the caller maps each variant to a translated message).

pub mod container;
pub mod docx;
mod docx_styles;
mod docx_xml;
pub mod fmt_ods;
pub mod fmt_xls;
pub mod fmt_xlsb;
pub mod fmt_xlsx;
pub mod numfmt;
mod ods_formula;
pub(crate) mod omml;
pub mod workbook;
pub(crate) mod xlsx;

use std::fmt;
use std::path::Path;

pub use workbook::{
    load_workbook_sheet_cancellable, Cancel, CellType, LoadOptions, NumFmtRef, Sheet, Workbook,
};
// What the tests of this module (`use super::*`) name directly.
#[cfg(test)]
pub use workbook::{
    display_text, load_workbook, load_workbook_sheet, Cell, CellValue, DisplayCtx, MergeRange,
};

/// The spreadsheet kinds konoma previews, by file extension (case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetKind {
    /// `.xlsx`
    Xlsx,
    /// `.xlsm` (macro-enabled; macros are never executed)
    Xlsm,
    /// `.xltx` (template)
    Xltx,
    /// `.xltm` (macro-enabled template)
    Xltm,
    /// `.xlsb` (binary)
    Xlsb,
    /// `.xls` (BIFF8)
    Xls,
    /// `.ods` (OpenDocument)
    Ods,
}

impl SheetKind {
    /// The kind for an extension without the dot (`"XLSX"` works).
    pub fn from_ext(ext: &str) -> Option<SheetKind> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "xlsx" => SheetKind::Xlsx,
            "xlsm" => SheetKind::Xlsm,
            "xltx" => SheetKind::Xltx,
            "xltm" => SheetKind::Xltm,
            "xlsb" => SheetKind::Xlsb,
            "xls" => SheetKind::Xls,
            "ods" => SheetKind::Ods,
            _ => return None,
        })
    }

    /// The kind for a path, from its extension.
    pub fn from_path(path: &Path) -> Option<SheetKind> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(SheetKind::from_ext)
    }
}

/// The locale that decides what a locale-dependent built-in format looks like (e.g. built-in
/// number 14 is `m/d/yy` in English and `yyyy/m/d` in Japanese). Defined by the number-format
/// engine so there is one type.
pub use numfmt::Locale;

/// Why a workbook could not be loaded. No user-facing text here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficeError {
    /// Password-protected / encrypted.
    Encrypted,
    /// A safety limit was exceeded; `what` names the limit (`"file"`, `"entries"`, `"entry"`,
    /// `"package"`, `"sheet area"`, `"sheet cells"`, `"text"`).
    TooLarge {
        /// Which limit.
        what: &'static str,
    },
    /// Damaged, truncated or not a spreadsheet container at all; the string is for logs.
    Corrupt(String),
    /// A valid file that is not a spreadsheet we read (docx, doc, unknown extension, ...).
    Unsupported,
    /// The file could not be opened or read from disk; the string is for logs.
    Io(String),
}

impl fmt::Display for OfficeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OfficeError::Encrypted => write!(f, "encrypted workbook"),
            OfficeError::TooLarge { what } => write!(f, "workbook too large ({what})"),
            OfficeError::Corrupt(m) => write!(f, "corrupt workbook: {m}"),
            OfficeError::Unsupported => write!(f, "unsupported file"),
            OfficeError::Io(m) => write!(f, "io error: {m}"),
        }
    }
}

impl std::error::Error for OfficeError {}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_complex;
#[cfg(test)]
mod tests_docx;
#[cfg(test)]
mod tests_docx_lists;
#[cfg(test)]
mod tests_docx_more;
#[cfg(test)]
mod tests_docx_robust;
#[cfg(test)]
mod tests_limits;
#[cfg(test)]
mod tests_numfmt;
#[cfg(test)]
mod tests_ods;
#[cfg(test)]
mod tests_survivors;
#[cfg(test)]
mod tests_xls;
#[cfg(test)]
mod tests_xlsb;
#[cfg(test)]
mod tests_xlsx;
