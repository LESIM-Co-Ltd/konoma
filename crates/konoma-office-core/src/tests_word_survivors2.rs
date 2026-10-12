//! Tests added after the second mutation audit of the Word / OpenDocument text preview: each pins
//! a rule or a limit that a mutated build (a dropped branch, a bound off by one) used to get away
//! with. The comment on each test says what it catches.

use super::docx::DocOptions;
use super::mathml::{fallback_text, is_math, to_latex};
use super::omml::{drawable, MAX_DRAWABLE_BYTES};
use super::tests_docx::{conv_with as dconv_with, para, Dx};
use super::tests_odt::{md as omd, p as op};

const NS: &str = "xmlns=\"http://www.w3.org/1998/Math/MathML\"";

fn wrap(body: &str) -> String {
    format!("<math {NS}>{body}</math>")
}

// ---------------------------------------------------------------------------------------------
// Word: footnotes
// ---------------------------------------------------------------------------------------------

/// The notes' total budget ends the reading of notes at the first one (in the notes part's own
/// order) that does not fit: the notes after it are not read, however small (catches the `break`
/// after `keep_note` saying no being dropped: the reading went on and held every later note, and
/// only the final assembly, which sorts them by reference, hid it).
#[test]
fn the_first_note_over_the_budget_ends_the_reading_of_notes() {
    let note = |id: u32, text: &str| {
        format!(r#"<w:footnote w:id="{id}"><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:footnote>"#)
    };
    let refs: String = (1..=3)
        .map(|i| format!(r#"<w:r><w:footnoteReference w:id="{i}"/></w:r>"#))
        .collect();
    // The oversized note stands first in the part, though it is referenced second.
    let d = Dx::new(&para(&format!(r#"<w:r><w:t>body</w:t></w:r>{refs}"#)))
        .footnotes(&(note(2, &"B".repeat(6000)) + &note(1, "small one") + &note(3, "small three")));
    let opts = DocOptions {
        max_markdown_bytes: 3000,
        ..DocOptions::default()
    };
    let doc = dconv_with(&d, &opts).unwrap();
    assert!(doc.truncated, "{}", doc.markdown);
    assert!(
        !doc.markdown.contains("]: "),
        "no note is read after the oversized one: {}",
        doc.markdown
    );
    assert_eq!(doc.notes, 0);
    // Without the oversized note all three small ones are shown (the budget is not the cause).
    let d = Dx::new(&para(&format!(r#"<w:r><w:t>body</w:t></w:r>{refs}"#)))
        .footnotes(&(note(1, "small one") + &note(2, "small two") + &note(3, "small three")));
    let doc = dconv_with(&d, &opts).unwrap();
    assert!(!doc.truncated);
    assert_eq!(doc.notes, 3, "{}", doc.markdown);
}

// ---------------------------------------------------------------------------------------------
// OpenDocument: a deletion range closed inside a hidden shape
// ---------------------------------------------------------------------------------------------

/// A deletion range may end inside a drawing shape that is itself inside the range (the shape is
/// not shown, but its `change-end` still closes the range): the paragraph after it must show
/// (catches the hidden shape not being walked, which left everything after it hidden).
#[test]
fn a_deletion_ended_inside_a_hidden_shape_does_not_hide_the_rest() {
    let regions = r#"<text:tracked-changes><text:changed-region xml:id="d1" text:id="d1"><text:deletion><text:p>OLD</text:p></text:deletion></text:changed-region></text:tracked-changes>"#;
    let body = format!(
        r#"{regions}{}<text:change-start text:change-id="d1"/><draw:custom-shape><text:p>in shape<text:change-end text:change-id="d1"/></text:p></draw:custom-shape>{}"#,
        op("before"),
        op("after the shape")
    );
    let m = omd(&body);
    assert!(m.contains("before"), "{m}");
    assert!(!m.contains("in shape"), "{m}");
    assert!(m.contains("after the shape"), "{m}");
}

/// The same for an inline frame (a text box in a paragraph): its `change-end` still closes the
/// range, so the next paragraph shows (catches the hidden frame not being walked).
#[test]
fn a_deletion_ended_inside_a_hidden_frame_does_not_hide_the_rest() {
    let regions = r#"<text:tracked-changes><text:changed-region xml:id="d1" text:id="d1"><text:deletion><text:p>OLD</text:p></text:deletion></text:changed-region></text:tracked-changes>"#;
    let body = format!(
        r#"{regions}<text:p>before<text:change-start text:change-id="d1"/>gone <draw:frame><draw:text-box><text:p>in frame<text:change-end text:change-id="d1"/></text:p></draw:text-box></draw:frame>then shown</text:p>{}"#,
        op("after the frame")
    );
    let m = omd(&body);
    assert!(m.contains("before"), "{m}");
    assert!(!m.contains("gone") && !m.contains("in frame"), "{m}");
    assert!(m.contains("then shown"), "{m}");
    assert!(m.contains("after the frame"), "{m}");
}

// ---------------------------------------------------------------------------------------------
// OMML: the drawable limit
// ---------------------------------------------------------------------------------------------

/// A formula of exactly `MAX_DRAWABLE_BYTES` is still tried and one byte more is not (catches
/// `>` becoming `>=`).
#[test]
fn drawable_accepts_exactly_the_limit_and_refuses_one_byte_more() {
    assert!(drawable(&"x".repeat(MAX_DRAWABLE_BYTES)));
    assert!(!drawable(&"x".repeat(MAX_DRAWABLE_BYTES + 1)));
}

// ---------------------------------------------------------------------------------------------
// MathML: limits
// ---------------------------------------------------------------------------------------------

/// `xml` of exactly `len` bytes: `body` in a `math` element, padded with white space between the
/// elements (which a formula ignores).
fn padded(body: &str, len: usize) -> String {
    let head = format!("<math {NS}>");
    let tail = "</math>";
    let fill = len - head.len() - body.len() - tail.len();
    format!("{head}{body}{}{tail}", " ".repeat(fill))
}

/// The input limit is `> 1 MiB`: exactly 1 MiB is read, one byte more is not (catches `>`
/// becoming `>=` in `fallback_text`).
#[test]
fn fallback_text_reads_exactly_the_input_limit_and_not_one_byte_more() {
    let limit = 1usize << 20;
    let at = padded("<mi>x</mi>", limit);
    assert_eq!(at.len(), limit);
    assert_eq!(fallback_text(&at), "x");
    assert!(is_math(&at));
    assert!(to_latex(&at, false).is_some());
    let over = padded("<mi>x</mi>", limit + 1);
    assert_eq!(fallback_text(&over), "");
    assert!(!is_math(&over));
    assert!(to_latex(&over, false).is_none());
}

/// An element nested 256 deep (`MAX_XML_DEPTH`; `math` is the first level) parses, one at 257 does
/// not (catches `>` becoming `>=` in the depth check of `parse`).
#[test]
fn mathml_nesting_of_exactly_the_depth_limit_parses_and_one_more_does_not() {
    // `depth` is the level of the innermost element (an `mi`).
    let nested = |depth: usize| {
        let mut s = format!("<math {NS}>");
        for _ in 2..depth {
            s.push_str("<mrow>");
        }
        s.push_str("<mi>x</mi>");
        for _ in 2..depth {
            s.push_str("</mrow>");
        }
        s.push_str("</math>");
        s
    };
    assert!(is_math(&nested(256)));
    assert!(!is_math(&nested(257)));
}

/// A formula of exactly `MAX_NODES` (100,000) elements parses, one more does not (catches `>`
/// becoming `>=` in the empty-element count).
#[test]
fn mathml_with_exactly_the_node_limit_of_empty_elements_parses() {
    let many = |n: usize| format!("<math {NS}>{}</math>", "<mi/>".repeat(n));
    // `math` itself is one element.
    assert!(is_math(&many(99_999)), "100,000 elements in all");
    assert!(!is_math(&many(100_000)), "100,001 elements in all");
}

/// A token keeps its text up to the first chunk that reaches 8 KiB: a piece arriving when it
/// holds exactly 8 KiB is dropped, one arriving at 8 KiB - 1 is kept (catches `<` becoming
/// `<=`). The text is read back through `fallback_text`.
#[test]
fn a_token_stops_collecting_text_at_exactly_the_limit() {
    let at = wrap(&format!("<mi>{}&amp;</mi>", "a".repeat(8 * 1024)));
    let t = fallback_text(&at);
    assert_eq!(t.len(), 8 * 1024, "the piece after the limit is dropped");
    assert!(!t.contains('&'));
    let below = wrap(&format!("<mi>{}&amp;</mi>", "a".repeat(8 * 1024 - 1)));
    let t = fallback_text(&below);
    assert_eq!(t.len(), 8 * 1024, "the piece at one byte below is kept");
    assert!(t.ends_with('&'));
}

/// Children past the 4,000th of an `mfenced` are not converted (catches the `take` being
/// removed). The 4,001st child is the only one that prints anything.
#[test]
fn an_mfenced_converts_at_most_4000_children() {
    let kids = |n: usize| format!("{}<mi>Z</mi>", "<mrow/>".repeat(n));
    let at = wrap(&format!(
        r#"<mfenced separators="">{}</mfenced>"#,
        kids(3999)
    ));
    assert!(
        to_latex(&at, false).is_some_and(|l| l.contains('Z')),
        "the 4,000th child is converted"
    );
    let over = wrap(&format!(
        r#"<mfenced separators="">{}</mfenced>"#,
        kids(4000)
    ));
    assert!(
        to_latex(&over, false).is_none_or(|l| !l.contains('Z')),
        "the 4,001st is not"
    );
}

/// Children past the 4,000th of an `mtable` are not read as rows (catches the `take` being
/// removed): a table whose only row stands after 4,000 other children is empty.
#[test]
fn an_mtable_reads_at_most_4000_children_as_rows() {
    let row = "<mtr><mtd><mi>Z</mi></mtd></mtr>";
    let at = wrap(&format!("<mtable>{}{row}</mtable>", "<mi/>".repeat(3999)));
    assert!(
        to_latex(&at, false).is_some_and(|l| l.contains('Z')),
        "the 4,000th child is read"
    );
    let over = wrap(&format!("<mtable>{}{row}</mtable>", "<mi/>".repeat(4000)));
    assert!(to_latex(&over, false).is_none(), "the 4,001st is not");
}

/// `mspace` of 1.5em is a `\qquad`, 1.49em a `\quad` (catches `>=` becoming `>`).
#[test]
fn mspace_of_one_and_a_half_em_is_a_qquad() {
    let l = |w: &str| {
        to_latex(
            &wrap(&format!(r#"<mi>a</mi><mspace width="{w}"/><mi>b</mi>"#)),
            false,
        )
        .unwrap()
    };
    assert!(l("1.5em").contains(r"\qquad"), "{}", l("1.5em"));
    let below = l("1.49em");
    assert!(
        below.contains(r"\quad") && !below.contains(r"\qquad"),
        "{below}"
    );
}
