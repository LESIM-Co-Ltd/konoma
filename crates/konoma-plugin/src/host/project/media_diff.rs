//! Shared media diff computation; terminal materialization and untrusted SVG supervision are host policy.
use super::md_diff::DiffBaseline;
use konoma_host_core::project::DiffTarget;
use konoma_widgets::media_diff::MediaDiffSide as KeySide;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, mpsc::Sender, Arc};

pub type MediaClassifier = dyn Fn(&Path, Option<&[u8]>) -> DiffTarget + Send + Sync;
pub type RasterizeSvg = fn(
    &[u8],
    &Path,
    u32,
    &dyn Fn() -> bool,
) -> Result<image::DynamicImage, konoma_svg::svg::RasterError>;
pub type MediaDiffWant = (PathBuf, u32, (u32, u32));
const FORMAT_HEAD: usize = 512;

pub use konoma_widgets::media_diff::{
    draw_media_diff, media_raster_box, MediaDiffDrawValues, MediaDiffRenderState, MediaDiffWords,
    MediaSideView,
};

/// Project an adopted side into drawing metadata. Pixels and cache ownership stay with the host.
pub fn media_side_view(side: &MediaDiffSide) -> MediaSideView {
    match side {
        MediaDiffSide::Picture(p) => MediaSideView::Picture {
            key: p.cache_key.to_string_lossy().into_owned(),
            natural_px: p.natural_px,
            bytes: p.bytes,
            page_count: p.page_count,
        },
        MediaDiffSide::Absent => MediaSideView::Absent,
        MediaDiffSide::Failed { reason } => MediaSideView::Failed {
            reason: reason.clone(),
        },
        MediaDiffSide::PageMissing => MediaSideView::PageMissing,
    }
}

#[derive(Clone, Copy)]
pub struct MediaDiffStateWords<'a> {
    pub computing: &'a str,
    pub no_changes: &'a str,
    pub summary: super::diff_frame::BinarySummaryWords<'a>,
}

