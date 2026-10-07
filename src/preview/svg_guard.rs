// Guard rails for SVG that konoma did not write (a file the user opens, an image a Markdown
// document points at, an SVG embedded in an Office document).
//
// usvg and resvg are written for trusted input. A hostile SVG can make them (1) recurse until the
// stack overflows (an abort — `catch_unwind` cannot catch it), (2) expand `<use>` chains
// exponentially, (3) allocate a layer per nesting level of a translucent group, or (4) spend
// minutes in a filter primitive. Every path that rasterizes an SVG goes through `load_tree` and then
// `check_render_budget` in this file, so the limits live in exactly one place.
//
// How the limits were chosen is in the constants' doc comments; the figures come from measuring
// konoma's own mermaid and math SVGs (the largest legitimate inputs konoma generates) and real
// icon/diagram files against what a crafted file needs to hurt.

use std::io::Read;
use std::path::PathBuf;

use resvg::usvg;
use usvg::roxmltree;

/// Largest SVG (after gunzip for an svgz) konoma will parse, in bytes. Real files: konoma's mermaid
/// output peaks around 1 MB for a 300-node diagram and a hand-drawn illustration with embedded
/// fonts or a map is typically a few MB; 32 MiB leaves a wide margin while keeping the element
/// count (and so parse time and memory) bounded for every other limit below.
pub(crate) const MAX_SVG_BYTES: usize = 32 << 20;

/// Deepest element nesting accepted. usvg's converter and resvg's renderer recurse once per level
/// and overflow a 2 MiB thread stack at about 500 levels (measured: 400 `<g opacity>` levels pass,
/// 1000 abort). Real files nest well under 40 levels (mermaid and math output: see the
/// `legitimate_generated_svgs_stay_far_below_the_limits` test); 256 is several times that and
/// still safe because deep documents are processed on a large stack (`STACK_BYTES`).
pub(crate) const MAX_XML_DEPTH: usize = 256;

/// Documents deeper than this are handled on a thread with a large stack instead of the caller's.
/// 64 levels is a sixth of what even a 2 MiB stack survives.
const INLINE_DEPTH: usize = 64;

/// Stack for the dedicated thread (virtual memory only; untouched pages cost nothing).
const STACK_BYTES: usize = 128 << 20;

/// Upper bound on the number of XML nodes roxmltree may build.
const MAX_XML_NODES: u32 = 2_000_000;

/// Upper bound on the number of elements after `<use>` expansion. One `<use>` repeated through
/// 22 levels of nesting is 4 million elements and took 14 s to convert (measured). A map with tens
/// of thousands of `<use>` of a small symbol is a legitimate few hundred thousand.
const MAX_EXPANDED_ELEMENTS: u64 = 1_000_000;

/// Bytes of pixel layers alive at once along one path of the tree (each translucent / clipped /
/// masked / filtered group needs a layer). 1 GiB = 16 full-size layers of a 4096x4096 canvas, or
/// several hundred for an ordinary 800 px diagram.
const MAX_LIVE_LAYER_BYTES: f64 = (1u64 << 30) as f64;

/// Total estimated filter work, in units of about 7 ns of resvg time each (one pixel through a
/// Gaussian blur is 12 units). Measured with `calibrate_filter_costs` (release build): a blur, offset
/// or turbulence octave costs 40-100 ns per pixel at any radius, a morphology costs about
/// `(2r+1)^2 / 3.5` units per pixel (r = 8 px: 0.4 us; r = 32 px: 8 us), and turbulence grows with
/// its octave count. 6e8 units is about 4 s of worst-case work on a worker thread: a drop shadow on
/// an 800 px icon is under 1e7, and a full-canvas blur at the 4096 px maximum is 2e8.
const MAX_FILTER_WORK: f64 = 6.0e8;

/// Total pixels of raster images embedded in one SVG (resvg decodes each to RGBA at render time).
const MAX_EMBEDDED_RASTER_PX: f64 = 64.0 * 1024.0 * 1024.0;

/// Cap on the number of tree nodes visited by the budget check.
const MAX_TREE_NODES: usize = 3_000_000;

