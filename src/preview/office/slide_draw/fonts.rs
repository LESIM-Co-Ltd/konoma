//! Font substitution and measurement for slide text.
//!
//! Slides name Office fonts (Calibri, Yu Gothic, ...) that a Mac or a Linux box rarely has. Each
//! name is mapped to a **stack**: the name itself first (used when the machine really has it),
//! then metric-compatible or look-alike faces that exist on macOS and Linux, then a generic
//! family. The same stack is written to the SVG's `font-family` and used to measure, so
//! "the face resvg draws" and "the face the line breaking measured" are the same one
//! (`preview::mermaid::text_metrics` resolves a stack exactly like usvg does).
//!
//! A stack is resolved to its **first installed face**, and that is exactly what usvg does too: its
//! font selector queries the whole `font-family` list and hands harfrust a single face per span.
//! When that face lacks a character, usvg does *not* walk the rest of the stack; it asks a
//! system-wide selector (`FontResolver::default_fallback_selector`, database order), and if the
//! face it names covers the whole run it re-shapes the whole run with it. `text_metrics` follows
//! that loop (`TextMetrics::measure`), so measuring and drawing agree here as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::preview::mermaid::text_metrics::TextMetrics;
use crate::preview::svg::shared_fontdb;
use resvg::usvg::fontdb;

/// Which of the three typefaces of a run a character is drawn with (DrawingML `latin` / `ea` /
/// `cs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
    Latin,
    EastAsian,
    Complex,
}

/// Classifies a character: CJK ideographs, kana, hangul, CJK punctuation and full-width forms are
/// East Asian; Arabic, Hebrew, Indic, Thai scripts are complex; everything else is Latin.
pub fn script_of(c: char) -> Script {
    let u = c as u32;
    match u {
        0x2E80..=0x2FDF
        | 0x3000..=0x303F
        | 0x3040..=0x30FF
        | 0x3100..=0x318F
        | 0x31C0..=0x31FF
        | 0x3200..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFEF
        | 0x1B000..=0x1B16F
        | 0x20000..=0x3FFFF => Script::EastAsian,
        0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0E7F | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF => {
            Script::Complex
        }
        _ => Script::Latin,
    }
}

/// Lower-case, half-width, no spaces or hyphens: the key the substitution table is matched on.
fn norm(name: &str) -> String {
    name.chars()
        .filter_map(|c| {
            let c = match c as u32 {
                0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
                0x3000 => ' ',
                _ => c,
            };
            if c == ' ' || c == '-' || c == '_' || c == '\u{a0}' {
                None
            } else {
                Some(c.to_ascii_lowercase())
            }
        })
        .collect()
}

const GOTHIC: &[&str] = &[
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "YuGothic",
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "IPAexGothic",
    "sans-serif",
];
const MINCHO: &[&str] = &[
    "Hiragino Mincho ProN",
    "YuMincho",
    "Noto Serif CJK JP",
    "IPAexMincho",
    "serif",
];
const SANS: &[&str] = &["Helvetica Neue", "Arial", "Liberation Sans", "sans-serif"];
const CARLITO: &[&str] = &[
    "Carlito",
    "Helvetica Neue",
    "Arial",
    "Liberation Sans",
    "sans-serif",
];
const CAMBRIA: &[&str] = &["Caladea", "Georgia", "serif"];
const ARIAL: &[&str] = &["Arial", "Liberation Sans", "Helvetica", "sans-serif"];
const TIMES: &[&str] = &["Times New Roman", "Liberation Serif", "Times", "serif"];
const MONO: &[&str] = &["Menlo", "Courier New", "Liberation Mono", "monospace"];
const SEGOE: &[&str] = &["Helvetica Neue", "Arial", "sans-serif"];