/// Resolve every display state through one adapter, including terminals without pictures.
pub fn media_diff_draw_state(
    outcome: Option<&MediaDiffOutcome>,
    pictures: bool,
    words: MediaDiffStateWords<'_>,
    format_size: impl Fn(u64) -> String,
) -> MediaDiffRenderState {
    let Some(outcome) = outcome else {
        return MediaDiffRenderState::Centered(words.computing.to_string());
    };
    if let (
        true,
        MediaDiffOutcome::Ready {
            same_bytes,
            old,
            new,
            ..
        },
    ) = (pictures, outcome)
    {
        return MediaDiffRenderState::Ready {
            same_bytes: *same_bytes,
            old: media_side_view(old),
            new: media_side_view(new),
        };
    }
    let summary = super::diff_frame::binary_summary_values(outcome)
        .and_then(|values| super::diff_frame::binary_summary(values, words.summary, format_size));
    MediaDiffRenderState::Centered(summary.unwrap_or_else(|| words.no_changes.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaBase {
    Head,
    JjParent,
    FollowStart,
}

pub use konoma_host_core::project::MediaDiffKind;

/// One side's fully-decoded picture, still carrying its pixel data — only ever lives inside
/// [`MediaDiffComputed`] on its way from the worker thread to `App::apply_media_diff`, which moves
/// the pixels into `md_image_cache` (under a `media_diff_url` key) and keeps only a lightweight
/// [`MediaDiffPicture`] in `App::media_diff_landed` — the same split `MdImageResult`/`MdImgEntry`
/// already have for mermaid/math renders.
#[derive(Debug)]
pub struct MediaDiffPictureDecoded {
    /// This side's **intrinsic** size, in the kind's own natural unit — a raster image's real
    /// (file-header) pixel dimensions, an SVG's **viewBox** size, or a PDF page's size in **points**.
    /// **Not** `image.dimensions()`/`frames[0].dimensions()` (the decoded raster's own pixel size),
    /// which depends on kind-specific rendering choices not comparable across the two sides of one
    /// diff: a PDF page rasterizes to a fixed longest side regardless of its point size, an SVG
    /// rasterizes to fit the caller's `raster_px` box regardless of its viewBox, and a GIF's frames
    /// may be downscaled by the inline decode budget — so two *different* intrinsic sizes can decode
    /// to *similar or identical* raster dimensions, which would make `preview::media_diff::layout`'s
    /// shared-scale math compare apples to oranges (a physically smaller PDF page rendered visibly
    /// *larger* than the original, both having rasterized to the same longest side). `layout`'s own
    /// fit math must be driven by *this* field — the terminal encoder never re-derives its own fit
    /// from the decoded raster instead (`App::poll_md_encode` always passes media-diff pictures
    /// through `Resize::Scale`, never `Fit`, resizing the decoded raster to exactly the cell box
    /// `layout` already decided).
    pub natural_px: (u32, u32),
    pub bytes: u64,
    pub page_count: Option<u32>,
    pub image: image::DynamicImage,
    pub frames: Option<Vec<(image::DynamicImage, std::time::Duration)>>,
    pub svg: Option<Arc<Vec<u8>>>,
    /// The file an SVG side was drawn from (the base of its relative references); drawn again
    /// from the same base. `None` for the kinds that keep no SVG.
    pub svg_base: Option<PathBuf>,
    pub cache_key: PathBuf,
}

/// One side's outcome, still carrying pixel data where present (`MediaDiffComputed`'s own per-side
/// shape). Boxes its `Picture` payload so `MediaDiffComputed::Ready`'s two sides don't double the
/// cost of `MediaDiffPictureDecoded`'s own size (`clippy::large_enum_variant`) — a plain pointer
/// indirection, not a behavior change.
#[derive(Debug)]
pub enum MediaDiffSideDecoded {
    /// The file doesn't exist on this side (new/untracked, or deleted).
    Absent,
    /// Bytes were read but could not be decoded as this diff's `kind` (corrupt image, encrypted/
    /// unparsable PDF, invalid SVG).
    Failed {
        reason: String,
    },
    /// A PDF page beyond this side's own page count.
    PageMissing,
    Picture(Box<MediaDiffPictureDecoded>),
}

/// The lightweight, stored form of one side's picture — everything a caption/layout needs, with no
/// pixel data (that already lives in `md_image_cache` under `cache_key`).
#[derive(Clone, Debug)]
pub struct MediaDiffPicture {
    // Read by `ui/preview.rs::draw_media_caption` (git-gated) for the caption's "WxH", and by
    // `ui/preview.rs::media_side_natural_px` (git-gated) as `preview::media_diff::layout`'s own
    // sizing input — dead on `--no-default-features`. See `MediaDiffPictureDecoded::natural_px`'s
    // own doc comment for why the *intrinsic* size (not the decoded raster's) is what sizing math
    // must use.
    pub natural_px: (u32, u32),
    pub bytes: u64,
    pub page_count: Option<u32>,
    pub cache_key: PathBuf,
}

/// The stored (landed) form of one side's outcome — the `MediaDiffSideDecoded` counterpart with no
/// pixel data.
#[derive(Clone, Debug)]
pub enum MediaDiffSide {
    Absent,
    Failed {
        // Read by `ui/preview.rs::draw_media_side`'s "cannot display: <reason>" placeholder
        // (git-gated) — dead on `--no-default-features`.
        reason: String,
    },
    PageMissing,
    Picture(MediaDiffPicture),
}

/// The pure computation's raw result (`App::compute_media_diff`) — still carrying pixel data where a
/// side decoded into a picture. Travels from the worker thread to the run loop inside
/// [`MediaDiffResult`]; `App::apply_media_diff` turns it into a stored [`MediaDiffOutcome`].
#[derive(Debug)]
pub enum MediaDiffComputed {
    /// Both sides were resolved against a picture-capable `kind` (their own decode may still have
    /// individually failed/been absent — see [`MediaDiffSideDecoded`]).
    Ready {
        kind: MediaDiffKind,
        base: MediaBase,
        same_bytes: bool,
        old: MediaDiffSideDecoded,
        new: MediaDiffSideDecoded,
    },
    /// Not a picture-capable kind, or a side is over the byte cap: sizes only, no decode.
    Summary {
        base: MediaBase,
        same_bytes: bool,
        old_len: Option<u64>,
        new_len: Option<u64>,
    },
    /// The worker panicked, or the request could not be resolved at all (`compute_or_fallback`'s
    /// fallback value).
    Unavailable,
}

/// The stored (landed) form of [`MediaDiffComputed`] — the `Ready`/`Summary`/`Unavailable` shapes are
/// identical, only the per-side payload loses its pixel data (see [`MediaDiffSide`]).
#[derive(Clone, Debug)]
pub enum MediaDiffOutcome {
    Ready {
        kind: MediaDiffKind,
        // Read by `ui/preview.rs::draw_media_caption` (git-gated: the side-by-side view is only
        // ever drawn there) for the old-side caption's base label — dead on a `--no-default-
        // features` build, where that whole render path is compiled out.
        base: MediaBase,
        same_bytes: bool,
        old: MediaDiffSide,
        new: MediaDiffSide,
    },
    Summary {
        // Carried through for symmetry with `Ready` and because a future consumer (or a test) may
        // want it, but the binary-summary line itself never names a base — only sizes — so nothing
        // in production reads this field today.
        #[allow(dead_code)]
        base: MediaBase,
        same_bytes: bool,
        old_len: Option<u64>,
        new_len: Option<u64>,
    },
    Unavailable,
}

/// A media-diff computation request handed to the worker thread (or run synchronously —
/// `App::spawn_or_sync_media_diff`). Carries everything `App::compute_media_diff` needs as plain
/// data, resolved on the UI thread ahead of time (mirrors [`MdDiffRequest`]'s own doc comment on why
/// `preview_rules`/`preview_commands` ride along here instead of `&Config`: the worker thread has no
/// access to `App`/`Config` at all).
pub struct MediaDiffRequest {
    pub gen: u64,
    pub path: PathBuf,
    pub root: PathBuf,
    pub baseline: DiffBaseline,
    /// 1-based PDF page (ignored for every other kind).
    pub page: u32,
    /// Target pixel box an SVG/PDF side is rasterized to fit (the pane's own image area in px —
    /// `preview::media_diff::layout`'s output converted to px by the caller). Ignored for a raw
    /// raster image, whose decode doesn't depend on the display box at all.
    pub raster_px: (u32, u32),
    pub classify: Arc<MediaClassifier>,
    pub rasterize_svg: RasterizeSvg,
    /// Set by the UI thread when nobody wants this computation any more (the want changed or the
    /// baseline moved): a drawing of an untrusted SVG then stops at once instead of holding its
    /// child process for the whole time limit.
    pub cancel: Arc<AtomicBool>,
}

/// Result of a media-diff computation, returned via `App::media_diff_tx`. Staleness is judged by
/// `gen` exactly like [`MdDiffResult`].
pub struct MediaDiffResult {
    pub gen: u64,
    pub path: PathBuf,
    /// The 1-based PDF page this result answers for (mirrors `MediaDiffRequest::page`).
    pub page: u32,
    /// The raster target this result was decoded at (mirrors `MediaDiffRequest::raster_px`).
    pub raster_px: (u32, u32),
    pub computed: MediaDiffComputed,
}

/// `App::media_diff_landed`'s key: `(path, page, raster_px, gen)` it answers for, plus the outcome
/// itself. A named alias only to keep the field declaration under `clippy::type_complexity`'s
/// threshold — see `App::media_diff_landed`'s own doc comment for what each element means.
pub type MediaDiffLandedKey = (PathBuf, u32, (u32, u32), u64, MediaDiffOutcome);

pub fn live_cache_keys(outcome: &MediaDiffOutcome) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    if let MediaDiffOutcome::Ready { old, new, .. } = outcome {
        if let MediaDiffSide::Picture(p) = old {
            set.insert(p.cache_key.clone());
        }
        if let MediaDiffSide::Picture(p) = new {
            set.insert(p.cache_key.clone());
        }
    }
    set
}

/// Ready sides are independent of the raster target and the current page's pending request.
pub fn ready_sides(outcome: Option<&MediaDiffOutcome>) -> Option<(&MediaDiffSide, &MediaDiffSide)> {
    match outcome? {
        MediaDiffOutcome::Ready { old, new, .. } => Some((old, new)),
        _ => None,
    }
}

pub fn max_page_count(outcome: Option<&MediaDiffOutcome>) -> Option<u32> {
    let (old, new) = ready_sides(outcome)?;
    let count = |side: &MediaDiffSide| match side {
        MediaDiffSide::Picture(p) => p.page_count.unwrap_or(1),
        _ => 1,
    };
    Some(count(old).max(count(new)))
}

pub fn can_page(outcome: Option<&MediaDiffOutcome>) -> bool {
    max_page_count(outcome).is_some_and(|n| n > 1)
}

pub fn showing_pictures(outcome: Option<&MediaDiffOutcome>) -> bool {
    ready_sides(outcome).is_some_and(|(old, new)| {
        matches!(old, MediaDiffSide::Picture(_)) || matches!(new, MediaDiffSide::Picture(_))
    })
}

/// Turn one shared 1-based page, bounded by the larger side. Recheck on every key.
pub fn page_turn(outcome: Option<&MediaDiffOutcome>, page: u32, dir: i32) -> u32 {
    let cur = page.max(1);
    let Some(max) = max_page_count(outcome).filter(|n| *n > 1) else {
        return cur;
    };
    if dir >= 0 {
        cur.saturating_add(1).min(max)
    } else {
        cur.saturating_sub(1).max(1)
    }
}

/// Move decoded pixels into the host's existing picture cache, keeping only drawing metadata.
/// The host owns request numbering and the trust of any retained SVG.
pub fn materialize_media_diff(
    computed: MediaDiffComputed,
    mut sink: impl FnMut(MediaDiffPictureDecoded),
) -> MediaDiffOutcome {
    match computed {
        MediaDiffComputed::Ready {
            kind,
            base,
            same_bytes,
            old,
            new,
        } => MediaDiffOutcome::Ready {
            kind,
            base,
            same_bytes,
            old: materialize_side(old, &mut sink),
            new: materialize_side(new, &mut sink),
        },
        MediaDiffComputed::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        } => MediaDiffOutcome::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        },
        MediaDiffComputed::Unavailable => MediaDiffOutcome::Unavailable,
    }
}