/// Largest external image file an SVG may pull in through `<image href="file">`.
const MAX_EXTERNAL_IMAGE_BYTES: u64 = 64 << 20;

/// Why the structural checks refused an SVG. Kept distinct so the `<image>` resolver can tell
/// "this is not XML" (raster data) from "this is XML we refuse"; converted to [`SvgFail`] for the
/// user.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Reject {
    NotXml,
    TooLarge,
    TooDeep,
    TooManyElements,
    TooManyNodes,
}

/// Why an SVG could not be drawn, in terms the user can be told. Produced by the pre-checks here,
/// by the render budget, and by the supervisor of the drawing process (`svg_proc`: time, memory,
/// a crash of the process).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SvgFail {
    /// Elements nested deeper than [`MAX_XML_DEPTH`].
    TooDeep,
    /// Larger than [`MAX_SVG_BYTES`] (after gunzip for an svgz).
    TooLarge,
    /// Too many elements (`<use>` expansion, or the XML node limit).
    TooComplex,
    /// The converted tree needs more drawing work or layer memory than the budget allows.
    TooHeavy,
    /// The drawing process ran past its wall-clock limit and was stopped.
    Timeout,
    /// The drawing process grew past its memory limit and was stopped.
    Memory,
    /// The drawing process died by itself (a stack overflow or an allocation failure).
    Crashed,
    /// Not an SVG, or one the renderer cannot parse.
    Invalid,
}

impl SvgFail {
    /// Stable one-byte code used on the wire between the parent and the drawing process.
    pub(crate) fn code(self) -> u8 {
        match self {
            SvgFail::TooDeep => 1,
            SvgFail::TooLarge => 2,
            SvgFail::TooComplex => 3,
            SvgFail::TooHeavy => 4,
            SvgFail::Timeout => 5,
            SvgFail::Memory => 6,
            SvgFail::Crashed => 7,
            SvgFail::Invalid => 8,
        }
    }

    pub(crate) fn from_code(c: u8) -> Option<SvgFail> {
        Some(match c {
            1 => SvgFail::TooDeep,
            2 => SvgFail::TooLarge,
            3 => SvgFail::TooComplex,
            4 => SvgFail::TooHeavy,
            5 => SvgFail::Timeout,
            6 => SvgFail::Memory,
            7 => SvgFail::Crashed,
            8 => SvgFail::Invalid,
            _ => return None,
        })
    }

    /// The message the user sees (a catalog entry: the sentence says what the file did, never
    /// blames the terminal).
    pub fn msg(self) -> crate::i18n::Msg {
        use crate::i18n::Msg;
        match self {
            SvgFail::TooDeep => Msg::SvgTooDeep,
            SvgFail::TooLarge => Msg::SvgTooLarge,
            SvgFail::TooComplex => Msg::SvgTooComplex,
            SvgFail::TooHeavy => Msg::SvgTooHeavy,
            SvgFail::Timeout => Msg::SvgTimeout,
            SvgFail::Memory => Msg::SvgMemory,
            SvgFail::Crashed => Msg::SvgCrashed,
            SvgFail::Invalid => Msg::SvgInvalid,
        }
    }

    /// A short English phrase for places that carry a plain reason string (the media diff's
    /// per-side failure), not a translated message.
    pub fn reason(self) -> &'static str {
        match self {
            SvgFail::TooDeep => "svg elements nested too deeply",
            SvgFail::TooLarge => "svg too large",
            SvgFail::TooComplex => "svg has too many elements",
            SvgFail::TooHeavy => "svg needs too much drawing work",
            SvgFail::Timeout => "svg took too long to draw",
            SvgFail::Memory => "svg needed too much memory to draw",
            SvgFail::Crashed => "svg renderer stopped while drawing it",
            SvgFail::Invalid => "invalid svg",
        }
    }

    /// Every reason.
    pub(crate) const ALL: [SvgFail; 8] = [
        SvgFail::TooDeep,
        SvgFail::TooLarge,
        SvgFail::TooComplex,
        SvgFail::TooHeavy,
        SvgFail::Timeout,
        SvgFail::Memory,
        SvgFail::Crashed,
        SvgFail::Invalid,
    ];

    /// The stable text form used where a failure travels as a string (`ImageFailure::code`).
    pub(crate) fn code_text(self) -> &'static str {
        match self {
            SvgFail::TooDeep => "svg-too-deep",
            SvgFail::TooLarge => "svg-too-large",
            SvgFail::TooComplex => "svg-too-complex",
            SvgFail::TooHeavy => "svg-too-heavy",
            SvgFail::Timeout => "svg-timeout",
            SvgFail::Memory => "svg-memory",
            SvgFail::Crashed => "svg-crashed",
            SvgFail::Invalid => "svg-invalid",
        }
    }
}

