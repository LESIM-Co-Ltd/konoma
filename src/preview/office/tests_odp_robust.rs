//! OpenDocument presentation reader tests: limits, hostile and damaged input, speed.

use super::docx::odt::odp::MAX_MASTER_NODES;
use super::docx::pptx::*;
use super::docx::*;
use super::tests::{deflated, tmp, write};
use super::tests_docx::tiny_png;
use super::tests_odp::*;
use super::tests_pptx::check_headings;
use super::*;

fn assert_no_panic(r: &Result<Document, OfficeError>) {
    if let Err(OfficeError::Corrupt(m)) = r {
        assert!(!m.starts_with("panic"), "a panic was caught: {m}");
    }
}

fn with_opts(f: impl FnOnce(&mut DocOptions)) -> DocOptions {
    let mut o = DocOptions::default();
    f(&mut o);
    o
}

fn many_slides(n: usize) -> Op {
    let pages: String = (0..n)
        .map(|i| page(&(title(&format!("S{i}")) + &tf(1.0, 3.0, 5.0, 1.0, "text"))))
        .collect();
    Op::new(&pages)
}

/// A presentation that touches every reader path (the fuzz base).
fn rich() -> Op {
    let table = format!(
        r#"<table:table><table:table-row><table:table-cell table:number-columns-spanned="2">{}</table:table-cell><table:covered-table-cell/></table:table-row></table:table>"#,
        tp("a|b")
    );
    let s1 = [
        title("Title *x*"),
        fr("outline", 1.0, 3.0, 8.0, 5.0, &tbx(&format!(
            r#"<text:list text:style-name="L1"><text:list-item>{}<text:list><text:list-item>{}</text:list-item></text:list></text:list-item></text:list>"#,
            tp("one"),
            tp("two")
        ))),
        format!(
            "<draw:g>{}{}</draw:g>",
            tf(12.0, 3.0, 4.0, 1.0, "in group"),
            tf(12.0, 5.0, 4.0, 1.0, "also")
        ),
        fr("", 1.0, 10.0, 8.0, 3.0, &table),
        fr("", 12.0, 10.0, 4.0, 3.0, r#"<draw:image xlink:href="Pictures/i.png"/><svg:desc>figure</svg:desc>"#),
        fr("", 1.0, 14.0, 4.0, 3.0, r#"<draw:object xlink:href="./Object 1"/>"#),
        fr("", 6.0, 14.0, 4.0, 3.0, r#"<draw:object xlink:href="./Object 2"/>"#),
        notes(&notes_frame("note one")),
    ]
    .concat();
    let hidden_page = page_with("dpH", "Default", &title("Second"));
    Op::new(&(page_with("dp1", "Default", &s1) + &hidden_page))
        .auto(&format!("{L1}{HIDDEN}"))
        .master(&format!(
            r#"<style:master-page style:name="Default">{}</style:master-page>"#,
            fr("outline", 1.0, 3.0, 8.0, 5.0, &tbx(""))
        ))
        .picture("i.png", 4)
        .object("Object 1", &chart_xml_min())
        .object("Object 2", MATH)
}

const MATH: &str = r#"<math xmlns="http://www.w3.org/1998/Math/MathML"><mrow><mi>x</mi><mo>=</mo><mn>1</mn></mrow></math>"#;

fn chart_xml_min() -> String {
    format!(
        r#"<office:document-content {}><office:body><office:chart><chart:chart><chart:title><text:p>Chart</text:p></chart:title></chart:chart></office:chart></office:body></office:document-content>"#,
        pns()
    )
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
                    b"<draw:frame>",
                    b"</draw:frame>",
                    b"<text:p>",
                    b"</text:p>",
                    b"<draw:g>",
                    b"</draw:g>",
                    b"<table:table-cell>",
                    b"<draw:page>",
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
fn the_unmutated_rich_deck_reads_fully() {
    let d = doc(&rich());
    assert!(!d.truncated);
    assert_eq!(d.slides.len(), 2);
    assert!(d.slides[1].hidden);
    for want in [
        "## Slide 1: Title \\*x\\*",
        "- one\n  - two",
        "in group",
        "| a\\|b |",
        "![figure](office-img://",
        "\\[chart: Chart]",
        "$$\nx=1\n$$",
        "> **Notes**  \n> note one",
        "## Slide 2: Second (hidden)",
    ] {
        assert!(d.markdown.contains(want), "{want:?} in\n{}", d.markdown);
    }
    check_headings(&Document {
        markdown: d.markdown.replace(" (hidden)", ""),
        ..d.clone()
    });
}

#[test]
fn mutated_presentations_never_panic_and_keep_the_headings_in_step() {
    let base = rich();
    let entries = base.entries();
    let xml: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, (n, _))| n.ends_with(".xml"))
        .map(|(i, _)| i)
        .collect();
    let dir = tmp("odpfuzz");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut ok, mut err) = (0, 0);
    for _ in 0..400 {
        let target = xml[rng.below(xml.len())];
        let mut e = entries.clone();
        e[target].1 = mutate(&e[target].1, &mut rng);
        let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        let p = write(&dir, "f.odp", &deflated(&refs));
        let r = load_presentation(&p, &DocOptions::default());
        assert_no_panic(&r);
        match r {
            Ok(d) => {
                ok += 1;
                let h2 = d.markdown.lines().filter(|l| l.starts_with("## ")).count();
                assert_eq!(h2, d.slides.len(), "{}", d.markdown);
            }
            Err(_) => err += 1,
        }
    }
    assert!(
        ok > 100,
        "the mutations should often leave a readable deck: ok {ok} err {err}"
    );
}

#[test]
fn every_prefix_of_a_deck_is_handled() {
    for bytes in [
        rich().bytes(),
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/office/slides.odp"
        ))
        .unwrap_or_default(),
    ] {
        let dir = tmp("odpprefix");
        for n in (0..bytes.len()).step_by(bytes.len() / 50 + 1) {
            let p = write(&dir, "f.odp", &bytes[..n]);
            assert_no_panic(&load_presentation(&p, &DocOptions::default()));
        }
    }
}