/// Materialize a single side through the same sink as a complete outcome.
pub fn materialize_side(
    side: MediaDiffSideDecoded,
    mut sink: impl FnMut(MediaDiffPictureDecoded),
) -> MediaDiffSide {
    match side {
        MediaDiffSideDecoded::Absent => MediaDiffSide::Absent,
        MediaDiffSideDecoded::Failed { reason } => MediaDiffSide::Failed { reason },
        MediaDiffSideDecoded::PageMissing => MediaDiffSide::PageMissing,
        MediaDiffSideDecoded::Picture(p) => {
            let stored = MediaDiffPicture {
                natural_px: p.natural_px,
                bytes: p.bytes,
                page_count: p.page_count,
                cache_key: p.cache_key.clone(),
            };
            sink(*p);
            MediaDiffSide::Picture(stored)
        }
    }
}

/// Retain ordinary pictures and the adopted outcome's media keys. `None` leaves the diff.
pub fn prune_media_diff_picture_cache<V>(
    cache: &mut std::collections::HashMap<PathBuf, V>,
    outcome: Option<&MediaDiffOutcome>,
) {
    let live = outcome.map(live_cache_keys).unwrap_or_default();
    cache.retain(|key, _| {
        !konoma_widgets::media_diff::is_media_diff_url(&key.to_string_lossy()) || live.contains(key)
    });
}

