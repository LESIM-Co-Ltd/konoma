//! OpenFormula (the formula text an ods stores, `of:=SUM([.B3:.B4])`) shown the way Excel writes
//! it (`SUM(B3:B4)`), which is what a reader of a spreadsheet expects to see.
//!
//! Only the *notation* changes: the `of:=` prefix goes, a reference `[.B3]` / `[.B3:.B4]` /
//! `[Sheet1.B3]` / `['My Sheet'.$B$3:.$C$4]` loses its brackets and the dot, a sheet is written
//! `Sheet1!B3`, and the argument separator `;` is `,`. Text in `"..."` is never touched. A
//! formula this does not understand (inline arrays, a range across two sheets, a reference to
//! another file, unbalanced brackets or quotes) is returned as it is, prefix and all.

/// The formula as Excel writes it, without the leading `=` (the form every other format has).
pub(crate) fn to_a1(raw: &str) -> String {
    let body = ["of:=", "oooc:=", "msoxl:="]
        .iter()
        .find_map(|p| raw.strip_prefix(p));
    match body {
        // `msoxl:` is already Excel's notation.
        Some(b) if raw.starts_with("msoxl:") => b.to_string(),
        Some(b) => convert(b).unwrap_or_else(|| raw.to_string()),
        // Not marked as OpenFormula: shown as written.
        None => raw.to_string(),
    }
}

fn convert(body: &str) -> Option<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                // A string: copied whole (`""` is a quote inside it).
                out.push('"');
                i += 1;
                loop {
                    let c = *chars.get(i)?;
                    out.push(c);
                    i += 1;
                    if c == '"' {
                        if chars.get(i) == Some(&'"') {
                            out.push('"');
                            i += 1;
                        } else {
                            break;
                        }
                    }
                }
            }
            '[' => {
                let (text, next) = reference(&chars, i + 1)?;
                out.push_str(&text);
                i = next;
            }
            // An array literal has its own separators; not converted.
            '{' | '}' => return None,
            ';' => {
                out.push(',');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(out)
}

/// One part of a reference: an optional sheet, then `.`, then the cell (or column / row).
fn part(chars: &[char], mut i: usize) -> Option<(Option<String>, String, usize)> {
    let mut sheet = None;
    if chars.get(i) == Some(&'$') {
        i += 1;
    }
    if chars.get(i) == Some(&'\'') {
        // A quoted sheet name (`''` is a quote inside it).
        let mut name = String::from("'");
        i += 1;
        loop {
            let c = *chars.get(i)?;
            name.push(c);
            i += 1;
            if c == '\'' {
                if chars.get(i) == Some(&'\'') {
                    name.push('\'');
                    i += 1;
                } else {
                    break;
                }
            }
        }
        sheet = Some(name);
        if chars.get(i) != Some(&'.') {
            // `'file:///x.ods'#$Sheet.A1`: a reference to another file.
            return None;
        }
    } else {
        let start = i;
        while chars
            .get(i)
            .is_some_and(|&c| c != '.' && c != ':' && c != ']')
        {
            i += 1;
        }
        if i > start {
            sheet = Some(chars[start..i].iter().collect());
        }
        if chars.get(i) != Some(&'.') {
            return None;
        }
    }
    i += 1; // the dot
    let start = i;
    while chars.get(i).is_some_and(|&c| c != ':' && c != ']') {
        i += 1;
    }
    let cell: String = chars[start..i].iter().collect();
    if cell.is_empty() || cell.contains(['[', '\'', '"', ' ']) {
        return None;
    }
    Some((sheet, cell, i))
}

/// A reference after its `[`: the Excel text and the index after the `]`.
fn reference(chars: &[char], at: usize) -> Option<(String, usize)> {
    let (sheet, cell, mut i) = part(chars, at)?;
    let mut text = match &sheet {
        Some(s) => format!("{s}!{cell}"),
        None => cell,
    };
    if chars.get(i) == Some(&':') {
        let (sheet2, cell2, j) = part(chars, i + 1)?;
        // A second sheet is the same one (written out again) or a range over several sheets,
        // which Excel writes differently: not converted.
        match (&sheet, &sheet2) {
            (_, None) => {}
            (Some(a), Some(b)) if a == b => {}
            _ => return None,
        }
        text.push(':');
        text.push_str(&cell2);
        i = j;
    }
    (chars.get(i) == Some(&']')).then_some((text, i + 1))
}

#[cfg(test)]
mod tests {
    use super::to_a1;

    #[test]
    fn a_range_a_cell_and_the_separator() {
        assert_eq!(to_a1("of:=SUM([.B3:.B4])"), "SUM(B3:B4)");
        assert_eq!(to_a1("of:=[.B3]+[.C3]"), "B3+C3");
        assert_eq!(to_a1("of:=IF([.A1]>0;[.B1];[.C1])"), "IF(A1>0,B1,C1)");
        assert_eq!(to_a1("of:=SUM([.B2:.B6])"), "SUM(B2:B6)");
        assert_eq!(to_a1("of:=1/0"), "1/0");
    }

    #[test]
    fn sheets_and_absolute_references() {
        assert_eq!(to_a1("of:=[Sheet1.B3]"), "Sheet1!B3");
        assert_eq!(to_a1("of:=[$Sheet1.$B$3]"), "Sheet1!$B$3");
        assert_eq!(to_a1("of:=SUM([Sheet1.B3:.B4])"), "SUM(Sheet1!B3:B4)");
        assert_eq!(to_a1("of:=SUM([$Sheet1.B3:.$C$4])"), "SUM(Sheet1!B3:$C$4)");
        assert_eq!(to_a1("of:=SUM([Sheet1.B3:Sheet1.B4])"), "SUM(Sheet1!B3:B4)");
        assert_eq!(to_a1("of:=[$'My Sheet'.B3]"), "'My Sheet'!B3");
        assert_eq!(to_a1("of:=[$'It''s'.B3:.C4]"), "'It''s'!B3:C4");
        assert_eq!(to_a1("of:=SUM([.B:.B])+SUM([.1:.1])"), "SUM(B:B)+SUM(1:1)");
        assert_eq!(to_a1("of:=[$日本.A1]"), "日本!A1");
    }

    #[test]
    fn text_in_quotes_is_left_alone() {
        assert_eq!(
            to_a1(r#"of:=CONCATENATE("[.A1];x";[.B1])"#),
            r#"CONCATENATE("[.A1];x",B1)"#
        );
        assert_eq!(to_a1(r#"of:=[.A1]&"a""b;[.c]""#), r#"A1&"a""b;[.c]""#);
    }

    #[test]
    fn what_is_not_understood_is_shown_as_it_is() {
        for raw in [
            "of:={1;2|3;4}",
            "of:=SUM([Sheet1.A1:Sheet2.B2])",
            "of:=['file:///x.ods'#$Sheet.A1]",
            "of:=[.A1",
            r#"of:="unclosed"#,
            "of:=[.]",
            "of:=[A1]",
            "of:=[.A1:]",
        ] {
            assert_eq!(to_a1(raw), raw, "{raw}");
        }
        // Not marked as OpenFormula: as written.
        assert_eq!(to_a1("SUM(A1:A2)"), "SUM(A1:A2)");
        assert_eq!(to_a1(""), "");
        assert_eq!(to_a1("msoxl:=SUM(A1:A2)"), "SUM(A1:A2)");
    }
}