impl From<Reject> for SvgFail {
    fn from(r: Reject) -> SvgFail {
        match r {
            Reject::NotXml => SvgFail::Invalid,
            Reject::TooLarge => SvgFail::TooLarge,
            Reject::TooDeep => SvgFail::TooDeep,
            Reject::TooManyElements | Reject::TooManyNodes => SvgFail::TooComplex,
        }
    }
}

/// Decompress an svgz, bounded. `Err` if the output would exceed `MAX_SVG_BYTES`.
fn gunzip_bounded(data: &[u8]) -> Result<Vec<u8>, Reject> {
    let mut out = Vec::new();
    let mut dec = flate2::read::GzDecoder::new(data).take(MAX_SVG_BYTES as u64 + 1);
    dec.read_to_end(&mut out).map_err(|_| Reject::NotXml)?;
    if out.len() > MAX_SVG_BYTES {
        return Err(Reject::TooLarge);
    }
    Ok(out)
}

/// The decoded bytes of an SVG (gunzipped when it is an svgz), size-checked.
fn plain_bytes(data: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, Reject> {
    if data.starts_with(&[0x1f, 0x8b]) {
        return gunzip_bounded(data).map(std::borrow::Cow::Owned);
    }
    if data.len() > MAX_SVG_BYTES {
        return Err(Reject::TooLarge);
    }
    Ok(std::borrow::Cow::Borrowed(data))
}

fn parse_xml(text: &str) -> Result<roxmltree::Document<'_>, Reject> {
    let opt = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_XML_NODES,
        ..Default::default()
    };
    roxmltree::Document::parse_with_options(text, opt).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => Reject::TooManyNodes,
        _ => Reject::NotXml,
    })
}

/// Maximum element nesting depth of `doc` (iterative: roxmltree stores nodes in document order, so
/// a parent always precedes its children).
fn xml_depth(doc: &roxmltree::Document) -> usize {
    let mut depth: Vec<u32> = vec![0; doc.descendants().count() + 1];
    let mut max = 0u32;
    for n in doc.descendants() {
        let d = match n.parent() {
            Some(p) => depth[p.id().get_usize()] + u32::from(n.is_element()),
            None => 0,
        };
        depth[n.id().get_usize()] = d;
        max = max.max(d);
    }
    max as usize
}

fn href_target<'a>(n: roxmltree::Node<'a, 'a>) -> Option<&'a str> {
    n.attributes()
        .find(|a| a.name() == "href")
        .map(|a| a.value().trim())
        .and_then(|v| v.strip_prefix('#'))
}