/// Raster images and summaries ignore the requested raster box.
pub fn normalize_raster_px(kind: Option<MediaDiffKind>, raster_px: (u32, u32)) -> (u32, u32) {
    match kind {
        Some(MediaDiffKind::Svg | MediaDiffKind::Pdf) => raster_px,
        _ => (0, 0),
    }
}

pub fn classify_kind_with(
    target: DiffTarget,
    path: &Path,
    bytes: Option<&[u8]>,
) -> Option<MediaDiffKind> {
    match target {
        DiffTarget::Picture | DiffTarget::Svg => media_format(path, bytes),
        DiffTarget::Markdown | DiffTarget::Text | DiffTarget::Other => None,
    }
}

pub fn media_format(path: &Path, bytes: Option<&[u8]>) -> Option<MediaDiffKind> {
    if let Some(kind) = media_format_from_extension(path) {
        return kind;
    }
    media_format_from_content(path, bytes)
}

/// Outer None means the suffix cannot settle the format; Some(None) is a source diagram.
pub(crate) fn media_format_from_extension(path: &Path) -> Option<Option<MediaDiffKind>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "svg" => Some(Some(MediaDiffKind::Svg)),
        "pdf" => Some(Some(MediaDiffKind::Pdf)),
        // A diagram's source is text whatever it contains (a label may well say `<svg`): decided
        // by the name, before the content is looked at.
        "mmd" | "mermaid" => Some(None),
        e if image::ImageFormat::from_extension(e).is_some() => Some(Some(MediaDiffKind::Image)),
        _ => None,
    }
}

fn media_format_from_content(path: &Path, bytes: Option<&[u8]>) -> Option<MediaDiffKind> {
    let head: Vec<u8>;
    let bytes = match bytes {
        Some(b) => b,
        None => {
            use std::io::Read;
            let Ok(mut f) = std::fs::File::open(path) else {
                return None;
            };
            let mut buf = vec![0u8; FORMAT_HEAD];
            let n = f.read(&mut buf).unwrap_or(0);
            buf.truncate(n);
            head = buf;
            &head
        }
    };
    let bytes = &bytes[..bytes.len().min(FORMAT_HEAD)];
    match konoma_host_core::sniff::mime_of_bytes(bytes).as_deref() {
        Some("application/pdf") => Some(MediaDiffKind::Pdf),
        Some(m) if m.starts_with("image/") => Some(MediaDiffKind::Image),
        // XML is how an SVG is detected when the name does not say; any other known type is not
        // a picture.
        Some(m) if m != "text/xml" => None,
        _ => {
            let text = String::from_utf8_lossy(bytes);
            starts_with_svg_element(&text).then_some(MediaDiffKind::Svg)
        }
    }
}

/// Whether the document's first element is `<svg`: what comes before it may only be a BOM,
/// whitespace, an XML declaration or processing instruction, comments and a DOCTYPE. A text that
/// merely mentions `<svg` somewhere is not an SVG.
fn starts_with_svg_element(text: &str) -> bool {
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        let skipped = if let Some(r) = rest.strip_prefix("<?") {
            r.split_once("?>").map(|(_, after)| after)
        } else if let Some(r) = rest.strip_prefix("<!--") {
            r.split_once("-->").map(|(_, after)| after)
        } else if rest.len() >= 9
            && rest.is_char_boundary(9)
            && rest[..9].eq_ignore_ascii_case("<!doctype")
        {
            // A DOCTYPE may hold an internal subset in `[ ... ]`.
            skip_doctype(&rest[9..])
        } else {
            return rest.starts_with("<svg");
        };
        match skipped {
            Some(after) => rest = after.trim_start(),
            // Cut off inside the prolog (the head is short): no `<svg` was reached.
            None => return false,
        }
    }
}

/// The text after the `>` that closes a DOCTYPE whose body (after `<!DOCTYPE`) is `body`.
fn skip_doctype(body: &str) -> Option<&str> {
    let mut depth = 0usize;
    for (i, c) in body.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '>' if depth == 0 => return Some(&body[i + 1..]),
            _ => {}
        }
    }
    None
}