/// The substitution list for a font name. `script` decides what an unknown name falls back to
/// (a Japanese gothic list for East Asian text, a Latin sans list otherwise).
pub fn substitutes(name: &str, script: Script) -> &'static [&'static str] {
    let n = norm(name);
    let has = |ks: &[&str]| ks.iter().any(|k| n.contains(k));
    if n.is_empty() {
        return if script == Script::EastAsian {
            GOTHIC
        } else {
            SANS
        };
    }
    if has(&[
        "mincho",
        "明朝",
        "notoserifcjk",
        "notoserifjp",
        "ipaexmincho",
        "ipamincho",
        "bizudmincho",
        "hiraginomincho",
    ]) {
        return MINCHO;
    }
    if has(&[
        "gothic",
        "ゴシック",
        "meiryo",
        "メイリオ",
        "hiraginokaku",
        "hiraginosans",
        "notosanscjk",
        "notosansjp",
        "ipaexgothic",
        "ipagothic",
        "bizudgothic",
        "osaka",
    ]) {
        return GOTHIC;
    }
    match n.as_str() {
        "calibri" | "calibrilight" | "aptos" | "aptosdisplay" | "aptosnarrow" | "aptossemibold"
        | "carlito" => CARLITO,
        "cambria" | "cambriamath" | "caladea" => CAMBRIA,
        "arial" | "arialnarrow" | "arialblack" | "arialunicodems" | "helvetica"
        | "helveticaneue" => ARIAL,
        "timesnewroman" | "times" | "georgia" | "garamond" | "bookantiqua" | "palatinolinotype" => {
            TIMES
        }
        "couriernew" | "courier" | "consolas" | "lucidaconsole" | "menlo" | "monaco" => MONO,
        "segoeui" | "segoeuilight" | "segoeuisemibold" | "tahoma" | "verdana" | "trebuchetms" => {
            SEGOE
        }
        _ => {
            if script == Script::EastAsian {
                GOTHIC
            } else {
                SANS
            }
        }
    }
}

/// The font stack for `name` and `script`: the name itself first, then its substitutes (without
/// repeating the name).
pub fn stack_for(name: Option<&str>, script: Script) -> Vec<String> {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    let default = match script {
        Script::EastAsian => "Yu Gothic",
        _ => "Calibri",
    };
    let name = name.unwrap_or(default);
    let mut v: Vec<String> = Vec::new();
    // Quotes would break out of the quoted family name in the SVG attribute.
    let clean: String = name
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '<' | '>' | '&' | '\\') && !c.is_control())
        .collect();
    if !clean.is_empty() && !is_generic(&clean) {
        v.push(clean.clone());
    }
    for s in substitutes(name, script) {
        if !v.iter().any(|x| x.eq_ignore_ascii_case(s)) {
            v.push((*s).to_string());
        }
    }
    v
}

fn is_generic(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy"
    )
}

/// A typeface name as a quoted CSS string that usvg's font-family parser reads back as the same
/// name. That parser ends a string at the next quote character of the kind that opened it and
/// does not unescape anything (a backslash only stops a quote from ending the string, and is kept
/// in the name), so escaping cannot work: the name is quoted with the kind of quote it does not
/// contain; a name with both kinds loses its single quotes, and backslashes (which would hide the
/// closing quote) are dropped.
pub(super) fn quote_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| *c != '\\').collect();
    if !clean.contains('\'') {
        format!("'{clean}'")
    } else if !clean.contains('"') {
        format!("\"{clean}\"")
    } else {
        format!("'{}'", clean.replace('\'', ""))
    }
}

