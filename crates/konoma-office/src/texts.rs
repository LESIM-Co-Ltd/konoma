//! The words the spreadsheet plugin shows: the reasons a workbook cannot be shown, the cell popup
//! labels and the title's "hidden". Each is an English / Japanese pair.

use konoma_office_core::container::Limits;
use konoma_office_core::OfficeError;
use konoma_plugin::{t, Failure, Tr};

/// `8000000` -> `8,000,000`.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `256 MiB`, `2 GiB`.
fn bytes(b: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if b.is_multiple_of(1024 * MIB) {
        format!("{} GiB", b / (1024 * MIB))
    } else {
        format!("{} MiB", b / MIB)
    }
}

/// A pair whose `{n}` is the limit the readers enforce, so a changed limit can never leave the
/// sentence stating an old number.
fn with_limit(en: &str, ja: &str, limit: &str) -> Tr {
    Tr::new(en.replace("{n}", limit)).ja(ja.replace("{n}", limit))
}

/// The explanation of a workbook that could not be read.
pub(crate) fn error_text(err: &OfficeError) -> Tr {
    let l = Limits::default();
    match err {
        OfficeError::Encrypted => t!(
            en: "[spreadsheet] cannot preview — the file is password-protected (encrypted)",
            ja: "[表計算] 表示不可 — パスワードで保護(暗号化)されています",
        ),
        OfficeError::Corrupt(_) => t!(
            en: "[spreadsheet] cannot preview — the file is damaged or not a valid workbook",
            ja: "[表計算] 表示不可 — ファイルが壊れているか、正しいブックではありません",
        ),
        OfficeError::Unsupported => t!(
            en: "[spreadsheet] cannot preview — this file is not a spreadsheet format konoma reads",
            ja: "[表計算] 表示不可 — konoma が読めない形式のファイルです",
        ),
        OfficeError::Io(_) => t!(
            en: "[spreadsheet] cannot preview — the file could not be read",
            ja: "[表計算] 表示不可 — ファイルを読み込めませんでした",
        ),
        OfficeError::TooLarge { what } => match *what {
            "file" => with_limit(
                "[spreadsheet] too large to preview — the file is over the {n} limit",
                "[表計算] 大きすぎて表示できません — ファイルが上限 {n} を超えています",
                &bytes(l.max_file_bytes),
            ),
            "entries" => with_limit(
                "[spreadsheet] too large to preview — the package holds more than {n} parts",
                "[表計算] 大きすぎて表示できません — パッケージ内の部品数が上限 {n} を超えています",
                &group_thousands(l.max_entries as u64),
            ),
            "entry" => with_limit(
                "[spreadsheet] too large to preview — one part expands beyond the {n} limit",
                "[表計算] 大きすぎて表示できません — 1 つの部品の展開サイズが上限 {n} を超えています",
                &bytes(l.max_part_bytes),
            ),
            "package" => with_limit(
                "[spreadsheet] too large to preview — the package expands beyond the {n} limit",
                "[表計算] 大きすぎて表示できません — パッケージの展開サイズが上限 {n} を超えています",
                &bytes(l.max_total_bytes),
            ),
            "sheet area" | "sheet cells" => with_limit(
                "[spreadsheet] too large to preview — a sheet's used range exceeds the {n} cell limit",
                "[表計算] 大きすぎて表示できません — シートの使用範囲が上限 {n} セルを超えています",
                &group_thousands(l.max_dense_cells),
            ),
            "text" => with_limit(
                "[spreadsheet] too large to preview — its text (shared strings counted once per cell that uses them) exceeds the {n} limit",
                "[表計算] 大きすぎて表示できません — 文字列の合計（共有文字列は使用するセルごとに数えます）が上限 {n} を超えています",
                &bytes(l.max_text_bytes),
            ),
            _ => t!(
                en: "[spreadsheet] too large to preview — a safety limit was exceeded",
                ja: "[表計算] 大きすぎて表示できません — 安全のための上限を超えました",
            ),
        },
    }
}

/// A workbook that opened but has no visible sheet.
pub(crate) fn no_visible_sheets() -> Tr {
    t!(
        en: "[spreadsheet] nothing to show — every sheet in this workbook is hidden",
        ja: "[表計算] 表示できるシートがありません — すべてのシートが非表示です",
    )
}

/// The failure for a workbook error (the kind tells the host what sort of problem it was).
pub(crate) fn failure_of(err: &OfficeError) -> Failure {
    let message = error_text(err);
    let failure = match err {
        OfficeError::Encrypted | OfficeError::Unsupported => Failure::unsupported(message),
        OfficeError::TooLarge { .. } => Failure::too_large(message),
        OfficeError::Corrupt(_) => Failure::corrupt(message),
        OfficeError::Io(_) => Failure::io(message),
    };
    match err {
        OfficeError::Corrupt(d) | OfficeError::Io(d) => failure.with_detail(d.clone()),
        other => failure.with_detail(other.to_string()),
    }
}

/// The word that follows the count of hidden sheets in the title (`+1 hidden`).
pub(crate) fn hidden_word(lang: &str) -> &'static str {
    if lang == "ja" {
        "非表示"
    } else {
        "hidden"
    }
}

/// The labels of the cell popup.
pub(crate) mod cell {
    use super::*;

    pub fn address() -> Tr {
        t!(en: "Cell", ja: "セル")
    }
    pub fn displayed() -> Tr {
        t!(en: "Displayed", ja: "表示")
    }
    pub fn raw() -> Tr {
        t!(en: "Raw value", ja: "生の値")
    }
    pub fn type_label() -> Tr {
        t!(en: "Type", ja: "型")
    }
    pub fn formula() -> Tr {
        t!(en: "Formula", ja: "数式")
    }
    pub fn format() -> Tr {
        t!(en: "Number format", ja: "表示書式")
    }
}