/// Number of elements `<use>` expansion would produce, saturating at `MAX_EXPANDED_ELEMENTS + 1`.
/// `None` means a reference chain too long to follow (rejected like an over-budget count).
fn expanded_elements(doc: &roxmltree::Document) -> Option<u64> {
    use std::collections::HashMap;
    let mut by_id: HashMap<&str, roxmltree::NodeId> = HashMap::new();
    let mut any_use = false;
    for n in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = n.attribute("id") {
            by_id.entry(id).or_insert(n.id());
        }
        any_use |= n.tag_name().name() == "use";
    }
    let total = doc.descendants().filter(|n| n.is_element()).count() as u64;
    if !any_use {
        return Some(total);
    }

    struct Ctx<'a, 'i> {
        doc: &'a roxmltree::Document<'i>,
        by_id: HashMap<&'a str, roxmltree::NodeId>,
        memo: HashMap<roxmltree::NodeId, u64>,
        visiting: std::collections::HashSet<roxmltree::NodeId>,
    }
    fn walk(cx: &mut Ctx, id: roxmltree::NodeId, level: usize) -> Option<u64> {
        if level > 1024 {
            return None;
        }
        if let Some(&v) = cx.memo.get(&id) {
            return Some(v);
        }
        if !cx.visiting.insert(id) {
            return Some(0); // a reference cycle: usvg skips it
        }
        let node = cx.doc.get_node(id)?;
        let mut sum = 1u64;
        if node.tag_name().name() == "use" {
            if let Some(&t) = href_target(node).and_then(|h| cx.by_id.get(h)) {
                sum = sum.saturating_add(walk(cx, t, level + 1)?);
            }
        }
        for c in node.children().filter(|c| c.is_element()) {
            sum = sum.saturating_add(walk(cx, c.id(), level + 1)?);
            if sum > MAX_EXPANDED_ELEMENTS {
                break;
            }
        }
        cx.visiting.remove(&id);
        let sum = sum.min(MAX_EXPANDED_ELEMENTS + 1);
        cx.memo.insert(id, sum);
        Some(sum)
    }
    let mut cx = Ctx {
        doc,
        by_id: by_id.into_iter().collect(),
        memo: HashMap::new(),
        visiting: Default::default(),
    };
    let root = doc.root_element().id();
    walk(&mut cx, root, 0)
}

/// Structural checks on the XML of an SVG: size, node count, nesting depth and `<use>` expansion.
fn check_xml(doc: &roxmltree::Document) -> Result<(), Reject> {
    let depth = xml_depth(doc);
    if depth > MAX_XML_DEPTH {
        return Err(Reject::TooDeep);
    }
    match expanded_elements(doc) {
        Some(n) if n <= MAX_EXPANDED_ELEMENTS => Ok(()),
        _ => Err(Reject::TooManyElements),
    }
}

/// The deepest element nesting of `text`, found by a flat scan of the markup (no recursion, no
/// allocation) *before* roxmltree sees it — roxmltree recurses once per level and overflows even a
/// 128 MiB stack on a few hundred thousand levels. The scan follows the XML rules that decide where
/// a tag starts and ends (comments, CDATA, processing instructions, DOCTYPE, quoted attribute
/// values), so content that merely looks like tags cannot move the answer. Stops and refuses as
/// soon as the nesting passes `MAX_XML_DEPTH`.
///
/// A DOCTYPE entity whose value contains markup is refused outright: roxmltree would expand it
/// into elements the scan never saw. (SVGs from old editors declare namespace entities — plain
/// text — which pass.)
fn scan_depth(text: &[u8]) -> Result<usize, Reject> {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        hay[from.min(hay.len())..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    }
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while let Some(lt) = text[i..].iter().position(|&b| b == b'<').map(|p| p + i) {
        let rest = &text[lt..];
        if rest.starts_with(b"<!--") {
            i = find(text, lt + 4, b"-->").ok_or(Reject::NotXml)? + 3;
        } else if rest.starts_with(b"<![CDATA[") {
            i = find(text, lt + 9, b"]]>").ok_or(Reject::NotXml)? + 3;
        } else if rest.starts_with(b"<?") {
            i = find(text, lt + 2, b"?>").ok_or(Reject::NotXml)? + 2;
        } else if rest.starts_with(b"<!") {
            // DOCTYPE: skip to the closing `>` outside brackets and quotes.
            let (mut j, mut bracket, mut quote) = (lt + 2, 0i32, 0u8);
            loop {
                let &c = text.get(j).ok_or(Reject::NotXml)?;
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    } else if c == b'<' {
                        return Err(Reject::TooDeep);
                    }
                } else {
                    match c {
                        b'"' | b'\'' => quote = c,
                        b'[' => bracket += 1,
                        b']' => bracket -= 1,
                        b'>' if bracket <= 0 => break,
                        b'<' if text[j..].starts_with(b"<!--") => {
                            j = find(text, j + 4, b"-->").ok_or(Reject::NotXml)? + 2;
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            i = j + 1;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(text, lt + 2, b">").ok_or(Reject::NotXml)? + 1;
        } else {
            // A start tag: find its `>` outside quoted attribute values.
            let (mut j, mut quote) = (lt + 1, 0u8);
            loop {
                let &c = text.get(j).ok_or(Reject::NotXml)?;
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                j += 1;
            }
            let self_closing = text[j - 1] == b'/';
            max = max.max(depth + 1);
            if max > MAX_XML_DEPTH {
                return Err(Reject::TooDeep);
            }
            if !self_closing {
                depth += 1;
            }
            i = j + 1;
        }
    }
    Ok(max)
}

/// Run `f` on a thread with a large stack when `deep`, otherwise inline, reporting why it failed. A panic on the helper thread is `Invalid`.
fn with_stack_why<T: Send>(
    deep: bool,
    f: impl FnOnce() -> Result<T, SvgFail> + Send,
) -> Result<T, SvgFail> {
    if !deep {
        return f();
    }
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(STACK_BYTES)
            .spawn_scoped(s, move || {
                BIG_STACK.with(|b| b.set(true));
                f()
            })
            .map_err(|_| SvgFail::Invalid)?
            .join()
            .unwrap_or(Err(SvgFail::Invalid))
    })
}

