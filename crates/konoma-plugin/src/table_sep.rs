//! The field separator of a CSV/TSV table, in one place for the plugin that reads it and for the
//! hosts that copy a row of it.

use std::path::Path;

/// The field separator of a CSV/TSV input: the note from the binding wins (`"tsv"` is the tab, any
/// other note is the comma), otherwise the extension decides.
pub fn delimiter(hint: Option<&str>, path: &Path) -> u8 {
    match hint {
        Some("tsv") => b'\t',
        Some(_) => b',',
        None => {
            let tsv = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
            if tsv {
                b'\t'
            } else {
                b','
            }
        }
    }
}