/// The words for the type of a cell.
pub(crate) mod ty {
    use super::*;

    pub fn empty() -> Tr {
        t!(en: "empty", ja: "空")
    }
    pub fn number() -> Tr {
        t!(en: "number", ja: "数値")
    }
    pub fn text() -> Tr {
        t!(en: "text", ja: "文字列")
    }
    pub fn boolean() -> Tr {
        t!(en: "boolean", ja: "真偽値")
    }
    pub fn error() -> Tr {
        t!(en: "error", ja: "エラー")
    }
    pub fn date_time() -> Tr {
        t!(en: "date / time", ja: "日付・時刻")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use konoma_office_core::container::Limits;

    #[test]
    fn group_thousands_groups_by_three() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1,000");
        assert_eq!(group_thousands(8_000_000), "8,000,000");
        assert_eq!(group_thousands(10_000), "10,000");
    }

    fn every_error() -> Vec<(Option<OfficeError>, &'static str)> {
        let big = |what| Some(OfficeError::TooLarge { what });
        vec![
            (None, "nothing to show"),
            (Some(OfficeError::Encrypted), "password-protected"),
            (big("file"), "the file is over the"),
            (big("entries"), "parts"),
            (big("entry"), "one part expands"),
            (big("package"), "package expands"),
            (big("sheet area"), "used range exceeds"),
            (big("sheet cells"), "used range exceeds"),
            (big("text"), "its text"),
            (big("something new"), "a safety limit was exceeded"),
            (Some(OfficeError::Corrupt("x".into())), "damaged"),
            (Some(OfficeError::Unsupported), "not a spreadsheet format"),
            (Some(OfficeError::Io("x".into())), "could not be read"),
        ]
    }

    fn text_of(err: &Option<OfficeError>) -> Tr {
        match err {
            None => no_visible_sheets(),
            Some(e) => error_text(e),
        }
    }

    /// Every way a workbook can fail has its own explanation: the user is told *which* limit or
    /// problem it was, so two failures must never share a sentence by accident.
    #[test]
    fn every_office_error_has_its_own_message() {
        let cases = every_error();
        for lang in ["en", "ja"] {
            let mut seen: Vec<(String, String)> = Vec::new();
            for (err, key) in &cases {
                let text = text_of(err).get(lang).to_string();
                if lang == "en" {
                    assert!(text.contains(key), "{err:?}: {text}");
                }
                // "sheet area" and "sheet cells" are one sentence by design.
                let same_by_design = matches!(
                    err,
                    Some(OfficeError::TooLarge {
                        what: "sheet cells"
                    })
                );
                if !same_by_design {
                    if let Some((other, _)) = seen.iter().find(|(_, t)| *t == text) {
                        panic!("{err:?} and {other} read the same: {text}");
                    }
                }
                seen.push((format!("{err:?}"), text));
            }
        }
    }

    /// The numbers in the "too large" sentences come from the limits the readers enforce: a
    /// placeholder is never left in the text and the figure is the real one, in both languages.
    #[test]
    fn too_large_messages_state_the_enforced_limits() {
        let l = Limits::default();
        let cases: [(&'static str, String); 6] = [
            ("file", format!("{} MiB", l.max_file_bytes / 1024 / 1024)),
            ("entries", group_thousands(l.max_entries as u64)),
            ("entry", format!("{} MiB", l.max_part_bytes / 1024 / 1024)),
            (
                "package",
                format!("{} GiB", l.max_total_bytes / 1024 / 1024 / 1024),
            ),
            ("sheet area", group_thousands(l.max_dense_cells)),
            ("text", format!("{} MiB", l.max_text_bytes / 1024 / 1024)),
        ];
        for lang in ["en", "ja"] {
            for (what, figure) in &cases {
                let text = error_text(&OfficeError::TooLarge { what })
                    .get(lang)
                    .to_string();
                assert!(!text.contains("{n}"), "{what}: {text}");
                assert!(text.contains(figure.as_str()), "{what} {lang}: {text}");
            }
        }
    }

    #[test]
    fn a_failure_carries_the_kind_and_the_log_detail() {
        let f = failure_of(&OfficeError::Corrupt("bad zip".into()));
        assert_eq!(f.kind, konoma_plugin::FailureKind::Corrupt);
        assert_eq!(f.detail.as_deref(), Some("bad zip"));
        assert_eq!(
            failure_of(&OfficeError::TooLarge { what: "file" }).kind,
            konoma_plugin::FailureKind::TooLarge
        );
        assert_eq!(
            failure_of(&OfficeError::Encrypted).kind,
            konoma_plugin::FailureKind::Unsupported
        );
        assert_eq!(
            failure_of(&OfficeError::Io("denied".into())).kind,
            konoma_plugin::FailureKind::Io
        );
    }

    #[test]
    fn the_labels_and_type_words_are_in_both_languages() {
        for t in [
            cell::address(),
            cell::displayed(),
            cell::raw(),
            cell::type_label(),
            cell::formula(),
            cell::format(),
            ty::empty(),
            ty::number(),
            ty::text(),
            ty::boolean(),
            ty::error(),
            ty::date_time(),
        ] {
            assert_ne!(t.en(), t.get("ja"), "{} has no Japanese", t.en());
        }
        assert_eq!(hidden_word("en"), "hidden");
        assert_eq!(hidden_word("ja"), "非表示");
    }
}