thread_local! {
    /// This thread was started by `with_stack` and has the large stack.
    static BIG_STACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Depth of nested SVG-in-SVG `<image>` parsing on this thread.
    static NESTING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Whether an SVG document nested inside the one being drawn (a `data:` URL, or a file an
/// `<image href>` points at) may be handed to usvg: the same flat and structural checks as the
/// top-level document, one nesting level deeper, on the same thread.
fn nested_svg_acceptable(data: &[u8]) -> bool {
    let nested = NESTING.with(|n| n.get());
    if nested >= 3 {
        return false;
    }
    match plain_bytes(data) {
        Err(_) => false,
        Ok(bytes) => match scan_depth(&bytes) {
            Err(Reject::TooDeep | Reject::TooLarge) => false,
            // Not markup we can follow: not ours to judge (raster data under text/plain).
            Err(_) => true,
            // A nested document deeper than the inline limit needs the large stack.
            Ok(d) if d > INLINE_DEPTH && !BIG_STACK.with(|b| b.get()) => false,
            Ok(_) => match std::str::from_utf8(&bytes).map(parse_xml) {
                Ok(Ok(doc)) => check_xml(&doc).is_ok(),
                // Not XML: garbage. Not ours to judge.
                _ => true,
            },
        },
    }
}

/// Run `f` one SVG-nesting level deeper. The level is restored even if `f` panics (the drawing
/// process catches the panic and goes on to the next drawing on this thread).
fn nested<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(u32);
    impl Drop for Restore {
        fn drop(&mut self) {
            NESTING.with(|n| n.set(self.0));
        }
    }
    let level = NESTING.with(|n| n.get());
    NESTING.with(|n| n.set(level + 1));
    let _restore = Restore(level);
    f()
}

/// Does `data` look like a raster image (so it is not an SVG document)? Only the formats usvg
/// itself sniffs.
fn looks_raster(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(&[0xff, 0xd8, 0xff])
        || data.starts_with(b"GIF8")
        || (data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP")
}

/// `usvg::Options` for untrusted input: the shared font database, a data-URL resolver that runs
/// embedded SVG through the same XML checks, and a file resolver that refuses anything that is not
/// a bounded regular file (`/dev/zero` as an image would otherwise be read forever) and runs a
/// referenced SVG file through those same checks.
///
/// The drawing of an untrusted SVG happens in a process of its own (`svg_proc`), so what these
/// checks protect is that process from dying on the common hostile shapes, with a reason, instead
/// of by its stack overflowing.
pub(crate) fn options(resources_dir: Option<PathBuf>) -> usvg::Options<'static> {
    let default_data = usvg::ImageHrefResolver::default_data_resolver();
    let default_string = usvg::ImageHrefResolver::default_string_resolver();
    // The file resolver hands an SVG file to the data resolver (`image/svg+xml`), so the second
    // closure needs its own copy of the default.
    let data_for_files = usvg::ImageHrefResolver::default_data_resolver();
    usvg::Options {
        resources_dir,
        fontdb: super::svg::shared_fontdb(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime, data, opts| {
                if mime == "image/svg+xml" || mime == "text/plain" {
                    if !nested_svg_acceptable(&data) {
                        return None;
                    }
                    return nested(|| default_data(mime, data, opts));
                }
                default_data(mime, data, opts)
            }),
            resolve_string: Box::new(move |href, opts| {
                let path = opts.get_abs_path(std::path::Path::new(href));
                match std::fs::metadata(&path) {
                    Ok(m) if m.is_file() && m.len() <= MAX_EXTERNAL_IMAGE_BYTES => {}
                    _ => return None,
                }
                // A referenced SVG is checked like an embedded one (it used to reach usvg
                // unchecked: 1000 nested groups in a file next to the SVG aborted the process).
                let bytes = std::fs::read(&path).ok()?;
                if looks_raster(&bytes) {
                    return default_string(href, opts);
                }
                if !nested_svg_acceptable(&bytes) {
                    return None;
                }
                nested(|| data_for_files("image/svg+xml", std::sync::Arc::new(bytes), opts))
            }),
        },
        ..usvg::Options::default()
    }
}