#[test]
fn a_content_xml_that_is_not_xml_is_corrupt_not_a_panic() {
    let e = [
        ("mimetype".to_string(), MIME_PRES.as_bytes().to_vec()),
        ("content.xml".to_string(), b"\x00\x01 not xml <<<".to_vec()),
    ];
    let refs: Vec<(&str, &[u8])> = e.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let dir = tmp("odpgarbage");
    let p = write(&dir, "x.odp", &deflated(&refs));
    let r = load_presentation(&p, &DocOptions::default());
    assert_no_panic(&r);
    assert!(r.is_err() || r.unwrap().slides.is_empty());
}

#[test]
fn a_damaged_automatic_styles_part_is_corrupt_but_a_damaged_styles_part_is_not() {
    let o = Op::new(&page(&title("T")));
    let r = load_op(&o.clone().styles("<<<"), &DocOptions::default()).unwrap();
    assert_eq!(r.markdown, "## Slide 1: T");
    let r = load_op(&o.master("<<<"), &DocOptions::default()).unwrap();
    assert_eq!(r.markdown, "## Slide 1: T");
}

// ---------------------------------------------------------------------------------------------
// limits
// ---------------------------------------------------------------------------------------------

#[test]
fn the_slide_count_is_capped() {
    let opts = with_opts(|o| o.max_slides = 3);
    let d = load_op(&many_slides(10), &opts).unwrap();
    assert_eq!(d.slides.len(), 3);
    assert!(d.truncated);
    check_headings(&d);
    // Exactly at the cap is not truncation.
    let d = load_op(&many_slides(3), &opts).unwrap();
    assert_eq!(d.slides.len(), 3);
    assert!(!d.truncated);
}

#[test]
fn shapes_per_slide_are_capped_and_the_next_slide_is_read() {
    let s: String = (0..50)
        .map(|i| tf(1.0, 1.0 + i as f64, 5.0, 0.5, &format!("t{i}")))
        .collect();
    let opts = with_opts(|o| o.max_slide_shapes = 10);
    let d = load_op(&Op::new(&(page(&s) + &page(&title("next")))), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.contains("t0") && d.markdown.contains("t9"));
    assert!(!d.markdown.contains("t10"), "{}", d.markdown);
    assert!(d.markdown.contains("## Slide 2: next"));
    check_headings(&d);
}