/// Which base to read the old side from, and what it actually resolved to. Without a context,
/// Git/Jj selection keeps the host's ordinary detection rule.
///
/// Unlike `App::compute_md_diff`'s baseline handling, a [`DiffBaseline::Empty`] falls back to the
/// committed baseline (`base_contents`) here rather than becoming "no old side": a media diff has no
/// line/block-level fallback to defer to the way the Markdown/text diff does, so pairing against
/// `HEAD`/`@-` is strictly more useful than showing "new file" for one that in fact has real history
/// — the caption always reports whichever base was actually used (`MediaBase`).
///
/// [`DiffBaseline::FollowHead`] gets the identical fallback when `konoma_vcs_core::vcs::contents_at` returns
/// `None` — **not** `unwrap_or_default()` into an empty byte vector. Text can default to empty
/// because an empty old side just means "all added", but for a picture an empty `Vec<u8>` is a
/// zero-byte file that fails to decode, not "absent": the old side would render as
/// [`MediaDiffSideDecoded::Failed`] instead of [`MediaDiffSideDecoded::Absent`], and `old_len` would
/// report `Some(0)` instead of `None`. `blob_at` returning `None` means either the file genuinely
/// didn't exist at the pinned follow-start `sha` (still resolves to `Absent`, correctly relabeled) or
/// the lookup itself failed for an unrelated reason — the committed-baseline fallback is truthful
/// either way.
pub fn resolve_old_bytes(
    baseline: &DiffBaseline,
    root: &Path,
    path: &Path,
) -> (Option<Vec<u8>>, MediaBase) {
    resolve_old_bytes_with_context(baseline, root, path, super::DiffReadContext::default())
}

fn resolve_old_bytes_with_context(
    baseline: &DiffBaseline,
    root: &Path,
    path: &Path,
    context: super::DiffReadContext,
) -> (Option<Vec<u8>>, MediaBase) {
    let vcs_base = if is_jj(root) {
        MediaBase::JjParent
    } else {
        MediaBase::Head
    };
    match baseline {
        DiffBaseline::Vcs => {
            use konoma_vcs_core::vcs::Preference;
            let base = match context.backend {
                Preference::Auto => vcs_base,
                Preference::Git => MediaBase::Head,
                Preference::Jj => MediaBase::JjParent,
            };
            (context.base_contents(root, path), base)
        }
        DiffBaseline::FollowSnapshot(bytes) => (Some(bytes.clone()), MediaBase::FollowStart),
        DiffBaseline::FollowHead { sha } => {
            match konoma_vcs_core::vcs::contents_at(root, sha, path) {
                Some(bytes) => (Some(bytes), MediaBase::FollowStart),
                None => (konoma_vcs_core::vcs::base_contents(root, path), vcs_base),
            }
        }
        DiffBaseline::Empty => (konoma_vcs_core::vcs::base_contents(root, path), vcs_base),
    }
}

/// `compute_media_diff_with_cap`'s byte/cap/base half: both sides' bytes (or why each is missing),
/// their lengths, whether the new side exists as a regular file, whether either side is over `cap`,
/// whether the two sides are byte-identical, and which base the old side was actually read against.
/// No classification, no decoding — see `classify_and_decode_media_diff` for the other half.
struct MediaDiffByteResolution {
    old_bytes: Option<Vec<u8>>,
    new_bytes: Option<Vec<u8>>,
    new_exists: bool,
    over_cap: bool,
    same_bytes: bool,
    old_len: Option<u64>,
    new_len: Option<u64>,
    base: MediaBase,
}

/// Resolves everything `compute_media_diff_with_cap` needs to know before it can even ask what kind
/// of picture this is: both sides' bytes, sizes, existence, and the cap check. Pure extraction of
/// that half of the original function — same order of operations, same I/O, nothing moved earlier or
/// later.
fn resolve_media_diff_bytes(
    req: &MediaDiffRequest,
    cap: u64,
    context: super::DiffReadContext,
) -> MediaDiffByteResolution {
    // The old side has no size-only API to check first — `resolve_old_bytes` always hands back
    // the full blob or nothing, so its length is only known *after* reading it. The new side is a
    // plain file on disk, so `fs::metadata` gets its size for free: a file already over `cap` is
    // never read into memory at all (skipping a multi-hundred-MB read that would be thrown away
    // immediately).
    let (old_bytes, base) =
        resolve_old_bytes_with_context(&req.baseline, &req.root, &req.path, context);
    let old_len = old_bytes.as_ref().map(|b| b.len() as u64);
    let old_over_cap = old_len.is_some_and(|n| n > cap);

    let new_meta = std::fs::metadata(&req.path).ok();
    let new_meta_len = new_meta.as_ref().map(|m| m.len());
    // Existence, independent of whether the bytes get read (an over-cap file still exists, its
    // read is just skipped below) — decides which classification rule applies just below
    // (`Config::resolve_preview`'s path rules for an existing file, vs. sniffing the *old* bytes
    // for one that's gone). Requires `is_file()`, not just "metadata answers at all": a directory
    // now sitting where a regular file used to be (a rename/rewrite race) still has metadata but
    // can't be read/classified as a picture — treating it as "exists" would route through
    // `resolve_preview`'s path rules, whose mime sniff fails outright on a directory and degrades
    // the whole diff to the binary summary even when the *old* side is a perfectly good picture.
    // Treating it as absent instead sniffs the old bytes, same as a genuinely deleted file.
    let new_exists = new_meta.is_some_and(|m| m.is_file());
    let new_over_cap = new_meta_len.is_some_and(|n| n > cap);
    let new_bytes = if new_over_cap {
        None // known too large from metadata alone — never read.
    } else {
        std::fs::read(&req.path).ok()
    };
    // The metadata-derived size when the file was never read (over cap); otherwise the actual
    // bytes' own length. Both agree except on a rare mid-write race, where the bytes' length is
    // the more honest of the two since it's what was actually decoded.
    let new_len = if new_over_cap {
        new_meta_len
    } else {
        new_bytes.as_ref().map(|b| b.len() as u64)
    };
    let over_cap = old_over_cap || new_over_cap;
    // Differing lengths are conclusive without a byte-for-byte compare; only two sides that were
    // both actually read (never true for an over-cap or absent side) and already length-match are
    // compared further.
    let same_bytes = match (&old_bytes, &new_bytes) {
        (Some(o), Some(n)) => o.len() == n.len() && o == n,
        _ => false,
    };

    MediaDiffByteResolution {
        old_bytes,
        new_bytes,
        new_exists,
        over_cap,
        same_bytes,
        old_len,
        new_len,
        base,
    }
}