/// The flat checks that need no XML parser: the size and the nesting depth. This is the fast
/// refusal made before a drawing process is started. A compressed document is only size-checked
/// here (the file's own length): decompressing it is the drawing process's job, where a bomb costs
/// that process's memory and not this one's.
pub(crate) fn precheck(data: &[u8]) -> Result<(), SvgFail> {
    if data.len() > MAX_SVG_BYTES {
        return Err(SvgFail::TooLarge);
    }
    if !data.starts_with(&[0x1f, 0x8b]) {
        scan_depth(data)?;
    }
    Ok(())
}

/// Parse `data` into a usvg tree after the structural checks, calling `then` with the tree on a
/// stack large enough for it. `Err` says why the SVG was refused or did not parse.
///
/// This is the only place konoma turns untrusted SVG bytes into a `usvg::Tree`.
pub(crate) fn load_tree<T: Send>(
    data: &[u8],
    resources_dir: Option<PathBuf>,
    then: impl FnOnce(usvg::Tree) -> Result<T, SvgFail> + Send,
) -> Result<T, SvgFail> {
    // roxmltree recurses on nesting, so depth is bounded by a flat scan *before* parsing, and
    // the stack decision comes from it (on the unzipped text for an svgz).
    let deep = scan_depth(&plain_bytes(data)?)? > INLINE_DEPTH;
    with_stack_why(deep, || {
        let bytes = plain_bytes(data)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| SvgFail::Invalid)?;
        let doc = parse_xml(text)?;
        check_xml(&doc)?;
        let opt = options(resources_dir);
        let tree = usvg::Tree::from_xmltree(&doc, &opt).map_err(|_| SvgFail::Invalid)?;
        then(tree)
    })
}

/// Walk `tree` the way resvg will render it at `scale` (device px per user unit) and refuse it when
/// the work or memory is out of proportion: peak pixel layers alive along one path, estimated
/// filter work, embedded raster pixels, and node count.
pub(crate) fn check_render_budget(tree: &usvg::Tree, scale: f32) -> bool {
    let size = tree.size();
    let canvas_px = (f64::from(size.width() * scale) * f64::from(size.height() * scale)).max(1.0);
    let mut cx = Budget {
        scale: f64::from(scale),
        canvas_px,
        filter_work: 0.0,
        raster_px: 0.0,
        nodes: 0,
    };
    // The filter work is checked as it is added up (`Budget::filter`), so there is no total to
    // check again here.
    cx.group(tree.root(), 0.0, 0)
}

struct Budget {
    scale: f64,
    /// Pixels of the canvas being drawn (device px).
    canvas_px: f64,
    filter_work: f64,
    raster_px: f64,
    nodes: usize,
}

impl Budget {
    /// Device pixels of a layer for `g` — resvg clamps layers to a window around the canvas, so a
    /// huge bounding box costs at most 25x the canvas.
    fn layer_px(&self, g: &usvg::Group) -> f64 {
        let b = g.abs_layer_bounding_box();
        let px = f64::from(b.width()) * f64::from(b.height()) * self.scale * self.scale;
        px.min(self.canvas_px * 25.0)
    }