#[test]
fn shapes_inside_groups_count_towards_the_cap() {
    let inner: String = (0..50)
        .map(|i| tf(1.0, 1.0 + i as f64, 5.0, 0.5, &format!("g{i}")))
        .collect();
    let opts = with_opts(|o| o.max_slide_shapes = 10);
    let d = load_op(&Op::new(&page(&format!("<draw:g>{inner}</draw:g>"))), &opts).unwrap();
    assert!(d.truncated);
    assert!(!d.markdown.contains("g20"), "{}", d.markdown);
}

#[test]
fn a_slide_over_the_node_budget_keeps_its_heading_and_the_others_are_read() {
    let huge =
        tf(1.0, 1.0, 5.0, 1.0, "").replace("<text:p></text:p>", &"<text:p/>".repeat(300_000));
    let d = doc(&Op::new(&(page(&huge) + &page(&title("after")))));
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 2);
    assert!(d.markdown.contains("## Slide 1\n"), "{}", d.markdown);
    assert!(d.markdown.contains("## Slide 2: after"));
    check_headings(&d);
}

#[test]
fn a_slide_over_the_text_budget_keeps_its_heading() {
    let opts = with_opts(|o| o.max_slide_part_bytes = 5_000);
    let big = tf(1.0, 1.0, 5.0, 1.0, &"w".repeat(20_000));
    let d = load_op(&Op::new(&(page(&big) + &page(&title("small")))), &opts).unwrap();
    assert!(d.truncated);
    assert!(!d.markdown.contains("wwww"));
    assert!(d.markdown.contains("## Slide 2: small"));
    check_headings(&d);
}

#[test]
fn the_markdown_budget_cuts_the_deck_and_the_headings_stay_in_step() {
    let opts = with_opts(|o| o.max_markdown_bytes = 4_000);
    let pages: String = (0..200)
        .map(|i| page(&(title(&format!("S{i}")) + &tf(1.0, 3.0, 5.0, 1.0, &"word ".repeat(30)))))
        .collect();
    let d = load_op(&Op::new(&pages), &opts).unwrap();
    assert!(d.truncated);
    // The text is cut where the body budget ends; the headings of the later slides go on in the
    // room kept for them, so every slide keeps its place and its picture.
    assert!(d.markdown.len() <= 4_000 + opts.max_slides * 1_024);
    assert_eq!(d.slides.len(), 200);
    assert_eq!(d.slide_scenes.len(), 200);
    // (the last slide's text is gone: its section is the heading alone)
    assert!(!d
        .markdown
        .rsplit("## Slide 200")
        .next()
        .unwrap()
        .contains("word"));
    assert!(d.markdown.contains("## Slide 200: S199"));
    check_headings(&d);
}

#[test]
fn the_line_budget_cuts_the_deck() {
    let opts = with_opts(|o| o.max_markdown_lines = 50);
    let d = load_op(&many_slides(100), &opts).unwrap();
    assert!(d.truncated);
    assert!(d.markdown.lines().count() <= 50 + 2 * opts.max_slides);
    assert_eq!(d.slides.len(), 100);
    assert_eq!(d.slide_scenes.len(), 100);
    check_headings(&d);
}

#[test]
fn a_deck_whose_text_fills_the_budget_keeps_every_slide_in_the_picture_view() {
    let body: String = (0..40)
        .map(|k| format!("<text:p>line {k} of the body</text:p>"))
        .collect();
    let pages: String = (0..300)
        .map(|i| page(&(title(&format!("S{i}")) + &tbx_frame(&body))))
        .collect();
    let d = load_op(&Op::new(&pages), &with_opts(|_| {})).unwrap();
    assert!(d.truncated);
    assert_eq!(d.slides.len(), 300);
    assert_eq!(d.slide_scenes.len(), 300);
    assert!(d.markdown.contains("## Slide 300: S299"));
    assert!(d.markdown.contains("line 39 of the body"));
    assert_eq!(d.picture_markdown.matches("](office-img://").count(), 300);
    check_headings(&d);
}

