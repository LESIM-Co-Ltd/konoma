//! The limits of `svg_guard`, pinned one by one.
//!
//! The drawing of an untrusted SVG happens in a child process (`svg_proc`), so a missed limit no
//! longer takes the application down — but the limits are what make the child's refusals fast and
//! what the child's own survival rests on, so each one is tested the way it is meant to work:
//! the exact value, the last input that passes and the first that does not, and the property it
//! protects (a hostile nested document is *not drawn*, which a pixel comparison shows).
//!
//! Lives in a module of `svg_guard` itself to reach the private tokenizer, the budget and the
//! constants.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use super::*;

// ---- building blocks --------------------------------------------------------------------------

fn svg(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100" viewBox="0 0 100 100">{body}</svg>"#
    )
}

fn gz(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn wrap(open: &str, close: &str, n: usize, inner: &str) -> String {
    format!("{}{}{}", open.repeat(n), inner, close.repeat(n))
}

/// A picture of `doc` at 100 px, or why not. Drawn on a thread with a large stack, so that a debug
/// build's frames never decide a test (the production drawing process has its own 8 MiB main stack
/// and the guard's helper thread for deep documents).
fn draw(doc: &str, base: Option<&Path>) -> Result<image::RgbaImage, SvgFail> {
    let (doc, base) = (doc.to_string(), base.map(Path::to_path_buf));
    big(move || {
        super::super::svg::rasterize_guarded(doc.as_bytes(), base.as_deref(), 100)
            .map(|i| i.into_rgba8())
    })
}

fn red_pixels(img: &image::RgbaImage) -> usize {
    img.pixels()
        .filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60 && p[3] > 200)
        .count()
}

/// `<image>` showing `inner` through a data URL.
fn embed(mime: &str, inner: &[u8]) -> String {
    format!(
        r#"<image width="100" height="100" href="data:{mime};base64,{}"/>"#,
        b64(inner)
    )
}

/// A document that paints the whole canvas red, `depth` elements deep (`<svg>` counts as one).
fn red_at_depth(depth: usize) -> String {
    assert!(depth >= 2);
    svg(&wrap(
        "<g>",
        "</g>",
        depth - 2,
        r#"<rect width="100" height="100" fill="red"/>"#,
    ))
}

fn blue_outer(image: &str) -> String {
    svg(&format!(
        r##"<rect width="100" height="100" fill="#00f"/>{image}"##
    ))
}

/// Whether the outer document showed the embedded one (red over blue) or went on without it.
fn shows_inner(outer: &str) -> bool {
    let img = draw(outer, None).expect("the outer document draws whatever the inner one does");
    red_pixels(&img) > 1000
}

/// Run `f` on a thread with a large stack: the XML parser recurses once per level of nesting, and
/// a debug build's frames at 257 levels do not fit the test harness's 2 MiB thread (exactly the
/// hazard the flat scan exists for).
fn big<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn_scoped(s, f)
            .unwrap()
            .join()
            .unwrap()
    })
}

// ---- the numbers ------------------------------------------------------------------------------

/// Every limit has the value its documentation gives it. A change is a decision, made here.
#[test]
fn the_limits_have_the_documented_values() {
    assert_eq!(MAX_SVG_BYTES, 32 << 20);
    assert_eq!(MAX_XML_DEPTH, 256);
    assert_eq!(INLINE_DEPTH, 64);
    assert_eq!(STACK_BYTES, 128 << 20);
    assert_eq!(MAX_XML_NODES, 2_000_000);
    assert_eq!(MAX_EXPANDED_ELEMENTS, 1_000_000);
    assert_eq!(MAX_LIVE_LAYER_BYTES, (1u64 << 30) as f64);
    assert_eq!(MAX_FILTER_WORK, 6.0e8);
    assert_eq!(MAX_EMBEDDED_RASTER_PX, 64.0 * 1024.0 * 1024.0);
    assert_eq!(MAX_TREE_NODES, 3_000_000);
    assert_eq!(MAX_EXTERNAL_IMAGE_BYTES, 64 << 20);
}

// ---- size and gzip ----------------------------------------------------------------------------

#[test]
fn a_plain_document_is_accepted_up_to_exactly_the_size_limit() {
    assert_eq!(
        plain_bytes(&vec![b' '; MAX_SVG_BYTES]).map(|b| b.len()),
        Ok(MAX_SVG_BYTES)
    );
    assert_eq!(
        plain_bytes(&vec![b' '; MAX_SVG_BYTES + 1]).err(),
        Some(Reject::TooLarge)
    );
}

#[test]
fn a_compressed_document_is_accepted_up_to_exactly_the_size_limit_after_decompression() {
    let at = gz(&vec![b' '; MAX_SVG_BYTES]);
    assert!(at.len() < 1 << 20, "the test file is small: {}", at.len());
    assert_eq!(gunzip_bounded(&at).map(|v| v.len()), Ok(MAX_SVG_BYTES));
    assert_eq!(plain_bytes(&at).map(|b| b.len()), Ok(MAX_SVG_BYTES));
    let over = gz(&vec![b' '; MAX_SVG_BYTES + 1]);
    assert_eq!(gunzip_bounded(&over), Err(Reject::TooLarge));
    assert_eq!(plain_bytes(&over).err(), Some(Reject::TooLarge));
}

/// The bound is on what is *produced*: a stream that goes wrong after the limit is never reached.
#[test]
fn decompression_stops_at_the_limit_and_does_not_read_on() {
    // 40 MiB of spaces, compressed, with the end of the stream cut off: reading on past the limit
    // would hit the broken end (an error); stopping at the limit reports the size instead.
    let mut z = gz(&vec![b' '; 40 << 20]);
    z.truncate(z.len() - 12);
    assert_eq!(gunzip_bounded(&z), Err(Reject::TooLarge));
    assert_eq!(plain_bytes(&z).err(), Some(Reject::TooLarge));
}

/// A gzip bomb: 300 MB of spaces in a few hundred KB. Refused after producing at most one byte
/// past the limit, not after producing all of it.
#[test]
fn a_gzip_bomb_is_refused_quickly_and_without_allocating_what_it_claims() {
    let bomb = gz(&vec![b' '; 300 << 20]);
    let t = Instant::now();
    assert_eq!(gunzip_bounded(&bomb), Err(Reject::TooLarge));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    let doc = {
        let mut d = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"5\" height=\"5\">".to_vec();
        d.extend(std::iter::repeat_n(b' ', 40 << 20));
        d.extend_from_slice(b"</svg>");
        gz(&d)
    };
    assert_eq!(
        super::super::svg::rasterize_guarded(&doc, None, 50).err(),
        Some(SvgFail::TooLarge)
    );
}