/// `compute_media_diff_with_cap`'s classify+decode half: resolves the picture kind (from the new
/// file when it exists, or by sniffing the old bytes when it doesn't), degrades to
/// [`MediaDiffComputed::Summary`] for a non-picture kind or an over-cap side, and otherwise decodes
/// both sides independently. Pure extraction of that half of the original function — same order of
/// operations, same I/O, nothing moved earlier or later.
fn classify_and_decode_media_diff(
    req: &MediaDiffRequest,
    bytes: MediaDiffByteResolution,
) -> MediaDiffComputed {
    let MediaDiffByteResolution {
        old_bytes,
        new_bytes,
        new_exists,
        over_cap,
        same_bytes,
        old_len,
        new_len,
        base,
    } = bytes;

    let target = (req.classify)(
        &req.path,
        if new_exists {
            None
        } else {
            old_bytes.as_deref()
        },
    );

    let sniffed = if new_exists {
        new_bytes.as_deref()
    } else {
        old_bytes.as_deref()
    };
    let Some(kind) = classify_kind_with(target, &req.path, sniffed) else {
        return MediaDiffComputed::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        };
    };
    if over_cap {
        return MediaDiffComputed::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        };
    }

    let old = decode_side(
        kind,
        old_bytes.as_deref(),
        &req.path,
        req.page,
        req.raster_px,
        KeySide::Old,
        (&req.cancel, req.rasterize_svg),
    );
    let new = decode_side(
        kind,
        new_bytes.as_deref(),
        &req.path,
        req.page,
        req.raster_px,
        KeySide::New,
        (&req.cancel, req.rasterize_svg),
    );
    MediaDiffComputed::Ready {
        kind,
        base,
        same_bytes,
        old,
        new,
    }
}

/// Whether `root` is answered by the jj backend — `konoma_vcs_core::vcs::VcsKind::Jj` only exists behind
/// `feature = "git"`, so this stays a plain `false` (git is the only backend) on a no-git build,
/// exactly like every other jj-only branch in this module.
pub fn is_jj(root: &Path) -> bool {
    konoma_vcs_core::vcs::detect(root) != konoma_vcs_core::vcs::VcsKind::Git
}

/// Decode one side of a picture-capable diff. `None` bytes (the side is absent — a new/untracked
/// file's old side, or a deleted file's new side) is the only case common to every kind.
fn decode_side(
    kind: MediaDiffKind,
    bytes: Option<&[u8]>,
    path: &Path,
    page: u32,
    raster_px: (u32, u32),
    side: KeySide,
    svg: (&std::sync::atomic::AtomicBool, RasterizeSvg),
) -> MediaDiffSideDecoded {
    let (cancel, rasterize_svg) = svg;
    let Some(bytes) = bytes else {
        return MediaDiffSideDecoded::Absent;
    };
    let hash = konoma_widgets::media_diff::fnv1a64(bytes);
    match kind {
        MediaDiffKind::Image => decode_image_side(bytes, hash, page, side),
        MediaDiffKind::Svg => decode_svg_side(
            bytes,
            path,
            raster_px,
            hash,
            page,
            side,
            (cancel, rasterize_svg),
        ),
        MediaDiffKind::Pdf => decode_pdf_side(bytes, page, raster_px, hash, side),
    }
}

/// A raw raster image (PNG/JPG/…) or an animated GIF — GIF is tried first (mirrors
/// `App::ensure_md_image`'s own "GIF, else still image" order for an inline Markdown image), since a
/// multi-frame GIF also successfully decodes as a still (its first frame) and would otherwise never
/// be detected as animated. `raster_px` doesn't apply — the decode is at the image's own native
/// resolution, exactly like every other raster-image preview path in konoma.
fn decode_image_side(bytes: &[u8], hash: u64, page: u32, side: KeySide) -> MediaDiffSideDecoded {
    use image::GenericImageView;
    if let Some((frames, canvas_px)) = konoma_svg::image::decode_gif_bytes_inline(bytes) {
        let Some((first, _)) = frames.first() else {
            return MediaDiffSideDecoded::Failed {
                reason: "empty gif".to_string(),
            };
        };
        let first_frame = first.clone();
        let key = konoma_widgets::media_diff::media_diff_url(side, hash, page, None);
        return MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
            // The GIF's own logical-screen size (read from the header) — not `first.dimensions()`,
            // which the inline decode budget may have downscaled for memory.
            natural_px: canvas_px,
            bytes: bytes.len() as u64,
            page_count: None,
            image: first_frame,
            frames: Some(frames),
            svg: None,
            svg_base: None,
            cache_key: PathBuf::from(key),
        }));
    }
    match konoma_svg::image::decode_static_bytes(bytes) {
        Some(img) => {
            let (w, h) = img.dimensions();
            let key = konoma_widgets::media_diff::media_diff_url(side, hash, page, None);
            MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
                natural_px: (w, h),
                bytes: bytes.len() as u64,
                page_count: None,
                image: img,
                frames: None,
                svg: None,
                svg_base: None,
                cache_key: PathBuf::from(key),
            }))
        }
        None => MediaDiffSideDecoded::Failed {
            reason: "image decode failed".to_string(),
        },
    }
}