fn tbx_frame(inner: &str) -> String {
    format!(
        r#"<draw:frame svg:x="1cm" svg:y="3cm" svg:width="20cm" svg:height="5cm"><draw:text-box>{inner}</draw:text-box></draw:frame>"#
    )
}

#[test]
fn one_enormous_paragraph_is_cut() {
    let d = doc(&Op::new(&page(&tf(
        1.0,
        1.0,
        5.0,
        1.0,
        &"x".repeat(3_000_000),
    ))));
    assert!(d.markdown.len() <= 1_000_000);
    check_headings(&d);
}

#[test]
fn repeat_attributes_cannot_inflate_a_table_or_a_paragraph() {
    let rows = format!(
        r#"<table:table><table:table-row table:number-rows-repeated="1000000000"><table:table-cell table:number-columns-repeated="1000000000">{}</table:table-cell></table:table-row></table:table>"#,
        tp("x")
    );
    let t = std::time::Instant::now();
    let d = doc(&Op::new(&page(&fr("", 1.0, 1.0, 5.0, 5.0, &rows))));
    assert!(t.elapsed().as_secs() < 10, "{:?}", t.elapsed());
    assert!(d.markdown.len() <= 1_000_000);
    check_headings(&d);
    let spaces = tf(1.0, 1.0, 5.0, 1.0, r#"a<text:s text:c="999999999"/>b"#);
    let d = doc(&Op::new(&page(&spaces)));
    assert!(d.markdown.len() < 10_000, "{}", d.markdown.len());
    // Repeated columns over many slides together: the output budget is for the whole deck.
    let many: String = (0..50)
        .map(|_| page(&fr("", 1.0, 1.0, 5.0, 5.0, &rows)))
        .collect();
    let d = doc(&Op::new(&many));
    assert!(d.markdown.len() <= 1_000_000);
    assert!(d.truncated);
    check_headings(&d);
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    let mut g = tf(1.0, 1.0, 5.0, 1.0, "core");
    for _ in 0..200 {
        g = format!("<draw:g>{g}</draw:g>");
    }
    let d = doc(&Op::new(&page(&g)));
    check_headings(&d);
    let mut l = tp("deepest");
    for _ in 0..300 {
        l = format!("<text:list><text:list-item>{l}</text:list-item></text:list>");
    }
    let r = load_op(
        &Op::new(&page(&fr("", 1.0, 1.0, 5.0, 5.0, &tbx(&l)))),
        &DocOptions::default(),
    );
    assert_no_panic(&r);
    let mut s = "x".to_string();
    for _ in 0..3000 {
        s = format!("<text:span>{s}</text:span>");
    }
    let r = load_op(
        &Op::new(&page(&fr("", 1.0, 1.0, 5.0, 5.0, &tbx(&tp(&s))))),
        &DocOptions::default(),
    );
    assert_no_panic(&r);
    let mut f = tp("inner");
    for _ in 0..100 {
        f = format!("<draw:frame><draw:text-box>{f}</draw:text-box></draw:frame>");
    }
    let r = load_op(&Op::new(&page(&f)), &DocOptions::default());
    assert_no_panic(&r);
}

#[test]
fn extreme_and_broken_coordinates_are_harmless() {
    let weird = [
        ("1e300cm", "1e300cm", "1e300cm", "1e300cm"),
        ("NaNcm", "inf", "-infinity", "abc"),
        ("-99999999999cm", "99999999999cm", "0cm", "0cm"),
        ("1cm", "1cm", "-5cm", "-5cm"),
        ("", "", "", ""),
        ("1cm 2cm", "50%", "3em", "1,5cm"),
        ("0.0000000001cm", "-0cm", "0cm", "0cm"),
    ];
    let s: String = weird
        .iter()
        .map(|(x, y, w, h)| {
            format!(
                r#"<draw:frame svg:x="{x}" svg:y="{y}" svg:width="{w}" svg:height="{h}"><draw:text-box>{}</draw:text-box></draw:frame>"#,
                tp("w")
            )
        })
        .collect();
    let d = doc(&Op::new(&page(&s)));
    assert_eq!(
        d.markdown.matches('w').count(),
        weird.len(),
        "{}",
        d.markdown
    );
    check_headings(&d);
}

#[test]
fn broken_or_hostile_transforms_are_ignored() {
    let transforms = [
        "rotate (",
        "rotate ()",
        "rotate (abc)",
        "rotate (1e999)",
        "scale (0 0)",
        "scale (1e300 1e300) scale (1e300 1e300) scale (1e300 1e300)",
        "matrix (1 2 3)",
        "matrix (1 0 0 1 1e999cm 0cm)",
        "translate (1cm) bogus (1)",
        "skewX (1.5707963267949)",
        &"rotate (0.1) ".repeat(1000),
        ")(",
        "translate (1xx 2xx)",
        "",
    ];
    let s: String = transforms
        .iter()
        .map(|t| {
            format!(
                r#"<draw:custom-shape svg:width="2cm" svg:height="2cm" svg:x="1cm" svg:y="1cm" draw:transform="{t}"><text:p>t</text:p></draw:custom-shape>"#
            )
        })
        .collect();
    let d = doc(&Op::new(&page(&s)));
    assert_eq!(
        d.markdown.matches('t').count(),
        transforms.len(),
        "{}",
        d.markdown
    );
}

#[test]
fn many_master_pages_are_capped() {
    let masters: String = (0..3000)
        .map(|i| {
            format!(
                r#"<style:master-page style:name="M{i}">{}</style:master-page>"#,
                fr("outline", 1.0, 1.0, 1.0, 1.0, &tbx(""))
            )
        })
        .collect();
    let s = fr_nopos("outline", &tbx(&tp("x")));
    let d = doc(&Op::new(&page_with("dp1", "M10", &s)).master(&masters));
    assert!(
        d.truncated,
        "the pages past the cap are left out, and it says so"
    );
    assert_eq!(d.markdown, "## Slide 1\n\nx");
    let d = doc(&Op::new(&page_with("dp1", "M2999", &s)).master(&masters));
    assert_eq!(d.markdown, "## Slide 1\n\nx");
}

#[test]
fn many_chart_objects_stop_being_read_at_the_chart_budget() {
    // 2,000 frames, each pointing at its own 3 KiB object part: the 4 MiB budget ends the reading
    // (the rest fall back to what a Writer frame shows) and nothing is slow.
    let mut o = Op::new("");
    let mut s = String::new();
    for i in 0..2000 {
        let big = format!("{}<!--{}-->", chart_xml_min(), "p".repeat(3000));
        o = o.object(&format!("Object {i}"), &big);
        s.push_str(&fr(
            "",
            1.0,
            1.0 + i as f64 * 0.01,
            1.0,
            1.0,
            &format!(r#"<draw:object xlink:href="./Object {i}"/>"#),
        ));
    }
    o.pages = page(&s);
    let t = std::time::Instant::now();
    let d = doc(&o);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
    let charts = d.markdown.matches("chart: Chart").count();
    assert!(charts > 100 && charts < 2000, "{charts}");
    check_headings(&d);
}

#[test]
fn a_chart_drawn_in_every_frame_of_one_object_is_read_per_frame_within_the_budget() {
    let o = Op::new(&page(
        &(0..200)
            .map(|i| {
                fr(
                    "",
                    1.0,
                    1.0 + i as f64,
                    1.0,
                    0.5,
                    r#"<draw:object xlink:href="./Object 1"/>"#,
                )
            })
            .collect::<String>(),
    ))
    .object("Object 1", &chart_xml_min());
    let d = doc(&o);
    assert_eq!(d.markdown.matches("chart: Chart").count(), 200);
}

#[test]
fn formula_objects_are_capped() {
    let mut o = Op::new("");
    let mut s = String::new();
    for i in 0..30 {
        o = o.object(&format!("Object {i}"), MATH);
        s.push_str(&fr(
            "",
            1.0,
            1.0 + i as f64,
            1.0,
            0.5,
            &format!(r#"<draw:object xlink:href="./Object {i}"/>"#),
        ));
    }
    o.pages = page(&s);
    let opts = with_opts(|o| o.max_math_objects = 10);
    let d = load_op(&o, &opts).unwrap();
    assert!(d.truncated);
    assert!(d.math_total <= 10, "{}", d.math_total);
    check_headings(&d);
}

#[test]
fn pictures_are_capped() {
    let mut o = Op::new("");
    let mut s = String::new();
    for i in 0..20 {
        o = o.picture(&format!("p{i}.png"), i as u8);
        s.push_str(&fr(
            "",
            1.0,
            1.0 + i as f64,
            1.0,
            0.5,
            &format!(r#"<draw:image xlink:href="Pictures/p{i}.png"/>"#),
        ));
    }
    o.pages = page(&s);
    let opts = with_opts(|o| o.max_images = 5);
    let d = load_op(&o, &opts).unwrap();
    assert!(d.truncated);
    assert_eq!(d.images.len(), 5);
}

#[test]
fn notes_frames_are_capped() {
    let frames: String = (0..100).map(|i| notes_frame(&format!("note{i}"))).collect();
    let d = doc(&Op::new(&page(&notes(&frames))));
    assert!(d.markdown.contains("note0") && d.markdown.contains("note7"));
    assert!(!d.markdown.contains("note8"), "{}", d.markdown);
}

#[test]
fn a_cancelled_load_ends_early_without_an_error() {
    let c = Cancel::new(|| true);
    let dir = tmp("odpcancel");
    let p = write(&dir, "t.odp", &many_slides(30).bytes());
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
    let dir = tmp("odpcancel2");
    let p = write(&dir, "t.odp", &many_slides(200).bytes());
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
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
    check_headings(&d);
}

#[test]
fn a_slide_with_thousands_of_shapes_reads_quickly() {
    let s: String = (0..3000)
        .map(|i| {
            tf(
                (i % 40) as f64,
                (i / 40) as f64 * 0.3,
                0.9,
                0.2,
                &format!("c{i}"),
            )
        })
        .collect();
    let t = std::time::Instant::now();
    let d = doc(&Op::new(&page(&s)));
    assert!(d.markdown.contains("c10"));
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}

#[test]
fn a_million_pages_stop_at_the_cap_without_reading_them_all() {
    // 200,000 empty `draw:page` elements (a few MB): the cap ends the reading at 1,000.
    let pages = r#"<draw:page draw:name="p"/>"#.repeat(200_000);
    let t = std::time::Instant::now();
    let d = doc(&Op::new(&pages));
    assert_eq!(d.slides.len(), 1_000);
    assert!(d.truncated);
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
}

#[test]
fn a_package_over_the_container_limits_is_refused() {
    let opts = with_opts(|o| {
        o.limits = container::Limits {
            max_entries: 3,
            ..container::Limits::default()
        }
    });
    let o = Op::new(&page(&title("T")))
        .picture("a.png", 1)
        .picture("b.png", 2);
    assert!(matches!(
        load_op(&o, &opts).unwrap_err(),
        OfficeError::TooLarge { .. }
    ));
}

#[test]
fn the_xml_read_over_a_deck_is_budgeted_and_the_slides_read_so_far_are_kept() {
    let one = page(&(title("S") + &tf(1.0, 3.0, 5.0, 1.0, "text"))).len() as u64;
    let opts = with_opts(|o| o.max_pptx_read_total = one * 10 + 2_000);
    let d = load_op(&many_slides(100), &opts).unwrap();
    assert!(d.truncated);
    assert!(
        d.slides.len() >= 5 && d.slides.len() < 40,
        "{}",
        d.slides.len()
    );
    check_headings(&d);
}

#[test]
fn a_picture_path_cannot_leave_the_package_or_be_a_directory() {
    let o = Op::new(&page(
        &(fr(
            "",
            1.0,
            1.0,
            1.0,
            1.0,
            r#"<draw:image xlink:href="Pictures/../../x.png"/>"#,
        ) + &fr(
            "",
            1.0,
            3.0,
            1.0,
            1.0,
            r#"<draw:image xlink:href="Pictures/"/>"#,
        )),
    ))
    .part("x.png", &tiny_png(1));
    let d = doc(&o);
    assert!(d.images.is_empty());
}

// ---------------------------------------------------------------------------------------------
// the memory the master pages keep
// ---------------------------------------------------------------------------------------------

/// The text of every shape drawn under and on a slide, flattened.
fn drawn_texts(d: &Document, slide: usize) -> Vec<String> {
    use super::slide_draw::Item;
    fn walk(items: &[Item], out: &mut Vec<String>) {
        for i in items {
            match i {
                Item::Shape(s) => out.push(
                    s.text
                        .iter()
                        .flat_map(|t| &t.paragraphs)
                        .flat_map(|p| &p.runs)
                        .map(|r| r.text.as_str())
                        .collect(),
                ),
                Item::Group(g) => walk(&g.items, out),
                Item::Picture(_) => {}
            }
        }
    }
    let sc = &d.slide_scenes[slide];
    let mut v = Vec::new();
    for u in &sc.underlay {
        walk(u, &mut v);
    }
    walk(&sc.items, &mut v);
    v
}

/// A master page named `name` with one text shape reading `mark`, then `pad` raw XML.
fn marked_master(name: &str, mark: &str, pad: &str) -> String {
    format!(
        r#"<style:master-page style:name="{name}">{}{pad}</style:master-page>"#,
        tf(1.0, 1.0, 5.0, 1.0, mark)
    )
}

fn marked_deck(masters: &str, uses: &[&str]) -> Op {
    let pages: String = uses
        .iter()
        .map(|m| page_with("dp1", m, &tf(1.0, 5.0, 5.0, 1.0, "OWN")))
        .collect();
    Op::new(&pages).master(masters)
}

fn has(d: &Document, slide: usize, text: &str) -> bool {
    drawn_texts(d, slide).iter().any(|t| t == text)
}

#[test]
fn a_master_page_that_exactly_fits_the_xml_budget_is_kept() {
    let m = marked_master("Default", "MMARK", "");
    let o = with_opts(|o| o.max_master_xml = m.len() as u64);
    let d = load_op(&marked_deck(&m, &["Default"]), &o).unwrap();
    assert!(!d.truncated);
    assert!(has(&d, 0, "MMARK") && has(&d, 0, "OWN"));
}

#[test]
fn a_master_page_one_byte_over_the_xml_budget_is_left_out_and_the_slide_is_still_drawn() {
    let m = marked_master("Default", "MMARK", "");
    let o = with_opts(|o| o.max_master_xml = m.len() as u64 - 1);
    let d = load_op(&marked_deck(&m, &["Default"]), &o).unwrap();
    assert!(d.truncated);
    assert_eq!(d.slide_scenes.len(), 1);
    assert!(!has(&d, 0, "MMARK"), "{:?}", drawn_texts(&d, 0));
    assert!(has(&d, 0, "OWN"));
    assert!(d.markdown.contains("OWN"), "{}", d.markdown);
}

#[test]
fn the_xml_budget_is_a_total_over_the_master_pages() {
    // 50 small masters, a budget for ten: the first ten are kept, the rest left out.
    let masters: String = (0..50)
        .map(|i| marked_master(&format!("M{i}"), &format!("MM{i}"), ""))
        .collect();
    let one = marked_master("M0", "MM0", "").len();
    // (The names differ in length: M0..M9 are shorter than M10..M49, so a budget of ten of the
    // longer ones keeps at least these.)
    let o = with_opts(|o| o.max_master_xml = (one * 10 + 40) as u64);
    let d = load_op(&marked_deck(&masters, &["M0", "M9", "M49"]), &o).unwrap();
    assert!(d.truncated);
    assert!(has(&d, 0, "MM0"), "{:?}", drawn_texts(&d, 0));
    assert!(has(&d, 1, "MM9"));
    assert!(!has(&d, 2, "MM49"), "{:?}", drawn_texts(&d, 2));
    assert!(has(&d, 2, "OWN"));
    // With room for all of them none is left out.
    let all = load_op(&marked_deck(&masters, &["M49"]), &DocOptions::default()).unwrap();
    assert!(!all.truncated);
    assert!(has(&all, 0, "MM49"));
}

#[test]
fn one_huge_master_page_is_left_out_and_the_small_ones_after_it_are_kept() {
    let huge = marked_master("Huge", "HUGE", &"<draw:g/>".repeat(20_000));
    let small = marked_master("Small", "SMALL", "");
    let o = with_opts(|o| o.max_master_xml = (small.len() * 4) as u64);
    let d = load_op(
        &marked_deck(&format!("{huge}{small}"), &["Huge", "Small"]),
        &o,
    )
    .unwrap();
    assert!(d.truncated);
    assert!(!has(&d, 0, "HUGE") && has(&d, 0, "OWN"));
    assert!(has(&d, 1, "SMALL"), "{:?}", drawn_texts(&d, 1));
}

#[test]
fn a_huge_master_page_before_a_small_one_does_not_use_up_the_budget() {
    // The page that was left out is not counted: the small one after it fits exactly.
    let huge = marked_master("Huge", "HUGE", &"<draw:g/>".repeat(1_000));
    let small = marked_master("Small", "SMALL", "");
    let o = with_opts(|o| o.max_master_xml = small.len() as u64);
    let d = load_op(&marked_deck(&format!("{huge}{small}"), &["Small"]), &o).unwrap();
    assert!(d.truncated);
    assert!(has(&d, 0, "SMALL"));
}

#[test]
fn the_node_count_of_the_master_pages_is_bounded_too() {
    // A page of tiny elements is a few MB of XML (well inside the byte budget) but a node each.
    // The page is 1 node, the shape 3 more: it fits with exactly the cap and not with one more.
    let at = |g: usize| {
        let m = marked_master("Default", "MMARK", &"<draw:g/>".repeat(g));
        load_op(&marked_deck(&m, &["Default"]), &DocOptions::default()).unwrap()
    };
    let fits = at(MAX_MASTER_NODES - 4);
    assert!(!fits.truncated);
    assert!(has(&fits, 0, "MMARK"));
    let over = at(MAX_MASTER_NODES - 3);
    assert!(over.truncated);
    assert!(!has(&over, 0, "MMARK") && has(&over, 0, "OWN"));
}

#[test]
fn the_node_count_is_a_total_over_the_master_pages() {
    // Two pages that fit the cap alone but not together: the first is kept.
    let each = MAX_MASTER_NODES / 2 + 10;
    let a = marked_master("A", "AAA", &"<draw:g/>".repeat(each));
    let b = marked_master("B", "BBB", &"<draw:g/>".repeat(each));
    let d = load_op(
        &marked_deck(&format!("{a}{b}"), &["A", "B"]),
        &DocOptions::default(),
    )
    .unwrap();
    assert!(d.truncated);
    assert!(has(&d, 0, "AAA") && !has(&d, 1, "BBB") && has(&d, 1, "OWN"));
}

#[test]
fn master_styles_with_no_master_page_and_other_children_are_not_truncated() {
    let d = doc(&marked_deck(
        r#"<draw:layer-set><draw:layer draw:name="layout"/></draw:layer-set>"#,
        &["Nope"],
    ));
    assert!(!d.truncated);
    assert!(has(&d, 0, "OWN"));
}

#[test]
fn master_pages_after_other_children_and_unnamed_pages_are_read() {
    let m = format!(
        r#"<draw:layer-set><draw:layer draw:name="layout"/></draw:layer-set><style:master-page>{}</style:master-page>{}"#,
        tf(1.0, 1.0, 5.0, 1.0, "NONAME"),
        marked_master("Default", "MMARK", "")
    );
    let d = doc(&marked_deck(&m, &["Default"]));
    assert!(!d.truncated);
    assert!(has(&d, 0, "MMARK") && !has(&d, 0, "NONAME"));
}
