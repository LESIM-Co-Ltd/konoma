//! PowerPoint reader tests: limits, hostile and damaged input, speed.

use super::container::Limits;
use super::docx::pptx::*;
use super::docx::*;
use super::tests::{deflated, tmp, write, write_cfb};
use super::tests_docx::tiny_png;
use super::tests_pptx::*;
use super::*;

fn assert_no_panic(r: &Result<Document, OfficeError>) {
    if let Err(OfficeError::Corrupt(m)) = r {
        assert!(!m.starts_with("panic"), "a panic was caught: {m}");
    }
}

/// A presentation that touches every reader path (the fuzz base).
fn rich() -> Px {
    let s1 = [
        title("Title *x*"),
        body(
            (I, 2 * I, 4 * I, 3 * I),
            &(pa("one")
                + &pl(1, "two")
                + &pp("", r#"<a:buAutoNum type="alphaLcPeriod"/>"#, "three")),
        ),
        group(
            (5 * I, I, 3 * I, 3 * I),
            (0, 0, 100, 100),
            &[
                tb(0, 0, 50, 50, &pa("in group")),
                tb(50, 50, 50, 50, &pa("also")),
            ]
            .concat(),
        ),
        table(
            (0, 5 * I, 4 * I, I),
            &(tr(&["a|b", "c"]) + &tr(&["1", "2"])),
        ),
        pic("rIdImg", "figure", Some((5 * I, 5 * I, I, I))),
        chart_frame("rIdChart", (0, 6 * I, I, I)),
        smart_frame("rIdDm", (4 * I, 6 * I, I, I)),
        tb(0, 0, I, I, &math_para_for_fuzz()),
    ]
    .concat();
    let sld = sl(&s1)
        .notes(&notes_body(&(pa("note one") + &pa("# note two"))))
        .rel("rIdImg", "image", "../media/i.png", false)
        .rel("rIdChart", "chart", "../charts/chart1.xml", false)
        .rel("rIdDm", "diagramData", "../diagrams/data1.xml", false)
        .rel("rIdDr", "diagramDrawing", "../diagrams/drawing1.xml", false)
        .rel("rIdLink", "hyperlink", "https://e.example/", true);
    Px::new(vec![sld, sl(&title("Second")).hidden()])
        .media("i.png", tiny_png(7))
        .part("ppt/charts/chart1.xml", &chart_part(Some("Chart")))
        .part("ppt/diagrams/data1.xml", &dm(&["d1", "d2"], Some("rIdDr")))
        .part(
            "ppt/diagrams/drawing1.xml",
            &drawing(&[(0, 0, "dr1"), (200_000, 0, "dr2")]),
        )
}

fn math_para_for_fuzz() -> String {
    r#"<a:p><a:r><a:t>E = </a:t></a:r><mc:AlternateContent><mc:Choice Requires="a14"><a14:m><m:oMath><m:r><m:t>mc</m:t></m:r></m:oMath></a14:m></mc:Choice></mc:AlternateContent></a:p>"#.to_string()
}

#[test]
fn the_unmutated_rich_deck_reads_fully() {
    let d = doc(&rich());
    for want in [
        "## Slide 1: Title \\*x\\*",
        "- one",
        "a. three",
        "in group",
        "| a\\|b |",
        "![figure](office-img://",
        "\\[chart: Chart]",
        "- dr1",
        "> **Notes**",
        "> \\# note two",
        "## Slide 2: Second (hidden)",
    ] {
        assert!(d.markdown.contains(want), "{want:?} in\n{}", d.markdown);
    }
    assert!(!d.truncated);
    check_headings(&d);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(src: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut v = src.to_vec();
    for _ in 0..=rng.below(4) {
        if v.is_empty() {
            break;
        }
        match rng.below(7) {
            0 => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(v.len());
                let n = 1 + rng.below(30);
                v.drain(i..(i + n).min(v.len()));
            }
            2 => {
                let i = rng.below(v.len());
                let n = 1 + rng.below(60);
                let chunk: Vec<u8> = v[i..(i + n).min(v.len())].to_vec();
                let at = rng.below(v.len());
                for (k, b) in chunk.into_iter().enumerate() {
                    v.insert(at + k, b);
                }
            }
            3 => {
                let n = rng.below(v.len());
                v.truncate(n);
            }
            4 => {
                let tags: [&[u8]; 8] = [
                    b"<p:sp>",
                    b"</p:sp>",
                    b"<a:p>",
                    b"</a:p>",
                    b"<p:grpSp>",
                    b"</p:grpSp>",
                    b"<a:tc>",
                    b"<a:t>",
                ];
                let at = rng.below(v.len());
                let t = tags[rng.below(tags.len())];
                for (k, b) in t.iter().enumerate() {
                    v.insert(at + k, *b);
                }
            }
            5 => {
                let i = rng.below(v.len());
                v[i] = [b'<', b'>', b'&', b'"', b'/', 0, 0xFF][rng.below(7)];
            }
            _ => {
                let at = rng.below(v.len());
                for k in 0..rng.below(5000) {
                    v.insert(at + k, b'9');
                }
            }
        }
    }
    v
}

#[test]
fn mutated_presentations_never_panic_and_keep_the_headings_in_step() {
    let base = rich();
    let entries = base.entries();
    let xml: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| n.ends_with(".xml") || n.ends_with(".rels"))
        .map(|(i, _)| i)
        .collect();
    let dir = tmp("pptxfuzz");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut ok, mut err) = (0, 0);
    for _ in 0..400 {
        let target = xml[rng.below(xml.len())];
        let mut e = entries.clone();
        e[target].1 = mutate(&e[target].1, &mut rng);
        let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        let p = write(&dir, "f.pptx", &deflated(&refs));
        let r = load_presentation(&p, &DocOptions::default());
        assert_no_panic(&r);
        match r {
            Ok(d) => {
                ok += 1;
                check_headings(&d);
            }
            Err(_) => err += 1,
        }
    }
    assert!(
        ok > 150,
        "the mutations should mostly leave a readable deck: ok {ok} err {err}"
    );
}

