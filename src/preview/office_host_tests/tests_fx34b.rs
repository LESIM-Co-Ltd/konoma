//! Moved from `konoma-office-core/src/tests_fx34b.rs`: the tests that need the host's Markdown renderer.

use super::tests_docx::*;
use super::tests_odt as od;
pub use konoma_office_core::tests_fx34b::*;

/// What the reader sees must hold the text itself, as text: no heading, quote, list or rule.
fn assert_literal(m: &str, t: &str, what: &str) {
    let v = visible(m).replace('\u{200B}', "");
    assert!(
        v.contains(t),
        "{what}: `{t}` is not shown as text\nMD: {m:?}\nVIS: {v:?}"
    );
}

/// The renderer strips `<sup>`/`<sub>` before it reads blocks, so the first character inside a tag
/// at a line start begins the line (catches `esc(core, false)`: `<sup># x</sup>` became a heading,
/// `<sup>- x</sup>` a list, `<sup>***</sup>` a rule).
#[test]
fn text_in_sup_or_sub_at_a_line_start_is_not_taken_for_a_block() {
    for t in STARTERS {
        for (tag, rpr) in [("sup", SUP), ("sub", SUB)] {
            let m = md(&para(&runp(rpr, &xe(t))));
            assert_literal(&m, t, &format!("{tag} at the start"));
            // After a line break inside the run, and after a break between runs.
            let m = md(&para(&format!(
                "<w:r><w:rPr>{rpr}</w:rPr><w:t>a</w:t><w:br/><w:t>{}</w:t></w:r>",
                xe(t)
            )));
            assert_literal(&m, t, &format!("{tag} after a break"));
            let m = md(&para(
                &(run("a") + "<w:r><w:br/></w:r>" + &runp(rpr, &xe(t))),
            ));
            assert_literal(&m, t, &format!("{tag} after a break run"));
        }
    }
}

/// The same guard for the first paragraph of a note (a different line start).
#[test]
fn text_in_sup_at_a_note_start_is_not_taken_for_a_block() {
    for t in ["# Heading injected", "- item", "***"] {
        let note = format!(
            r#"<w:footnote w:id="1"><w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:footnote>"#,
            xe(t)
        );
        let _ = note;
        let note = format!(
            r#"<w:footnote w:id="1"><w:p><w:r><w:rPr>{SUP}</w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:footnote>"#,
            xe(t)
        );
        let d = Dx::new(&para(
            &(run("body") + r#"<w:r><w:footnoteReference w:id="1"/></w:r>"#),
        ))
        .footnotes(&note);
        let m = conv(&d).markdown;
        assert_literal(&m, t, "note start");
    }
}

/// OpenDocument writes a raise as `style:text-position`; it goes through the same emitter.
#[test]
fn odt_raised_text_at_a_line_start_is_not_taken_for_a_block() {
    let auto = od::text_style("T1", r#"style:text-position="super 58%""#, None)
        + &od::text_style("T2", r#"style:text-position="sub 58%""#, None);
    for t in STARTERS {
        for s in ["T1", "T2"] {
            let m = od::md_auto(&od::p(&od::span(s, &xe(t))), &auto);
            assert_literal(&m, t, &format!("odt {s}"));
            let m = od::md_auto(
                &od::p(&format!("a<text:line-break/>{}", od::span(s, &xe(t)))),
                &auto,
            );
            assert_literal(&m, t, &format!("odt {s} after a break"));
        }
    }
}

fn xe(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The first marker of every block kind a Markdown line can start with.
const STARTERS: [&str; 10] = [
    "# Heading injected",
    "> quoted",
    "- item",
    "+ plus",
    "* star",
    "1. one",
    "---",
    "***",
    "___",
    "=== eq",
];