/// An SVG side: natural size is the intrinsic (viewBox) size, never the raster's — the same
/// distinction `MdImgEntry::layout_px` already draws for a mermaid/math render. Rasterized at
/// `raster_px`'s larger side (`preview::svg::rasterize_trusted` takes a single max-side target, same as
/// every other SVG call site in this codebase).
fn decode_svg_side(
    bytes: &[u8],
    path: &Path,
    raster_px: (u32, u32),
    hash: u64,
    page: u32,
    side: KeySide,
    svg: (&std::sync::atomic::AtomicBool, RasterizeSvg),
) -> MediaDiffSideDecoded {
    let (cancel, rasterize_svg) = svg;
    let max_px = raster_px.0.max(raster_px.1).max(1);
    // Git's old version of a file: drawn by a supervised child process, like any SVG from a file.
    let img = match rasterize_svg(bytes, path, max_px, &|| {
        cancel.load(std::sync::atomic::Ordering::SeqCst)
    }) {
        Ok(img) => img,
        Err(why) => {
            return MediaDiffSideDecoded::Failed {
                reason: match why {
                    konoma_svg::svg::RasterError::Failed(f) => f.reason(),
                    // The request was cancelled: the result is dropped by whoever cancelled it.
                    konoma_svg::svg::RasterError::Cancelled => "cancelled",
                }
                .to_string(),
            };
        }
    };
    // A size that only drawing reveals (no width/height/viewBox) falls back to the raster's.
    let natural_px = konoma_svg::svg::intrinsic_size_bytes(bytes).unwrap_or_else(|| {
        use image::GenericImageView;
        img.dimensions()
    });
    let key = konoma_widgets::media_diff::media_diff_url(side, hash, page, Some(raster_px));
    MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
        natural_px,
        bytes: bytes.len() as u64,
        page_count: None,
        image: img,
        frames: None,
        svg: Some(Arc::new(bytes.to_vec())),
        svg_base: Some(path.to_path_buf()),
        cache_key: PathBuf::from(key),
    }))
}

/// A PDF side. `page` beyond this side's own page count is [`MediaDiffSideDecoded::PageMissing`],
/// judged independently per side — the two sides can have different page counts. Natural size is the
/// page's own size in PDF points (`page_dimensions_bytes`), not the raster's. `render_page_bytes` is
/// `hayro`-only (no `qlmanage`/`sips` fallback) — see that function's own doc comment for why.
fn decode_pdf_side(
    bytes: &[u8],
    page: u32,
    raster_px: (u32, u32),
    hash: u64,
    side: KeySide,
) -> MediaDiffSideDecoded {
    let Some(page_count) = konoma_media_core::pdf::page_count_bytes(bytes) else {
        return MediaDiffSideDecoded::Failed {
            reason: "invalid pdf".to_string(),
        };
    };
    if page < 1 || page > page_count {
        return MediaDiffSideDecoded::PageMissing;
    }
    let Some((pw, ph)) = konoma_media_core::pdf::page_dimensions_bytes(bytes, page) else {
        return MediaDiffSideDecoded::Failed {
            reason: "pdf page dimensions unavailable".to_string(),
        };
    };
    let Some(img) = konoma_media_core::pdf::render_page_bytes(bytes, page) else {
        return MediaDiffSideDecoded::Failed {
            reason: "pdf page render failed".to_string(),
        };
    };
    let key = konoma_widgets::media_diff::media_diff_url(side, hash, page, Some(raster_px));
    MediaDiffSideDecoded::Picture(Box::new(MediaDiffPictureDecoded {
        natural_px: (pw.ceil().max(1.0) as u32, ph.ceil().max(1.0) as u32),
        bytes: bytes.len() as u64,
        page_count: Some(page_count),
        image: img,
        frames: None,
        svg: None,
        svg_base: None,
        cache_key: PathBuf::from(key),
    }))
}

/// One decode in flight, with one replaceable latest want behind it. Invalidation clears the
/// logical pending key, but the physical slot stays occupied until its own result arrives.
#[derive(Default)]
pub struct MediaDiffWorker {
    read_context: super::DiffReadContext,
    gen: u64,
    pending: Option<MediaDiffWant>,
    busy: bool,
    queued: Option<MediaDiffWant>,
    cancel: Arc<AtomicBool>,
    landed: Option<MediaDiffLandedKey>,
    tx: Option<Sender<MediaDiffResult>>,
    compute: Option<Arc<MediaCompute>>,
}