#[test]
fn a_small_svgz_draws_and_a_corrupt_one_is_refused() {
    let doc = svg(r#"<rect width="100" height="100" fill="red"/>"#);
    let z = gz(doc.as_bytes());
    let img = super::super::svg::rasterize_guarded(&z, None, 100).expect("an svgz draws");
    assert!(red_pixels(&img.into_rgba8()) > 9000);
    // Cut in the middle of the stream, and garbage after the magic number.
    assert_eq!(
        super::super::svg::rasterize_guarded(&z[..z.len() / 2], None, 100).err(),
        Some(SvgFail::Invalid)
    );
    let mut junk = vec![0x1f, 0x8b, 8, 0];
    junk.extend((0..200u32).map(|i| (i * 7 % 251) as u8));
    assert_eq!(gunzip_bounded(&junk), Err(Reject::NotXml));
    assert_eq!(
        super::super::svg::rasterize_guarded(&junk, None, 100).err(),
        Some(SvgFail::Invalid)
    );
}

// ---- the XML parse ----------------------------------------------------------------------------

/// Old editors write a DOCTYPE with entities; they must keep drawing.
#[test]
fn a_small_svg_that_uses_a_doctype_entity_draws() {
    let doc = r##"<!DOCTYPE svg [ <!ENTITY red "#ff0000"> ]>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="&red;"/></svg>"##;
    let img = draw(doc, None).expect("an entity in an attribute value is fine");
    assert!(red_pixels(&img) > 9000);
}

/// Comments, text and processing instructions are nodes too: a document of two million comments has
/// two elements and is still refused, by the parser's own limit.
#[test]
fn more_xml_nodes_than_the_limit_are_refused_even_when_few_of_them_are_elements() {
    let mut doc = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="5" height="5">"#);
    doc.push_str(&"<!---->".repeat(MAX_XML_NODES as usize + 100));
    doc.push_str("</svg>");
    assert!(doc.len() < MAX_SVG_BYTES);
    assert_eq!(
        super::super::svg::rasterize_guarded(doc.as_bytes(), None, 50).err(),
        Some(SvgFail::TooComplex)
    );
    // A million comments are fine.
    let mut ok = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="5" height="5">"#);
    ok.push_str(&"<!---->".repeat(1_000_000));
    ok.push_str("</svg>");
    assert!(super::super::svg::rasterize_guarded(ok.as_bytes(), None, 50).is_ok());
}

#[test]
fn more_xml_nodes_than_the_limit_are_refused_as_too_many_elements() {
    let mut doc = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="5" height="5">"#);
    doc.push_str(&"<g/>".repeat(MAX_XML_NODES as usize + 100));
    doc.push_str("</svg>");
    assert!(doc.len() < MAX_SVG_BYTES);
    let t = Instant::now();
    assert_eq!(
        super::super::svg::rasterize_guarded(doc.as_bytes(), None, 50).err(),
        Some(SvgFail::TooComplex)
    );
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
}

// ---- depth ------------------------------------------------------------------------------------

#[test]
fn xml_depth_counts_elements_only_and_reports_the_deepest_branch() {
    let depth = |s: &str| xml_depth(&roxmltree::Document::parse(s).unwrap());
    assert_eq!(depth("<a/>"), 1);
    assert_eq!(depth("<a><b><c/></b></a>"), 3);
    // Text, comments and processing instructions do not add a level.
    assert_eq!(depth("<a>text<!-- note --><?pi x?></a>"), 1);
    assert_eq!(depth("<a><b>text<!-- c --></b></a>"), 2);
    // The deepest branch comes first, a shallow one last: the answer is the deepest.
    assert_eq!(depth("<a><b><c><d/></c></b><e/></a>"), 4);
}

#[test]
fn check_xml_refuses_beyond_the_depth_limit_and_not_before() {
    let doc = |n: usize| red_at_depth(n);
    let check = |s: String| big(move || check_xml(&roxmltree::Document::parse(&s).unwrap()));
    assert_eq!(check(doc(MAX_XML_DEPTH)), Ok(()));
    assert_eq!(check(doc(MAX_XML_DEPTH + 1)), Err(Reject::TooDeep));
}

#[test]
fn scan_depth_agrees_with_the_parser_at_the_limit() {
    let at = red_at_depth(MAX_XML_DEPTH);
    let over = red_at_depth(MAX_XML_DEPTH + 1);
    // `<svg>` + the groups + `<rect>`: the deepest element is at level MAX_XML_DEPTH.
    assert_eq!(scan_depth(at.as_bytes()), Ok(MAX_XML_DEPTH));
    assert_eq!(scan_depth(over.as_bytes()), Err(Reject::TooDeep));
}

/// The tokenizer reads markup the way XML does: what merely looks like a tag does not count, and
/// what is a tag counts exactly once.
#[test]
fn scan_depth_follows_the_xml_rules_for_what_is_a_tag() {
    let depth = |s: &str| scan_depth(s.as_bytes());
    // Plain nesting, self-closing elements, and closing tags that bring the level back down.
    assert_eq!(depth("<a><b><c/></b></a>"), Ok(3));
    assert_eq!(depth("<a><b/><b/><b/></a>"), Ok(2));
    assert_eq!(depth("<a><b></b><c><d/></c></a>"), Ok(3));
    // Comments, CDATA and processing instructions contain things that look like tags.
    assert_eq!(depth("<a><!-- <b><c><d> --><e/></a>"), Ok(2));
    assert_eq!(depth("<a><![CDATA[ <b><c><d> ]]><e/></a>"), Ok(2));
    assert_eq!(
        depth("<?xml version=\"1.0\"?><a><?pi <b><c> ?><e/></a>"),
        Ok(2)
    );
    // The end of each of those is found exactly: text right after it is not swallowed.
    assert_eq!(depth("<a><!--x--><b><c/></b></a>"), Ok(3));
    assert_eq!(depth("<a><![CDATA[x]]><b><c/></b></a>"), Ok(3));
    assert_eq!(depth("<?p?><a><b><c/></b></a>"), Ok(3));
    // CDATA may hold quote characters, brackets and tags.
    assert_eq!(depth("<a><![CDATA[ it's <b><c> ]]><d/></a>"), Ok(2));
    assert_eq!(depth("<a><![CDATA[ \"x <b> ]]><d/></a>"), Ok(2));
    // A `>` or a `<` inside a quoted attribute value is not the end of the tag.
    assert_eq!(depth(r#"<a x="1>2"><b y='<c>'><d/></b></a>"#), Ok(3));
    assert_eq!(depth(r#"<a x='say "hi" >'><b/></a>"#), Ok(2));
    // A DOCTYPE with an internal subset: brackets, quotes and comments inside it are skipped.
    assert_eq!(
        depth("<!DOCTYPE svg [ <!ENTITY e \"a>b\"> <!-- ] \" --> ]><a><b/></a>"),
        Ok(2)
    );
    assert_eq!(depth("<!DOCTYPE a [ <!ELEMENT a (b)> ]><a><b/></a>"), Ok(2));
    // An unclosed construct is not XML.
    for broken in [
        "<a><!-- never closed",
        "<a><![CDATA[ never",
        "<a><?pi never",
        "<a x=\"never>",
    ] {
        assert_eq!(depth(broken), Err(Reject::NotXml), "{broken}");
    }
    // An entity whose value is markup would be expanded into elements the scan never saw.
    assert_eq!(
        depth("<!DOCTYPE a [ <!ENTITY o \"<g>\"> ]><a>&o;</a>"),
        Err(Reject::TooDeep)
    );
}

/// The scan counts the level of a start tag before its own children, and a self-closing tag leaves
/// no level behind.
#[test]
fn scan_depth_levels_are_exact_for_self_closing_and_nested_tags() {
    let n = MAX_XML_DEPTH;
    let many = |k: usize| format!("{}<x/>{}", "<g>".repeat(k), "</g>".repeat(k));
    // k groups + the self-closing leaf = k + 1 levels.
    assert_eq!(scan_depth(many(n - 1).as_bytes()), Ok(n));
    assert_eq!(scan_depth(many(n).as_bytes()), Err(Reject::TooDeep));
    // A self-closing tag does not leave its level open: a long run of them stays shallow.
    let flat = format!("<g>{}</g>", "<x/>".repeat(1000));
    assert_eq!(scan_depth(flat.as_bytes()), Ok(2));
    // ... and a run of open-close pairs does not accumulate either.
    let pairs = format!("<g>{}</g>", "<x></x>".repeat(1000));
    assert_eq!(scan_depth(pairs.as_bytes()), Ok(2));
}

// ---- use expansion ----------------------------------------------------------------------------

fn expanded(body: &str) -> Option<u64> {
    let doc = svg(body);
    let parsed = roxmltree::Document::parse(&doc).unwrap();
    expanded_elements(&parsed)
}

/// `g0 <- g1 <- ... <- gN`, each holding a `<use>` of the previous one, defined in the order that
/// makes the walk follow the whole chain (a target defined first would already be counted).
fn use_chain(n: usize) -> String {
    let mut s = String::from("<defs>");
    for i in (1..=n).rev() {
        s.push_str(&format!(r##"<g id="g{i}"><use href="#g{}"/></g>"##, i - 1));
    }
    s.push_str(r#"<g id="g0"><rect width="1" height="1"/></g>"#);
    s.push_str(&format!(r##"</defs><use href="#g{n}"/>"##));
    s
}

fn use_bomb(levels: usize) -> String {
    let mut s = String::from(r#"<defs><g id="u0"><rect width="1" height="1"/></g>"#);
    for i in 1..=levels {
        s.push_str(&format!(
            r##"<g id="u{i}"><use href="#u{}"/><use href="#u{}"/></g>"##,
            i - 1,
            i - 1
        ));
    }
    s.push_str(&format!(r##"</defs><use href="#u{levels}"/>"##));
    s
}

#[test]
fn expanded_elements_counts_what_use_would_produce() {
    // No `use`: the plain element count (svg + g + rect).
    assert_eq!(expanded(r#"<g><rect width="1" height="1"/></g>"#), Some(3));
    // svg, defs, g#a, rect, then the `use` (1) and the copy of g#a with its rect (2).
    assert_eq!(
        expanded(r##"<defs><g id="a"><rect width="1" height="1"/></g></defs><use href="#a"/>"##),
        Some(7)
    );
    // Two uses of a two-element group: each adds 1 + 2.
    assert_eq!(
        expanded(r##"<g id="a"><rect/></g><use href="#a"/><use href="#a"/>"##),
        Some(1 + 2 + 2 * 3)
    );
    // A `use` of something that does not exist adds only itself.
    assert_eq!(expanded(r##"<use href="#nowhere"/>"##), Some(2));
}

#[test]
fn the_first_element_with_an_id_is_the_one_a_use_points_at() {
    // usvg resolves a reference to the first element with that id; so does the count.
    let small_first = r##"<g id="a"/><g id="a"><rect/><rect/><rect/><rect/></g><use href="#a"/>"##;
    let big_first = r##"<g id="a"><rect/><rect/><rect/><rect/></g><g id="a"/><use href="#a"/>"##;
    assert_eq!(expanded(small_first), Some(1 + 1 + 5 + 2));
    assert_eq!(expanded(big_first), Some(1 + 5 + 1 + 1 + 5));
}

#[test]
fn a_reference_cycle_adds_nothing_beyond_itself() {
    // usvg skips the cycle: `g#a` (1) holds a `use` (1) of `g#a`, which is already being counted.
    assert_eq!(
        expanded(r##"<g id="a"><use href="#a"/></g>"##),
        Some(1 + 1 + 1)
    );
    assert_eq!(
        expanded(r##"<g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g>"##),
        // svg (1) + a (1 + use (1 + b (1 + use (1 + a: a cycle, 0)))) + b (already counted: 2).
        Some(1 + 4 + 2)
    );
}

#[test]
fn the_expansion_count_saturates_just_above_the_limit() {
    // 2^22 leaves: far over the limit, and reported as exactly one more than it, not the sum.
    assert_eq!(expanded(&use_bomb(22)), Some(MAX_EXPANDED_ELEMENTS + 1));
}

#[test]
fn a_reference_chain_longer_than_the_walk_can_follow_is_refused() {
    // Each hop is two levels of the walk (the `use`, then its target): 500 hops are fine, 600 are
    // past the 1024-level cap and are refused as too complex, not followed.
    assert!(expanded(&use_chain(500)).is_some());
    assert_eq!(expanded(&use_chain(600)), None);
    let doc = svg(&use_chain(600));
    assert_eq!(
        check_xml(&roxmltree::Document::parse(&doc).unwrap()),
        Err(Reject::TooManyElements)
    );
}

#[test]
fn the_element_count_limit_is_exact() {
    // svg + n `<g/>` = n + 1 elements.
    let doc = |n: usize| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="5" height="5">{}</svg>"#,
            "<g/>".repeat(n)
        )
    };
    let check = |n: usize| check_xml(&roxmltree::Document::parse(&doc(n)).unwrap());
    assert_eq!(
        check(MAX_EXPANDED_ELEMENTS as usize - 1),
        Ok(()),
        "exactly the limit passes"
    );
    assert_eq!(
        check(MAX_EXPANDED_ELEMENTS as usize),
        Err(Reject::TooManyElements),
        "one more does not"
    );
}

// ---- SVG inside SVG: data URLs ---------------------------------------------------------------

/// A hostile inner document is *not drawn*: the outer one comes out exactly as if the `<image>`
/// were not there. (Before this was tested only for the time it took.)
fn assert_inner_is_left_out(what: &str, inner: &[u8], mime: &str) {
    let with = draw(&blue_outer(&embed(mime, inner)), None)
        .unwrap_or_else(|e| panic!("{what}: the outer document must still draw: {e:?}"));
    let without = draw(&blue_outer(""), None).unwrap();
    assert_eq!(red_pixels(&with), 0, "{what}: the inner document was drawn");
    assert!(
        with.as_raw() == without.as_raw(),
        "{what}: the outer document differs from one without the image"
    );
}

#[test]
fn a_shallow_embedded_svg_is_drawn_and_a_too_deep_one_is_not() {
    let svg_mime = "image/svg+xml";
    assert!(shows_inner(&blue_outer(&embed(
        svg_mime,
        red_at_depth(10).as_bytes()
    ))));
    assert_inner_is_left_out(
        "depth limit + 1",
        red_at_depth(MAX_XML_DEPTH + 1).as_bytes(),
        svg_mime,
    );
    assert_inner_is_left_out("depth 1000", red_at_depth(1000).as_bytes(), svg_mime);
}

/// An embedded document is parsed on the stack of the drawing it is part of: one deeper than
/// `INLINE_DEPTH` is only safe where the guard already moved to the large stack.
#[test]
fn an_embedded_svg_deeper_than_the_inline_depth_needs_the_outer_one_to_be_on_the_large_stack() {
    let svg_mime = "image/svg+xml";
    let inner = |depth: usize| embed(svg_mime, red_at_depth(depth).as_bytes());
    // A shallow outer document is on the caller's stack: inner documents up to the inline depth.
    assert!(
        shows_inner(&blue_outer(&inner(INLINE_DEPTH))),
        "exactly the inline depth"
    );
    assert_inner_is_left_out(
        "inline depth + 1",
        red_at_depth(INLINE_DEPTH + 1).as_bytes(),
        svg_mime,
    );
    // An outer document deeper than the inline depth is moved to the large stack, so a deeper
    // inner one is safe too — up to the depth limit.
    let deep_outer = |image: &str| {
        svg(&format!(
            r##"{}<rect width="100" height="100" fill="#00f"/>{image}{}"##,
            "<g>".repeat(INLINE_DEPTH + 6),
            "</g>".repeat(INLINE_DEPTH + 6)
        ))
    };
    assert!(shows_inner(&deep_outer(&inner(INLINE_DEPTH + 40))));
    assert!(shows_inner(&deep_outer(&inner(MAX_XML_DEPTH))));
    let too_deep = deep_outer(&inner(MAX_XML_DEPTH + 1));
    let img = draw(&too_deep, None).unwrap();
    assert_eq!(
        red_pixels(&img),
        0,
        "over the depth limit even on the large stack"
    );
}

#[test]
fn the_large_stack_is_chosen_by_the_depth_of_the_whole_document() {
    let on_big_stack = |doc: &str| {
        let doc = doc.to_string();
        big(move || {
            load_tree(
                doc.as_bytes(),
                None,
                |_tree| Ok(BIG_STACK.with(|b| b.get())),
            )
        })
    };
    assert_eq!(on_big_stack(&red_at_depth(5)), Ok(false));
    assert_eq!(
        on_big_stack(&red_at_depth(INLINE_DEPTH)),
        Ok(false),
        "exactly the inline depth stays"
    );
    assert_eq!(
        on_big_stack(&red_at_depth(INLINE_DEPTH + 1)),
        Ok(true),
        "one more moves"
    );
    assert_eq!(on_big_stack(&red_at_depth(MAX_XML_DEPTH)), Ok(true));
    // And an svgz is judged on what is inside it.
    let z = gz(red_at_depth(INLINE_DEPTH + 1).as_bytes());
    let on_big_z = big(move || load_tree(&z, None, |_t| Ok(BIG_STACK.with(|b| b.get()))));
    assert_eq!(on_big_z, Ok(true));
}

#[test]
fn a_panic_on_the_large_stack_is_a_refusal_not_a_crash() {
    let r: Result<(), SvgFail> = with_stack_why(true, || panic!("a renderer bug"));
    assert_eq!(r, Err(SvgFail::Invalid));
    assert_eq!(with_stack_why(true, || Ok(5)), Ok(5));
    assert_eq!(with_stack_why(false, || Ok(6)), Ok(6));
}

#[test]
fn text_plain_is_treated_as_a_possible_svg_and_checked_like_one() {
    // An SVG sent as text/plain is still parsed by usvg, so it gets the same refusals.
    assert!(shows_inner(&blue_outer(&embed(
        "text/plain",
        red_at_depth(10).as_bytes()
    ))));
    assert_inner_is_left_out(
        "text/plain, too deep",
        red_at_depth(1000).as_bytes(),
        "text/plain",
    );
    // Raster data under text/plain is not markup, so it is not judged and still draws.
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    assert!(
        shows_inner(&blue_outer(&embed("image/png", &png))),
        "control: a png draws"
    );
    assert!(
        shows_inner(&blue_outer(&embed("text/plain", &png))),
        "a png sent as text/plain draws"
    );
}

#[test]
fn an_embedded_use_bomb_or_gzip_bomb_is_left_out() {
    let bomb = svg(&use_bomb(22));
    let red_bomb = svg(&format!(
        r##"<rect width="100" height="100" fill="red"/>{}"##,
        use_bomb(22)
    ));
    let _ = bomb;
    assert_inner_is_left_out("use bomb", red_bomb.as_bytes(), "image/svg+xml");
    // Refused by the check, not by waiting for usvg to give up (a bomb twice as big takes it twice
    // as long: tens of seconds from here).
    let bigger = svg(&format!(
        r##"<rect width="100" height="100" fill="red"/>{}"##,
        use_bomb(28)
    ));
    let t = Instant::now();
    assert_inner_is_left_out("bigger use bomb", bigger.as_bytes(), "image/svg+xml");
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "{:?}",
        t.elapsed()
    );
    // The same inner document, small, draws.
    let small = svg(&format!(
        r##"<rect width="100" height="100" fill="red"/>{}"##,
        use_bomb(3)
    ));
    assert!(shows_inner(&blue_outer(&embed(
        "image/svg+xml",
        small.as_bytes()
    ))));
    // A gzip bomb inside the data URL: refused by the bounded gunzip, not inflated.
    let mut doc = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/>"##.to_vec();
    doc.extend(std::iter::repeat_n(b' ', MAX_SVG_BYTES + 1000));
    doc.extend_from_slice(b"</svg>");
    let t = Instant::now();
    assert_inner_is_left_out("gzip bomb", &gz(&doc), "image/svg+xml");
    assert!(t.elapsed() < Duration::from_secs(10));
    // And the same document, gzipped and small, draws.
    let ok = gz(red_at_depth(5).as_bytes());
    assert!(shows_inner(&blue_outer(&embed("image/svg+xml", &ok))));
    // An svgz holding a too-deep document is refused after decompression.
    assert_inner_is_left_out(
        "deep svgz",
        &gz(red_at_depth(1000).as_bytes()),
        "image/svg+xml",
    );
}

/// `levels` nested documents: the outer one is blue, each embeds the next, the last paints red.
fn chain_of_embedded_documents(levels: usize) -> String {
    let mut doc = red_at_depth(3);
    for _ in 0..levels {
        doc = blue_outer(&embed("image/svg+xml", doc.as_bytes()));
    }
    doc
}

#[test]
fn svg_in_svg_is_followed_three_levels_and_no_further() {
    // `levels` embeddings below the outer document: the red one is drawn for 1..=3, not for 4.
    for levels in 1..=3 {
        assert!(
            shows_inner(&chain_of_embedded_documents(levels)),
            "{levels} levels of embedding are followed"
        );
    }
    assert!(
        !shows_inner(&chain_of_embedded_documents(4)),
        "the fourth is not"
    );
    assert!(!shows_inner(&chain_of_embedded_documents(8)));
}

#[test]
fn the_nesting_count_is_given_back_after_each_embedded_document() {
    // Five sibling images, one per strip: every one is drawn (a count that was never given back
    // would refuse the later ones).
    let strip = |i: usize| {
        let inner = svg(&format!(
            r##"<rect x="{}" width="20" height="100" fill="red"/>"##,
            i * 20
        ));
        embed("image/svg+xml", inner.as_bytes())
    };
    let images: String = (0..5).map(strip).collect();
    let img = draw(&blue_outer(&images), None).unwrap();
    assert_eq!(red_pixels(&img), 100 * 100, "all five strips are red");
}

#[test]
fn a_panic_while_following_an_embedded_document_does_not_leave_the_count_raised() {
    let before = NESTING.with(|n| n.get());
    let _ = std::panic::catch_unwind(|| nested(|| panic!("while drawing the embedded document")));
    assert_eq!(NESTING.with(|n| n.get()), before);
}

// ---- SVG inside SVG: files --------------------------------------------------------------------

fn write_png_noise(path: &Path, side: u32) {
    // Noise does not compress: the file is big enough to tell a size limit of 1 KiB from 64 MiB.
    let mut x = 12345u32;
    let img = image::RgbaImage::from_fn(side, side, |_, _| {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        image::Rgba([255, (x >> 24) as u8 / 4, (x >> 16) as u8 / 4, 255])
    });
    img.save(path).unwrap();
}

fn tmp(prefix: &str) -> crate::test_support::TmpDir {
    let dir = crate::test_support::unique_tmp(prefix);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_referenced_image_file_is_drawn_when_it_is_a_regular_file_within_the_limit() {
    let dir = tmp("svg_guard_ext_png");
    write_png_noise(&dir.join("p.png"), 64);
    let size = std::fs::metadata(dir.join("p.png")).unwrap().len();
    assert!(size > 4096 && size < 64 << 20, "{size}");
    let outer = blue_outer(r#"<image width="100" height="100" href="p.png"/>"#);
    let with = draw(&outer, Some(&dir)).unwrap();
    assert!(red_pixels(&with) > 5000, "the png beside the svg is drawn");
    // Not found beside it: the outer document still draws, with nothing in its place.
    let without = draw(&outer, None).unwrap();
    assert_eq!(red_pixels(&without), 0);
}

#[test]
fn a_referenced_file_over_the_limit_is_not_read() {
    let dir = tmp("svg_guard_ext_big");
    // PNG signature then zeros, sparse: 64 MiB + 1.
    let f = std::fs::File::create(dir.join("big.png")).unwrap();
    f.set_len(MAX_EXTERNAL_IMAGE_BYTES + 1).unwrap();
    use std::os::unix::fs::FileExt;
    f.write_at(b"\x89PNG\r\n\x1a\n", 0).unwrap();
    let opts = options(Some(dir.to_path_buf()));
    let t = Instant::now();
    assert!((opts.image_href_resolver.resolve_string)("big.png", &opts).is_none());
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    // A directory and a missing file are refused too.
    std::fs::create_dir_all(dir.join("d.png")).unwrap();
    assert!((opts.image_href_resolver.resolve_string)("d.png", &opts).is_none());
    assert!((opts.image_href_resolver.resolve_string)("nope.png", &opts).is_none());
}

#[cfg(unix)]
#[test]
fn a_referenced_fifo_does_not_block_the_drawing() {
    let dir = tmp("svg_guard_ext_fifo");
    let path = std::ffi::CString::new(dir.join("pipe.png").to_str().unwrap()).unwrap();
    // SAFETY: a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let outer = blue_outer(r#"<image width="100" height="100" href="pipe.png"/>"#);
    let t = Instant::now();
    assert!(draw(&outer, Some(&dir)).is_ok());
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
}

#[test]
fn a_referenced_svg_file_gets_the_same_checks_as_an_embedded_one() {
    let dir = tmp("svg_guard_ext_svg");
    std::fs::write(dir.join("ok.svg"), red_at_depth(10)).unwrap();
    std::fs::write(dir.join("okz.svgz"), gz(red_at_depth(10).as_bytes())).unwrap();
    std::fs::write(dir.join("deep.svg"), red_at_depth(1000)).unwrap();
    std::fs::write(dir.join("deepz.svgz"), gz(red_at_depth(1000).as_bytes())).unwrap();
    let mut bomb = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="red"/>"##.to_vec();
    bomb.extend(std::iter::repeat_n(b' ', MAX_SVG_BYTES + 100));
    bomb.extend_from_slice(b"</svg>");
    std::fs::write(dir.join("bomb.svgz"), gz(&bomb)).unwrap();
    std::fs::write(
        dir.join("useb.svg"),
        svg(&format!(
            r##"<rect width="100" height="100" fill="red"/>{}"##,
            use_bomb(22)
        )),
    )
    .unwrap();
    let outer = |target: &str| {
        blue_outer(&format!(
            r#"<image width="100" height="100" href="{target}"/>"#
        ))
    };
    for good in ["ok.svg", "okz.svgz"] {
        let img = draw(&outer(good), Some(&dir)).unwrap();
        assert!(red_pixels(&img) > 9000, "{good} is drawn");
    }
    let bare = draw(&blue_outer(""), None).unwrap();
    for bad in ["deep.svg", "deepz.svgz", "bomb.svgz", "useb.svg"] {
        let t = Instant::now();
        let img = draw(&outer(bad), Some(&dir)).unwrap();
        assert!(img.as_raw() == bare.as_raw(), "{bad} was drawn");
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "{bad}: {:?}",
            t.elapsed()
        );
    }
}

/// usvg does not let a document that is itself drawn as an `<image>` load files (the SVG rule for
/// "SVG as an image"), so a chain of files stops at the first link whatever the nesting limit says.
/// This pins that: if a usvg upgrade ever changes it, the chain is bounded by the nesting limit
/// (`svg_in_svg_is_followed_three_levels_and_no_further` shows the same limit for data URLs).
#[test]
fn a_file_referenced_from_a_document_that_is_itself_an_image_is_not_loaded() {
    let dir = tmp("svg_guard_ext_chain");
    std::fs::write(dir.join("f0.svg"), red_at_depth(3)).unwrap();
    std::fs::write(
        dir.join("f1.svg"),
        blue_outer(&format!(
            r#"<image width="100" height="100" href="{}/f0.svg"/>"#,
            dir.display()
        )),
    )
    .unwrap();
    let shows = |k: usize| {
        let outer = blue_outer(&format!(
            r#"<image width="100" height="100" href="f{k}.svg"/>"#
        ));
        red_pixels(&draw(&outer, Some(&dir)).unwrap()) > 1000
    };
    assert!(shows(0), "one file reference is followed");
    assert!(!shows(1), "a reference made from inside it is not");
}

// ---- the render budget ------------------------------------------------------------------------

fn tree_of(doc: &str) -> usvg::Tree {
    let doc = doc.to_string();
    big(move || load_tree(doc.as_bytes(), None, Ok).expect("the document parses"))
}

/// The budget's verdict on `doc` drawn at `scale`.
fn within_budget(doc: &str, scale: f32) -> bool {
    let tree = tree_of(doc);
    big(move || check_render_budget(&tree, scale))
}

/// The filter work the budget charges `doc`, in the budget's own units.
fn filter_work(doc: &str, scale: f32) -> f64 {
    let tree = tree_of(doc);
    let size = tree.size();
    let canvas_px = (f64::from(size.width() * scale) * f64::from(size.height() * scale)).max(1.0);
    let mut cx = Budget {
        scale: f64::from(scale),
        canvas_px,
        filter_work: 0.0,
        raster_px: 0.0,
        nodes: 0,
        work_limit: MAX_FILTER_WORK,
    };
    big(move || {
        cx.group(tree.root(), 0.0, 0);
        cx.filter_work
    })
}

/// A 100 x 100 rect through one filter that covers exactly the rect.
fn filtered(primitives: &str) -> String {
    svg(&format!(
        r##"<filter id="f" x="0" y="0" width="1" height="1">{primitives}</filter><rect width="100" height="100" filter="url(#f)"/>"##
    ))
}

/// What each kind of filter primitive costs per pixel, relative to the plain ones (`feFlood` = 3):
/// the weights are the calibration the limit rests on.
#[test]
fn each_filter_primitive_is_charged_its_calibrated_weight() {
    let base = filter_work(&filtered(r#"<feFlood flood-color="red"/>"#), 1.0);
    assert!(base > 1000.0, "{base}");
    let weight = |prims: &str| filter_work(&filtered(prims), 1.0) / base * 3.0;
    let near = |got: f64, want: f64| (got - want).abs() < 1e-6 * want;
    let cases: [(&str, &str, f64); 14] = [
        ("flood", r#"<feFlood flood-color="red"/>"#, 3.0),
        ("offset", r#"<feOffset dx="1" dy="1"/>"#, 3.0),
        ("blur", r#"<feGaussianBlur stdDeviation="2"/>"#, 12.0),
        (
            "drop shadow",
            r#"<feDropShadow dx="1" dy="1" stdDeviation="1"/>"#,
            12.0,
        ),
        (
            "turbulence x1",
            r#"<feTurbulence baseFrequency="0.05" numOctaves="1"/>"#,
            8.0 + 2.5,
        ),
        (
            "turbulence x4",
            r#"<feTurbulence baseFrequency="0.05" numOctaves="4"/>"#,
            8.0 + 10.0,
        ),
        (
            "morphology r1",
            r#"<feMorphology radius="1"/>"#,
            3.0 + 9.0 / 3.5,
        ),
        (
            "morphology r3",
            r#"<feMorphology radius="3"/>"#,
            3.0 + 49.0 / 3.5,
        ),
        (
            "morphology rx3 ry1",
            r#"<feMorphology radius="3 1"/>"#,
            3.0 + 7.0 * 3.0 / 3.5,
        ),
        (
            "convolve 3x3",
            r#"<feConvolveMatrix order="3" kernelMatrix="1 1 1 1 1 1 1 1 1"/>"#,
            2.0 + 9.0,
        ),
        (
            "convolve 5x5",
            &format!(
                r#"<feConvolveMatrix order="5" kernelMatrix="{}"/>"#,
                "1 ".repeat(25)
            ),
            2.0 + 25.0,
        ),
        (
            "diffuse lighting",
            r#"<feDiffuseLighting><feDistantLight azimuth="1" elevation="30"/></feDiffuseLighting>"#,
            40.0,
        ),
        (
            "specular lighting",
            r#"<feSpecularLighting specularExponent="2"><feDistantLight azimuth="1" elevation="30"/></feSpecularLighting>"#,
            40.0,
        ),
        (
            "displacement map",
            r#"<feDisplacementMap in="SourceGraphic" in2="SourceGraphic" scale="3"/>"#,
            6.0,
        ),
    ];
    for (name, prims, want) in cases {
        let got = weight(prims);
        assert!(
            near(got, want),
            "{name}: charged {got} per pixel, expected {want}"
        );
    }
    // Primitives of one filter add up.
    assert!(near(
        weight(r#"<feFlood/><feGaussianBlur stdDeviation="1"/>"#),
        15.0
    ));
}

#[test]
fn a_filter_region_is_charged_at_most_the_layer_it_is_drawn_into() {
    // A region of 200,000 x 200,000 user units around a 10 x 10 rect: resvg clamps the layer to a
    // window of 25 canvases, and so does the budget.
    let doc = svg(
        r##"<filter id="f" x="-100000" y="-100000" width="200000" height="200000" filterUnits="userSpaceOnUse"><feFlood/></filter><rect width="10" height="10" filter="url(#f)"/>"##,
    );
    let canvas_px = 100.0 * 100.0;
    let got = filter_work(&doc, 1.0);
    assert!((got - 25.0 * canvas_px * 3.0).abs() < 1.0, "{got}");
    // At twice the scale the canvas has four times the pixels.
    let got2 = filter_work(&doc, 2.0);
    assert!((got2 - 4.0 * got).abs() < 1.0, "{got2} vs {got}");
}

#[test]
fn a_filter_is_charged_at_the_size_it_is_drawn_after_transforms() {
    let flood = r#"<feFlood/>"#;
    let at = |transform: &str, side: u32| {
        svg(&format!(
            r##"<filter id="f" x="0" y="0" width="1" height="1">{flood}</filter><g transform="{transform}"><rect width="{side}" height="{side}" filter="url(#f)"/></g>"##
        ))
    };
    let plain = filter_work(&at("scale(1)", 30), 1.0);
    let scaled = filter_work(&at("scale(3)", 10), 1.0);
    assert!((plain - scaled).abs() < 1e-6 * plain, "{plain} vs {scaled}");
    // A rotation keeps the area: a rotated, scaled group costs what the scaled one does.
    let turned = filter_work(&at("rotate(30) scale(3)", 10), 1.0);
    assert!(
        (turned - scaled).abs() < 1e-6 * scaled,
        "{turned} vs {scaled}"
    );
    // The scale the whole picture is drawn at multiplies the area, not the side.
    let doubled = filter_work(&at("scale(3)", 10), 2.0);
    assert!((doubled - 4.0 * scaled).abs() < 1e-6 * doubled);
}

#[test]
fn the_filter_work_limit_is_exact() {
    let one = filter_work(&filtered(r#"<feFlood/>"#), 1.0);
    let n = (MAX_FILTER_WORK / one).floor() as usize;
    let floods = |count: usize| filtered(&"<feFlood/>".repeat(count));
    assert!(
        within_budget(&floods(n), 1.0),
        "{n} floods are the last allowed"
    );
    assert!(!within_budget(&floods(n + 1), 1.0), "{} are over", n + 1);
    // Spread over many filtered elements, the work still adds up (it is one budget per drawing).
    let each = n / 4 + 1;
    let many: String = (0..5)
        .map(|_| r##"<rect width="100" height="100" filter="url(#f)"/>"##)
        .collect();
    let doc = svg(&format!(
        r##"<filter id="f" x="0" y="0" width="1" height="1">{}</filter>{many}"##,
        "<feFlood/>".repeat(each)
    ));
    assert!(
        !within_budget(&doc, 1.0),
        "five elements, each a quarter of the limit"
    );
}

/// 15 live translucent layers of a 4096 px canvas fit a 1 GiB budget (64 MiB each); the 17th, or a
/// mask or clip path in the way, does not.
fn translucent(n: usize, inner: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096" viewBox="0 0 4096 4096">{}</svg>"#,
        wrap(r#"<g opacity="0.9">"#, "</g>", n, inner)
    )
}

const FULL: &str = r#"<rect width="4096" height="4096"/>"#;

#[test]
fn the_live_layer_limit_is_exact() {
    // Sixteen 64 MiB layers are exactly the 1 GiB budget: allowed. Seventeen are not.
    assert!(within_budget(&translucent(16, FULL), 1.0));
    assert!(!within_budget(&translucent(17, FULL), 1.0));
}

#[test]
fn a_mask_or_a_clip_path_needs_a_layer_of_its_own() {
    let masked = r##"<mask id="m"><rect width="4096" height="4096" fill="white"/></mask><rect width="4096" height="4096" mask="url(#m)"/>"##;
    let clipped = r##"<clipPath id="c"><rect width="2048" height="2048"/></clipPath><rect width="4096" height="4096" clip-path="url(#c)"/>"##;
    // N translucent groups + the masked rect's own layer + its mask's layer = N + 2 layers.
    assert!(within_budget(&translucent(14, masked), 1.0), "16 layers");
    assert!(!within_budget(&translucent(15, masked), 1.0), "17 layers");
    // N groups + the clipped rect's layer + its clip path's layer = N + 2 layers.
    assert!(within_budget(&translucent(14, clipped), 1.0), "16 layers");
    assert!(!within_budget(&translucent(15, clipped), 1.0), "17 layers");
}

#[test]
fn what_a_mask_or_a_clip_path_contains_is_charged_too() {
    // Sixteen layers of translucent groups *inside the mask*.
    let in_mask = |n: usize| {
        format!(
            r##"<mask id="m">{}</mask><rect width="4096" height="4096" mask="url(#m)"/>"##,
            wrap(
                r#"<g opacity="0.9">"#,
                "</g>",
                n,
                r#"<rect width="4096" height="4096" fill="white"/>"#
            )
        )
    };
    let doc = |n: usize| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096" viewBox="0 0 4096 4096">{}</svg>"#,
            in_mask(n)
        )
    };
    assert!(within_budget(&doc(2), 1.0));
    assert!(
        !within_budget(&doc(16), 1.0),
        "16 layers inside the mask, plus two"
    );
    // Clip paths whose children are clipped by the one before: layers nest, one per link.
    let nested_clips = |n: usize| {
        let mut defs =
            String::from(r#"<clipPath id="c0"><rect width="4096" height="4096"/></clipPath>"#);
        for i in 1..n {
            defs.push_str(&format!(
                r##"<clipPath id="c{i}"><rect width="4096" height="4096" clip-path="url(#c{})"/></clipPath>"##,
                i - 1
            ));
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096" viewBox="0 0 4096 4096"><defs>{defs}</defs><rect width="4096" height="4096" clip-path="url(#c{})"/></svg>"##,
            n - 1
        )
    };
    assert!(within_budget(&nested_clips(2), 1.0));
    assert!(
        !within_budget(&nested_clips(24), 1.0),
        "a long nesting of clip paths"
    );
}

/// A chain in which each clip path (or mask) is itself clipped (masked) by the one before it: a
/// thousand links wanted 4 GB, and each link is a layer that is alive while the next is drawn.
#[test]
fn a_chain_of_clip_paths_or_masks_costs_a_layer_per_link() {
    let clips = |n: usize, side: u32| {
        let mut defs =
            format!(r#"<clipPath id="c0"><rect width="{side}" height="{side}"/></clipPath>"#);
        for i in 1..n {
            defs.push_str(&format!(
                r##"<clipPath id="c{i}" clip-path="url(#c{})"><rect width="{side}" height="{side}"/></clipPath>"##,
                i - 1
            ));
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}" viewBox="0 0 {side} {side}"><defs>{defs}</defs><rect width="{side}" height="{side}" clip-path="url(#c{})"/></svg>"##,
            n - 1
        )
    };
    let masks = |n: usize, side: u32| {
        let mut defs =
            format!(r#"<mask id="m0"><rect width="{side}" height="{side}" fill="white"/></mask>"#);
        for i in 1..n {
            defs.push_str(&format!(
                r##"<mask id="m{i}" mask="url(#m{})"><rect width="{side}" height="{side}" fill="white"/></mask>"##,
                i - 1
            ));
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}" viewBox="0 0 {side} {side}"><defs>{defs}</defs><rect width="{side}" height="{side}" mask="url(#m{})"/></svg>"##,
            n - 1
        )
    };
    // The documents of the bypass: 1000 links on an ordinary 800 px canvas (2 MB a layer).
    assert!(!within_budget(&clips(1000, 800), 1.0), "1000 clip paths");
    assert!(!within_budget(&masks(1000, 800), 1.0), "1000 masks");
    // The boundary on a 4096 px canvas (64 MiB a layer, 1 GiB in all): a handful of links are
    // ordinary, a long chain is not.
    assert!(within_budget(&clips(3, 4096), 1.0));
    assert!(within_budget(&masks(3, 4096), 1.0));
    assert!(!within_budget(&clips(24, 4096), 1.0));
    assert!(!within_budget(&masks(24, 4096), 1.0));
    // And a chain of a hundred on a small canvas is a few megabytes, and draws.
    assert!(within_budget(&clips(100, 100), 1.0));
    assert!(within_budget(&masks(100, 100), 1.0));
}

#[test]
fn what_a_pattern_or_an_embedded_svg_contains_is_charged_too() {
    let heavy = |n: usize| {
        wrap(
            r#"<g opacity="0.9">"#,
            "</g>",
            n,
            r#"<rect width="4096" height="4096"/>"#,
        )
    };
    let patterned = |n: usize| {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096" viewBox="0 0 4096 4096"><pattern id="p" width="4096" height="4096" patternUnits="userSpaceOnUse">{}</pattern><rect width="4096" height="4096" fill="url(#p)"/></svg>"##,
            heavy(n)
        )
    };
    assert!(within_budget(&patterned(2), 1.0));
    assert!(
        !within_budget(&patterned(20), 1.0),
        "20 layers inside a pattern tile"
    );

    let inner = |n: usize| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096" viewBox="0 0 4096 4096">{}</svg>"#,
            heavy(n)
        )
    };
    let embedded = |n: usize| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="4096" viewBox="0 0 4096 4096"><image width="4096" height="4096" href="data:image/svg+xml;base64,{}"/></svg>"#,
            b64(inner(n).as_bytes())
        )
    };
    assert!(within_budget(&embedded(2), 1.0));
    assert!(
        !within_budget(&embedded(20), 1.0),
        "20 layers inside an embedded svg"
    );
}

#[test]
fn a_layer_is_never_charged_more_than_25_canvases() {
    // An enormous translucent rect on a small canvas: resvg clamps the layer, and so does the budget.
    let doc = svg(r#"<g opacity="0.5"><rect width="1000000" height="1000000"/></g>"#);
    assert!(
        within_budget(&doc, 8.0),
        "800 x 800 px canvas: 25 clamped layers are 64 MB"
    );
}

/// The budget walks the drawn tree to a depth of four times the XML depth limit, as a backstop.
/// The deepest tree usvg will build from a `use` chain is far below that, so this cap is never the
/// first limit hit; the test says so, and notes where usvg itself gives up.
#[test]
fn the_budgets_depth_cap_is_beyond_what_usvg_will_build() {
    let chain = |hops: usize| {
        let mut s =
            String::from(r#"<defs><g id="g0" opacity="0.9"><rect width="10" height="10"/></g>"#);
        for i in 1..=hops {
            s.push_str(&format!(
                r##"<g id="g{i}" opacity="0.9"><use href="#g{}"/></g>"##,
                i - 1
            ));
        }
        s.push_str(&format!(r##"</defs><use href="#g{hops}"/>"##));
        svg(&s)
    };
    let depth = |doc: String| {
        big(move || {
            load_tree(doc.as_bytes(), None, |t| {
                fn d(g: &usvg::Group) -> usize {
                    1 + g
                        .children()
                        .iter()
                        .map(|n| {
                            if let usvg::Node::Group(c) = n {
                                d(c)
                            } else {
                                0
                            }
                        })
                        .max()
                        .unwrap_or(0)
                }
                Ok(d(t.root()))
            })
        })
    };
    // Two groups per hop (the `use` and its target).
    assert_eq!(depth(chain(100)), Ok(203));
    assert!(depth(chain(300)).unwrap() < MAX_XML_DEPTH * 4);
    assert_eq!(
        depth(chain(800)),
        Err(SvgFail::Invalid),
        "usvg refuses a chain this long itself"
    );
}

#[test]
fn embedded_rasters_are_charged_by_their_pixels() {
    use super::super::hostile_tests::png_header_only;
    let image = |w: u32, h: u32| {
        format!(
            r#"<image width="10" height="10" href="data:image/png;base64,{}"/>"#,
            b64(&png_header_only(w, h))
        )
    };
    let limit = MAX_EMBEDDED_RASTER_PX as u32;
    assert_eq!(limit, 8192 * 8192);
    assert!(
        within_budget(&svg(&image(8192, 8192)), 1.0),
        "exactly the limit is allowed"
    );
    assert!(
        !within_budget(&svg(&image(8192, 8193)), 1.0),
        "one row more is not"
    );
    // They add up across images.
    assert!(within_budget(&svg(&image(6000, 6000)), 1.0));
    assert!(!within_budget(
        &svg(&format!("{}{}", image(6000, 6000), image(6000, 6000))),
        1.0
    ));
    // Many small ones too.
    assert!(
        !within_budget(&svg(&image(2000, 2000).repeat(17)), 1.0),
        "17 x 4 MP"
    );
    assert!(
        within_budget(&svg(&image(2000, 2000).repeat(16)), 1.0),
        "16 x 4 MP"
    );
}

#[test]
fn the_tree_node_cap_is_beyond_what_the_xml_limits_let_through() {
    // The budget's own cap on tree nodes can never be the first limit hit: a document that could
    // have that many drawn elements is refused earlier, by the XML node limit and the `use`
    // expansion limit. (If those are ever raised past this, this test says so.)
    assert!(MAX_TREE_NODES as u64 > MAX_EXPANDED_ELEMENTS);
    assert!(MAX_TREE_NODES as u64 > u64::from(MAX_XML_NODES));
}