#[test]
fn every_prefix_of_a_deck_is_handled() {
    let bytes = rich().bytes();
    let dir = tmp("pptxprefix");
    for n in (0..bytes.len()).step_by(bytes.len() / 40 + 1) {
        let p = write(&dir, "f.pptx", &bytes[..n]);
        assert_no_panic(&load_presentation(&p, &DocOptions::default()));
    }
}

// ---------------------------------------------------------------------------------------------
// what is not a presentation
// ---------------------------------------------------------------------------------------------

#[test]
fn files_that_are_not_presentations() {
    let dir = tmp("pptxnot");
    let opts = DocOptions::default();
    let p = write(&dir, "a.pptx", b"");
    assert!(matches!(
        load_presentation(&p, &opts),
        Err(OfficeError::Corrupt(_))
    ));
    let p = write(&dir, "b.pptx", b"just text, not a zip at all");
    assert!(matches!(
        load_presentation(&p, &opts),
        Err(OfficeError::Corrupt(_))
    ));
    let p = write(&dir, "c.pptx", &deflated(&[("hello.txt", b"hi")]));
    assert_eq!(
        load_presentation(&p, &opts).unwrap_err(),
        OfficeError::Unsupported
    );
    // A Word document and a workbook renamed .pptx.
    let docx = super::tests_docx::conv_bytes("<w:p><w:r><w:t>x</w:t></w:r></w:p>");
    let p = write(&dir, "d.pptx", &docx);
    assert_eq!(
        load_presentation(&p, &opts).unwrap_err(),
        OfficeError::Unsupported
    );
    let p = write(&dir, "e.pptx", &deflated(&[
        ("_rels/.rels", format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#, super::tests_docx::REL_BASE).as_bytes()),
        ("xl/workbook.xml", b"<workbook/>"),
    ]));
    assert_eq!(
        load_presentation(&p, &opts).unwrap_err(),
        OfficeError::Unsupported
    );
}

#[test]
fn an_encrypted_presentation_and_a_legacy_ppt() {
    let dir = tmp("pptxenc");
    let p = dir.join("enc.pptx");
    write_cfb(
        &p,
        &[
            ("/EncryptionInfo", b"\x04\x00\x04\x00info"),
            ("/EncryptedPackage", b"\x10\x00\x00\x00\x00\x00\x00\x00data"),
        ],
    );
    assert_eq!(
        load_presentation(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Encrypted
    );
    let p = dir.join("old.ppt");
    write_cfb(&p, &[("/PowerPoint Document", b"x")]);
    assert_eq!(
        load_presentation(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Unsupported
    );
}

#[test]
fn an_opendocument_presentation_is_read_and_other_opendocument_types_are_not() {
    let dir = tmp("pptxodp");
    let entries = |mime: &'static str| {
        deflated(&[
            ("mimetype", mime.as_bytes()),
            ("content.xml", b"<office:document-content/>"),
        ])
    };
    // (A presentation with nothing in it: no slides.)
    let p = write(
        &dir,
        "x.odp",
        &entries("application/vnd.oasis.opendocument.presentation"),
    );
    assert_eq!(
        load_presentation(&p, &DocOptions::default())
            .unwrap()
            .slides
            .len(),
        0
    );
    let p = write(
        &dir,
        "y.odt",
        &entries("application/vnd.oasis.opendocument.text"),
    );
    assert_eq!(
        load_presentation(&p, &DocOptions::default()).unwrap_err(),
        OfficeError::Unsupported
    );
}

// ---------------------------------------------------------------------------------------------
// damaged slides keep their place
// ---------------------------------------------------------------------------------------------

#[test]
fn a_slide_part_that_is_missing_or_unlisted_keeps_its_heading() {
    let px = Px::new(vec![sl(&title("a")), sl(&title("b")), sl(&title("c"))]);
    // Drop slide2.xml from the package.
    let e: Vec<(String, Vec<u8>)> = px
        .entries()
        .into_iter()
        .filter(|(n, _)| n != "ppt/slides/slide2.xml")
        .collect();
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxmiss");
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(d.markdown, "## Slide 1: a\n\n## Slide 2\n\n## Slide 3: c");
    assert!(d.truncated);
    check_headings(&d);
}

#[test]
fn a_slide_id_with_no_relationship_keeps_its_place() {
    let px = Px::new(vec![sl(&title("a")), sl(&title("b"))]);
    let mut e = px.entries();
    for (n, b) in &mut e {
        if n == "ppt/_rels/presentation.xml.rels" {
            let s = String::from_utf8(b.clone()).unwrap();
            // Remove the relationship of the first slide.
            let start = s.find("<Relationship Id=\"rIdS0\"").unwrap();
            let end = s[start..].find("/>").unwrap() + start + 2;
            *b = format!("{}{}", &s[..start], &s[end..]).into_bytes();
        }
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxnorel");
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(d.markdown, "## Slide 1\n\n## Slide 2: b");
    assert!(d.truncated);
}

#[test]
fn a_broken_slide_xml_keeps_its_heading_and_the_others_are_read() {
    let px = Px::new(vec![sl(&title("a")), sl(&title("b"))]);
    let mut e = px.entries();
    for (n, b) in &mut e {
        if n == "ppt/slides/slide1.xml" {
            *b = b"<p:sld><p:cSld><p:spTree><p:sp>unclosed".to_vec();
        }
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxbroken");
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(d.markdown, "## Slide 1\n\n## Slide 2: b");
    assert!(d.truncated);
    check_headings(&d);
}

#[test]
fn a_damaged_layout_or_master_costs_only_the_inheritance() {
    let px = Px::new(vec![sl(
        &[title("T"), body((0, 0, I, I), &pa("x"))].concat()
    )]);
    let mut e = px.entries();
    for (n, b) in &mut e {
        if n.contains("slideLayout1.xml") && !n.contains("rels")
            || n.contains("slideMaster1.xml") && !n.contains("rels")
        {
            *b = b"not xml <<<".to_vec();
        }
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxlay");
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    // The text is there; the master's bullets are not known.
    assert!(
        d.markdown.starts_with("## Slide 1: T\n\n"),
        "{}",
        d.markdown
    );
    assert!(d.markdown.contains('x'));
}

// ---------------------------------------------------------------------------------------------
// limits
// ---------------------------------------------------------------------------------------------

fn many_slides(n: usize) -> Px {
    Px::new(
        (0..n)
            .map(|i| sl(&[title(&format!("S{i}")), tb(0, I, I, I, &pa("text"))].concat()))
            .collect(),
    )
}

#[test]
fn the_slide_count_is_capped() {
    let opts = DocOptions {
        max_slides: 3,
        ..DocOptions::default()
    };
    let d = load_px(&many_slides(10), &opts).unwrap();
    assert_eq!(d.slides.len(), 3);
    assert!(d.truncated);
    check_headings(&d);
    // Exactly at the cap is not truncation.
    let d = load_px(&many_slides(3), &opts).unwrap();
    assert_eq!(d.slides.len(), 3);
    assert!(!d.truncated);
}

#[test]
fn a_thousand_slides_is_the_default_cap() {
    assert_eq!(DocOptions::default().max_slides, 1_000);
}

#[test]
fn shapes_per_slide_are_capped() {
    let s: String = (0..50)
        .map(|i| tb(0, i * 1000, I, 500, &pa(&format!("t{i}"))))
        .collect();
    let opts = DocOptions {
        max_slide_shapes: 10,
        ..DocOptions::default()
    };
    let d = load_px(&Px::new(vec![sl(&s), sl(&title("next"))]), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.contains("t0") && d.markdown.contains("t9"));
    assert!(!d.markdown.contains("t10"), "{}", d.markdown);
    check_headings(&d);
    assert_eq!(d.slides.len(), 2);
}

#[test]
fn shapes_inside_groups_count_towards_the_cap() {
    let inner: String = (0..50)
        .map(|i| tb(0, i, 1, 1, &pa(&format!("g{i}"))))
        .collect();
    let s = group((0, 0, I, I), (0, 0, 100, 100), &inner);
    let opts = DocOptions {
        max_slide_shapes: 10,
        ..DocOptions::default()
    };
    let d = load_px(&Px::new(vec![sl(&s)]), &opts).unwrap();
    assert!(d.truncated);
    assert!(!d.markdown.contains("g20"), "{}", d.markdown);
}

#[test]
fn the_total_xml_read_is_budgeted_and_every_slide_keeps_its_heading() {
    let px = many_slides(40);
    let one = px
        .entries()
        .iter()
        .find(|(n, _)| n == "ppt/slides/slide1.xml")
        .unwrap()
        .1
        .len() as u64;
    let opts = DocOptions {
        // Master, layout and about eight slides.
        max_pptx_read_total: one * 12,
        ..DocOptions::default()
    };
    let d = load_px(&px, &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 40, "{}", d.markdown);
    check_headings(&d);
    assert!(d.markdown.contains("## Slide 3: S2"), "{}", d.markdown);
    assert!(d.markdown.contains("## Slide 40\n") || d.markdown.ends_with("## Slide 40"));
}

#[test]
fn a_part_over_the_per_part_cap_is_skipped_not_read() {
    let big = tb(0, 0, I, I, &pa(&"w".repeat(20_000)));
    let px = Px::new(vec![sl(&big), sl(&title("small"))]);
    let opts = DocOptions {
        max_slide_part_bytes: 5_000,
        ..DocOptions::default()
    };
    let d = load_px(&px, &opts).unwrap();
    assert!(d.truncated);
    assert!(!d.markdown.contains("wwww"));
    assert!(d.markdown.contains("## Slide 2: small"));
    check_headings(&d);
}

#[test]
fn the_markdown_budget_cuts_the_deck_at_a_heading_boundary_of_what_fits() {
    let opts = DocOptions {
        max_markdown_bytes: 4_000,
        ..DocOptions::default()
    };
    let slides: Vec<Sl> = (0..200)
        .map(|i| {
            sl(&[
                title(&format!("S{i}")),
                tb(0, I, I, I, &pa(&"word ".repeat(30))),
            ]
            .concat())
        })
        .collect();
    let d = load_px(&Px::new(slides), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.len() <= 4_000);
    assert!(d.slides.len() < 200 && !d.slides.is_empty());
    check_headings(&d);
}

#[test]
fn the_line_budget_cuts_the_deck_and_the_headings_stay_in_step() {
    let opts = DocOptions {
        max_markdown_lines: 50,
        ..DocOptions::default()
    };
    let d = load_px(&many_slides(100), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 50);
    check_headings(&d);
}

#[test]
fn one_enormous_paragraph_is_cut() {
    let text = "x".repeat(3_000_000);
    let d = doc(&Px::new(vec![sl(&tb(0, 0, I, I, &pa(&text)))]));
    assert!(d.truncated || d.markdown.len() < 1_000_000);
    assert!(d.markdown.len() <= 1_000_000);
    check_headings(&d);
}

#[test]
fn a_slide_with_a_million_nodes_is_cut_by_the_node_budget() {
    let s = tb(0, 0, I, I, &"<a:p/>".repeat(300_000));
    let d = doc(&Px::new(vec![sl(&s), sl(&title("after"))]));
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 2);
    assert!(d.markdown.contains("## Slide 2: after"));
}

#[test]
fn a_package_over_the_container_limits_is_refused() {
    let opts = DocOptions {
        limits: Limits {
            max_total_bytes: 100,
            ..Limits::default()
        },
        ..DocOptions::default()
    };
    assert!(matches!(
        load_px(&many_slides(3), &opts),
        Err(OfficeError::TooLarge { .. })
    ));
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    // Groups in groups: past the XML depth limit the slide is refused whole, below it the
    // group depth cap stops the descent.
    for n in [10usize, 25, 60, 150, 400] {
        let mut s = tb(0, 0, I, I, &pa("core"));
        for _ in 0..n {
            s = group((0, 0, I, I), (0, 0, I, I), &s);
        }
        let r = load_px(&Px::new(vec![sl(&s)]), &DocOptions::default());
        assert_no_panic(&r);
        if let Ok(d) = r {
            check_headings(&d);
        }
    }
    // Alternate content in alternate content.
    let mut s = tb(0, 0, I, I, &pa("core"));
    for _ in 0..200 {
        s = format!("<mc:AlternateContent><mc:Choice>{s}</mc:Choice></mc:AlternateContent>");
    }
    assert_no_panic(&load_px(&Px::new(vec![sl(&s)]), &DocOptions::default()));
    // Fields in fields.
    let mut inner = String::from("<a:r><a:t>x</a:t></a:r>");
    for _ in 0..200 {
        inner = format!("<mc:AlternateContent><mc:Choice><a14:m>{inner}</a14:m></mc:Choice></mc:AlternateContent>");
    }
    let s = tb(0, 0, I, I, &paras(&inner));
    assert_no_panic(&load_px(&Px::new(vec![sl(&s)]), &DocOptions::default()));
}

#[test]
fn group_scales_that_are_zero_infinite_or_huge_are_harmless() {
    for (off, ch) in [
        ((0, 0, 0, 0), (0, 0, 0, 0)),
        ((0, 0, I, I), (0, 0, 0, 0)),
        ((0, 0, i64::MAX, i64::MAX), (0, 0, 1, 1)),
        (
            (i64::MIN, i64::MIN, i64::MAX, i64::MAX),
            (i64::MAX, i64::MAX, 1, 1),
        ),
        ((0, 0, -I, -I), (0, 0, I, I)),
        ((0, 0, I, I), (0, 0, -5, -5)),
    ] {
        let s = group(
            off,
            ch,
            &[tb(5, 5, 5, 5, &pa("in")), tb(0, 3 * I, 1, 1, &pa("out"))].concat(),
        );
        let d = doc(&Px::new(vec![sl(&s)]));
        assert!(d.markdown.contains("in"), "{off:?} {ch:?}: {}", d.markdown);
        check_headings(&d);
    }
}

#[test]
fn extreme_coordinates_and_non_numbers_are_harmless() {
    let weird = r#"<a:xfrm rot="99999999999999999999"><a:off x="9999999999999999999999" y="-9999999999999999999"/><a:ext cx="abc" cy=""/></a:xfrm>"#;
    let s = [
        sp_raw(1, "", weird, &pa("weird one")),
        sp_raw(
            2,
            "",
            &xf(i64::MAX, i64::MIN, i64::MAX, i64::MAX),
            &pa("extreme"),
        ),
        tb(0, 0, I, I, &pa("normal")),
    ]
    .concat();
    let d = doc(&Px::new(vec![sl(&s)]));
    for t in ["weird one", "extreme", "normal"] {
        assert!(d.markdown.contains(t), "{t}: {}", d.markdown);
    }
}

// ---------------------------------------------------------------------------------------------
// hostile content
// ---------------------------------------------------------------------------------------------

#[test]
fn private_use_markers_and_control_characters_cannot_forge_tokens() {
    let tx = pa("a&#xE000;0&#xE001;b&#x1;c&#x7F;d&#x2028;e");
    let d = doc(&Px::new(vec![sl(&tb(0, 0, I, I, &tx))]));
    assert!(!d.markdown.contains('\u{E000}') && !d.markdown.contains('\u{E001}'));
    assert!(!d.markdown.chars().any(|c| (c as u32) < 0x20 && c != '\n'));
}

#[test]
fn relationship_targets_cannot_leave_the_package() {
    let s = pic("rId1", "x", Some((0, 0, I, I)));
    let sld = sl(&s).rel("rId1", "image", "../../../../../../etc/passwd", false);
    let d = doc(&Px::new(vec![sld]));
    assert!(d.images.is_empty());
}

#[test]
fn a_layout_relationship_to_a_slide_or_to_itself_is_harmless() {
    let px = Px::new(vec![sl(&title("a"))]);
    let mut e = px.entries();
    for (n, b) in &mut e {
        if n == "ppt/slides/_rels/slide1.xml.rels" {
            *b = format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="{}/slideLayout" Target="slide1.xml"/></Relationships>"#,
                super::tests_docx::REL_BASE
            )
            .into_bytes();
        }
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxloop");
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(d.markdown, "## Slide 1: a");
}

#[test]
fn the_same_slide_listed_twice_is_two_slides() {
    let px = Px::new(vec![sl(&title("a"))]);
    let mut e = px.entries();
    for (n, b) in &mut e {
        if n == "ppt/presentation.xml" {
            let s = String::from_utf8(b.clone()).unwrap();
            *b = s
                .replace(
                    r#"<p:sldId id="256" r:id="rIdS0"/>"#,
                    r#"<p:sldId id="256" r:id="rIdS0"/><p:sldId id="257" r:id="rIdS0"/>"#,
                )
                .into_bytes();
        }
    }
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("pptxdup");
    let p = write(&dir, "t.pptx", &deflated(&refs));
    let d = load_presentation(&p, &DocOptions::default()).unwrap();
    assert_eq!(d.markdown, "## Slide 1: a\n\n## Slide 2: a");
    check_headings(&d);
}

#[test]
fn a_chart_drawn_many_times_is_read_once_within_the_budget() {
    let s: String = (0..60)
        .map(|i| chart_frame("rIdC", (0, i * 1000, 10, 10)))
        .collect();
    let px = Px::new(vec![sl(&s).rel(
        "rIdC",
        "chart",
        "../charts/chart1.xml",
        false,
    )])
    .part("ppt/charts/chart1.xml", &chart_part(Some("Once")));
    let d = doc(&px);
    assert_eq!(d.markdown.matches("\\[chart: Once]").count(), 60);
}

#[test]
fn many_big_charts_stop_being_read_at_the_chart_budget() {
    // 20 different 400 KB charts: titles are read only while the 4 MiB budget lasts.
    let mut px = Px::new(vec![]);
    let mut s = String::new();
    let mut sld = sl("");
    for i in 0..20 {
        s += &chart_frame(&format!("rIdC{i}"), (0, i * 1000, 10, 10));
        sld = sld.rel(
            &format!("rIdC{i}"),
            "chart",
            &format!("../charts/c{i}.xml"),
            false,
        );
        let mut c = chart_part(Some(&format!("T{i}")));
        c.extend(std::iter::repeat_n(b' ', 400_000));
        px = px.part(&format!("ppt/charts/c{i}.xml"), &c);
    }
    sld.shapes = s;
    px.slides = vec![sld];
    let d = doc(&px);
    assert_eq!(d.markdown.matches("\\[chart").count(), 20);
    assert!(d.markdown.contains("\\[chart: T0]"));
    assert!(
        d.markdown.contains("\\[chart]"),
        "the budget should run out: {}",
        d.markdown
    );
}

#[test]
fn a_cancelled_load_ends_early_without_an_error() {
    let c = Cancel::new(|| true);
    let dir = tmp("pptxcancel");
    let p = write(&dir, "t.pptx", &many_slides(30).bytes());
    let d = load_presentation_cancellable(&p, &DocOptions::default(), Some(&c)).unwrap();
    assert!(d.truncated);
    assert!(d.slides.len() < 30);
    check_headings(&d);
}

#[test]
fn cancelling_midway_stops_the_conversion() {
    let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n2 = n.clone();
    let c = Cancel::new(move || n2.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 40);
    let dir = tmp("pptxcancel2");
    let p = write(&dir, "t.pptx", &many_slides(200).bytes());
    let d = load_presentation_cancellable(&p, &DocOptions::default(), Some(&c)).unwrap();
    assert!(d.truncated);
    assert!(
        !d.slides.is_empty() && d.slides.len() < 200,
        "{}",
        d.slides.len()
    );
    check_headings(&d);
}

#[test]
fn a_big_deck_reads_quickly() {
    let t = std::time::Instant::now();
    let d = doc(&many_slides(900));
    assert_eq!(d.slides.len(), 900);
    // Debug build; the point is that it is linear, not quadratic.
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
    check_headings(&d);
}

#[test]
fn a_slide_with_thousands_of_shapes_reads_quickly() {
    let s: String = (0..3000)
        .map(|i| {
            tb(
                (i % 40) * 1000,
                (i / 40) * 100_000,
                900,
                90_000,
                &pa(&format!("c{i}")),
            )
        })
        .collect();
    let t = std::time::Instant::now();
    let d = doc(&Px::new(vec![sl(&s)]));
    // (3,000 paragraphs are 6,000 lines: the line budget cuts the end.)
    assert!(d.markdown.contains("c10") && d.truncated);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}