/// The CSS `font-family` value of a stack (names quoted, generics bare). Not XML-escaped.
pub fn css_family(stack: &[String]) -> String {
    stack
        .iter()
        .map(|s| {
            if is_generic(s) {
                s.clone()
            } else {
                quote_name(s)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

type Key = (String, bool, bool);
/// A face of one particular database (ids are only unique within a database; the fontless
/// database used for drawing SVG without text has ids of its own).
type FaceKey = (usize, fontdb::ID);

/// What a family stack resolves to, remembered at two levels. Thousands of distinct family names
/// resolve to a handful of faces, and opening a face (to read its metrics and kerning) is the
/// expensive step; the lookup of a name is cheap, so the name table may be emptied when full
/// without costing much, while the faces are kept (there are only as many as the machine has).
#[derive(Default)]
struct FontCaches {
    /// (css family, bold, italic) -> the face the stack resolves to (`None`: no face at all).
    by_family: HashMap<Key, Option<fontdb::ID>>,
    /// face -> its measuring state, filled once outside the lock.
    by_face: HashMap<FaceKey, Arc<OnceLock<Option<Arc<TextMetrics>>>>>,
}

/// Most distinct family strings remembered; stacks come from the file.
const MAX_FAMILY_ENTRIES: usize = 4096;

fn cache() -> &'static Mutex<FontCaches> {
    static C: OnceLock<Mutex<FontCaches>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FontCaches::default()))
}

/// The measuring face for a stack, bold and italic flags (cached per process).
///
/// The lock is held only to read and write the tables, never while a face is looked up or opened:
/// a document with thousands of typefaces does not make every other measurement wait.
pub fn metrics_for(stack: &[String], bold: bool, italic: bool) -> Option<Arc<TextMetrics>> {
    let db = shared_fontdb();
    let db_id = Arc::as_ptr(&db) as usize;
    let key = (css_family(stack), bold, italic);
    let known = {
        let c = cache().lock().unwrap_or_else(|e| e.into_inner());
        c.by_family.get(&key).copied()
    };
    let face = match known {
        Some(f) => f?,
        None => {
            let families: Vec<fontdb::Family> = stack
                .iter()
                .map(|s| match s.to_ascii_lowercase().as_str() {
                    "serif" => fontdb::Family::Serif,
                    "sans-serif" => fontdb::Family::SansSerif,
                    "monospace" => fontdb::Family::Monospace,
                    "cursive" => fontdb::Family::Cursive,
                    "fantasy" => fontdb::Family::Fantasy,
                    _ => fontdb::Family::Name(s.as_str()),
                })
                .chain(std::iter::once(fontdb::Family::Serif))
                .collect();
            let weight = if bold {
                fontdb::Weight::BOLD
            } else {
                fontdb::Weight::NORMAL
            };
            let face = TextMetrics::query_face(&db, &families, weight, italic);
            let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
            if c.by_family.len() >= MAX_FAMILY_ENTRIES {
                c.by_family.clear();
            }
            c.by_family.insert(key, face);
            face?
        }
    };
    let cell = {
        let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(c.by_face.entry((db_id, face)).or_default())
    };
    cell.get_or_init(|| TextMetrics::resolve_face(db, face).ok().map(Arc::new))
        .clone()
}

/// Width in px of `text` (one line, no wrapping) at `size_px`.
///
/// Measured against the face the stack resolves to; with no font at all, a crude estimate
/// (East-Asian characters one em, others half an em) so layout still produces something.
pub fn measure(stack: &[String], bold: bool, italic: bool, text: &str, size_px: f64) -> f64 {
    if text.is_empty() || !(size_px.is_finite() && size_px > 0.0) {
        return 0.0;
    }
    match metrics_for(stack, bold, italic) {
        Some(m) => m.measure(text, size_px as f32) as f64,
        None => {
            text.chars()
                .map(|c| {
                    if script_of(c) == Script::EastAsian {
                        1.0
                    } else {
                        0.5
                    }
                })
                .sum::<f64>()
                * size_px
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_names_for_the_same_face_share_one_measuring_state() {
        let one = metrics_for(&["Zzz Typeface One".to_string()], false, false);
        let two = metrics_for(&["Zzz Typeface Two".to_string()], false, false);
        match (one, two) {
            (Some(a), Some(b)) => assert!(Arc::ptr_eq(&a, &b), "unknown names fall to one face"),
            (None, None) => {}
            _ => panic!("the same fallback must give the same answer"),
        }
        // bold is another face (when the machine has one); the same stack gives the same state
        let again = metrics_for(&["Zzz Typeface One".to_string()], false, false);
        let first = metrics_for(&["Zzz Typeface One".to_string()], false, false);
        assert_eq!(again.is_some(), first.is_some());
        if let (Some(a), Some(b)) = (again, first) {
            assert!(Arc::ptr_eq(&a, &b));
        }
    }

    #[test]
    fn emptying_the_name_table_keeps_the_faces() {
        let stack = stack_for(Some("Calibri"), Script::Latin);
        let before = metrics_for(&stack, false, false);
        for i in 0..MAX_FAMILY_ENTRIES + 20 {
            let _ = metrics_for(&[format!("Distinct Typeface {i}")], false, false);
        }
        let after = metrics_for(&stack, false, false);
        match (before, after) {
            (Some(a), Some(b)) => assert!(Arc::ptr_eq(&a, &b), "the state survives the table"),
            (None, None) => {}
            _ => panic!("the answer changed"),
        }
        let table = cache().lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            table.by_family.len() <= MAX_FAMILY_ENTRIES,
            "{}",
            table.by_family.len()
        );
        // as many measuring states as there are faces in play, not as many as there were names
        assert!(table.by_face.len() < 50, "{}", table.by_face.len());
    }

    #[test]
    fn a_thousand_distinct_typefaces_are_resolved_without_holding_the_lock() {
        // Another thread measures while this one resolves new names: neither waits for the other
        // for longer than a table access (a tight loop here would starve it if the lock were held
        // across the lookup of every name).
        let done = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            let h = s.spawn(|| {
                let mut n = 0u32;
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = measure(
                        &stack_for(Some("Arial"), Script::Latin),
                        false,
                        false,
                        "Hello",
                        16.0,
                    );
                    n += 1;
                }
                n
            });
            for i in 0..1000 {
                let _ = metrics_for(&[format!("Another Typeface {i}")], i % 2 == 0, false);
            }
            done.store(true, std::sync::atomic::Ordering::Relaxed);
            assert!(h.join().unwrap() > 0, "the other thread made progress");
        });
    }

    #[test]
    fn script_classes() {
        assert_eq!(script_of('a'), Script::Latin);
        assert_eq!(script_of('1'), Script::Latin);
        assert_eq!(script_of(' '), Script::Latin);
        assert_eq!(script_of('あ'), Script::EastAsian);
        assert_eq!(script_of('漢'), Script::EastAsian);
        assert_eq!(script_of('、'), Script::EastAsian);
        assert_eq!(script_of('「'), Script::EastAsian);
        assert_eq!(script_of('Ａ'), Script::EastAsian);
        assert_eq!(script_of('한'), Script::EastAsian);
        assert_eq!(script_of('م'), Script::Complex);
        assert_eq!(script_of('א'), Script::Complex);
        assert_eq!(script_of('😀'), Script::Latin);
        assert_eq!(script_of('𠀋'), Script::EastAsian);
    }

    #[test]
    fn substitution_table() {
        let s = |n: &str, sc| stack_for(Some(n), sc);
        assert_eq!(s("Calibri", Script::Latin)[0], "Calibri");
        assert_eq!(s("Calibri", Script::Latin)[1], "Carlito");
        assert!(s("Calibri Light", Script::Latin).contains(&"Carlito".to_string()));
        assert!(s("Aptos", Script::Latin).contains(&"Carlito".to_string()));
        assert!(s("Cambria", Script::Latin).contains(&"Caladea".to_string()));
        assert!(s("Arial", Script::Latin).contains(&"Liberation Sans".to_string()));
        assert!(s("Times New Roman", Script::Latin).contains(&"Liberation Serif".to_string()));
        assert!(s("Consolas", Script::Latin).contains(&"Menlo".to_string()));
        assert!(s("Courier New", Script::Latin).contains(&"Liberation Mono".to_string()));
        assert!(s("Segoe UI", Script::Latin).contains(&"Helvetica Neue".to_string()));
        for n in [
            "游ゴシック",
            "Yu Gothic",
            "メイリオ",
            "Meiryo",
            "ＭＳ Ｐゴシック",
            "MS PGothic",
            "MS Gothic",
            "Hiragino Kaku Gothic Pro",
            "Noto Sans JP",
        ] {
            let v = s(n, Script::EastAsian);
            assert!(v.contains(&"Hiragino Sans".to_string()), "{n}: {v:?}");
            assert!(v.contains(&"Noto Sans CJK JP".to_string()), "{n}");
            assert_eq!(v.last().unwrap(), "sans-serif");
        }
        for n in [
            "游明朝",
            "Yu Mincho",
            "ＭＳ 明朝",
            "MS Mincho",
            "ＭＳ Ｐ明朝",
        ] {
            let v = s(n, Script::EastAsian);
            assert!(
                v.contains(&"Hiragino Mincho ProN".to_string()),
                "{n}: {v:?}"
            );
            assert_eq!(v.last().unwrap(), "serif");
        }
        // unknown: itself, then the sans / gothic list by script
        let v = s("Zapfino Ultra", Script::Latin);
        assert_eq!(v[0], "Zapfino Ultra");
        assert_eq!(v.last().unwrap(), "sans-serif");
        let v = s("Zapfino Ultra", Script::EastAsian);
        assert!(v.contains(&"Hiragino Sans".to_string()));
        // default font when none is named
        assert_eq!(stack_for(None, Script::Latin)[0], "Calibri");
        assert_eq!(stack_for(Some("  "), Script::EastAsian)[0], "Yu Gothic");
    }

    #[test]
    fn css_is_quoted_and_safe() {
        let v = stack_for(Some("Evil'; } <x> \"font"), Script::Latin);
        let css = css_family(&v);
        assert!(!css.contains('<') && !css.contains('"'));
        assert_eq!(css.matches('\'').count() % 2, 0, "{css}");
        let css = css_family(&stack_for(Some("Calibri"), Script::Latin));
        assert!(css.starts_with("'Calibri', 'Carlito'"));
        assert!(css.ends_with("sans-serif"));
        assert!(!css.contains("'sans-serif'"));
    }

    /// What usvg's parser reads back from a quoted name.
    fn read_back(css: &str) -> Vec<String> {
        svgtypes::parse_font_families(css)
            .expect("the list parses")
            .into_iter()
            .map(|f| match f {
                svgtypes::FontFamily::Named(n) => n,
                other => format!("{other}"),
            })
            .collect()
    }

    #[test]
    fn a_name_with_a_quote_is_read_back_as_itself() {
        // (Both kinds of quote and backslashes, as an XML attribute can carry them.)
        assert_eq!(quote_name("Plain Name"), "'Plain Name'");
        assert_eq!(quote_name("Bob's Font"), "\"Bob's Font\"");
        assert_eq!(quote_name("The \"Font\""), "'The \"Font\"'");
        assert_eq!(quote_name("Both ' and \""), "'Both  and \"'");
        assert_eq!(quote_name("Back\\slash"), "'Backslash'");
        assert_eq!(quote_name("End\\"), "'End'");
        for name in [
            "Plain Name",
            "Bob's Font",
            "The \"Font\"",
            "Back\\slash",
            "End\\",
            "a, b",
            "Both ' and \"",
        ] {
            let css = css_family(&[name.to_string(), "sans-serif".to_string()]);
            let got = read_back(&css);
            assert_eq!(got.len(), 2, "{css}");
            let want: String = name
                .chars()
                .filter(|c| *c != '\\')
                .filter(|c| !(name.contains('"') && name.contains('\'') && *c == '\''))
                .collect();
            assert_eq!(got[0], want, "{css}");
        }
    }

    #[test]
    fn measurement_scales_and_handles_odd_input() {
        let st = stack_for(Some("Arial"), Script::Latin);
        let a = measure(&st, false, false, "Hello world", 16.0);
        let b = measure(&st, false, false, "Hello world", 32.0);
        assert!(a > 0.0);
        assert!((b / a - 2.0).abs() < 0.05, "{a} {b}");
        assert_eq!(measure(&st, false, false, "", 16.0), 0.0);
        assert_eq!(measure(&st, false, false, "x", f64::NAN), 0.0);
        assert_eq!(measure(&st, false, false, "x", -3.0), 0.0);
        // CJK is wider than Latin per character
        let cjk = measure(
            &stack_for(Some("Yu Gothic"), Script::EastAsian),
            false,
            false,
            "漢字",
            20.0,
        );
        assert!(cjk > 20.0, "{cjk}");
    }
}