type MediaCompute = dyn Fn(&MediaDiffRequest) -> MediaDiffComputed + Send + Sync;

impl MediaDiffWorker {
    /// Set the choice copied into each dispatched computation, including synchronous fallback.
    pub fn with_read_context(mut self, context: super::DiffReadContext) -> Self {
        self.read_context = context;
        self
    }

    pub fn with_compute(compute: Arc<MediaCompute>) -> Self {
        Self {
            compute: Some(compute),
            ..Self::default()
        }
    }
    pub fn attach(&mut self, tx: Sender<MediaDiffResult>) {
        self.tx = Some(tx);
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    pub fn queued(&self) -> &Option<MediaDiffWant> {
        &self.queued
    }
    pub fn take_queued(&mut self) -> Option<MediaDiffWant> {
        self.queued.take()
    }

    pub fn landed_for(&self, path: &Path, page: u32, px: (u32, u32)) -> Option<MediaDiffOutcome> {
        self.landed
            .as_ref()
            .filter(|(p, pg, rp, gen, _)| p == path && *pg == page && *rp == px && *gen == self.gen)
            .map(|(_, _, _, _, o)| o.clone())
    }

    pub fn outcome_for(&self, path: &Path) -> Option<&MediaDiffOutcome> {
        self.landed
            .as_ref()
            .filter(|(p, ..)| p == path)
            .map(|(_, _, _, _, o)| o)
    }

    pub fn needs(&self, path: &Path, page: u32, px: (u32, u32)) -> bool {
        let matches = |slot: &Option<MediaDiffWant>| {
            slot.as_ref()
                .is_some_and(|(p, pg, rp)| p == path && *pg == page && *rp == px)
        };
        self.landed_for(path, page, px).is_none()
            && !matches(&self.pending)
            && !matches(&self.queued)
    }

    pub fn kick(&mut self, want: MediaDiffWant) -> Option<MediaDiffWant> {
        if self.busy {
            self.queued = Some(want);
            self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            None
        } else {
            Some(want)
        }
    }

    pub fn dispatch(&mut self, mut req: MediaDiffRequest) -> Option<MediaDiffResult> {
        self.gen = self.gen.wrapping_add(1);
        req.gen = self.gen;
        self.pending = Some((req.path.clone(), req.page, req.raster_px));
        self.busy = true;
        self.cancel = Arc::default();
        req.cancel = self.cancel.clone();
        self.spawn_or_sync(req)
    }

    fn spawn_or_sync(&self, req: MediaDiffRequest) -> Option<MediaDiffResult> {
        let compute = self.compute.clone();
        let context = self.read_context;
        let fallback = MediaDiffResult {
            gen: req.gen,
            path: req.path.clone(),
            page: req.page,
            raster_px: req.raster_px,
            computed: MediaDiffComputed::Unavailable,
        };
        let run = move || {
            let computed = match compute {
                Some(compute) => compute(&req),
                None => Self::compute_with_context(
                    &req,
                    konoma_media_core::pdf::PAGE_COUNT_MAX_BYTES,
                    context,
                ),
            };
            MediaDiffResult {
                gen: req.gen,
                path: req.path,
                page: req.page,
                raster_px: req.raster_px,
                computed,
            }
        };
        let Some(tx) = self.tx.clone() else {
            return Some(run());
        };
        let config = konoma_vcs_core::vcs::WorkerConfig::capture();
        std::thread::spawn(move || {
            config.install();
            let result = konoma_widgets::markdown::compute_or_fallback(run, || fallback);
            let _ = tx.send(result);
        });
        None
    }

    /// Materialize accepted pixels with the host, then retain only the lightweight outcome.
    pub fn apply(
        &mut self,
        res: MediaDiffResult,
        materialize: impl FnOnce(MediaDiffComputed) -> MediaDiffOutcome,
    ) -> bool {
        self.busy = false;
        self.pending = None;
        if res.gen != self.gen || self.cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let outcome = materialize(res.computed);
        self.landed = Some((res.path, res.page, res.raster_px, res.gen, outcome));
        true
    }

    pub fn invalidate(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        self.gen = self.gen.wrapping_add(1);
        self.landed = None;
        self.pending = None;
    }

    pub fn compute_media_diff(req: &MediaDiffRequest) -> MediaDiffComputed {
        Self::compute_media_diff_with_cap(req, konoma_media_core::pdf::PAGE_COUNT_MAX_BYTES)
    }
    pub fn compute_media_diff_with_cap(req: &MediaDiffRequest, cap: u64) -> MediaDiffComputed {
        Self::compute_with_context(req, cap, super::DiffReadContext::default())
    }

    fn compute_with_context(
        req: &MediaDiffRequest,
        cap: u64,
        context: super::DiffReadContext,
    ) -> MediaDiffComputed {
        classify_and_decode_media_diff(req, resolve_media_diff_bytes(req, cap, context))
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn generation_for_test(&mut self) -> &mut u64 {
        &mut self.gen
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn queued_for_test(&mut self) -> &mut Option<MediaDiffWant> {
        &mut self.queued
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cancel_for_test(&self) -> &Arc<AtomicBool> {
        &self.cancel
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn busy_for_test(&mut self) -> &mut bool {
        &mut self.busy
    }
}