    fn group(&mut self, g: &usvg::Group, live: f64, level: usize) -> bool {
        if level > MAX_XML_DEPTH * 4 {
            return false;
        }
        let mut live = live;
        if g.should_isolate() {
            let layer = self.layer_px(g);
            live += layer * 4.0;
            if live > MAX_LIVE_LAYER_BYTES {
                return false;
            }
            for f in g.filters() {
                if !self.filter(f, g, layer) {
                    return false;
                }
            }
            // A mask is rendered into a pixmap of its own, and so is a clip path.
            if g.mask().is_some() || g.clip_path().is_some() {
                live += layer * 4.0;
                if live > MAX_LIVE_LAYER_BYTES {
                    return false;
                }
            }
            // A mask is clipped by its own mask, a clip path by its own clip path, and so on: each
            // link of such a chain is drawn into a layer of its own while the ones before it are
            // still alive, so a long chain costs a layer per link (a chain of a thousand clip
            // paths wanted 4 GB).
            let mut mask = g.mask();
            let mut chain_live = live;
            while let Some(m) = mask {
                if !self.group(m.root(), chain_live, level + 1) {
                    return false;
                }
                mask = m.mask();
                if mask.is_some() {
                    chain_live += layer * 4.0;
                    if chain_live > MAX_LIVE_LAYER_BYTES {
                        return false;
                    }
                }
            }
            let mut clip = g.clip_path();
            let mut chain_live = live;
            while let Some(c) = clip {
                if !self.group(c.root(), chain_live, level + 1) {
                    return false;
                }
                clip = c.clip_path();
                if clip.is_some() {
                    chain_live += layer * 4.0;
                    if chain_live > MAX_LIVE_LAYER_BYTES {
                        return false;
                    }
                }
            }
        }
        for n in g.children() {
            self.nodes += 1;
            if self.nodes > MAX_TREE_NODES {
                return false;
            }
            let ok = match n {
                usvg::Node::Group(c) => self.group(c, live, level + 1),
                usvg::Node::Image(img) => self.image(img, live, level),
                _ => true,
            };
            if !ok {
                return false;
            }
            let mut sub_ok = true;
            n.subroots(|r| {
                if sub_ok {
                    sub_ok = self.group(r, live, level + 1);
                }
            });
            if !sub_ok {
                return false;
            }
        }
        true
    }

    fn image(&mut self, img: &usvg::Image, live: f64, level: usize) -> bool {
        match img.kind() {
            usvg::ImageKind::SVG(t) => self.group(t.root(), live, level + 1),
            _ => {
                let s = img.size();
                self.raster_px += f64::from(s.width()) * f64::from(s.height());
                self.raster_px <= MAX_EMBEDDED_RASTER_PX
            }
        }
    }

    fn filter(&mut self, f: &usvg::filter::Filter, g: &usvg::Group, layer_px: f64) -> bool {
        // The filter region in device px, never more than the layer it is drawn into.
        let r = f.rect();
        let ts = g.abs_transform();
        let user_scale = f64::from((ts.sx * ts.sy - ts.kx * ts.ky).abs()).sqrt() * self.scale;
        let region =
            (f64::from(r.width()) * f64::from(r.height()) * user_scale * user_scale).min(layer_px);
        for p in f.primitives() {
            use usvg::filter::Kind;
            let per_px = match p.kind() {
                Kind::Turbulence(t) => 8.0 + 2.5 * f64::from(t.num_octaves()),
                Kind::Morphology(m) => {
                    let rx = f64::from(m.radius_x().get()) * user_scale;
                    let ry = f64::from(m.radius_y().get()) * user_scale;
                    3.0 + (2.0 * rx + 1.0) * (2.0 * ry + 1.0) / 3.5
                }
                Kind::ConvolveMatrix(c) => {
                    2.0 + f64::from(c.matrix().columns() * c.matrix().rows())
                }
                Kind::DiffuseLighting(_) | Kind::SpecularLighting(_) => 40.0,
                Kind::GaussianBlur(_) | Kind::DropShadow(_) => 12.0,
                Kind::DisplacementMap(_) => 6.0,
                _ => 3.0,
            };
            self.filter_work += region * per_px;
            if self.filter_work > MAX_FILTER_WORK {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "svg_guard_tests.rs"]
mod tests;
