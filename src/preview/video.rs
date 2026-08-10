// Video thumbnail preview: extract a single representative frame and return it as a DynamicImage.
// The result is put onto the app's image_src just like SVG, and from there flows through the normal
// image path (prepare_image → worker re-encode → kitty graphics) unchanged.
// **We do not "play" video inside the terminal (thumbnail only)** — real-time playback via kitty
// graphics would be CPU-prohibitive and is parser-bound to the point of breaking down in Ghostty
// (investigated 2026-06-27; see docs/AUDIT).
//
// **Extractor priority order** (the same shape `preview::pdf` uses for hayro → qlmanage/sips):
//   1. Pure Rust, in-process, no external tool at all — a demultiplexer reads the container and
//      `rust_h264`/`rust_h265` decodes one keyframe. This covers **H.264 (AVC) and HEVC (H.265)
//      inside mp4/m4v/mov and mkv/webm**: between them, a screen recording, a `git`-tracked demo
//      clip, anything a browser can play, what an iPhone records by default, and the container
//      almost every ripped or re-encoded file on a disk arrives in.
//   2. `ffmpegthumbnailer`, then `ffmpeg`. Now needed only for **VP9, AV1 and the older codecs**
//      (MPEG-4 Part 2, ProRes, …) — which is what a `.webm` usually turns out to be — plus the
//      **`.avi` container** and the H.264/HEVC profiles step 1 deliberately refuses (below). Both
//      remain strictly optional: if neither is installed this returns None and the caller degrades
//      to a hint message (PRD §5 ease of distribution, principle #3 "unsupported must fail safely").
//
// **Two containers, two codecs, one everything-else.** The container layer is the only part that
// forks: `re_mp4` indexes ISO-BMFF, `symphonia-format-mkv` demultiplexes Matroska/WebM. From the
// parameter-set guard down — decoder dispatch, `NativeFrame`, YUV→RGB, the thumbnail shrink, the
// `catch_silent` wrapper, what `[external] video` does and doesn't gate — the two share one
// implementation, so "supported" means the same thing in both and neither can drift from the other.
// The one thing Matroska needs of its own is **which packet is a keyframe**: it has no sample table
// and symphonia's `Packet` carries no such flag, so [`sample_has_keyframe`] reads the NAL type
// instead of being told (see [`thumbnail_native_mkv`] for how the search is bounded).
//
// **Why the pure-Rust path is narrow on purpose.**
//   * *Container*: only `mp4`/`m4v`/`mov`/`mkv`/`webm` are even opened. Anything else (`.avi`, …)
//     returns None before a single byte is read.
//   * *Codec family*: from the track's `codec_string` in mp4 (`avc1`/`avc3` for H.264, `hvc1`/`hev1`
//     for HEVC) and from the codec ID in Matroska; VP9, AV1 and the rest are rejected by name rather
//     than being left to fail somewhere deeper. This picks which decoder the file belongs to and is a
//     cheap early filter, **not** a second opinion on the one below: both come from the same
//     `avcC`/`hvcC` record, which Matroska stores verbatim as `CodecPrivate`.
//   * *Stream properties*: **each decoder gets its own guard, because they are dangerous in
//     different places** — see `sps_is_decodable` (H.264) and `hevc_sps_is_decodable` (HEVC). Both
//     read the **bitstream's own sequence parameter set** rather than the container's summary of it,
//     and both refuse *before* decoding rather than sanity-checking the image afterwards.
//     `rust_h264` decodes Baseline/Main/High **8-bit 4:2:0** only, and —
//     measured, this is the important part — it does **not** report an error on the ones it cannot
//     do. It returns a wrong image. High 10 (`profile_idc` 110), 4:2:2 (122) and 4:4:4 (244) yield a
//     picture that *looks plausible*: on a 10-bit sample the decoded keyframe had mean luma 126.6
//     against a correct 128.0, yet **53.0% of its pixels differed from ffmpeg's, by up to 58 levels**.
//     Worse, **the profile number cannot express the whole condition**: monochrome (4:0:0) video is
//     `profile_idc` 100, identical to ordinary High 4:2:0, and differs only in the SPS's
//     `chroma_format_idc` — which `rust_h264` parses and then never reads, so it decodes such a
//     stream as if it had chroma planes and desynchronizes at once (measured on `ffmpeg -pix_fmt
//     gray -profile:v high`: **97.2% of luma samples wrong, mean |Δ| 83.9, max 255** — a grayscale
//     test pattern rendered as pink and green diagonal stripes). No after-the-fact sanity check on
//     the decoded image (mean, variance, "does it look uniform") catches any of this.
//     `rust_h265` decodes **Main and Main 10 4:2:0** (8- and 10-bit), and fails in a friendlier
//     way — measured, its own SPS parser returns `Unsupported` for 4:2:2, 4:4:4 and monochrome, so
//     the wrong-picture hazard H.264 has does not reproduce there. Its guard is therefore mostly
//     about refusing what is *unverified* rather than what is known-broken: 12-bit is accepted by
//     that parser and decodes without complaint, on a path its own README calls untested. See
//     `hevc_sps_is_decodable` for which clause does what, and why the refusal is still the right call.
//   * *Size*: the SPS's coded dimensions, again rather than the container's `tkhd`, because the
//     decoder sizes its buffers from those. An absurd sample size is refused rather than allocated too.
// Everything refused this way simply falls through to step 2, so the user-visible effect of the
// narrowness is "a tool is needed for that file", never a wrong picture.
//
// **What "supported" does and does not promise.** For the streams that pass the gate above, the
// decoded frame matched ffmpeg byte for byte on the bundled samples — all 115,200 Y/U/V samples of
// `samples/sample.mp4` (H.264), of `samples/sample-hevc.mp4` (HEVC Main) and of `samples/sample.mkv`
// (H.264 in Matroska), and all 460,800 samples of a 640x480 Main 10 clip compared at full 16-bit
// precision. That is not a guarantee for every such stream, and the same measurement makes the point
// both ways: on a 640x480 H.264 mkv, the first keyframe came back byte-identical to ffmpeg's while
// the keyframe one second later differed on **32 of 460,800 samples, by one level each**; an H.264
// clip carrying custom quantization matrices (SPS/PPS scaling lists) decoded with **2.6% of samples
// differing, by up to 50 levels** — visually the same picture, but not bit-identical. Acceptable for
// a thumbnail, and worth knowing before anyone writes a test that asserts equality with ffmpeg in
// general.
//
// The whole pure-Rust path runs inside `catch_silent`, like every other pre-1.0 decoder konoma
// drives (resvg, hayro, mermaid-rs-renderer): a hostile or malformed file must degrade to the
// external chain, not take down the preview. It genuinely earns its keep here — both `rust_h264`
// (an index-out-of-bounds on interlaced H.264, which is `profile_idc` 100 and so passes every guard
// above) and `re_mp4` (a capacity overflow parsing a fuzzed `avcC`) were observed panicking on real
// inputs during review.
//
// One honest caveat on the ordering: the guards above are konoma's, but the container parse runs
// *before* them, so "refused rather than allocated" describes what this module decides, not the read
// it has to do first to decide anything. In mp4 that parse is bounded by the index rather than the
// file (`mdat` is skipped, not read — a 1 GB clip costs 19 MB of RSS, measured); Matroska has no
// index to lean on, so it is bounded by an explicit read budget instead ([`BudgetedSource`]). Both
// land in `catch_silent` on failure like everything else.
//
// `allow_external` (= `[external] video`) gates **step 2 only**. Step 1 never launches a process, so
// H.264 and HEVC thumbnails keep working with the flag off, in either container — exactly the
// relationship `[external] pdf` has with hayro. Tool execution runs on the media worker thread, so a
// blocking child process never blocks the UI.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::DynamicImage;
use symphonia_core::codecs::video::{well_known as video_codecs, VideoCodecParameters};
use symphonia_core::codecs::video::{well_known::extra_data as extra_data_ids, VideoCodecId};
use symphonia_core::codecs::CodecParameters;
use symphonia_core::formats::{FormatOptions, FormatReader, MediaInfo, SeekMode, SeekTo, Track};
use symphonia_core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia_core::units::{Time, Timestamp};
use symphonia_format_mkv::MkvReader;

/// Maximum side (px) of the extracted thumbnail. Since the image path shrinks it further to terminal cells, large enough to look crisp is sufficient.
const THUMB_MAX_PX: u32 = 1024;

/// How long `ffmpegthumbnailer`/`ffmpeg` are allowed to run before being killed and treated as a
/// failure. Generous on purpose — extracting a frame from a large or slow-to-seek video can
/// legitimately take several seconds — this exists only to turn "never returns" into "eventually
/// gives up," not to police normal runtimes. Without this, a hung tool (a pathological input that
/// makes it loop forever, say) blocked `Command::status()` indefinitely: since these run on the
/// media worker thread (see the module doc comment above), that leaked the worker thread *and*
/// the child process forever, and left the busy-indicator spinner stuck on.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often [`spawn_and_wait_with_timeout`]'s poll loop re-checks the child. Small enough to
/// notice the deadline promptly without busy-polling.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Container extensions the pure-Rust path will even open, and which demultiplexer each one wants.
/// `re_mp4` indexes the ISO-BMFF family; `symphonia-format-mkv` handles Matroska and WebM (which are
/// the same container — WebM is a constrained Matroska profile and the demuxer reads both). Anything
/// else (`avi`, …) is the external chain's job and is rejected here without reading the file at all.
const NATIVE_CONTAINER_EXTS: [(&str, NativeContainer); 5] = [
    ("mp4", NativeContainer::Mp4),
    ("m4v", NativeContainer::Mp4),
    ("mov", NativeContainer::Mp4),
    ("mkv", NativeContainer::Matroska),
    ("webm", NativeContainer::Matroska),
];

/// `profile_idc` values `rust_h264` actually decodes correctly: Baseline (66), Main (77),
/// High (100). Necessary but **not sufficient** — the profile says nothing about chroma format or
/// bit depth, which is why [`sps_is_decodable`] checks those separately. See the module doc comment
/// for why all of this must be refused *before* decoding rather than checked afterwards.
const DECODABLE_PROFILE_IDCS: [u8; 3] = [66, 77, 100];

/// The `profile_idc` values whose SPS carries the `chroma_format_idc` / `bit_depth_*` block
/// (H.264 §7.3.2.1.1). For every other profile those fields are absent and 8-bit 4:2:0 is implied.
const PROFILES_WITH_CHROMA_BLOCK: [u8; 13] =
    [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// `chroma_format_idc` for 4:2:0 — the only sampling either decoder implements. 0 is monochrome
/// (4:0:0), 2 is 4:2:2, 3 is 4:4:4. The field means the same thing in H.264 §7.3.2.1.1 and
/// H.265 §7.3.2.2.1, so both guards share this constant.
const CHROMA_FORMAT_420: u32 = 1;

/// `general_profile_idc` values `rust_h265` actually decodes: Main (1) and Main 10 (2). Everything
/// else — most importantly the Range Extensions family (4), whose whole reason to exist is the
/// 4:2:2 / 4:4:4 / ≥12-bit / monochrome variants and whose extra coding tools are not implemented —
/// is refused. Necessary but not sufficient, exactly like [`DECODABLE_PROFILE_IDCS`]: see
/// [`hevc_sps_is_decodable`].
const DECODABLE_HEVC_PROFILE_IDCS: [u8; 2] = [1, 2];

/// Luma/chroma bit depths `rust_h265` is validated at. Its README reports byte-exact agreement with
/// FFmpeg at 8 and 10 bits, and says of the next one up: *"12-bit infrastructure is in place but
/// untested."* In place and untested is the combination worth refusing: its SPS parser accepts a
/// 12-bit stream (it only rejects `> 16`) and decodes it without complaint — measured, a 12-bit
/// fixture produced a picture and **no error at all**, where 4:2:2/4:4:4/monochrome all stop with
/// `Unsupported`. That picture happened to be byte-exact against FFmpeg, so this is a *precautionary*
/// refusal of a path its own author calls unverified, not a workaround for a measured failure —
/// the same "unknown is not a license to guess" stance the H.264 guard takes on unknown profiles.
const DECODABLE_HEVC_BIT_DEPTHS: [u32; 2] = [8, 10];

/// Largest `sps_max_sub_layers_minus1` H.265 allows (§7.4.3.2.1 constrains it to 0..=6, though the
/// field is 3 bits wide). A stream claiming 7 is malformed; refusing it keeps
/// [`skip_profile_tier_level`] from walking a sub-layer table the spec says cannot exist.
const MAX_HEVC_SUB_LAYERS_MINUS1: u32 = 6;

/// Upper bound on either video dimension for the pure-Rust path. Larger than any real-world clip
/// (8K is 7680 wide); this exists so a corrupt/hostile header claiming e.g. 60000×60000 is refused
/// instead of driving an allocation and a decode we'd have to wait out.
const NATIVE_MAX_DIMENSION: u32 = 8192;

/// Upper bound on one sample (one coded frame) read into memory by the pure-Rust path. A keyframe
/// of a 4K clip is on the order of a megabyte; 64 MiB is far past anything legitimate and only
/// exists to keep a corrupt `stsz`/`stco` entry from asking for a huge allocation.
const NATIVE_MAX_SAMPLE_BYTES: u64 = 64 * 1024 * 1024;

/// How many keyframes the pure-Rust path will try before giving up on "the frame isn't blank".
/// Videos routinely open on a black or white frame; the first keyframe at the 10% mark is usually
/// fine, but when it comes back flat we walk forward. Bounded because each attempt is a full decode.
const NATIVE_FLAT_FRAME_RETRIES: usize = 3;

/// Luma spread (max − min over the Y plane) below which a decoded frame is treated as "flat"
/// (a solid fade-in/out card) and the next keyframe is tried instead.
const NATIVE_FLAT_LUMA_SPREAD: u8 = 8;

/// Total bytes one Matroska attempt may read from the file, covering the header, the cue table, the
/// seek, and the packet walk together. Matroska has no sample table, so "find a keyframe" is a
/// forward scan — and on a file without cue points `symphonia` implements *seeking* as a forward scan
/// too, so without this, asking for the 10% mark of a 2 GB clip would read 200 MB before returning.
///
/// 32 MiB is far more than a well-formed file needs (a cue point turns the seek into a jump, and one
/// keyframe of a 4K clip is on the order of a megabyte) and small enough that hitting the ceiling is
/// quick. See [`thumbnail_native_mkv`] for what happens then: one retry from the beginning of the
/// file, and failing that the external chain.
const NATIVE_MKV_MAX_SCAN_BYTES: u64 = 32 * 1024 * 1024;

/// How many packets one Matroska attempt may walk past. Secondary to the byte budget above — bytes
/// are what actually costs — but it also bounds a pathological file that yields a great many tiny
/// packets, and it keeps the search from wandering minutes into a clip whose keyframes are absent or
/// unreadable. Comfortably past the ~10% mark of an ordinary clip, which is where the walk starts.
const NATIVE_MKV_MAX_PACKETS: usize = 2000;

/// H.264 NAL unit type for an IDR slice (§7.4.1) — a picture that can be decoded with no reference
/// to anything before it.
const H264_NAL_TYPE_IDR: u8 = 5;

/// H.265 NAL unit types for IRAP pictures (§7.4.2.2): BLA_W_LP, BLA_W_RADL, BLA_N_LP, IDR_W_RADL,
/// IDR_N_LP and CRA_NUT. These are the HEVC equivalent of an IDR — where decoding may start.
const HEVC_IRAP_NAL_TYPES: [u8; 6] = [16, 17, 18, 19, 20, 21];

/// Extract and return one representative frame from the video at `path`.
///
/// Tries the pure-Rust H.264/HEVC mp4 path first (no external process, works regardless of
/// `allow_external`), then falls back to `ffmpegthumbnailer`/`ffmpeg` — but only if `allow_external`
/// (= `[external] video`) permits launching them. Returns None when nothing could produce a frame,
/// and the caller degrades to a safe fallback with a hint.
pub fn thumbnail(path: &Path, allow_external: bool) -> Option<DynamicImage> {
    if let Some(img) = thumbnail_native(path) {
        return Some(img);
    }
    if !allow_external {
        return None;
    }
    thumbnail_external(path)
}

/// Extract one keyframe entirely in-process (`re_mp4` + `rust_h264`/`rust_h265`). `None` means "this
/// file isn't something the built-in decoders handle" (wrong container/codec/profile, corrupt index,
/// or a caught panic) — never a crash, and never a wrong picture: [`thumbnail`] then tries the
/// external tools. Wrapped in `catch_silent` for the same reason `preview::pdf::render_page_native`
/// is: these are pre-1.0 decoders being pointed at arbitrary user files.
fn thumbnail_native(path: &Path) -> Option<DynamicImage> {
    crate::preview::markdown::catch_silent(|| thumbnail_native_inner(path)).flatten()
}

fn thumbnail_native_inner(path: &Path) -> Option<DynamicImage> {
    match native_container_kind(path)? {
        NativeContainer::Mp4 => thumbnail_native_mp4(path),
        NativeContainer::Matroska => thumbnail_native_mkv(path),
    }
}

/// The ISO-BMFF half: `re_mp4` reads the index, and the sample table says outright which samples are
/// keyframes and where their bytes are.
fn thumbnail_native_mp4(path: &Path) -> Option<DynamicImage> {
    // `Mp4::read` (not `read_bytes`) walks the boxes through a `Read + Seek`, so only the index
    // (`moov`/`stbl`) is materialized — a 4 GB recording never gets loaded into memory to take a
    // thumbnail of it. Only the one sample we choose is read, further down.
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mp4 = re_mp4::Mp4::read(&mut file, size).ok()?;

    let track = mp4
        .tracks()
        .values()
        .find(|t| t.kind == Some(re_mp4::TrackKind::Video))?;

    // Guard 1 — codec family, from the container. A cheap early filter that picks which decoder (if
    // any) this file even belongs to, and rejects VP9/AV1/everything else by name; it is *not* the
    // profile check below (both come from the same `avcC`/`hvcC` record), so it stops nothing that
    // guard 2 wouldn't.
    let kind = native_codec_kind(track.codec_string(&mp4).as_deref())?;

    let cfg = track.raw_codec_config(&mp4)?;

    // Guard 2 — the load-bearing one, shared with the Matroska path: the parameter sets are read out
    // of the **bitstream**, not out of the container's summary of it.
    let stream = native_stream_from_config(kind, &cfg)?;

    // Every failure inside the closure is a `continue`, not an early return: one unreadable sample
    // should cost us that keyframe, not the whole file. `pick_keyframes` already caps the list, so
    // skipping cannot turn into an unbounded walk.
    let mut candidates = pick_keyframes(&track.samples, NATIVE_FLAT_FRAME_RETRIES).into_iter();
    thumbnail_from_keyframes(&stream, || loop {
        let Some(sample) = track.samples.get(candidates.next()?) else {
            continue;
        };
        if sample.size == 0 || sample.size > NATIVE_MAX_SAMPLE_BYTES {
            continue;
        }
        if let Some(buf) = read_sample_bytes(&mut file, sample.offset, sample.size) {
            return Some(buf);
        }
    })
}

/// Containers the pure-Rust path can demultiplex, and which demultiplexer does it. The layer below
/// this — parameter-set guard, decoder, YUV→RGB, thumbnail shrink — is identical for both, so this
/// is the only place the two diverge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeContainer {
    /// ISO-BMFF (`mp4`/`m4v`/`mov`), indexed by `re_mp4`.
    Mp4,
    /// Matroska and WebM (`mkv`/`webm`), demultiplexed by `symphonia-format-mkv`.
    Matroska,
}

/// Which demultiplexer a path's extension asks for, or None for a container the pure-Rust path does
/// not open at all (`avi`, …) — decided before the file is opened, so such a file costs nothing.
/// ASCII case-insensitive.
fn native_container_kind(path: &Path) -> Option<NativeContainer> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    NATIVE_CONTAINER_EXTS
        .iter()
        .find(|(known, _)| ext.eq_ignore_ascii_case(known))
        .map(|(_, container)| *container)
}

/// Build the decoder-ready parameter sets for a track, after checking that its **bitstream** is
/// something the matching decoder gets right.
///
/// This is guard 2, the load-bearing one, and it is shared by both containers on purpose: `cfg` is
/// the very same `avcC`/`hvcC` record either way (Matroska stores it verbatim as `CodecPrivate`), so
/// a stream that mp4 would refuse must be refused in mkv too. See `sps_is_decodable` /
/// `hevc_sps_is_decodable` and the module doc comment for what each clause is protecting against.
fn native_stream_from_config(kind: NativeCodecKind, cfg: &[u8]) -> Option<NativeStream<'_>> {
    match kind {
        NativeCodecKind::Avc => {
            let avcc = rust_h264::nal::parse_avcc_config(cfg).ok()?;
            let sps = parse_sps_facts(&avcc.sps_nals.first()?.rbsp)?;
            if !sps_is_decodable(&sps) {
                return None;
            }
            Some(NativeStream::Avc(avcc))
        }
        NativeCodecKind::Hevc => {
            let hvcc = parse_hvcc(cfg)?;
            let sps = parse_hevc_sps_facts(&hvcc.sps_rbsp)?;
            if !hevc_sps_is_decodable(&sps) {
                return None;
            }
            Some(NativeStream::Hevc(hvcc))
        }
    }
}

/// Decode candidate keyframes, best first, and return the first one that is a real picture.
///
/// The container layer above supplies the bytes (`next_keyframe` returns one length-prefixed sample
/// at a time, in the order they should be tried, and None when it has run out or hit its own read
/// budget); everything from here down is codec- and container-independent. Bounded by
/// [`NATIVE_FLAT_FRAME_RETRIES`] here as well as by the caller, so a source that never says "no more"
/// still cannot loop.
fn thumbnail_from_keyframes(
    stream: &NativeStream<'_>,
    mut next_keyframe: impl FnMut() -> Option<Vec<u8>>,
) -> Option<DynamicImage> {
    let mut last: Option<DynamicImage> = None;
    for _ in 0..NATIVE_FLAT_FRAME_RETRIES {
        let Some(buf) = next_keyframe() else {
            break;
        };
        let Some(frame) = decode_keyframe(stream, &buf) else {
            continue;
        };
        let flat = luma_spread(&frame.y) < NATIVE_FLAT_LUMA_SPREAD;
        let Some(img) = frame_to_image(&frame) else {
            continue;
        };
        if !flat {
            return Some(shrink_to_thumb(img));
        }
        // A genuinely single-color video exists, so a flat frame is kept as the answer of last
        // resort rather than turned into "no thumbnail". The **first** one is kept: if every
        // candidate is flat we fall back to the keyframe nearest the 10% mark, which is the one the
        // selection rule wanted in the first place.
        if last.is_none() {
            last = Some(img);
        }
    }
    last.map(shrink_to_thumb)
}

/// The Matroska half. Same three steps as the mp4 path — pick the video track, check its bitstream,
/// decode a keyframe — with one structural difference the container forces: **Matroska has no sample
/// table**. There is no `stss` saying which frames are keyframes and no `stsz`/`stco` saying where
/// their bytes are, so instead of indexing straight to the frame we want, we position the demuxer and
/// then read packets forward, deciding for ourselves which one is a keyframe (see
/// [`sample_has_keyframe`] — symphonia's `Packet` carries no such flag).
///
/// "Read forward" is exactly the shape that must not be allowed to run away on a large file, so the
/// whole attempt reads through a [`BudgetedSource`] and gives up at [`NATIVE_MKV_MAX_SCAN_BYTES`].
/// Two attempts at most:
///
/// 1. Seek to the 10% mark (matching what the mp4 path and `ffmpegthumbnailer` pick), then scan.
///    On a file with cue points — which is nearly every real `.mkv`, since `mkvmerge` and `ffmpeg`
///    both write them — this jumps straight there and costs almost nothing.
/// 2. If that found nothing, scan from the very beginning instead. This is what covers a **cue-less**
///    file, where symphonia implements the seek as a forward walk that our read budget cuts short:
///    the honest fallback is the first keyframe rather than the one at 10%.
///
/// Worst case is therefore two bounded passes, and a file too big to reach a keyframe within the
/// budget degrades to the external chain like any other refusal.
///
/// One honest difference from the mp4 path: that one picks the keyframe *nearest* the 10% mark
/// because the sample table lets it look both ways, while this one can only walk forward from where
/// the seek landed, so it takes the first keyframe **at or after** it.
fn thumbnail_native_mkv(path: &Path) -> Option<DynamicImage> {
    match mkv_attempt(path, true, MkvBudget::DEFAULT) {
        MkvAttempt::Frame(img) => Some(img),
        // Only worth a second pass if the first one started somewhere other than the beginning.
        MkvAttempt::NoFrame { sought: true } => {
            match mkv_attempt(path, false, MkvBudget::DEFAULT) {
                MkvAttempt::Frame(img) => Some(img),
                _ => None,
            }
        }
        _ => None,
    }
}

/// What one [`mkv_attempt`] is allowed to read. A parameter rather than the constants baked in at
/// the point of use, purely for testability — production always passes [`MkvBudget::DEFAULT`], and a
/// test can pass something tiny to prove the ceiling really is what stops the scan (the same reason
/// [`spawn_and_wait_with_timeout`] takes its timeout).
#[derive(Debug, Clone, Copy)]
struct MkvBudget {
    bytes: u64,
    packets: usize,
}

impl MkvBudget {
    const DEFAULT: Self = Self {
        bytes: NATIVE_MKV_MAX_SCAN_BYTES,
        packets: NATIVE_MKV_MAX_PACKETS,
    };
}

/// How one [`mkv_attempt`] ended. The distinction that matters is "this file is not ours" (do not
/// spend a second read budget on it) versus "ours, but we did not reach a usable keyframe from where
/// we started" (a second pass from the beginning may well work).
enum MkvAttempt {
    /// A finished thumbnail.
    Frame(DynamicImage),
    /// Wrong container, a codec neither built-in decoder handles, a bitstream the guard refused, or
    /// an unreadable file. Retrying changes nothing.
    Refused,
    /// A decodable stream, but no keyframe turned into a picture before the packet/byte budget ran
    /// out. `sought` says whether this attempt had asked to start at the 10% mark.
    NoFrame { sought: bool },
}

fn mkv_attempt(path: &Path, seek_to_ten_percent: bool, budget: MkvBudget) -> MkvAttempt {
    let Some(source) = BudgetedSource::open(path, budget.bytes) else {
        return MkvAttempt::Refused;
    };
    let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let Ok(mut reader) = MkvReader::try_new(mss, FormatOptions::default()) else {
        return MkvAttempt::Refused;
    };

    // Guard 1 — codec family, from the container, exactly as the mp4 path uses `codec_string`: pick
    // which decoder (if any) this track belongs to and reject VP9/AV1/everything else by ID, before
    // any of it reaches a decoder.
    let Some(track) = find_mkv_track(&reader) else {
        return MkvAttempt::Refused;
    };

    // Guard 2 — the load-bearing one, shared verbatim with the mp4 path. Matroska stores the same
    // `avcC`/`hvcC` record in `CodecPrivate`, so this reads the same bitstream parameter sets.
    let Some(stream) = native_stream_from_config(track.kind, &track.cfg) else {
        return MkvAttempt::Refused;
    };
    let length_size = stream_length_size(&stream);

    let mut sought = false;
    if seek_to_ten_percent {
        if let Some(time) = track.ten_percent {
            // A failed seek is not fatal: the demuxer restores its iterator state on error, so the
            // scan below simply starts wherever it already was (the beginning).
            sought = reader
                .seek(
                    SeekMode::Coarse,
                    SeekTo::Time {
                        time,
                        track_id: Some(track.id),
                    },
                )
                .is_ok();
        }
    }

    let mut packets_left = budget.packets;
    let found = thumbnail_from_keyframes(&stream, || {
        while packets_left > 0 {
            packets_left -= 1;
            // `Err` is the read budget being spent, or a corrupt element — either way, stop. `None`
            // is end of stream.
            let packet = reader.next_packet().ok()??;
            if packet.track_id != track.id
                || packet.data.is_empty()
                || packet.data.len() as u64 > NATIVE_MAX_SAMPLE_BYTES
                || !sample_has_keyframe(track.kind, &packet.data, length_size)
            {
                continue;
            }
            return Some(packet.data.into_vec());
        }
        None
    });

    match found {
        Some(img) => MkvAttempt::Frame(img),
        None => MkvAttempt::NoFrame { sought },
    }
}

/// The one video track [`mkv_attempt`] settled on, lifted out of the reader so the borrow ends
/// before the packet walk needs `&mut reader`.
struct MkvTrack {
    id: u32,
    kind: NativeCodecKind,
    /// The `avcC`/`hvcC` record, verbatim out of Matroska's `CodecPrivate`.
    cfg: Vec<u8>,
    /// Where to seek to, or None when the file does not say enough about its own length to ask.
    ten_percent: Option<Time>,
}

/// The first video track with a codec a built-in decoder handles, as owned data.
///
/// Returns everything the rest of the attempt needs in one go precisely so the reader's borrow ends
/// here: the packet walk afterwards needs `&mut reader`, and the parameter-set bytes have to outlive
/// it. Taking `&dyn FormatReader` keeps this to the two container-agnostic accessors it actually
/// uses (`tracks`, `media_info`).
fn find_mkv_track(reader: &dyn FormatReader) -> Option<MkvTrack> {
    reader.tracks().iter().find_map(|t| {
        let CodecParameters::Video(v) = t.codec_params.as_ref()? else {
            return None;
        };
        let kind = mkv_codec_kind(v.codec)?;
        Some(MkvTrack {
            id: t.id,
            kind,
            cfg: extra_data_for(v, kind)?.to_vec(),
            ten_percent: ten_percent_time(reader.media_info(), t),
        })
    })
}

/// The built-in decoder for a Matroska track's codec ID, or None for one neither handles. The ID
/// counterpart of [`native_codec_kind`], and the same early filter: **VP9 and AV1 are rejected here**,
/// by name, rather than being handed to a decoder that cannot read them. (A WebM file is usually one
/// of those two, so this is the clause that makes `.webm` support honest: konoma opens the container,
/// finds a codec it has no decoder for, and degrades to ffmpeg.)
fn mkv_codec_kind(codec: VideoCodecId) -> Option<NativeCodecKind> {
    match codec {
        video_codecs::CODEC_ID_H264 => Some(NativeCodecKind::Avc),
        video_codecs::CODEC_ID_HEVC => Some(NativeCodecKind::Hevc),
        _ => None,
    }
}

/// The decoder configuration record for `kind`, picked **by extra-data ID** rather than by position.
///
/// A track can carry several extra-data blocks (Dolby Vision configuration alongside the HEVC one,
/// for instance), and they are not in a guaranteed order, so `extra_data.first()` would sooner or
/// later hand `parse_hvcc` something that is not an `hvcC` at all. The IDs are distinct per codec
/// (AVC = 1, HEVC = 2), which also means a mislabelled track cannot smuggle an `avcC` into the HEVC
/// parser: the ID has to agree with the codec ID that chose the decoder.
fn extra_data_for(params: &VideoCodecParameters, kind: NativeCodecKind) -> Option<&[u8]> {
    let wanted = match kind {
        NativeCodecKind::Avc => extra_data_ids::VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG,
        NativeCodecKind::Hevc => extra_data_ids::VIDEO_EXTRA_DATA_ID_HEVC_DECODER_CONFIG,
    };
    params
        .extra_data
        .iter()
        .find(|e| e.id == wanted)
        .map(|e| &*e.data)
}

/// The playback position 10% into the file, or None if it cannot be worked out — matching what the
/// mp4 path and `ffmpegthumbnailer` both aim at, so which frame a user sees does not depend on which
/// extractor produced it.
///
/// Asked of the **media**, not the track: Matroska writes the duration once for the whole segment,
/// and `symphonia`'s demuxer accordingly leaves every `Track::duration` as `None` (measured — an
/// earlier version of this read the track's and so never once performed a seek). Expressed as a
/// `Time` in seconds rather than a raw timestamp for the same reason: the segment and the track can
/// in principle be on different timebases, and `SeekTo::Time` makes the demuxer do that conversion
/// with the track's own.
///
/// Returning None also covers the case where the demuxer's `seek` would **panic**: both of its arms
/// do `track.time_base.unwrap()` on the strength of a comment claiming a track always has one. That
/// would be contained by `catch_silent` like any other decoder panic, but not asking is better than
/// being caught, so a track without a timebase is simply never asked to seek.
fn ten_percent_time(media: &MediaInfo, track: &Track) -> Option<Time> {
    track.time_base?;
    let total = media
        .time_base?
        .calc_time(Timestamp::new(i64::try_from(media.duration?.get()).ok()?))?;
    Some(Time::from_nanos(i64::try_from(total.as_nanos() / 10).ok()?))
}

/// The width of the length prefix in front of each NAL inside a sample, from whichever parameter-set
/// record this track has. Matroska stores H.264/HEVC in the same length-prefixed form mp4 does, so
/// this is what lets the same NAL walk serve both containers.
fn stream_length_size(stream: &NativeStream<'_>) -> usize {
    match stream {
        NativeStream::Avc(avcc) => avcc.length_size,
        NativeStream::Hevc(hvcc) => hvcc.length_size,
    }
}

/// Whether a length-prefixed sample contains a NAL that can start decoding on its own.
///
/// This exists because **symphonia's `Packet` has no keyframe flag** — it carries `track_id`, the
/// timestamps, a duration and the data, and nothing else — while mp4's sample table states it
/// outright (`is_sync`). Handing a non-keyframe to a freshly created decoder does not produce an
/// error; it produces a picture built from references that were never decoded, which is the
/// wrong-picture failure mode this module exists to avoid.
///
/// The NAL type lives in a different place in each codec, and getting *that* wrong would silently
/// classify every packet as a keyframe or none of them:
///   * H.264 (§7.3.1): a 1-byte header, type in the low 5 bits. Type 5 is an IDR slice.
///   * H.265 (§7.4.2.2): a **2-byte** header, type in bits 6..1 of the first byte. Types 16..=21 are
///     the IRAP pictures (BLA, IDR, CRA) — the ones that begin a decodable segment.
fn sample_has_keyframe(kind: NativeCodecKind, sample: &[u8], length_size: usize) -> bool {
    let mut found = false;
    for_each_nal(sample, length_size, |nal| {
        let Some(&first) = nal.first() else {
            return;
        };
        found |= match kind {
            NativeCodecKind::Avc => first & 0x1f == H264_NAL_TYPE_IDR,
            NativeCodecKind::Hevc => HEVC_IRAP_NAL_TYPES.contains(&((first >> 1) & 0x3f)),
        };
    });
    found
}

/// A file read through a hard byte budget, so nothing inside the demuxer can read without bound.
///
/// The budget is enforced here, at the source, rather than by counting the packets we get back,
/// because the walk that actually needs bounding happens **inside** `symphonia`: on a Matroska file
/// with no cue points, `seek` is implemented as a forward scan, so asking for the 10% mark of a 2 GB
/// file would read 200 MB before returning. Once the budget is spent every read fails, which
/// `symphonia` reports as an ordinary I/O error and [`mkv_attempt`] treats as "give up" — the same
/// safe degradation every other refusal in this module takes.
///
/// Seeks are free and deliberately not charged: skipping to a cue point costs no I/O, and charging
/// for it would penalise exactly the files that are cheapest to handle.
struct BudgetedSource {
    file: File,
    len: Option<u64>,
    remaining: u64,
}

impl BudgetedSource {
    fn open(path: &Path, budget: u64) -> Option<Self> {
        let file = File::open(path).ok()?;
        let len = file.metadata().ok().map(|m| m.len());
        Some(Self {
            file,
            len,
            remaining: budget,
        })
    }
}

impl std::io::Read for BudgetedSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("video scan read budget exhausted"));
        }
        let cap = usize::try_from(self.remaining).unwrap_or(usize::MAX);
        let take = buf.len().min(cap);
        let read = self.file.read(&mut buf[..take])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

impl std::io::Seek for BudgetedSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

impl MediaSource for BudgetedSource {
    fn is_seekable(&self) -> bool {
        // Only ever wraps a regular file opened by path; `byte_len` being present says the metadata
        // call succeeded, which is the same thing symphonia's own `impl MediaSource for File` checks.
        self.len.is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.len
    }
}

/// Which built-in decoder a track belongs to, decided from the container's codec string. The two
/// differ in every layer below this — parameter-set record, bitstream syntax, what the decoder gets
/// wrong — so this is where the paths fork and nothing further down has to ask again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeCodecKind {
    /// H.264 / AVC, decoded by `rust_h264`.
    Avc,
    /// H.265 / HEVC, decoded by `rust_h265`. What an iPhone records by default.
    Hevc,
}

/// One track's parameter sets, in whatever shape its decoder wants them, so the shared keyframe loop
/// can hand a sample to either decoder without knowing which codec it is.
enum NativeStream<'a> {
    Avc(rust_h264::nal::AvccConfig<'a>),
    Hevc(HvccConfig),
}

/// The built-in decoder for a track's `codec_string`, or None for a codec neither handles (VP9, AV1,
/// MPEG-4 Part 2, ProRes, …), which degrades to the external chain.
fn native_codec_kind(codec: Option<&str>) -> Option<NativeCodecKind> {
    if codec_string_is_avc(codec) {
        Some(NativeCodecKind::Avc)
    } else if codec_string_is_hevc(codec) {
        Some(NativeCodecKind::Hevc)
    } else {
        None
    }
}

/// Whether a track's `codec_string` names H.264/AVC. `avc1` is the length-prefixed sample entry
/// every muxer in practice writes; `avc3` is the in-band-parameter-set variant, accepted here for
/// completeness (`re_mp4` 0.5 only ever reports `avc1…`, and returns no codec config for a box shape
/// it doesn't model, so such a file falls out at the next guard anyway).
fn codec_string_is_avc(codec: Option<&str>) -> bool {
    codec
        .map(|c| c.starts_with("avc1") || c.starts_with("avc3"))
        .unwrap_or(false)
}

/// Whether a track's `codec_string` names H.265/HEVC. `hvc1` and `hev1` are the two sample entries
/// the spec defines — they differ in whether parameter sets may also appear inline in the samples,
/// not in the bitstream itself, and [`parse_hvcc`] reads the same `hvcC` record for both. iPhones
/// and macOS screen recordings write `hvc1`.
fn codec_string_is_hevc(codec: Option<&str>) -> bool {
    codec
        .map(|c| c.starts_with("hvc1") || c.starts_with("hev1"))
        .unwrap_or(false)
}

/// What the sequence parameter set actually says. Only the fields this module gates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SpsFacts {
    profile_idc: u8,
    /// 0 = monochrome (4:0:0), 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4.
    chroma_format_idc: u32,
    bit_depth_luma: u32,
    bit_depth_chroma: u32,
    /// Coded (pre-crop) size in luma samples — what the decoder actually allocates for.
    width: u32,
    height: u32,
}

/// Whether `rust_h264` decodes this stream *correctly*. This is the single check standing between
/// the user and a silently wrong picture, and every clause of it is load-bearing:
///
/// * **profile** — High 10 (110), 4:2:2 (122) and 4:4:4 (244) decode without an error and produce a
///   plausible-looking but materially wrong image (module doc comment has the measurements).
/// * **chroma format** — the profile does *not* imply it. **Monochrome video is `profile_idc` 100,
///   exactly like ordinary High 4:2:0**, and differs only in `chroma_format_idc = 0` here; since
///   `rust_h264` parses that field and then never reads it, it decodes the stream as if it had
///   chroma planes and desynchronizes immediately. Measured on `ffmpeg -pix_fmt gray -profile:v
///   high`: **97.2% of luma samples wrong, mean |Δ| 83.9, max 255** — a grayscale test pattern came
///   out as pink and green diagonal stripes. That is why this reads the bitstream instead of the
///   container's `profile_idc` byte, which cannot express any of this.
/// * **bit depth** — likewise absent from the profile number for some profiles, and 10-bit samples
///   are what made High 10 wrong in the first place.
/// * **dimensions** — checked here rather than against the container's `tkhd`, because the decoder
///   sizes its buffers from *these* numbers. The two need not agree: a `tkhd` claiming 320×240 in
///   front of an 8192×4320 SPS made `rust_h264` allocate 189 MB (worst case at its own 16384×16384
///   limit is ~1 GB) for a frame this module would then throw away.
fn sps_is_decodable(sps: &SpsFacts) -> bool {
    DECODABLE_PROFILE_IDCS.contains(&sps.profile_idc)
        && sps.chroma_format_idc == CHROMA_FORMAT_420
        && sps.bit_depth_luma == 8
        && sps.bit_depth_chroma == 8
        && sps.width > 0
        && sps.height > 0
        && sps.width <= NATIVE_MAX_DIMENSION
        && sps.height <= NATIVE_MAX_DIMENSION
}

/// Parse the fields of an SPS RBSP that [`sps_is_decodable`] gates on (H.264 §7.3.2.1.1). `rbsp` is
/// `NalUnit::rbsp` as `rust_h264` hands it over: the NAL header byte removed and emulation-prevention
/// bytes already stripped, so `rbsp[0]` is `profile_idc`.
///
/// Returns None on anything malformed or truncated, which the caller treats exactly like "not
/// decodable" — a parameter set we cannot read is not one we should decode on the strength of.
/// Deliberately stops at `frame_mbs_only_flag`; cropping and VUI come later and change nothing this
/// module decides (the coded size is the one that bounds the decoder's allocation).
fn parse_sps_facts(rbsp: &[u8]) -> Option<SpsFacts> {
    let profile_idc = *rbsp.first()?;
    // rbsp[1] = constraint flags, rbsp[2] = level_idc — neither is gated on here.
    let mut r = BitReader::new(rbsp.get(3..)?);

    r.ue()?; // seq_parameter_set_id

    // Absent for Baseline/Main/Extended, where 8-bit 4:2:0 is the only possibility.
    let (mut chroma_format_idc, mut bit_depth_luma, mut bit_depth_chroma) =
        (CHROMA_FORMAT_420, 8, 8);
    if PROFILES_WITH_CHROMA_BLOCK.contains(&profile_idc) {
        chroma_format_idc = r.ue()?;
        if chroma_format_idc == 3 {
            r.bit()?; // separate_colour_plane_flag
        }
        bit_depth_luma = 8 + r.ue()?;
        bit_depth_chroma = 8 + r.ue()?;
        r.bit()?; // qpprime_y_zero_transform_bypass_flag
        if r.bit()? == 1 {
            // seq_scaling_matrix_present_flag: 8 lists for 4:2:0/4:2:2, 12 for 4:4:4.
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.bit()? == 1 {
                    r.skip_scaling_list(if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }

    r.ue()?; // log2_max_frame_num_minus4
    match r.ue()? {
        0 => {
            r.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            r.bit()?; // delta_pic_order_always_zero_flag
            r.se()?; // offset_for_non_ref_pic
            r.se()?; // offset_for_top_to_bottom_field
            let cycle = r.ue()?;
            // Bounded by the spec at 255; a larger value is a malformed/hostile stream, and looping
            // on it would be the read-a-huge-count-and-spin shape this module exists to avoid.
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                r.se()?; // offset_for_ref_frame[i]
            }
        }
        2 => {}
        _ => return None, // pic_order_cnt_type is 0..=2; anything else is malformed
    }
    r.ue()?; // max_num_ref_frames
    r.bit()?; // gaps_in_frame_num_value_allowed_flag

    let width_mbs = r.ue()?.checked_add(1)?;
    let height_map_units = r.ue()?.checked_add(1)?;
    let frame_mbs_only_flag = r.bit()?;

    Some(SpsFacts {
        profile_idc,
        chroma_format_idc,
        bit_depth_luma,
        bit_depth_chroma,
        width: width_mbs.checked_mul(16)?,
        // Field-coded (interlaced) streams store half-height map units.
        height: (2 - frame_mbs_only_flag)
            .checked_mul(height_map_units)?
            .checked_mul(16)?,
    })
}

/// The `hvcC` decoder configuration record (ISO/IEC 14496-15 §8.3.3.1), reduced to what this module
/// needs. Built by [`parse_hvcc`].
struct HvccConfig {
    /// Every parameter-set NAL in the record (VPS/SPS/PPS), concatenated as one **Annex B** byte
    /// stream. `rust_h265` does not accept the length-prefixed HVCC form at all, so this conversion
    /// is mandatory rather than a convenience — see [`decode_keyframe_hevc`].
    parameter_sets: Vec<u8>,
    /// The first SPS as an RBSP: 2-byte NAL header removed and emulation-prevention bytes stripped,
    /// i.e. the same shape `rust_h264` hands over for H.264, ready for [`parse_hevc_sps_facts`].
    sps_rbsp: Vec<u8>,
    /// Bytes of the length prefix in front of each NAL inside a sample (1, 2 or 4 in practice).
    length_size: usize,
}

/// NAL unit types this module looks for in an `hvcC` record (H.265 §7.4.2.2). Only the SPS is read;
/// the others are passed through to the decoder untouched.
const HEVC_NAL_TYPE_SPS: u8 = 33;

/// What the HEVC sequence parameter set actually says. Only the fields this module gates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HevcSpsFacts {
    general_profile_idc: u8,
    /// 0 = monochrome (4:0:0), 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4.
    chroma_format_idc: u32,
    bit_depth_luma: u32,
    bit_depth_chroma: u32,
    /// Coded (pre-conformance-window) size in luma samples — what the decoder allocates for.
    width: u32,
    height: u32,
}

/// Whether `rust_h265` decodes this stream *correctly*. The HEVC counterpart of [`sps_is_decodable`],
/// and deliberately not a copy of it — the two decoders fail differently, so the clauses carry
/// different weight here and it is worth being precise about which one does the work:
///
/// * **profile** — the clause that fires on real files. Every non-Main/Main 10 fixture built for
///   this (4:4:4, 4:2:2 10-bit, 12-bit, monochrome — `ffmpeg -c:v libx265 -pix_fmt …`) is signalled
///   as Range Extensions, `general_profile_idc` 4, whose coding tools `rust_h265` does not implement
///   at all. Unlike H.264 the profile number does not have to carry the chroma/bit-depth condition
///   on its back: HEVC signals both explicitly, and the clauses below read them.
/// * **bit depth** — defense in depth against a stream whose profile is not the truth (the same
///   reason this reads the bitstream instead of `hvcC`, one level further in). It is the only clause
///   with teeth of its own: 12-bit is the one refused case that `rust_h265` **does not** reject
///   itself — see [`DECODABLE_HEVC_BIT_DEPTHS`] for what was measured and why it is still refused.
///   Requiring luma and chroma to be *equal* as well costs nothing and keeps out a mismatched pair,
///   which nothing in Main/Main 10 can produce.
/// * **chroma format** — defense in depth, and honestly labelled as such: `rust_h265`'s own SPS
///   parser returns `Unsupported` for anything but 4:2:0 (measured on all three of 4:4:4, 4:2:2 and
///   monochrome), so unlike H.264's monochrome trap this one cannot silently produce a picture. It
///   is still checked because refusing *before* handing a file to a pre-1.0 decoder is cheaper and
///   more predictable than relying on it to refuse.
/// * **dimensions** — from the SPS rather than the container's `tkhd`, for the same reason as H.264:
///   the decoder sizes its buffers from these numbers and the two need not agree.
fn hevc_sps_is_decodable(sps: &HevcSpsFacts) -> bool {
    DECODABLE_HEVC_PROFILE_IDCS.contains(&sps.general_profile_idc)
        && sps.chroma_format_idc == CHROMA_FORMAT_420
        && sps.bit_depth_luma == sps.bit_depth_chroma
        && DECODABLE_HEVC_BIT_DEPTHS.contains(&sps.bit_depth_luma)
        && sps.width > 0
        && sps.height > 0
        && sps.width <= NATIVE_MAX_DIMENSION
        && sps.height <= NATIVE_MAX_DIMENSION
}

/// Parse the fields of an HEVC SPS RBSP that [`hevc_sps_is_decodable`] gates on (H.265 §7.3.2.2.1).
/// `rbsp` is [`HvccConfig::sps_rbsp`]: NAL header removed, emulation prevention already stripped.
///
/// Returns None on anything malformed or truncated, treated by the caller exactly like "not
/// decodable". Deliberately stops after `bit_depth_chroma_minus8`; everything past it (POC, sub-layer
/// ordering info, CTB sizes, the range-extension flags) changes nothing this module decides.
fn parse_hevc_sps_facts(rbsp: &[u8]) -> Option<HevcSpsFacts> {
    let mut r = BitReader::new(rbsp);
    r.bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = r.bits(3)?;
    if max_sub_layers_minus1 > MAX_HEVC_SUB_LAYERS_MINUS1 {
        return None;
    }
    r.bit()?; // sps_temporal_id_nesting_flag
    let general_profile_idc = skip_profile_tier_level(&mut r, max_sub_layers_minus1)?;

    r.ue()?; // sps_seq_parameter_set_id
    let chroma_format_idc = r.ue()?;
    if chroma_format_idc == 3 {
        r.bit()?; // separate_colour_plane_flag
    }
    let width = r.ue()?;
    let height = r.ue()?;
    if r.bit()? == 1 {
        // conformance_window_flag: four crop offsets, which only shrink the *displayed* rectangle.
        // The coded size read above is the one that bounds the decoder's allocation, so they are
        // read past rather than kept.
        r.ue()?;
        r.ue()?;
        r.ue()?;
        r.ue()?;
    }
    let bit_depth_luma = 8 + r.ue()?;
    let bit_depth_chroma = 8 + r.ue()?;

    Some(HevcSpsFacts {
        general_profile_idc,
        chroma_format_idc,
        bit_depth_luma,
        bit_depth_chroma,
        width,
        height,
    })
}

/// Consume a `profile_tier_level(profilePresentFlag = 1, maxNumSubLayersMinus1)` (H.265 §7.3.3),
/// returning only `general_profile_idc`.
///
/// This has to be exact rather than approximate: it sits in front of every field
/// [`parse_hevc_sps_facts`] actually reads, and it is **not self-delimiting** — miscounting by a
/// single bit does not fail, it silently shifts chroma format, dimensions and bit depth into
/// garbage, which is the one outcome the guard exists to prevent. Hence the sizes are spelled out:
/// 2 + 1 + 5 + 32 profile-compatibility flags + 48 (4 source/constraint flags, 43 constraint flags
/// whose meaning depends on the profile, and 1 `general_inbld_flag`/reserved bit) + 8 level, then a
/// sub-layer table that is present only when the stream has temporal sub-layers.
fn skip_profile_tier_level(r: &mut BitReader<'_>, max_sub_layers_minus1: u32) -> Option<u8> {
    r.bits(2)?; // general_profile_space
    r.bit()?; // general_tier_flag
    let general_profile_idc = r.bits(5)? as u8;
    r.bits(32)?; // general_profile_compatibility_flag[0..32]
    r.bits(24)?; // the 48 constraint/reserved bits, split because `bits` returns a u32
    r.bits(24)?;
    r.bits(8)?; // general_level_idc

    if max_sub_layers_minus1 > 0 {
        let mut profile_present = [false; MAX_HEVC_SUB_LAYERS_MINUS1 as usize];
        let mut level_present = [false; MAX_HEVC_SUB_LAYERS_MINUS1 as usize];
        for i in 0..max_sub_layers_minus1 as usize {
            // `max_sub_layers_minus1 <= 6` is enforced by the caller, so these stay in bounds.
            profile_present[i] = r.bit()? == 1;
            level_present[i] = r.bit()? == 1;
        }
        // reserved_zero_2bits[i] padding, which makes this region a fixed 16 bits regardless of how
        // many sub-layers there are.
        for _ in max_sub_layers_minus1..8 {
            r.bits(2)?;
        }
        for i in 0..max_sub_layers_minus1 as usize {
            if profile_present[i] {
                // Identical in shape to the general portion above, minus the 8-bit level: 88 bits.
                r.bits(2)?;
                r.bit()?;
                r.bits(5)?;
                r.bits(32)?;
                r.bits(24)?;
                r.bits(24)?;
            }
            if level_present[i] {
                r.bits(8)?; // sub_layer_level_idc[i]
            }
        }
    }
    Some(general_profile_idc)
}

/// Read an `hvcC` record (ISO/IEC 14496-15 §8.3.3.1): the length-prefix width, every parameter-set
/// NAL rewritten as an Annex B stream, and the SPS as a bare RBSP for the guard.
///
/// Written here rather than taken from `rust_h265` because the crate deliberately has no container
/// layer at all (its README: *"HVCC (length-prefixed, used in MP4) is not supported — callers must
/// convert to Annex B"*). Every read is bounds-checked against `cfg`, so a truncated or hostile
/// record ends the parse instead of over-reading; the output cannot exceed the input by more than
/// the start codes, which keeps a corrupt `numOfArrays`/`numNalus` from driving a large allocation.
///
/// The fixed prefix also carries `chromaFormat` and `bitDepth*` (bytes 16..19) — deliberately *not*
/// read. That is the container's self-declaration of the same facts [`parse_hevc_sps_facts`] takes
/// from the bitstream, and the whole point of the guard is that the two need not agree.
fn parse_hvcc(cfg: &[u8]) -> Option<HvccConfig> {
    // 22 fixed bytes then `numOfArrays`; a record shorter than that has no parameter sets to give.
    let num_arrays = *cfg.get(22)?;
    let length_size = usize::from((cfg.get(21)? & 0x03) + 1);

    let mut parameter_sets = Vec::new();
    let mut sps_rbsp: Option<Vec<u8>> = None;
    let mut i = 23usize;
    for _ in 0..num_arrays {
        // array_completeness(1) + reserved(1) + NAL_unit_type(6), then a 16-bit NAL count.
        let nal_type = cfg.get(i)? & 0x3f;
        let count = u16::from_be_bytes([*cfg.get(i + 1)?, *cfg.get(i + 2)?]);
        i += 3;
        for _ in 0..count {
            let len = usize::from(u16::from_be_bytes([*cfg.get(i)?, *cfg.get(i + 1)?]));
            i += 2;
            let nal = cfg.get(i..i.checked_add(len)?)?;
            i += len;
            parameter_sets.extend_from_slice(&[0, 0, 0, 1]);
            parameter_sets.extend_from_slice(nal);
            if nal_type == HEVC_NAL_TYPE_SPS && sps_rbsp.is_none() {
                // The HEVC NAL header is two bytes (H.264's is one), and the RBSP begins after it.
                sps_rbsp = Some(strip_emulation_prevention(nal.get(2..)?));
            }
        }
    }
    Some(HvccConfig {
        parameter_sets,
        sps_rbsp: sps_rbsp?,
        length_size,
    })
}

/// Undo the `00 00 03` → `00 00` escaping a NAL payload carries (H.265 §7.4.2, identical to H.264's),
/// turning it back into the RBSP the bit reader expects.
///
/// Not optional for the guard: `profile_tier_level` alone contributes 32 compatibility flags and 43
/// constraint flags that are zero in every ordinary stream, so a real SPS routinely *does* contain an
/// escape byte. Reading past one shifts every subsequent field — the exact silent-corruption failure
/// [`skip_profile_tier_level`] is written to avoid.
fn strip_emulation_prevention(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0usize;
    for &b in payload {
        if zeros >= 2 && b == 0x03 {
            zeros = 0; // drop the emulation-prevention byte itself
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Minimal MSB-first bit reader with the fixed-width and exp-Golomb codings parameter sets use.
/// Shared by both guards — H.264 and H.265 differ in which fields appear, not in how they are
/// coded. Every read is bounds-checked and returns None past the end, so a truncated parameter set
/// can only end the parse, never read past the buffer or loop forever.
struct BitReader<'a> {
    data: &'a [u8],
    /// Next bit to read, counted from the start of `data`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let shift = 7 - (self.pos % 8);
        self.pos += 1;
        Some(u32::from((byte >> shift) & 1))
    }

    /// `u(n)`: the next `n` bits as an unsigned value, MSB first. **All or nothing** — if the buffer
    /// ends partway through, this returns None without having consumed anything, so a truncated
    /// parameter set can't leave the reader mid-field. `n` above 32 would not fit the return type and
    /// is refused rather than silently truncated.
    fn bits(&mut self, n: u32) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let start = self.pos;
        let mut value = 0u32;
        for _ in 0..n {
            match self.bit() {
                Some(b) => value = (value << 1) | b,
                None => {
                    self.pos = start;
                    return None;
                }
            }
        }
        Some(value)
    }

    /// `ue(v)`: N leading zeros, a 1, then N more bits. Capped at 31 leading zeros — a longer run is
    /// malformed (the value could not fit in 32 bits anyway) and the cap is what makes this loop
    /// terminate on a buffer of zeros.
    fn ue(&mut self) -> Option<u32> {
        let mut leading = 0;
        while self.bit()? == 0 {
            leading += 1;
            if leading > 31 {
                return None;
            }
        }
        let mut value = 0u32;
        for _ in 0..leading {
            value = (value << 1) | self.bit()?;
        }
        // (1 << leading) - 1 + value; `leading <= 31` keeps the shift in range.
        (1u32 << leading).checked_sub(1)?.checked_add(value)
    }

    /// `se(v)`: `ue(v)` mapped to the alternating 0, 1, −1, 2, −2, … sequence.
    fn se(&mut self) -> Option<i32> {
        let k = self.ue()?;
        let magnitude = i64::from(k).div_euclid(2) + i64::from(k % 2);
        Some(if k % 2 == 1 {
            magnitude as i32
        } else {
            -(magnitude as i32)
        })
    }

    /// Consume one scaling list (H.264 §7.3.2.1.1.1) without keeping it — we only need to get past it.
    fn skip_scaling_list(&mut self, size: usize) -> Option<()> {
        let mut last_scale = 8i32;
        let mut next_scale = 8i32;
        for _ in 0..size {
            if next_scale != 0 {
                let delta = self.se()?;
                next_scale = (last_scale + delta + 256).rem_euclid(256);
            }
            if next_scale != 0 {
                last_scale = next_scale;
            }
        }
        Some(())
    }
}

/// Indices of the keyframes to try, best first: the sync sample nearest the 10% mark (what
/// `ffmpegthumbnailer` picks by default, so switching extractors doesn't change which frame a user
/// sees), then the ones after it, capped at `limit`. Empty when the track has no sync sample at all.
fn pick_keyframes(samples: &[re_mp4::Sample], limit: usize) -> Vec<usize> {
    let syncs: Vec<usize> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_sync)
        .map(|(i, _)| i)
        .collect();
    if syncs.is_empty() {
        return Vec::new();
    }
    let target = samples.len() / 10;
    // Position *within the sync list* of the keyframe closest to the 10% mark, so the retries below
    // are "the next keyframes", not "the next samples" (most of which are not decodable alone).
    let start = syncs
        .iter()
        .enumerate()
        .min_by_key(|(_, &idx)| idx.abs_diff(target))
        .map(|(pos, _)| pos)
        .unwrap_or(0);
    syncs[start..].iter().copied().take(limit).collect()
}

/// Read exactly the bytes of one sample. Only this range is touched — the rest of the file is never
/// read (see `Mp4::read` above).
fn read_sample_bytes(file: &mut File, offset: u64, size: u64) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0u8; usize::try_from(size).ok()?];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// One decoded picture, normalized to 8-bit 4:2:0 planes so everything downstream — flatness check,
/// YUV→RGB, thumbnail shrink — is written once and shared by both codecs. `y` is `width * height`
/// bytes and the chroma planes are `(width/2) * (height/2)`, matching what both decoders produce.
struct NativeFrame {
    width: u32,
    height: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

/// Decode one keyframe with whichever decoder this track needs.
fn decode_keyframe(stream: &NativeStream<'_>, sample: &[u8]) -> Option<NativeFrame> {
    match stream {
        NativeStream::Avc(avcc) => decode_keyframe_avc(avcc, sample),
        NativeStream::Hevc(hvcc) => decode_keyframe_hevc(hvcc, sample),
    }
}

/// Feed SPS/PPS and one sample's NAL units to a fresh decoder and take the first picture out of it.
///
/// `Decoder` (not `OrderedDecoder`) on purpose: we want *a* correct picture, not a correctly ordered
/// sequence, and the plain decoder hands back the IDR immediately instead of holding it to resolve
/// B-frame display order. A fresh decoder per attempt keeps a failed attempt from poisoning the next
/// keyframe's state.
fn decode_keyframe_avc(
    avcc: &rust_h264::nal::AvccConfig<'_>,
    sample: &[u8],
) -> Option<NativeFrame> {
    let mut decoder = rust_h264::decoder::Decoder::new();
    for nal in avcc.sps_nals.iter().chain(avcc.pps_nals.iter()) {
        decoder.decode_nal(nal).ok()?;
    }
    let mut decoded = None;
    for nal in rust_h264::nal::parse_avcc(sample, avcc.length_size) {
        if let Some(frame) = decoder.decode_nal(&nal).ok()? {
            decoded = Some(frame);
            break;
        }
    }
    let frame = decoded.or_else(|| decoder.flush())?;
    Some(NativeFrame {
        width: frame.width,
        height: frame.height,
        y: frame.y,
        u: frame.u,
        v: frame.v,
    })
}

/// The HEVC counterpart. Same shape as [`decode_keyframe_avc`] — fresh decoder, parameter sets
/// first, stop at the first picture — with two differences the codec forces:
///
/// * **Annex B.** `rust_h265` has no container layer and refuses length-prefixed HVCC data outright,
///   so the parameter sets (already converted by [`parse_hvcc`]) and this sample's NALs are joined
///   into one start-code-delimited stream. The copy is bounded by [`NATIVE_MAX_SAMPLE_BYTES`].
/// * **Bit depth.** Main 10 comes back as 16-bit planes, which [`hevc_frame_to_native`] scales down.
///
/// An `Err` from the decoder ends the attempt rather than being skipped past: it means the stream
/// used something unimplemented, and continuing from there is how a plausible-but-wrong picture gets
/// made. The caller then tries the next keyframe, and failing that the external chain.
fn decode_keyframe_hevc(hvcc: &HvccConfig, sample: &[u8]) -> Option<NativeFrame> {
    let mut stream = hvcc.parameter_sets.clone();
    append_as_annex_b(&mut stream, sample, hvcc.length_size);

    let mut decoder = rust_h265::Decoder::new();
    let mut decoded = None;
    for nal in rust_h265::parse_annex_b(&stream) {
        if let Some(frame) = decoder.decode_nal(&nal).ok()? {
            decoded = Some(frame);
            break;
        }
    }
    let frame = decoded.or_else(|| decoder.flush())?;
    hevc_frame_to_native(&frame)
}

/// Walk the NAL units of a length-prefixed sample (each NAL preceded by a `length_size`-byte
/// big-endian length), handing each payload to `f`.
///
/// One implementation shared by the two things that need to look inside a sample — the Annex B
/// conversion the HEVC decoder requires, and the keyframe test the Matroska path needs because its
/// packets carry no such flag. A prefix that runs past the end of the buffer ends the walk and keeps
/// whatever came before it: a truncated sample costs its tail, not the frame.
fn for_each_nal(sample: &[u8], length_size: usize, mut f: impl FnMut(&[u8])) {
    // Never zero in practice (both `avcC` and `hvcC` store the width minus one, so it is 1..=4), but
    // a zero would advance the cursor by nothing on every iteration and loop forever.
    if length_size == 0 {
        return;
    }
    let mut i = 0usize;
    while i + length_size <= sample.len() {
        let mut len = 0usize;
        for k in 0..length_size {
            len = (len << 8) | usize::from(sample[i + k]);
        }
        i += length_size;
        let Some(nal) = sample.get(i..i.saturating_add(len)) else {
            return;
        };
        f(nal);
        i += len;
    }
}

/// Rewrite a length-prefixed sample as Annex B start codes, appending to `out`.
fn append_as_annex_b(out: &mut Vec<u8>, sample: &[u8], length_size: usize) {
    for_each_nal(sample, length_size, |nal| {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    });
}

/// Normalize a `rust_h265` frame to 8-bit planes.
///
/// Main 10 hands back `PixelData::U16`, so the samples are rescaled with the actual linear map
/// (`v * 255 / 1023`, rounded to nearest) rather than shifted. The obvious `v >> 2` is a near miss,
/// not a disaster — measured across all 1024 possible inputs it lands on a different level for 170
/// of them, always by exactly one (85 too bright, 85 too dark), and it agrees at both endpoints —
/// but it divides by 4 where the range is 1023, and truncates on top of that. Since this runs once
/// per thumbnail on a single frame, the approximation buys nothing worth its inaccuracy.
fn hevc_frame_to_native(frame: &rust_h265::Frame) -> Option<NativeFrame> {
    // The guard already restricted this to 8 or 10, but the shift below must not be reachable with a
    // wild value from a decoder we do not control.
    if !(8..=16).contains(&frame.bit_depth) {
        return None;
    }
    let max = (1u32 << frame.bit_depth) - 1;
    Some(NativeFrame {
        width: frame.width,
        height: frame.height,
        y: plane_to_8bit(&frame.y, max),
        u: plane_to_8bit(&frame.u, max),
        v: plane_to_8bit(&frame.v, max),
    })
}

/// One plane of a `rust_h265` frame as 8-bit samples. 8-bit input is already there and is copied
/// unchanged; `max` is the full-scale value for the frame's bit depth, so the rescale is the identity
/// at 8 bits too.
fn plane_to_8bit(plane: &rust_h265::PixelData, max: u32) -> Vec<u8> {
    match plane {
        rust_h265::PixelData::U8(v) => v.clone(),
        rust_h265::PixelData::U16(v) => v
            .iter()
            .map(|&s| ((u32::from(s).min(max) * 255 + max / 2) / max) as u8)
            .collect(),
    }
}

/// max − min over the luma plane: 0 for a solid color, large for any real picture.
fn luma_spread(y: &[u8]) -> u8 {
    let mut min = u8::MAX;
    let mut max = u8::MIN;
    for &v in y {
        min = min.min(v);
        max = max.max(v);
    }
    max.saturating_sub(min)
}

/// Convert a decoded 8-bit 4:2:0 frame to RGB. Shared by both codecs — by the time a frame gets
/// here it is plain 8-bit planes either way ([`NativeFrame`]).
///
/// Limited ("TV") range, which is what essentially all H.264 and HEVC in the wild uses. The matrix is
/// picked by height — BT.709 at 720p and above, BT.601 below — which is the same heuristic every
/// player uses as a default. This is an **approximation**: the authoritative answer lives in the
/// SPS's VUI `colour_primaries`/`matrix_coefficients`, which neither decoder surfaces. Getting it
/// wrong shifts saturation slightly on an unusually-tagged clip; it never produces the kind of
/// structural garbage the profile guard exists to prevent. (HDR clips are tagged BT.2020 and would
/// be more than slightly off — but they are 10-bit HEVC in the Main 10 profile, which this path does
/// accept, so the honest statement is that an HDR thumbnail comes out flat and desaturated rather
/// than tone-mapped. ffmpeg's would too, without an explicit tonemap filter.)
fn frame_to_image(frame: &NativeFrame) -> Option<DynamicImage> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    // Both decoders size the chroma planes with truncating division, so an odd display dimension
    // leaves the last row/column without its own chroma sample; the clamps below reuse the last one.
    let (cw, ch) = (w / 2, h / 2);
    // The dimension bound is re-checked here, not just on the track header: the decoded size comes
    // from the SPS, and the two do not have to agree. This is what keeps the `w * h * 3` allocation
    // below bounded by something the header actually promised.
    if w < 2
        || h < 2
        || w > NATIVE_MAX_DIMENSION as usize
        || h > NATIVE_MAX_DIMENSION as usize
        || frame.y.len() < w * h
        || frame.u.len() < cw * ch
        || frame.v.len() < cw * ch
    {
        return None;
    }
    let bt709 = h >= 720;
    let (kr_v, kg_u, kg_v, kb_u) = if bt709 {
        (1.792_7_f32, -0.213_2, -0.532_9, 2.112_4)
    } else {
        (1.596_0_f32, -0.391_8, -0.813_0, 2.017_2)
    };

    let mut buf = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let cy = (y / 2).min(ch.saturating_sub(1));
        for x in 0..w {
            let cx = (x / 2).min(cw.saturating_sub(1));
            let yy = (f32::from(frame.y[y * w + x]) - 16.0) * 1.164_4;
            let u = f32::from(frame.u[cy * cw + cx]) - 128.0;
            let v = f32::from(frame.v[cy * cw + cx]) - 128.0;
            buf.push(clamp_u8(yy + kr_v * v));
            buf.push(clamp_u8(yy + kg_u * u + kg_v * v));
            buf.push(clamp_u8(yy + kb_u * u));
        }
    }
    let rgb = image::RgbImage::from_raw(w as u32, h as u32, buf)?;
    Some(DynamicImage::ImageRgb8(rgb))
}

fn clamp_u8(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

/// Shrink so the long side is at most [`THUMB_MAX_PX`], leaving anything smaller untouched (there is
/// nothing to gain from upscaling here — the image path scales to terminal cells anyway).
/// `Lanczos3` because the default `Nearest` has caused a visible quality regression before
/// (see [[image-preview-filter-nearest-gotcha]]).
fn shrink_to_thumb(img: DynamicImage) -> DynamicImage {
    let long = img.width().max(img.height());
    if long <= THUMB_MAX_PX {
        return img;
    }
    img.resize(
        THUMB_MAX_PX,
        THUMB_MAX_PX,
        image::imageops::FilterType::Lanczos3,
    )
}

/// The external chain: `ffmpegthumbnailer`, then `ffmpeg`. Reached only when the pure-Rust path
/// declined *and* `[external] video` allows launching a process.
fn thumbnail_external(path: &Path) -> Option<DynamicImage> {
    let out = temp_png_path();
    // Prefer ffmpegthumbnailer (dedicated, fast, auto-picks a representative frame); fall back to ffmpeg.
    let ok = run_ffmpegthumbnailer(path, &out) || run_ffmpeg(path, &out);
    let img = if ok {
        image::ImageReader::open(&out)
            .ok()
            .and_then(|r| r.with_guessed_format().ok())
            .and_then(|r| r.decode().ok())
    } else {
        None
    };
    let _ = std::fs::remove_file(&out); // delete the temp file right away (regardless of success)
    img
}

/// Extract with ffmpegthumbnailer. `-s` = max side px / `-q` = quality (1-10) / `-c png`. The representative frame is auto-selected
/// by default (playback position ~10%). Returns true if it succeeds and the output is non-empty.
fn run_ffmpegthumbnailer(path: &Path, out: &Path) -> bool {
    let mut cmd = Command::new("ffmpegthumbnailer");
    cmd.arg("-i")
        .arg(path)
        .arg("-o")
        .arg(out)
        .arg("-s")
        .arg(THUMB_MAX_PX.to_string())
        .arg("-q")
        .arg("8")
        .arg("-c")
        .arg("png")
        // stdin closed (not inherited): without this, ffmpegthumbnailer/ffmpeg contend with
        // crossterm for the same keypress bytes off konoma's own controlling terminal while the
        // media worker thread runs it (`Command`'s documented default, absent an explicit
        // `.stdin(...)`, is `Stdio::inherit()` — see `ffmpeg_tools_never_inherit_this_process_
        // stdin` below for how this is verified deterministically).
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    matches!(spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT), Some(s) if s.success())
        && out_is_nonempty(out)
}

/// Extract with ffmpeg. The `thumbnail` filter picks one representative frame from the first batch, and `scale` caps the max side
/// (`-2` = codec-compatible even dimensions). `-frames:v 1` = one frame / `-y` = overwrite / `-loglevel error` = quiet.
fn run_ffmpeg(path: &Path, out: &Path) -> bool {
    let vf = format!("thumbnail,scale='min({THUMB_MAX_PX},iw)':-2");
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        // `-nostdin`: ffmpeg's own documented switch for "never read stdin for interactive
        // playback controls" — belt-and-suspenders alongside `.stdin(Stdio::null())` below, which
        // is what actually prevents it from ever seeing konoma's terminal input in the first place.
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(vf)
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    matches!(spawn_and_wait_with_timeout(&mut cmd, TOOL_TIMEOUT), Some(s) if s.success())
        && out_is_nonempty(out)
}

/// Spawns `cmd` and waits for it to exit, **polling** rather than blocking indefinitely
/// (`Command::status()`'s behavior) — past `timeout` the child is killed and this returns `None`,
/// exactly like "the tool failed" to every caller here (principle #3: this degrades to `[can not
/// preview]`, it never hangs the app). `timeout` is a parameter (not a call to the `TOOL_TIMEOUT`
/// constant baked in here) purely for testability: production call sites above always pass
/// `TOOL_TIMEOUT`; tests pass something short so a deliberately-hanging fixture command doesn't
/// make the test suite itself slow.
fn spawn_and_wait_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let mut child: Child = cmd.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait(); // reap; the (now-meaningless, killed) status is discarded
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return None,
        }
    }
}

/// Whether the output file exists and is non-empty (verify by the actual file, since the content can be empty even with exit code 0).
fn out_is_nonempty(out: &Path) -> bool {
    std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false)
}

/// Lazily creates (once per process), and returns the path to, a private `0700` (owner-only)
/// subdirectory under the system temp dir for this module's extracted-frame PNGs.
/// `std::env::temp_dir()` is world-writable/world-readable on Linux (`/tmp` is mode `1777`) — a
/// rendered video frame would otherwise briefly sit at a pid-and-counter-predictable path readable
/// by any other local user on a shared box, since the PNG itself is created by `ffmpeg`/
/// `ffmpegthumbnailer` (an external process we don't control) under whatever mode their own
/// default umask picks (typically `0644`). Restricting the *directory* (rather than trying to
/// `chmod` the file after an external tool writes it, which we have no reliable hook for anyway)
/// is what actually closes this: POSIX requires execute/search permission on every ancestor
/// directory to open a file by path, so `0700` here blocks other users regardless of the mode the
/// external tool used for the file itself.
fn private_temp_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("konoma-vthumb-{}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            // The mode is applied atomically by `mkdir(2)` itself (masked by umask, but `0o700`
            // has no group/other bits for umask to strip), so there's no "create, then chmod" gap
            // where a wider-permission window briefly exists.
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
            // Defense-in-depth for the unlikely case the directory already existed with looser
            // permissions (e.g. a stale leftover from an earlier process that reused this pid).
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::create_dir(&dir);
        }
        dir
    })
    .clone()
}

/// Return a temp PNG path that does not collide within the process. Made unique with pid + an atomic counter (no dependence on randomness/time).
fn temp_png_path() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    private_temp_dir().join(format!("thumb-{n}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;

    /// Resolves a fixture bundled under the repo's `samples/` directory, anchored at
    /// `CARGO_MANIFEST_DIR` (baked in at compile time) rather than a bare relative path — a plain
    /// `Path::new("samples/…")` resolves against the test binary's **cwd**, which is only the crate
    /// root by convention. Tolerant of the one case where the fixture is legitimately absent
    /// (`samples/` is excluded from the published crate) by returning `None`, but saying so loudly
    /// instead of silently passing zero assertions. Mirrors `preview/pdf.rs`'s identical helper.
    fn sample_path_or_skip(name: &str) -> Option<PathBuf> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("samples")
            .join(name);
        if p.exists() {
            Some(p)
        } else {
            eprintln!(
                "SKIP: samples/{name} not found (excluded from the published crate) — this test verifies nothing this run"
            );
            None
        }
    }

    /// The inverse of [`BitReader`], for building synthetic parameter sets. Shared by the H.264 and
    /// H.265 builders below so the exp-Golomb encoding exists in exactly one place — two copies
    /// could drift, and a drifting *test* helper is worse than none.
    struct BitWriter {
        bytes: Vec<u8>,
        bits: u32,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                bits: 0,
            }
        }

        fn bit(&mut self, b: u32) {
            if self.bits.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if b == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 1 << (7 - (self.bits % 8));
            }
            self.bits += 1;
        }

        /// `u(n)`: `n` bits of `v`, MSB first — the inverse of `BitReader::bits`.
        fn bits(&mut self, n: u32, v: u32) {
            for i in (0..n).rev() {
                self.bit((v >> i) & 1);
            }
        }

        /// Exp-Golomb `ue(v)`, the inverse of `BitReader::ue`.
        fn ue(&mut self, v: u32) {
            let n = v + 1;
            let len = 32 - n.leading_zeros();
            for _ in 0..len - 1 {
                self.bit(0);
            }
            for i in (0..len).rev() {
                self.bit((n >> i) & 1);
            }
        }
    }

    /// Builds a synthetic SPS RBSP (as `rust_h264` hands it over: NAL header removed, emulation
    /// prevention already stripped) carrying exactly the fields [`sps_is_decodable`] gates on.
    /// Synthetic rather than a fixture because the interesting cases — monochrome, 4:2:2, 4:4:4,
    /// 10-bit — cannot be produced without the very tool this module exists to stop requiring.
    fn synthetic_sps(
        profile_idc: u8,
        chroma_format_idc: u32,
        bit_depth_luma: u32,
        bit_depth_chroma: u32,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.ue(0); // seq_parameter_set_id
        if PROFILES_WITH_CHROMA_BLOCK.contains(&profile_idc) {
            w.ue(chroma_format_idc);
            if chroma_format_idc == 3 {
                w.bit(0); // separate_colour_plane_flag
            }
            w.ue(bit_depth_luma - 8);
            w.ue(bit_depth_chroma - 8);
            w.bit(0); // qpprime_y_zero_transform_bypass_flag
            w.bit(0); // seq_scaling_matrix_present_flag
        }
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(2); // pic_order_cnt_type = 2
        w.ue(1); // max_num_ref_frames
        w.bit(0); // gaps_in_frame_num_value_allowed_flag
        w.ue(width / 16 - 1);
        w.ue(height / 16 - 1);
        w.bit(1); // frame_mbs_only_flag

        let mut rbsp = vec![profile_idc, 0x00, 30];
        rbsp.extend_from_slice(&w.bytes);
        rbsp
    }

    /// The guard that stands between the user and a **silently wrong picture**, exercised over every
    /// axis the profile number alone cannot express.
    ///
    /// `rust_h264` decodes Baseline/Main/High 8-bit 4:2:0 and nothing else, and for the rest it
    /// returns no error — it returns a wrong image. Measured: on a 10-bit clip, mean luma 126.6
    /// against a correct 128.0 while 53.0% of pixels differed by up to 58 levels; on a **monochrome**
    /// clip (`ffmpeg -pix_fmt gray -profile:v high`, which is `profile_idc` 100 exactly like ordinary
    /// High 4:2:0) 97.2% of luma samples were wrong by a mean of 83.9 and the grayscale test pattern
    /// came out as pink and green diagonal stripes. Nothing about the decoded image gives either
    /// away, so the refusal has to happen here, from the bitstream, before decoding.
    #[test]
    fn sps_guard_admits_only_8bit_420_baseline_main_high() {
        let decodable = |sps: &[u8]| parse_sps_facts(sps).is_some_and(|f| sps_is_decodable(&f));

        // Accepted: the three profiles, at 8-bit 4:2:0.
        for ok in [66u8, 77, 100] {
            assert!(
                decodable(&synthetic_sps(ok, 1, 8, 8, 320, 240)),
                "profile_idc {ok} の 8bit 4:2:0 は rust_h264 が正しくデコードできる(受理すべき)"
            );
        }

        // Rejected by chroma format — **`profile_idc` is 100 in every one of these**, so the profile
        // number cannot tell them apart from the accepted case above.
        assert!(
            !decodable(&synthetic_sps(100, 0, 8, 8, 320, 240)),
            "モノクロ(4:0:0)は profile 100 のまま=profile では区別できない。rust_h264 は chroma_format_idc を読まないので 4:2:0 として誤パースし絵が壊れる"
        );
        assert!(
            !decodable(&synthetic_sps(100, 2, 8, 8, 320, 240)),
            "4:2:2 は拒否"
        );
        assert!(
            !decodable(&synthetic_sps(100, 3, 8, 8, 320, 240)),
            "4:4:4 は拒否"
        );

        // Rejected by bit depth — again profile 100 throughout.
        assert!(
            !decodable(&synthetic_sps(100, 1, 10, 10, 320, 240)),
            "10bit は拒否"
        );
        assert!(
            !decodable(&synthetic_sps(100, 1, 8, 10, 320, 240)),
            "輝度だけ 8bit でも色差が 10bit なら拒否"
        );

        // Rejected by profile.
        for bad in [110u8, 122, 244] {
            assert!(
                !decodable(&synthetic_sps(bad, 1, 8, 8, 320, 240)),
                "profile_idc {bad} は壊れた絵を出すので拒否"
            );
        }
        for unknown in [0u8, 1, 88, 128, 255] {
            assert!(
                !decodable(&synthetic_sps(unknown, 1, 8, 8, 320, 240)),
                "未知の profile_idc {unknown} も拒否(未知は推測の許可証ではない)"
            );
        }

        // Rejected by size. This is the bound that actually constrains the decoder's allocation —
        // an 8192x4320 SPS behind a 320x240 `tkhd` made rust_h264 allocate 189 MB.
        assert!(
            decodable(&synthetic_sps(100, 1, 8, 8, 8192, 8192)),
            "上限ちょうどは受理"
        );
        assert!(
            !decodable(&synthetic_sps(100, 1, 8, 8, 8208, 8192)),
            "上限超の幅は拒否"
        );
        assert!(
            !decodable(&synthetic_sps(100, 1, 8, 8, 8192, 8208)),
            "上限超の高さは拒否"
        );

        // Unreadable parameter sets end the parse rather than being decoded on faith.
        assert!(parse_sps_facts(&[]).is_none(), "空の SPS");
        assert!(parse_sps_facts(&[100]).is_none(), "profile だけの SPS");
        assert!(
            parse_sps_facts(&[100, 0, 30]).is_none(),
            "ヘッダ 3 バイトだけの SPS"
        );
        let full = synthetic_sps(100, 1, 8, 8, 320, 240);
        assert!(
            parse_sps_facts(&full[..full.len() - 1]).is_none()
                || parse_sps_facts(&full[..4]).is_none(),
            "途中で切れた SPS はパースを終える(バッファ外を読まない・無限ループしない)"
        );
        // A buffer of zeros is all-zero exp-Golomb prefixes: the leading-zero cap must end it.
        assert!(parse_sps_facts(&[100, 0, 30, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    /// Ground truth for the parser: the real SPS in `samples/sample.mp4` must read back as exactly
    /// what the file is. Without this, the synthetic cases above only prove the parser is
    /// self-consistent with the test's own bit writer.
    #[test]
    fn sps_parse_matches_the_bundled_sample() {
        let Some(p) = sample_path_or_skip("sample.mp4") else {
            return;
        };
        let mut file = File::open(&p).unwrap();
        let size = file.metadata().unwrap().len();
        let mp4 = re_mp4::Mp4::read(&mut file, size).unwrap();
        let track = mp4
            .tracks()
            .values()
            .find(|t| t.kind == Some(re_mp4::TrackKind::Video))
            .expect("video track");
        let cfg = track.raw_codec_config(&mp4).unwrap();
        let avcc = rust_h264::nal::parse_avcc_config(&cfg).unwrap();
        let facts = parse_sps_facts(&avcc.sps_nals[0].rbsp).expect("SPS が読める");
        assert_eq!(
            facts,
            SpsFacts {
                profile_idc: 100,
                chroma_format_idc: 1,
                bit_depth_luma: 8,
                bit_depth_chroma: 8,
                width: 320,
                height: 240,
            },
            "実ファイルの SPS を取り違えている"
        );
    }

    /// The guard reads the **bitstream**, not the container's summary of it — and is wired into the
    /// extraction path, not merely present as a function.
    ///
    /// Both halves patch a single byte of `samples/sample.mp4` and assert opposite outcomes, which
    /// is what makes this discriminating:
    ///
    /// * Patching the **`avcC` record's** `AVCProfileIndication` (payload index 1) must change
    ///   *nothing*. That byte is the container's self-declaration; it is also where `codec_string`
    ///   comes from, so the two are not independent defenses. An earlier version of this module
    ///   gated on it, and flipping this one byte to 100 was enough to walk a genuine High 10 stream
    ///   straight past the guard.
    /// * Patching the **SPS's** `profile_idc` (the first byte of the SPS NAL's payload) must be
    ///   refused, even though the bitstream underneath is untouched, decodable 8-bit High. Drop the
    ///   guard and this returns a picture instead.
    #[test]
    fn guard_reads_the_bitstream_not_the_container() {
        let Some(src) = sample_path_or_skip("sample.mp4") else {
            return;
        };
        let original = std::fs::read(&src).unwrap();
        let avcc_at = original
            .windows(4)
            .position(|w| w == b"avcC")
            .expect("samples/sample.mp4 に avcC 記録がある")
            + 4;
        // avcC payload: [0]=configurationVersion, [1]=AVCProfileIndication, [2]=compat, [3]=level,
        // [4]=lengthSizeMinusOne, [5]=numOfSPS, [6..8]=spsLength, [8]=SPS NAL header, [9]=profile_idc.
        let container_profile = avcc_at + 1;
        let sps_profile = avcc_at + 9;
        assert_eq!(
            original[avcc_at], 1,
            "avcC の先頭は configurationVersion=1(検算)"
        );
        assert_eq!(original[container_profile], 100, "コンテナ申告は High(100)");
        assert_eq!(
            original[avcc_at + 8] & 0x1f,
            7,
            "SPS NAL(type 7)を指している(検算)"
        );
        assert_eq!(original[sps_profile], 100, "ビットストリームも High(100)");

        let dir = unique_tmp("konoma_video_bitstream_guard_test");
        std::fs::create_dir_all(&dir).unwrap();

        // Control: the unpatched copy really does decode, so a None below can only be the guard.
        let control = dir.join("control.mp4");
        std::fs::write(&control, &original).unwrap();
        assert!(
            thumbnail_native(&control).is_some(),
            "対照: 無改変のコピーはデコードできる"
        );

        for bad in [110u8, 122, 244] {
            let mut bytes = original.clone();
            bytes[container_profile] = bad;
            let p = dir.join(format!("container{bad}.mp4"));
            std::fs::write(&p, &bytes).unwrap();
            assert!(
                thumbnail_native(&p).is_some(),
                "コンテナの申告({bad})は判断材料でない=ビットストリームが 8bit 4:2:0 High なら描ける"
            );

            let mut bytes = original.clone();
            bytes[sps_profile] = bad;
            let p = dir.join(format!("sps{bad}.mp4"));
            std::fs::write(&p, &bytes).unwrap();
            assert!(
                thumbnail_native(&p).is_none(),
                "SPS が profile {bad} と言うならデコード前に拒否する(ビットストリームが実際にはデコードできても)"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The knobs [`hevc_sps_is_decodable`] gates on, as a struct rather than seven positional
    /// arguments — `synthetic_hevc_sps(4, 1, 8, 8, 320, 240, 0)` reads as noise, and two adjacent
    /// `u32`s that mean different things are exactly the swap this project has been bitten by.
    #[derive(Debug, Clone, Copy)]
    struct HevcSpsSpec {
        profile_idc: u8,
        chroma_format_idc: u32,
        bit_depth_luma: u32,
        bit_depth_chroma: u32,
        width: u32,
        height: u32,
        /// `sps_max_sub_layers_minus1`. Non-zero makes `profile_tier_level` grow a sub-layer table,
        /// which is the part of the skip most likely to be miscounted.
        sub_layers_minus1: u32,
    }

    impl Default for HevcSpsSpec {
        /// Plain Main profile, 8-bit 4:2:0, 320x240, no sub-layers — i.e. decodable. Every case
        /// below changes exactly one thing from here, so what is under test is never ambiguous.
        fn default() -> Self {
            Self {
                profile_idc: 1,
                chroma_format_idc: 1,
                bit_depth_luma: 8,
                bit_depth_chroma: 8,
                width: 320,
                height: 240,
                sub_layers_minus1: 0,
            }
        }
    }

    /// Builds a synthetic HEVC SPS RBSP (NAL header removed, emulation prevention already stripped,
    /// i.e. what [`parse_hvcc`] produces) carrying the fields [`hevc_sps_is_decodable`] gates on.
    /// Synthetic for the same reason the H.264 one is: producing a real 12-bit or 4:4:4 clip needs
    /// the very tool this module exists to stop requiring.
    fn synthetic_hevc_sps(spec: HevcSpsSpec) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.bits(4, 0); // sps_video_parameter_set_id
        w.bits(3, spec.sub_layers_minus1);
        w.bit(1); // sps_temporal_id_nesting_flag

        // --- profile_tier_level(1, sub_layers_minus1) ---
        w.bits(2, 0); // general_profile_space
        w.bit(0); // general_tier_flag
        w.bits(5, u32::from(spec.profile_idc));
        // 32 compatibility flags; bit `profile_idc` set, as a real encoder writes it.
        w.bits(32, 1u32 << (31 - spec.profile_idc.min(31)));
        w.bits(24, 0); // 48 constraint/reserved bits
        w.bits(24, 0);
        w.bits(8, 90); // general_level_idc
        if spec.sub_layers_minus1 > 0 {
            for _ in 0..spec.sub_layers_minus1 {
                w.bit(1); // sub_layer_profile_present_flag
                w.bit(1); // sub_layer_level_present_flag
            }
            for _ in spec.sub_layers_minus1..8 {
                w.bits(2, 0); // reserved_zero_2bits
            }
            for _ in 0..spec.sub_layers_minus1 {
                w.bits(2, 0); // sub_layer_profile_space
                w.bit(0); // sub_layer_tier_flag
                w.bits(5, 1); // sub_layer_profile_idc
                w.bits(32, 0); // sub_layer_profile_compatibility_flag[32]
                w.bits(24, 0); // 48 constraint/reserved bits
                w.bits(24, 0);
                w.bits(8, 90); // sub_layer_level_idc
            }
        }

        w.ue(0); // sps_seq_parameter_set_id
        w.ue(spec.chroma_format_idc);
        if spec.chroma_format_idc == 3 {
            w.bit(0); // separate_colour_plane_flag
        }
        w.ue(spec.width);
        w.ue(spec.height);
        w.bit(0); // conformance_window_flag
        w.ue(spec.bit_depth_luma - 8);
        w.ue(spec.bit_depth_chroma - 8);
        w.bytes
    }

    /// The HEVC guard, over every axis it gates on — the counterpart of
    /// `sps_guard_admits_only_8bit_420_baseline_main_high`, and load-bearing for the same reason:
    /// nothing about the decoded image reveals that a stream was outside what `rust_h265` implements,
    /// so the refusal has to happen here, from the bitstream, before decoding.
    ///
    /// What each group is actually protecting against was measured on real fixtures built with
    /// `ffmpeg -c:v libx265 -pix_fmt …` (4:4:4, 4:2:2 10-bit, 12-bit, gray):
    ///
    /// * **profile** — all four are signalled as Range Extensions (`general_profile_idc` 4), so in
    ///   practice this is the clause that fires. Its tools are not implemented at all.
    /// * **bit depth** — 12-bit is the one case `rust_h265` does **not** refuse by itself: it decoded
    ///   and returned a picture with no error (byte-exact against ffmpeg, as it happens), down a path
    ///   its own README calls untested. Refusing unverified is the point; see
    ///   [`DECODABLE_HEVC_BIT_DEPTHS`].
    /// * **chroma format** — 4:4:4, 4:2:2 and monochrome all stop inside `rust_h265` with
    ///   `Unsupported`, so this clause is defense in depth. Checked anyway: refusing before handing a
    ///   file to a pre-1.0 decoder is cheaper and more predictable than relying on it to refuse.
    /// * **sub-layers** — not a rejection at all, but the case that proves `profile_tier_level` is
    ///   skipped bit-exactly. That structure is not self-delimiting: miscount it by one bit and
    ///   chroma format, dimensions and bit depth all silently become garbage.
    #[test]
    fn hevc_sps_guard_admits_only_main_and_main10_420() {
        let facts = |spec: HevcSpsSpec| parse_hevc_sps_facts(&synthetic_hevc_sps(spec));
        let decodable = |spec: HevcSpsSpec| facts(spec).is_some_and(|f| hevc_sps_is_decodable(&f));

        // Accepted: Main 8-bit and Main 10 10-bit, both 4:2:0.
        assert!(decodable(HevcSpsSpec::default()), "Main 8bit 4:2:0 は受理");
        assert!(
            decodable(HevcSpsSpec {
                profile_idc: 2,
                bit_depth_luma: 10,
                bit_depth_chroma: 10,
                ..Default::default()
            }),
            "Main 10 の 10bit 4:2:0 は受理"
        );

        // The parse itself must be right, not merely permissive — otherwise "accepted" above could
        // be an accident of reading the wrong bits.
        assert_eq!(
            facts(HevcSpsSpec {
                profile_idc: 2,
                bit_depth_luma: 10,
                bit_depth_chroma: 10,
                width: 1920,
                height: 1080,
                ..Default::default()
            }),
            Some(HevcSpsFacts {
                general_profile_idc: 2,
                chroma_format_idc: 1,
                bit_depth_luma: 10,
                bit_depth_chroma: 10,
                width: 1920,
                height: 1080,
            })
        );

        // Rejected by chroma format. 0 = monochrome, 2 = 4:2:2, 3 = 4:4:4 (which also carries an
        // extra flag in the bitstream — if that were mis-skipped the fields after it would be
        // garbage, so this doubles as a check that the parse stays in step).
        for chroma in [0u32, 2, 3] {
            assert!(
                !decodable(HevcSpsSpec {
                    chroma_format_idc: chroma,
                    ..Default::default()
                }),
                "chroma_format_idc {chroma} は 4:2:0 でないので拒否"
            );
        }

        // Rejected by bit depth, with the profile left at Main so it is unambiguously this clause
        // doing the work.
        for depth in [9u32, 11, 12, 14, 16] {
            assert!(
                !decodable(HevcSpsSpec {
                    bit_depth_luma: depth,
                    bit_depth_chroma: depth,
                    ..Default::default()
                }),
                "{depth}bit は rust_h265 が検証していないので拒否(12bit はエラーも出さずに絵を返す)"
            );
        }
        assert!(
            !decodable(HevcSpsSpec {
                profile_idc: 2,
                bit_depth_luma: 8,
                bit_depth_chroma: 10,
                ..Default::default()
            }),
            "輝度と色差で bit depth が食い違うものは拒否"
        );

        // Rejected by profile. 4 is Range Extensions — what ffmpeg signals for every 4:4:4 / 4:2:2 /
        // 12-bit / monochrome clip — and the rest are the still-picture, scalable and multi-view
        // families `rust_h265` does not implement.
        for bad in [0u8, 3, 4, 5, 6, 7, 9, 31] {
            assert!(
                !decodable(HevcSpsSpec {
                    profile_idc: bad,
                    ..Default::default()
                }),
                "general_profile_idc {bad} は Main/Main 10 でないので拒否(未知は推測の許可証ではない)"
            );
        }

        // Rejected by size — the bound that actually constrains what the decoder allocates, read
        // from the SPS rather than the container's `tkhd`.
        assert!(
            decodable(HevcSpsSpec {
                width: 8192,
                height: 8192,
                ..Default::default()
            }),
            "上限ちょうどは受理"
        );
        for (w, h) in [(8193, 8192), (8192, 8193), (0, 240), (320, 0)] {
            assert!(
                !decodable(HevcSpsSpec {
                    width: w,
                    height: h,
                    ..Default::default()
                }),
                "{w}x{h} は拒否"
            );
        }

        // Sub-layers: the fields after `profile_tier_level` must still land exactly, at every legal
        // count. A miscounted skip would not error — it would quietly read a wrong chroma format.
        for n in 0..=MAX_HEVC_SUB_LAYERS_MINUS1 {
            assert_eq!(
                facts(HevcSpsSpec {
                    sub_layers_minus1: n,
                    width: 1280,
                    height: 720,
                    ..Default::default()
                }),
                Some(HevcSpsFacts {
                    general_profile_idc: 1,
                    chroma_format_idc: 1,
                    bit_depth_luma: 8,
                    bit_depth_chroma: 8,
                    width: 1280,
                    height: 720,
                }),
                "sub_layers_minus1={n} で profile_tier_level の読み飛ばしがずれている"
            );
        }
        // 7 is out of the spec's 0..=6 range, so the stream is malformed and is not decoded on faith.
        let malformed = HevcSpsSpec {
            sub_layers_minus1: 7,
            ..Default::default()
        };
        assert!(
            parse_hevc_sps_facts(&synthetic_hevc_sps(malformed)).is_none(),
            "sps_max_sub_layers_minus1=7 は規格外=拒否"
        );

        // Unreadable parameter sets end the parse rather than being decoded on faith.
        assert!(parse_hevc_sps_facts(&[]).is_none(), "空の SPS");
        let full = synthetic_hevc_sps(HevcSpsSpec::default());
        for cut in [1usize, 4, 8, 12] {
            assert!(
                parse_hevc_sps_facts(&full[..cut.min(full.len())]).is_none(),
                "{cut} バイトで切れた SPS はパースを終える(バッファ外を読まない)"
            );
        }
        // All-zero bits are all-zero exp-Golomb prefixes: the leading-zero cap must end it, and a
        // profile_idc of 0 is refused regardless.
        assert!(!parse_hevc_sps_facts(&[0u8; 24]).is_some_and(|f| hevc_sps_is_decodable(&f)));
    }

    /// Where the interesting bytes of an `hvcC` record sit inside a raw mp4, so a test can patch one
    /// of them. Located by walking the record (ISO/IEC 14496-15 §8.3.3.1) rather than by a fixed
    /// offset: the arrays are variable-length, and a hardcoded number would quietly start pointing at
    /// the wrong byte the day the fixture is re-encoded — which would make a patch-and-assert test
    /// pass for the wrong reason.
    struct HvccLayout {
        /// File offset of the record's first payload byte (`configurationVersion`).
        record: usize,
        /// File offset of the first SPS NAL, starting at its 2-byte NAL header.
        sps_nal: usize,
        sps_len: usize,
    }

    fn hvcc_layout(bytes: &[u8]) -> HvccLayout {
        let record = bytes
            .windows(4)
            .position(|w| w == b"hvcC")
            .expect("hvcC 記録がある")
            + 4;
        let num_arrays = bytes[record + 22];
        let mut i = record + 23;
        for _ in 0..num_arrays {
            let nal_type = bytes[i] & 0x3f;
            let count = u16::from_be_bytes([bytes[i + 1], bytes[i + 2]]);
            i += 3;
            for _ in 0..count {
                let len = usize::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]));
                i += 2;
                if nal_type == HEVC_NAL_TYPE_SPS {
                    return HvccLayout {
                        record,
                        sps_nal: i,
                        sps_len: len,
                    };
                }
                i += len;
            }
        }
        panic!("hvcC に SPS が無い");
    }

    /// Ground truth for the HEVC parser, and the only test that exercises [`parse_hvcc`] and
    /// [`strip_emulation_prevention`] on a real record: the SPS in `samples/sample-hevc.mp4` must
    /// read back as exactly what the file is.
    ///
    /// The emulation-prevention half is not hypothetical here — this fixture's SPS really does
    /// contain a `00 00 03` escape (asserted below, so the test says so if a re-encoded sample ever
    /// stops covering it). Reading past one shifts every subsequent field, which is the silent
    /// corruption the guard exists to prevent.
    #[test]
    fn hevc_sps_parse_matches_the_bundled_sample() {
        let Some(p) = sample_path_or_skip("sample-hevc.mp4") else {
            return;
        };
        let mut file = File::open(&p).unwrap();
        let size = file.metadata().unwrap().len();
        let mp4 = re_mp4::Mp4::read(&mut file, size).unwrap();
        let track = mp4
            .tracks()
            .values()
            .find(|t| t.kind == Some(re_mp4::TrackKind::Video))
            .expect("video track");
        assert_eq!(
            native_codec_kind(track.codec_string(&mp4).as_deref()),
            Some(NativeCodecKind::Hevc),
            "サンプルが HEVC として認識されていない"
        );

        let cfg = track.raw_codec_config(&mp4).unwrap();
        let hvcc = parse_hvcc(&cfg).expect("hvcC が読める");
        assert_eq!(hvcc.length_size, 4, "length prefix の幅");
        assert!(
            hvcc.parameter_sets.starts_with(&[0, 0, 0, 1]),
            "パラメータセットが Annex B(スタートコード)になっていない=rust_h265 が受け付けない形"
        );

        // The escaping really is present and really was undone — counted, not assumed, so this says
        // so out loud if a re-encoded sample ever stops covering it.
        let bytes = std::fs::read(&p).unwrap();
        let layout = hvcc_layout(&bytes);
        let raw_sps = &bytes[layout.sps_nal..layout.sps_nal + layout.sps_len];
        let escapes = raw_sps
            .windows(3)
            .filter(|w| w == &[0x00, 0x00, 0x03])
            .count();
        assert!(
            escapes > 0,
            "このサンプルの SPS に emulation prevention が無い=剥がす処理の検証が空振りしている"
        );
        assert_eq!(
            hvcc.sps_rbsp.len(),
            raw_sps.len() - 2 - escapes,
            "SPS の RBSP 化がずれている(2 バイトの NAL ヘッダと {escapes} 個の 0x03 だけが落ちるはず)"
        );

        let facts = parse_hevc_sps_facts(&hvcc.sps_rbsp).expect("SPS が読める");
        assert_eq!(
            facts,
            HevcSpsFacts {
                general_profile_idc: 1,
                chroma_format_idc: 1,
                bit_depth_luma: 8,
                bit_depth_chroma: 8,
                width: 320,
                height: 240,
            },
            "実ファイルの SPS を取り違えている"
        );
    }

    /// The HEVC counterpart of `guard_reads_the_bitstream_not_the_container`, and the reason that
    /// distinction matters even more here: unlike `avcC`, an `hvcC` record **does** carry
    /// `chromaFormat` and `bitDepthLuma/ChromaMinus8` (bytes 16..19) — precisely the facts the guard
    /// gates on. Reading them would be the obvious shortcut, and it would be wrong: they are the
    /// muxer's summary of the bitstream, written by whatever produced the file, and nothing verifies
    /// them against it.
    ///
    /// Both halves patch a single byte of `samples/sample-hevc.mp4` and assert opposite outcomes:
    ///
    /// * Patching the **record's** `chromaFormat` to 4:4:4 and its bit depths to 12 must change
    ///   *nothing* — the file still decodes, because the bitstream underneath is still Main 8-bit
    ///   4:2:0 and that is what the guard reads.
    /// * Patching the **SPS's** `general_profile_idc` to Range Extensions must be refused, even
    ///   though the bitstream underneath is untouched, decodable Main. Drop the guard and this
    ///   returns a picture instead.
    #[test]
    fn hevc_guard_reads_the_bitstream_not_the_container() {
        let Some(src) = sample_path_or_skip("sample-hevc.mp4") else {
            return;
        };
        let original = std::fs::read(&src).unwrap();
        let layout = hvcc_layout(&original);
        assert_eq!(
            original[layout.record], 1,
            "hvcC の先頭は configurationVersion=1(検算)"
        );
        assert_eq!(
            original[layout.record + 16] & 0x03,
            1,
            "コンテナ申告は 4:2:0(検算)"
        );
        // SPS NAL: [0..2] = NAL header, [2] = vps_id/max_sub_layers/nesting, [3] = profile_space(2)
        // + tier(1) + profile_idc(5). No emulation-prevention byte can occur this early (that needs
        // two zero bytes first, and the NAL header's first byte is 0x42), so the offset is stable.
        let sps_profile = layout.sps_nal + 3;
        assert_eq!(
            original[sps_profile] & 0x1f,
            1,
            "ビットストリームも Main(profile_idc=1)"
        );

        let dir = unique_tmp("konoma_video_hevc_bitstream_guard_test");
        std::fs::create_dir_all(&dir).unwrap();

        // Control: the unpatched copy really does decode, so a None below can only be the guard.
        let control = dir.join("control.mp4");
        std::fs::write(&control, &original).unwrap();
        assert!(
            thumbnail_native(&control).is_some(),
            "対照: 無改変のコピーはデコードできる"
        );

        // The container's own declaration of chroma format and bit depth is not a judgement input.
        let mut bytes = original.clone();
        bytes[layout.record + 16] = (bytes[layout.record + 16] & !0x03) | 3; // chromaFormat = 4:4:4
        bytes[layout.record + 17] = (bytes[layout.record + 17] & !0x07) | 4; // bitDepthLumaMinus8 = 4
        bytes[layout.record + 18] = (bytes[layout.record + 18] & !0x07) | 4; // ...Chroma
        let p = dir.join("container_lies.mp4");
        std::fs::write(&p, &bytes).unwrap();
        assert!(
            thumbnail_native(&p).is_some(),
            "hvcC の chromaFormat/bitDepth 申告は判断材料でない=ビットストリームが Main 8bit 4:2:0 なら描ける"
        );

        // The bitstream's is.
        for bad in [4u8, 5, 9] {
            let mut bytes = original.clone();
            bytes[sps_profile] = (bytes[sps_profile] & !0x1f) | bad;
            let p = dir.join(format!("sps{bad}.mp4"));
            std::fs::write(&p, &bytes).unwrap();
            assert!(
                thumbnail_native(&p).is_none(),
                "SPS が profile {bad} と言うならデコード前に拒否する(ビットストリームが実際にはデコードできても)"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The HEVC half of the point of this module: an ordinary HEVC mp4 — the family an iPhone
    /// records in — becomes a real picture with **no external tool involved at all**.
    /// `thumbnail_native` cannot spawn a process by construction, so a pass here can't be ffmpeg
    /// quietly doing the work.
    ///
    /// Checked the same three ways as the H.264 case, because "an image came back" is far too weak:
    /// the dimensions must be the source clip's own 320x240, the frame must not be flat (a decoder
    /// producing a solid gray rectangle would satisfy a size-only assertion), and it must be
    /// reachable through the public `thumbnail` entry point **with `allow_external` off** — which is
    /// what pins the ordering guarantee that the native step runs before the `[external] video` gate.
    #[test]
    fn native_path_extracts_a_real_frame_from_hevc_without_any_external_tool() {
        let Some(p) = sample_path_or_skip("sample-hevc.mp4") else {
            return;
        };
        let started = Instant::now();
        let img = thumbnail_native(&p).expect("HEVC mp4 は純 Rust 経路でサムネイルになるはず");
        let elapsed = started.elapsed();
        eprintln!("native thumbnail of samples/sample-hevc.mp4: {elapsed:?}");

        assert_eq!(
            (img.width(), img.height()),
            (320, 240),
            "元動画そのままの寸法(既定値やスケール後の寸法ではない)"
        );
        let luma: Vec<u8> = img.to_luma8().into_raw();
        let spread = luma_spread(&luma);
        assert!(
            spread > NATIVE_FLAT_LUMA_SPREAD,
            "一様な絵しか出ていない(デコード結果が絵になっていない): 輝度幅={spread}"
        );

        assert!(
            thumbnail(&p, false).is_some(),
            "[external] video = false でも純 Rust 経路が先に走る"
        );
    }

    /// Malformed HEVC input never panics and never guesses, at each of the three layers that can
    /// fail: a shredded `hvcC` (the guard's own parse), a shredded `mdat` (the decoder), and a
    /// truncated file (the container). The H.264 equivalent covers the same ground for `avcC`; this
    /// exists because the HEVC record is parsed by **this module's** hand-written walk rather than by
    /// the decoder crate, so its bounds checks are konoma's responsibility.
    #[test]
    fn hevc_native_path_degrades_safely_on_broken_input() {
        let Some(src) = sample_path_or_skip("sample-hevc.mp4") else {
            return;
        };
        let original = std::fs::read(&src).unwrap();
        let layout = hvcc_layout(&original);
        let dir = unique_tmp("konoma_video_hevc_broken_test");
        std::fs::create_dir_all(&dir).unwrap();

        // A NAL length inside the record that runs past the end of the record.
        let mut bytes = original.clone();
        bytes[layout.sps_nal - 2] = 0xff;
        bytes[layout.sps_nal - 1] = 0xff;
        let p = dir.join("hvcc_overlong.mp4");
        std::fs::write(&p, &bytes).unwrap();
        assert!(
            thumbnail_native(&p).is_none(),
            "hvcC の NAL 長が記録をはみ出していたら None(バッファ外を読まない)"
        );

        // A record claiming far more arrays than it contains.
        let mut bytes = original.clone();
        bytes[layout.record + 22] = 0xff;
        let p = dir.join("hvcc_many_arrays.mp4");
        std::fs::write(&p, &bytes).unwrap();
        assert!(
            thumbnail_native(&p).is_none(),
            "numOfArrays が過大でも None(panic しない)"
        );

        // Container and index intact, sample data nonsense — so the **decoder** is what gets handed
        // garbage, rather than the parse stopping earlier.
        let mut bytes = original.clone();
        let mdat = bytes
            .windows(4)
            .position(|w| w == b"mdat")
            .expect("samples/sample-hevc.mp4 に mdat がある")
            + 4;
        for b in bytes.iter_mut().skip(mdat) {
            *b = b.wrapping_mul(31).wrapping_add(7);
        }
        let p = dir.join("shredded.mp4");
        std::fs::write(&p, &bytes).unwrap();
        assert!(
            thumbnail_native(&p).is_none(),
            "サンプルデータが壊れた HEVC mp4 は None(panic しない)"
        );

        let p = dir.join("truncated.mp4");
        std::fs::write(&p, &original[..original.len() / 3]).unwrap();
        assert!(
            thumbnail_native(&p).is_none(),
            "途中で切れた HEVC mp4 は None(panic しない)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Emulation prevention must be undone before a parameter set is read, and only where the spec
    /// says it applies. Getting this wrong does not fail loudly — it shifts every field after the
    /// escape, so the guard would read a *different* chroma format or bit depth than the one the
    /// decoder will act on. That is the exact class of silent disagreement this module exists to
    /// prevent, so the rule is pinned here rather than left to the one real fixture.
    #[test]
    fn emulation_prevention_is_undone_before_reading_a_parameter_set() {
        // The escape byte itself disappears; the two zeros it protects stay.
        assert_eq!(
            strip_emulation_prevention(&[0x00, 0x00, 0x03, 0x01]),
            vec![0x00, 0x00, 0x01]
        );
        // Only after *two* zeros. A 0x03 anywhere else is real payload.
        assert_eq!(
            strip_emulation_prevention(&[0x00, 0x03, 0x00, 0x03]),
            vec![0x00, 0x03, 0x00, 0x03]
        );
        assert_eq!(strip_emulation_prevention(&[0x03; 4]), vec![0x03; 4]);
        // The zero run restarts after an escape, so `00 00 03 00 00 03` is two separate escapes.
        assert_eq!(
            strip_emulation_prevention(&[0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x02]),
            vec![0x00, 0x00, 0x00, 0x00, 0x02]
        );
        // ...and three zeros in a row are not themselves an escape sequence.
        assert_eq!(
            strip_emulation_prevention(&[0x00, 0x00, 0x00, 0x03, 0x04]),
            vec![0x00, 0x00, 0x00, 0x04]
        );
        assert!(strip_emulation_prevention(&[]).is_empty());
    }

    /// `BitReader::bits` underpins every fixed-width field of the HEVC guard, including the 96-bit
    /// `profile_tier_level` skip that must land exactly. Two properties matter: values are read MSB
    /// first, and a read that runs off the end consumes **nothing** — a partial read would leave the
    /// reader mid-field and turn "truncated parameter set" into "wrong parameter set".
    #[test]
    fn bit_reader_reads_fixed_width_fields_all_or_nothing() {
        let mut r = BitReader::new(&[0b1010_1100, 0b0011_0101]);
        assert_eq!(r.bits(4), Some(0b1010));
        assert_eq!(r.bits(0), Some(0), "0 ビットは常に 0 を返し何も消費しない");
        assert_eq!(r.bits(8), Some(0b1100_0011), "バイト境界をまたぐ");
        assert_eq!(r.bits(4), Some(0b0101));
        assert_eq!(r.bits(1), None, "尽きたら None");

        // Truncated: the 9-bit read fails and leaves the position where it was, so the following
        // 8-bit read still sees the whole first byte.
        let mut r = BitReader::new(&[0xff]);
        assert_eq!(r.bits(9), None);
        assert_eq!(r.bits(8), Some(0xff), "失敗した読みは位置を進めない");

        // 33 bits cannot fit the return type and are refused rather than truncated.
        let mut r = BitReader::new(&[0xff; 8]);
        assert_eq!(r.bits(33), None);
        assert_eq!(r.bits(32), Some(u32::MAX));
    }

    /// Main 10 hands back 16-bit samples, and turning them into a thumbnail must use the real linear
    /// map rather than `v >> 2`. The difference is small and this test says so honestly: measured
    /// over all 1024 inputs the shift differs on 170 of them, always by exactly one level. The point
    /// is that the exact map costs nothing here, so the whole plane is checked against it rather than
    /// against a few hand-picked values — that is what would catch a silent drift to the shift, or an
    /// off-by-one in the rounding term.
    #[test]
    fn ten_bit_samples_are_rescaled_not_truncated() {
        const MAX10: u32 = 1023;
        let all: Vec<u16> = (0..=1023).collect();
        let out = plane_to_8bit(&rust_h265::PixelData::U16(all.clone()), MAX10);

        let expected: Vec<u8> = all
            .iter()
            .map(|&v| ((u32::from(v) * 255 + MAX10 / 2) / MAX10) as u8)
            .collect();
        assert_eq!(out, expected, "10bit→8bit の対応が線形写像になっていない");

        // Endpoints and midpoint are exact...
        assert_eq!(out[0], 0, "黒は黒のまま");
        assert_eq!(out[1023], 255, "白は白のまま");
        assert_eq!(out[512], 128, "中間は中間");
        // ...and this really is not the shift: it differs, and only ever by one level.
        let shifted: Vec<u8> = all.iter().map(|&v| (v >> 2) as u8).collect();
        let differing = out.iter().zip(&shifted).filter(|(a, b)| a != b).count();
        let worst = out
            .iter()
            .zip(&shifted)
            .map(|(a, b)| i32::from(*a) - i32::from(*b))
            .map(i32::abs)
            .max()
            .unwrap();
        assert_eq!(differing, 170, "切り捨て(>> 2)と同じ結果になっている");
        assert_eq!(worst, 1, "差は常に 1 段(実測)");

        // 8-bit input is passed through untouched, and the same formula is the identity at max=255.
        assert_eq!(
            plane_to_8bit(&rust_h265::PixelData::U8(vec![0, 17, 128, 255]), 255),
            vec![0, 17, 128, 255]
        );
        // A sample above full scale (which a decoder should never emit) is clamped, not wrapped.
        assert_eq!(
            plane_to_8bit(&rust_h265::PixelData::U16(vec![4000]), MAX10),
            vec![255]
        );
    }

    /// A truncated or hostile `hvcC` ends the parse instead of over-reading, and a record with no
    /// SPS at all is refused rather than decoded on faith — there would be nothing to check it
    /// against, which is the same thing as being unable to check it.
    #[test]
    fn parse_hvcc_refuses_records_it_cannot_read() {
        assert!(parse_hvcc(&[]).is_none(), "空の記録");
        assert!(
            parse_hvcc(&[0u8; 22]).is_none(),
            "numOfArrays に届かない長さ"
        );
        // Well-formed prefix, zero arrays ⇒ no SPS ⇒ nothing to gate on.
        let mut cfg = vec![0u8; 23];
        cfg[0] = 1;
        assert!(parse_hvcc(&cfg).is_none(), "パラメータセットが無い記録");
        // One array claiming one NAL, but the length runs past the buffer.
        let mut cfg = vec![0u8; 23];
        cfg[0] = 1;
        cfg[22] = 1; // numOfArrays
        cfg.extend_from_slice(&[HEVC_NAL_TYPE_SPS, 0x00, 0x01]); // one SPS
        cfg.extend_from_slice(&[0xff, 0xff]); // ...of 65535 bytes, which are not there
        assert!(parse_hvcc(&cfg).is_none(), "記録をはみ出す NAL 長");
    }

    /// The H.264 half of the family check: it must recognise AVC and *only* AVC. HEVC (`hvc1…`) is
    /// not AVC and must not be routed to `rust_h264` — it has its own decoder and its own guard, and
    /// `codec_string_dispatch_picks_the_right_decoder` below is what checks it gets there.
    #[test]
    fn codec_string_guard_admits_only_avc() {
        assert!(codec_string_is_avc(Some("avc1.640028")));
        assert!(codec_string_is_avc(Some("avc1.42E01E")));
        assert!(codec_string_is_avc(Some("avc3.640028")));
        assert!(!codec_string_is_avc(Some("hvc1.1.6.L93.B0")));
        assert!(!codec_string_is_avc(Some("hev1.1.6.L93.B0")));
        assert!(!codec_string_is_avc(Some("vp09.00.10.08")));
        assert!(!codec_string_is_avc(Some("vp8")));
        assert!(!codec_string_is_avc(Some("av01.0.04M.08")));
        assert!(!codec_string_is_avc(Some("mp4a.40.2")));
        assert!(!codec_string_is_avc(None), "コーデック不明は拒否");
    }

    /// Each codec string reaches the decoder that can actually read it, and everything else still
    /// falls out to the external chain by name. Routing an HEVC track into `rust_h264` (or the
    /// reverse) would not be a near miss — the parameter-set records are different formats, so this
    /// is the check that keeps "supported" meaning the right decoder, not merely some decoder.
    #[test]
    fn codec_string_dispatch_picks_the_right_decoder() {
        use NativeCodecKind::{Avc, Hevc};
        assert_eq!(native_codec_kind(Some("avc1.640028")), Some(Avc));
        assert_eq!(native_codec_kind(Some("avc3.640028")), Some(Avc));
        // hvc1 is what iPhones and macOS screen recordings write; hev1 is the in-band variant.
        assert_eq!(native_codec_kind(Some("hvc1.1.6.L93.B0")), Some(Hevc));
        assert_eq!(native_codec_kind(Some("hev1.1.6.L93.B0")), Some(Hevc));
        assert_eq!(
            native_codec_kind(Some("hvc1.2.4.L90.90")),
            Some(Hevc),
            "Main 10"
        );
        // Neither decoder implements these, so they must not be claimed by either.
        for other in [
            "vp09.00.10.08",
            "vp8",
            "av01.0.04M.08",
            "mp4a.40.2",
            "mp4v.20.9",
            "apch",
        ] {
            assert_eq!(
                native_codec_kind(Some(other)),
                None,
                "{other} は内蔵デコーダの担当でない=外部ツールへ降格する"
            );
        }
        assert_eq!(native_codec_kind(None), None, "コーデック不明は拒否");
    }

    /// The container gate is by **extension**, decided before the file is opened, and it decides
    /// *which demultiplexer* the file gets — not merely whether to open it. An `.avi` or
    /// extensionless path therefore still costs nothing at all.
    ///
    /// The second half is what makes this discriminating rather than a restatement of the table: the
    /// fixture holds **byte-for-byte valid mp4 data** (a copy of `samples/sample.mp4`, which
    /// `native_path_extracts_a_real_frame_without_any_external_tool` shows really does decode) under
    /// an `.mkv` name. Naming it `.mkv` routes it to the Matroska demuxer, which correctly refuses
    /// it — so extension and content disagreeing degrades safely instead of producing a picture from
    /// the "wrong" reader. (Before mkv support this was refused by the extension alone; the outcome
    /// the user sees is the same, but the reason has moved one layer down.)
    #[test]
    fn native_path_only_opens_containers_it_indexes() {
        use NativeContainer::{Matroska, Mp4};
        assert_eq!(native_container_kind(Path::new("/x/clip.mp4")), Some(Mp4));
        assert_eq!(
            native_container_kind(Path::new("/x/clip.MP4")),
            Some(Mp4),
            "大小無視"
        );
        assert_eq!(native_container_kind(Path::new("/x/clip.m4v")), Some(Mp4));
        assert_eq!(native_container_kind(Path::new("/x/clip.mov")), Some(Mp4));
        assert_eq!(
            native_container_kind(Path::new("/x/clip.mkv")),
            Some(Matroska)
        );
        assert_eq!(
            native_container_kind(Path::new("/x/clip.webm")),
            Some(Matroska),
            "WebM は Matroska の部分集合=同じ demuxer が読む"
        );
        assert_eq!(
            native_container_kind(Path::new("/x/clip.WEBM")),
            Some(Matroska),
            "大小無視"
        );
        assert_eq!(
            native_container_kind(Path::new("/x/clip.avi")),
            None,
            "avi はどちらの demuxer の担当でもない=外部ツールへ降格"
        );
        assert_eq!(native_container_kind(Path::new("/x/clip")), None);

        let Some(src) = sample_path_or_skip("sample.mp4") else {
            return;
        };
        let bytes = std::fs::read(&src).unwrap();
        let dir = unique_tmp("konoma_video_ext_gate_test");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["clip.mkv", "clip.webm", "clip", "clip.avi"] {
            let disguised = dir.join(name);
            std::fs::write(&disguised, &bytes).unwrap();
            assert!(
                thumbnail_native(&disguised).is_none(),
                "{name}: 拡張子と中身が食い違うファイルは安全に None(別コンテナの reader が絵を作ってしまわない)"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The Matroska half of the point of this module: an ordinary `.mkv` becomes a real picture with
    /// **no external tool involved at all**. `thumbnail_native` cannot spawn a process by
    /// construction, so a pass here can't be ffmpeg quietly doing the work.
    ///
    /// Checked the same three ways as the two mp4 cases, because "an image came back" is far too
    /// weak: the dimensions must be the source clip's own 320x240, the frame must not be flat (a
    /// decoder producing a solid gray rectangle would satisfy a size-only assertion), and it must be
    /// reachable through the public `thumbnail` entry point **with `allow_external` off**, which
    /// pins the ordering guarantee that the native step runs before the `[external] video` gate.
    ///
    /// Measured while writing this, against `ffmpeg -f rawvideo -pix_fmt yuv420p` on the same frame:
    /// konoma's decoded Y/U/V planes for this fixture are **byte-identical** to ffmpeg's, all
    /// 115,200 samples of them.
    #[test]
    fn native_path_extracts_a_real_frame_from_mkv_without_any_external_tool() {
        let Some(p) = sample_path_or_skip("sample.mkv") else {
            return;
        };
        let started = Instant::now();
        let img = thumbnail_native(&p).expect("H.264 mkv は純 Rust 経路でサムネイルになるはず");
        eprintln!(
            "native thumbnail of samples/sample.mkv: {:?}",
            started.elapsed()
        );

        assert_eq!(
            (img.width(), img.height()),
            (320, 240),
            "元動画そのままの寸法(既定値やスケール後の寸法ではない)"
        );
        let luma: Vec<u8> = img.to_luma8().into_raw();
        let spread = luma_spread(&luma);
        assert!(
            spread > NATIVE_FLAT_LUMA_SPREAD,
            "一様な絵しか出ていない(デコード結果が絵になっていない): 輝度幅={spread}"
        );

        assert!(
            thumbnail(&p, false).is_some(),
            "[external] video = false でも純 Rust 経路が先に走る"
        );
    }

    /// The Matroska path really does aim at the 10% mark rather than settling for the first frame.
    ///
    /// This is not a cosmetic detail, and it is the one place where reading the wrong field fails
    /// **silently**: Matroska writes its duration once for the whole segment, so every
    /// `Track::duration` symphonia hands back is `None`. An earlier version of this module read the
    /// track's, got None every time, and therefore never performed a single seek — producing a
    /// perfectly good thumbnail of frame 0 with nothing to say it had skipped the step. Hence the
    /// assertion is on the position, not merely on `is_some()`.
    #[test]
    fn mkv_seek_target_comes_from_the_media_duration() {
        let Some(p) = sample_path_or_skip("sample.mkv") else {
            return;
        };
        let source = BudgetedSource::open(&p, NATIVE_MKV_MAX_SCAN_BYTES).unwrap();
        let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
        let reader = MkvReader::try_new(mss, FormatOptions::default()).unwrap();

        assert!(
            reader.tracks().iter().all(|t| t.duration.is_none()),
            "この前提が変わったら(トラックにも duration が入るようになったら)この検査の意味も変わる"
        );
        let track = find_mkv_track(&reader).expect("H.264 トラックが見つかる");
        let target = track
            .ten_percent
            .expect("セグメントの duration から 10% 位置が求まる");
        // The fixture is 3 seconds long, so 10% is 0.3s. Compared in nanoseconds against a small
        // window rather than exactly, since the duration comes back as a float in the file.
        let ns = target.as_nanos();
        assert!(
            (290_000_000..=310_000_000).contains(&ns),
            "10% 位置が 0.3 秒付近でない: {ns}ns"
        );
    }

    /// Which packet is a keyframe, which is the one thing the Matroska path has to decide for itself:
    /// **symphonia's `Packet` carries no keyframe flag** (unlike mp4's `is_sync` sample table), and
    /// handing a non-keyframe to a fresh decoder does not produce an error — it produces a picture
    /// assembled from references that were never decoded.
    ///
    /// The NAL type sits in a different place in each codec, and the two ways to get it wrong both
    /// fail quietly rather than loudly: read H.264's 5 bits out of HEVC's 2-byte header and every
    /// packet looks like a keyframe (or none does), with the only symptom being a wrong picture.
    #[test]
    fn keyframe_detection_reads_the_nal_type_of_each_codec() {
        use NativeCodecKind::{Avc, Hevc};

        /// One length-prefixed sample (4-byte lengths, as every real `avcC`/`hvcC` uses) holding the
        /// given NAL header bytes, each followed by a byte of payload.
        fn sample(nals: &[&[u8]]) -> Vec<u8> {
            let mut out = Vec::new();
            for nal in nals {
                out.extend_from_slice(&((nal.len() as u32) + 1).to_be_bytes());
                out.extend_from_slice(nal);
                out.push(0xab); // payload
            }
            out
        }

        // H.264: one header byte, type in the low 5 bits. 5 is an IDR slice; 1 is a non-IDR slice,
        // 7/8 are the parameter sets that accompany it.
        assert!(sample_has_keyframe(Avc, &sample(&[&[0x65]]), 4), "IDR(5)");
        assert!(
            sample_has_keyframe(Avc, &sample(&[&[0x67], &[0x68], &[0x65]]), 4),
            "SPS/PPS の後ろに IDR が続くのが普通の形"
        );
        assert!(
            !sample_has_keyframe(Avc, &sample(&[&[0x41]]), 4),
            "非 IDR スライス(1)はキーフレームでない=単体でデコードできない"
        );
        assert!(!sample_has_keyframe(Avc, &sample(&[&[0x67], &[0x68]]), 4));
        // The HEVC IDR type (19) sits in the low 5 bits of 0x33 — reading H.264's field out of an
        // HEVC stream would call this a keyframe. It must not, and vice versa below.
        assert!(!sample_has_keyframe(Avc, &sample(&[&[0x33]]), 4));

        // H.265: two header bytes, type in bits 6..1 of the first. 16..=21 are the IRAP pictures.
        for nal_type in HEVC_IRAP_NAL_TYPES {
            let header = [nal_type << 1, 0x01];
            assert!(
                sample_has_keyframe(Hevc, &sample(&[&header]), 4),
                "IRAP({nal_type}) はキーフレーム"
            );
        }
        for nal_type in [0u8, 1, 15, 22, 23, 32, 33, 34] {
            let header = [nal_type << 1, 0x01];
            assert!(
                !sample_has_keyframe(Hevc, &sample(&[&header]), 4),
                "NAL type {nal_type} は IRAP でない"
            );
        }
        // A real HEVC keyframe sample: VPS/SPS/PPS (32/33/34) then IDR_W_RADL (19).
        let params_then_idr = sample(&[&[64, 1], &[66, 1], &[68, 1], &[38, 1]]);
        assert!(sample_has_keyframe(Hevc, &params_then_idr, 4));
        // Same bytes read as H.264 must not be mistaken for one (0x40 & 0x1f = 0, 0x26 & 0x1f = 6).
        assert!(!sample_has_keyframe(Avc, &params_then_idr, 4));

        // Malformed samples answer "no" rather than reading past the end or looping.
        assert!(!sample_has_keyframe(Avc, &[], 4));
        assert!(
            !sample_has_keyframe(Avc, &[0, 0], 4),
            "長さ前置が途中で切れている"
        );
        assert!(
            !sample_has_keyframe(Avc, &[0x00, 0x00, 0x00, 0xff, 0x65], 4),
            "宣言された長さがバッファをはみ出している"
        );
        assert!(
            !sample_has_keyframe(Avc, &[0x00, 0x00, 0x00, 0x00], 4),
            "長さ 0 の NAL(中身が無い)"
        );
        // `length_size` of 0 cannot occur in a real record, but it would advance the cursor by
        // nothing on every iteration — the walk must refuse it rather than spin forever.
        assert!(!sample_has_keyframe(Avc, &sample(&[&[0x65]]), 0));
    }

    /// The read budget is what keeps "Matroska has no sample table, so walk forward" from turning
    /// into an unbounded read, and it has to hold **inside** symphonia: on a file with no cue points
    /// the demuxer implements seeking as a forward scan of its own, so asking for the 10% mark of a
    /// 2 GB clip would read 200 MB before returning anything.
    ///
    /// Both halves matter. The first pins the mechanism (reads succeed up to the ceiling, then fail
    /// rather than being silently short), and the second proves it is wired into the extraction path
    /// by running the *real* fixture — one that demonstrably decodes with the production budget —
    /// under a budget too small to reach a keyframe, and requiring it to give up rather than hang or
    /// guess.
    #[test]
    fn mkv_scan_gives_up_at_its_read_budget() {
        let dir = unique_tmp("konoma_video_mkv_budget_test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.bin");
        std::fs::write(&file, vec![7u8; 4096]).unwrap();

        let mut src = BudgetedSource::open(&file, 100).expect("open");
        assert_eq!(src.byte_len(), Some(4096), "元ファイルの長さは正直に返す");
        assert!(src.is_seekable());
        let mut buf = [0u8; 64];
        assert_eq!(src.read(&mut buf).unwrap(), 64, "予算内は普通に読める");
        assert_eq!(
            src.read(&mut buf).unwrap(),
            36,
            "予算の残りぶんだけ読んで止まる"
        );
        assert!(
            src.read(&mut buf).is_err(),
            "予算を使い切ったら以降は失敗する(黙って 0 バイト=EOF を返すと、呼び出し側が正常終了と取り違える)"
        );
        // A budget of zero fails on the very first read rather than looking like an empty file.
        assert!(BudgetedSource::open(&file, 0)
            .expect("open")
            .read(&mut buf)
            .is_err());
        std::fs::remove_dir_all(&dir).ok();

        let Some(p) = sample_path_or_skip("sample.mkv") else {
            return;
        };
        // Control: with the production budget this very file decodes, so a refusal below can only be
        // the budget doing it.
        assert!(
            matches!(
                mkv_attempt(&p, true, MkvBudget::DEFAULT),
                MkvAttempt::Frame(_)
            ),
            "対照: 既定の予算ならこのファイルは絵になる"
        );
        for bytes in [1u64, 512, 4096] {
            assert!(
                !matches!(
                    mkv_attempt(&p, true, MkvBudget { bytes, packets: 8 }),
                    MkvAttempt::Frame(_)
                ),
                "{bytes} バイトしか読めない予算では諦める(読み続けない)"
            );
        }
        // The packet ceiling is the other half of the bound: plenty of bytes, but no permission to
        // walk far enough to find a keyframe.
        assert!(
            !matches!(
                mkv_attempt(
                    &p,
                    false,
                    MkvBudget {
                        bytes: NATIVE_MKV_MAX_SCAN_BYTES,
                        packets: 0
                    }
                ),
                MkvAttempt::Frame(_)
            ),
            "パケット数の上限も効く"
        );
    }

    /// Codec dispatch on the Matroska side, which is by **ID** rather than by the mp4 path's codec
    /// string. The important half is the rejections: a `.webm` is usually VP9 or AV1, and neither
    /// built-in decoder implements either — they have to fall out here, by name, rather than be
    /// handed to a decoder that cannot read them.
    #[test]
    fn mkv_codec_id_dispatch_picks_the_right_decoder() {
        use NativeCodecKind::{Avc, Hevc};
        assert_eq!(mkv_codec_kind(video_codecs::CODEC_ID_H264), Some(Avc));
        assert_eq!(mkv_codec_kind(video_codecs::CODEC_ID_HEVC), Some(Hevc));
        for other in [
            video_codecs::CODEC_ID_VP9,
            video_codecs::CODEC_ID_AV1,
            video_codecs::CODEC_ID_VP8,
            video_codecs::CODEC_ID_MPEG4,
            video_codecs::CODEC_ID_THEORA,
            VideoCodecId::default(),
        ] {
            assert_eq!(
                mkv_codec_kind(other),
                None,
                "{other} は内蔵デコーダの担当でない=外部ツールへ降格する"
            );
        }
    }

    /// The decoder configuration record is chosen **by extra-data ID, not by position**.
    ///
    /// A track may carry several blocks — a Dolby Vision configuration alongside the HEVC one is the
    /// common case — and nothing promises an order, so `extra_data.first()` would sooner or later
    /// hand `parse_hvcc` something that is not an `hvcC` at all. Since both records start with a
    /// version byte and are then walked by length fields, the failure would not be a clean refusal:
    /// it would be a parse of unrelated bytes.
    #[test]
    fn extra_data_is_chosen_by_id_not_by_position() {
        use symphonia_core::codecs::video::VideoExtraData;
        let block = |id, byte| VideoExtraData {
            id,
            data: vec![byte; 4].into_boxed_slice(),
        };
        let mut params = VideoCodecParameters::default();
        // Deliberately out of order, with the Dolby Vision block first.
        params.extra_data.push(block(
            extra_data_ids::VIDEO_EXTRA_DATA_ID_DOLBY_VISION_CONFIG,
            0xDD,
        ));
        params.extra_data.push(block(
            extra_data_ids::VIDEO_EXTRA_DATA_ID_HEVC_DECODER_CONFIG,
            0xEE,
        ));
        params.extra_data.push(block(
            extra_data_ids::VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG,
            0xAA,
        ));

        assert_eq!(
            extra_data_for(&params, NativeCodecKind::Hevc),
            Some(&[0xEE; 4][..]),
            "先頭の Dolby Vision 記録を hvcC と取り違えている"
        );
        assert_eq!(
            extra_data_for(&params, NativeCodecKind::Avc),
            Some(&[0xAA; 4][..])
        );
        // A track with no record for this codec is refused rather than given someone else's.
        let mut only_avc = VideoCodecParameters::default();
        only_avc.extra_data.push(block(
            extra_data_ids::VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG,
            0xAA,
        ));
        assert_eq!(extra_data_for(&only_avc, NativeCodecKind::Hevc), None);
        assert_eq!(
            extra_data_for(&VideoCodecParameters::default(), NativeCodecKind::Avc),
            None,
            "CodecPrivate がまったく無いトラック(VP9 の webm がこの形)"
        );
    }

    /// Malformed Matroska never panics and never guesses, at each layer that can fail: a file that
    /// is not Matroska at all (the demuxer), a truncated one (the element walk), and one whose
    /// cluster payload has been shredded while the header and cues still parse — which is what
    /// reaches the **decoder** rather than stopping earlier.
    #[test]
    fn mkv_native_path_degrades_safely_on_broken_input() {
        assert!(thumbnail_native(Path::new("/no/such/video.mkv")).is_none());

        let dir = unique_tmp("konoma_video_mkv_broken_test");
        std::fs::create_dir_all(&dir).unwrap();

        let garbage = dir.join("garbage.mkv");
        std::fs::write(&garbage, b"not matroska at all, just some bytes\n").unwrap();
        assert!(thumbnail_native(&garbage).is_none(), "非 mkv は None");

        let Some(src) = sample_path_or_skip("sample.mkv") else {
            std::fs::remove_dir_all(&dir).ok();
            return;
        };
        let original = std::fs::read(&src).unwrap();

        let truncated = dir.join("truncated.mkv");
        std::fs::write(&truncated, &original[..original.len() / 3]).unwrap();
        assert!(
            thumbnail_native(&truncated).is_none(),
            "途中で切れた mkv は None(panic しない)"
        );

        // Shred everything after the first Cluster (`0x1F43B675`), leaving the EBML header, the
        // Tracks element and its `CodecPrivate` intact — so the guard passes and the **decoder** is
        // the thing handed nonsense, rather than the parse stopping before it.
        let cluster = original
            .windows(4)
            .position(|w| w == [0x1F, 0x43, 0xB6, 0x75])
            .expect("samples/sample.mkv に Cluster 要素がある");
        let mut bytes = original.clone();
        for b in bytes.iter_mut().skip(cluster + 4) {
            *b = b.wrapping_mul(31).wrapping_add(7);
        }
        let shredded = dir.join("shredded.mkv");
        std::fs::write(&shredded, &bytes).unwrap();
        assert!(
            thumbnail_native(&shredded).is_none(),
            "サンプルデータが壊れた mkv は None(panic しない)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The Matroska counterpart of `guard_reads_the_bitstream_not_the_container`, and the reason the
    /// guard is shared between the two containers rather than reimplemented: Matroska stores the very
    /// same `avcC` record in `CodecPrivate`, so a stream mp4 would refuse must be refused here too —
    /// and a *container* that misdeclares its stream must be ignored here too.
    ///
    /// Both halves patch a single byte of `samples/sample.mkv` and assert opposite outcomes, which
    /// is what makes this discriminating rather than a restatement of the mp4 test:
    ///
    /// * Patching the record's `AVCProfileIndication` must change **nothing** — that byte is the
    ///   container's self-declaration, and nothing verifies it against the bitstream.
    /// * Patching the **SPS's** `profile_idc` must be refused, even though the bitstream underneath
    ///   is untouched, decodable 8-bit High. Drop the guard and this returns a picture instead.
    ///
    /// The record is located by searching for the exact bytes the demuxer handed back, so this
    /// cannot quietly start patching the wrong offset the day the fixture is re-encoded.
    #[test]
    fn mkv_guard_reads_the_bitstream_not_the_container() {
        let Some(src) = sample_path_or_skip("sample.mkv") else {
            return;
        };
        let original = std::fs::read(&src).unwrap();

        let cfg = {
            let source = BudgetedSource::open(&src, NATIVE_MKV_MAX_SCAN_BYTES).unwrap();
            let mss = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
            let reader = MkvReader::try_new(mss, FormatOptions::default()).unwrap();
            let track = find_mkv_track(&reader).expect("H.264 トラックが見つかる");
            assert_eq!(track.kind, NativeCodecKind::Avc);
            track.cfg
        };
        let record = original
            .windows(cfg.len())
            .position(|w| w == cfg)
            .expect("CodecPrivate(avcC 記録)がファイル中にそのまま入っている");

        // avcC payload: [0]=configurationVersion, [1]=AVCProfileIndication, [2]=compat, [3]=level,
        // [4]=lengthSizeMinusOne, [5]=numOfSPS, [6..8]=spsLength, [8]=SPS NAL header, [9]=profile_idc.
        let container_profile = record + 1;
        let sps_profile = record + 9;
        assert_eq!(
            original[record], 1,
            "avcC の先頭は configurationVersion=1(検算)"
        );
        assert_eq!(original[container_profile], 100, "コンテナ申告は High(100)");
        assert_eq!(
            original[record + 8] & 0x1f,
            7,
            "SPS NAL(type 7)を指している(検算)"
        );
        assert_eq!(original[sps_profile], 100, "ビットストリームも High(100)");

        let dir = unique_tmp("konoma_video_mkv_bitstream_guard_test");
        std::fs::create_dir_all(&dir).unwrap();

        // Control: the unpatched copy really does decode, so a None below can only be the guard.
        let control = dir.join("control.mkv");
        std::fs::write(&control, &original).unwrap();
        assert!(
            thumbnail_native(&control).is_some(),
            "対照: 無改変のコピーはデコードできる"
        );

        for bad in [110u8, 122, 244] {
            let mut bytes = original.clone();
            bytes[container_profile] = bad;
            let p = dir.join(format!("container{bad}.mkv"));
            std::fs::write(&p, &bytes).unwrap();
            assert!(
                thumbnail_native(&p).is_some(),
                "CodecPrivate の申告({bad})は判断材料でない=ビットストリームが 8bit 4:2:0 High なら描ける"
            );

            let mut bytes = original.clone();
            bytes[sps_profile] = bad;
            let p = dir.join(format!("sps{bad}.mkv"));
            std::fs::write(&p, &bytes).unwrap();
            assert!(
                thumbnail_native(&p).is_none(),
                "SPS が profile {bad} と言うならデコード前に拒否する(mp4 と同じガードが mkv でも効く)"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole point of this batch: a plain H.264 mp4 becomes a real picture with **no external
    /// tool involved at all** — `thumbnail_native` cannot spawn a process by construction, so a pass
    /// here can't be ffmpeg quietly doing the work.
    ///
    /// "A real picture" is checked three ways, because "an image came back" is far too weak: the
    /// dimensions must be the source clip's own 320x240 (not some default), the frame must not be
    /// flat (a decoder that produced a solid gray rectangle would satisfy a size-only assertion),
    /// and it must be reachable through the public `thumbnail` entry point **with `allow_external`
    /// off** — which is what pins the ordering guarantee that the native step runs before the
    /// `[external] video` gate rather than behind it.
    #[test]
    fn native_path_extracts_a_real_frame_without_any_external_tool() {
        let Some(p) = sample_path_or_skip("sample.mp4") else {
            return;
        };
        let started = Instant::now();
        let img = thumbnail_native(&p).expect("H.264 mp4 は純 Rust 経路でサムネイルになるはず");
        let elapsed = started.elapsed();
        eprintln!("native thumbnail of samples/sample.mp4: {elapsed:?}");

        assert_eq!(
            (img.width(), img.height()),
            (320, 240),
            "元動画そのままの寸法(既定値やスケール後の寸法ではない)"
        );
        let luma: Vec<u8> = img.to_luma8().into_raw();
        let spread = luma_spread(&luma);
        assert!(
            spread > NATIVE_FLAT_LUMA_SPREAD,
            "一様な絵しか出ていない(デコード結果が絵になっていない): 輝度幅={spread}"
        );

        assert!(
            thumbnail(&p, false).is_some(),
            "[external] video = false でも純 Rust 経路が先に走る"
        );
    }

    /// Malformed input never panics and never guesses: a truncated/garbage mp4, and a real mp4
    /// whose sample data has been shredded (header and index still parse, so this reaches the
    /// decoder rather than stopping at the container), both come back None from the native path.
    /// The nonexistent-path case is covered here too, without touching the external chain.
    #[test]
    fn native_path_degrades_safely_on_broken_input() {
        assert!(thumbnail_native(Path::new("/no/such/video.mp4")).is_none());

        let dir = unique_tmp("konoma_video_broken_test");
        std::fs::create_dir_all(&dir).unwrap();

        let garbage = dir.join("garbage.mp4");
        std::fs::write(&garbage, b"not an mp4 at all, just some bytes\n").unwrap();
        assert!(thumbnail_native(&garbage).is_none(), "非 mp4 は None");

        if let Some(src) = sample_path_or_skip("sample.mp4") {
            let mut bytes = std::fs::read(&src).unwrap();
            let truncated = dir.join("truncated.mp4");
            std::fs::write(&truncated, &bytes[..bytes.len() / 3]).unwrap();
            assert!(
                thumbnail_native(&truncated).is_none(),
                "途中で切れた mp4 は None(panic しない)"
            );

            // Shred the `mdat` payload only, leaving `ftyp`/`moov` intact, so the container parses
            // and the **decoder** is the thing handed nonsense. Located by fourcc rather than a
            // fixed offset: an earlier version of this test shredded from byte 1024, which lands
            // inside `moov` (36..1371 in this file) — `re_mp4` then failed on the `stsc` table and
            // the decoder never saw a single byte, so the case this test names went unexercised
            // while still passing.
            let mdat = bytes
                .windows(4)
                .position(|w| w == b"mdat")
                .expect("samples/sample.mp4 に mdat がある")
                + 4;
            for b in bytes.iter_mut().skip(mdat) {
                *b = b.wrapping_mul(31).wrapping_add(7);
            }
            let shredded = dir.join("shredded.mp4");
            std::fs::write(&shredded, &bytes).unwrap();
            assert!(
                thumbnail_native(&shredded).is_none(),
                "サンプルデータが壊れた mp4 は None(panic しない)"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Keyframe selection: the ~10% mark (matching `ffmpegthumbnailer`'s default, so switching
    /// extractors doesn't change which frame the user sees), retries walk **forward through
    /// keyframes** rather than through the neighbouring non-decodable samples, and a track with no
    /// sync sample at all yields nothing to try instead of picking an arbitrary sample.
    #[test]
    fn keyframe_selection_targets_ten_percent_and_walks_forward() {
        fn track(sync_at: &[usize], len: usize) -> Vec<re_mp4::Sample> {
            (0..len)
                .map(|i| re_mp4::Sample {
                    id: i as u32,
                    is_sync: sync_at.contains(&i),
                    ..Default::default()
                })
                .collect()
        }

        // 100 samples ⇒ target index 10; keyframes at 0/9/30/60 ⇒ 9 is nearest, then 30, 60.
        let s = track(&[0, 9, 30, 60], 100);
        assert_eq!(pick_keyframes(&s, 3), vec![9, 30, 60]);
        // The cap is honored, and it counts keyframes (not samples).
        assert_eq!(pick_keyframes(&s, 1), vec![9]);
        // Nothing after the chosen one: the list just ends (no wrap-around, no padding).
        assert_eq!(pick_keyframes(&track(&[0, 9], 100), 3), vec![9]);
        // Only the first sample is a keyframe (the common short-clip case).
        assert_eq!(pick_keyframes(&track(&[0], 40), 3), vec![0]);
        // No sync sample at all ⇒ nothing is decodable standalone ⇒ degrade to the external chain.
        assert!(pick_keyframes(&track(&[], 40), 3).is_empty());
        assert!(pick_keyframes(&[], 3).is_empty());
    }

    /// Serializes the tests in this module that mutate process-global state (`PATH`, this
    /// process's own fd 0) so they never overlap each other's window — `extracts_correct_frame_
    /// when_ffmpeg_available` probes real `ffmpeg` on `PATH`, and `ffmpeg_tools_never_inherit_
    /// this_process_stdin` temporarily shadows `ffmpeg`/`ffmpegthumbnailer` on `PATH` with fakes.
    /// Without this, the two could interleave under the test harness's default parallelism and
    /// make the ffmpeg-availability probe spuriously see the fake script instead of the real tool.
    static PATH_MUTATING_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Returns None when external tools are missing or the target is not a video (does not crash; safe fallback).
    ///
    /// The nonexistent-path half is a weak check on its own: a missing path returns None whether
    /// or not `ffmpeg`/`ffmpegthumbnailer` are even installed (neither tool runs at all — `Command`
    /// still spawns fine against a bogus argument, but exits non-zero immediately), so it can't
    /// distinguish "the tool correctly rejected the input" from "no tool ever ran." The non-video
    /// half closes that gap: it writes a real, *existing* plain-text file with a `.mp4` name (so
    /// `path.is_file()` succeeds and both `run_ffmpegthumbnailer`/`run_ffmpeg` actually spawn and
    /// process it) via `unique_tmp` — on a machine with ffmpeg installed (confirmed present here:
    /// `ffmpeg version 8.1.1`), this exercises the *real* tool actually running and failing to parse
    /// a non-video container, not merely "the path didn't exist." On a machine without either tool
    /// this degrades to the same "no tool ever ran" case as the nonexistent-path half, which is
    /// still a legitimate (if weaker) pass — principle #3, "unsupported/missing tool degrades safely."
    #[test]
    fn nonexistent_or_nonvideo_returns_none() {
        assert!(
            thumbnail(Path::new("/no/such/video.mp4"), true).is_none(),
            "存在しないパスは None"
        );

        let dir = unique_tmp("konoma_video_nonvideo_test");
        std::fs::create_dir_all(&dir).unwrap();
        let not_a_video = dir.join("notes.mp4");
        std::fs::write(&not_a_video, b"this is plain text, not an mp4 container\n").unwrap();
        assert!(
            thumbnail(&not_a_video, true).is_none(),
            ".mp4 という名前だけの非動画ファイルは None(ffmpeg があれば実際に起動して拒否したことを検査する)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// If ffmpeg is on PATH, verify that a thumbnail can actually be extracted from a generated tiny video, down to **the extracted frame's
    /// content (color) matching the source video** (not "an image appeared" but "the correct frame appeared").
    /// Skipped on environments without ffmpeg (an optional dependency, so it does not break CI).
    #[test]
    fn extracts_correct_frame_when_ffmpeg_available() {
        let _guard = PATH_MUTATING_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let has_ffmpeg = Command::new("ffmpeg")
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !has_ffmpeg {
            eprintln!("skip: ffmpeg 不在");
            return;
        }
        // Generate a 64x64, 1-second solid-green video with lavfi and check the extracted frame's center is green.
        // `.with_extension` (not baking `.mp4` into the `unique_tmp` prefix) keeps the extension
        // at the very end of the path — ffmpeg infers the output muxer from it, so
        // `…-green.mp4_1234_5` (no trailing `.mp4`) fails to encode.
        let vid = unique_tmp("konoma-vthumb-test-green").with_extension("mp4");
        let _ = std::fs::remove_file(&vid);
        let made = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("color=c=green:s=64x64:d=1")
            .arg(&vid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(made, "テスト用動画の生成に失敗");

        let img = thumbnail(&vid, true).expect("ffmpeg があればサムネイルが取れるはず");
        assert!(img.width() > 0 && img.height() > 0, "サムネイル寸法が 0");
        // The center pixel is green-dominant (g > r and g > b) = the extracted frame really is from the source video.
        let rgba = img.to_rgba8();
        let px = rgba.get_pixel(rgba.width() / 2, rgba.height() / 2);
        let (r, g, b) = (px[0], px[1], px[2]);
        assert!(
            g > r && g > b && g > 60,
            "中央が緑でない(抽出フレームが元動画と不一致?): rgb=({r},{g},{b})"
        );
        std::fs::remove_file(&vid).ok();
    }

    /// Root cause: `run_ffmpegthumbnailer`/`run_ffmpeg` set stdout/stderr to `Stdio::null()` but
    /// never touch stdin, so `Command`'s documented default (`Stdio::inherit()`) hands the child
    /// *this process's own* stdin. Inside konoma that's the user's live controlling terminal, so
    /// ffmpeg/ffmpegthumbnailer — spawned from the media worker thread while the app is running —
    /// end up racing crossterm for the same keypress bytes (no `-nostdin` either).
    ///
    /// Proven deterministically and OS-independently, with **no timing/hang risk** either way:
    /// this test process's own fd 0 is redirected (via raw `dup`/`dup2`/`close` — always linked
    /// into any Unix Rust binary via libc, so no extra crate dependency is needed) to a small
    /// *regular* file holding a sentinel string. Reading a regular file can never block (unlike a
    /// live pipe/terminal), so there is no risk of this test hanging under either the buggy or the
    /// fixed behavior. Fake `ffmpeg`/`ffmpegthumbnailer` scripts are placed first on `PATH`; each
    /// just `cat`s whatever it receives on stdin into a marker file next to itself. If stdin is
    /// `/dev/null` (fixed), the marker is empty; if stdin is inherited (the bug), the marker
    /// contains the sentinel.
    #[cfg(unix)]
    #[test]
    fn ffmpeg_tools_never_inherit_this_process_stdin() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::io::AsRawFd;

        // No `libc` crate dependency needed: `dup`/`dup2`/`close` are POSIX functions always
        // linked into a Unix Rust binary (std itself is built on top of them) — declaring the FFI
        // signature ourselves is enough for the linker to resolve them.
        extern "C" {
            fn dup(fd: i32) -> i32;
            fn dup2(oldfd: i32, newfd: i32) -> i32;
            fn close(fd: i32) -> i32;
        }

        let _guard = PATH_MUTATING_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = unique_tmp("konoma_vthumb_stdin_leak_test");
        std::fs::create_dir_all(&dir).expect("create test dir");

        let sentinel_path = dir.join("sentinel.txt");
        std::fs::write(&sentinel_path, b"STDIN_LEAK_SENTINEL\n").unwrap();

        for name in ["ffmpeg", "ffmpegthumbnailer"] {
            let script = dir.join(name);
            std::fs::write(
                &script,
                format!("#!/bin/sh\ncat > \"$(dirname \"$0\")/captured_{name}.txt\"\nexit 1\n"),
            )
            .unwrap();
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }

        let orig_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.display(), orig_path);
        // SAFETY: serialized against the only other PATH-mutating test in this module via
        // `PATH_MUTATING_TESTS` above, and restored (via the `Restore` guard below) on every exit
        // path — including a failed assertion (this project disallows `panic = "abort"`, so
        // unwinding + `Drop` cleanup is reliable here).
        unsafe { std::env::set_var("PATH", &new_path) };

        let sentinel_file = std::fs::File::open(&sentinel_path).expect("open sentinel");
        // SAFETY: `dup`/`dup2` are called on valid, currently-open file descriptors; return
        // values are checked immediately below.
        let saved_stdin = unsafe { dup(0) };
        assert!(saved_stdin >= 0, "failed to save this process's fd 0");
        let rc = unsafe { dup2(sentinel_file.as_raw_fd(), 0) };
        assert_eq!(rc, 0, "failed to redirect fd 0 to the sentinel file");

        // Restores fd 0 and PATH on every exit path (normal return, `assert!` panic/unwind, or an
        // early `?`/`expect` panic) so a failing assertion never leaves this test process's own
        // stdin/PATH corrupted for whichever test runs next on this thread.
        struct Restore {
            orig_path: String,
            saved_stdin: i32,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    dup2(self.saved_stdin, 0);
                    close(self.saved_stdin);
                    std::env::set_var("PATH", &self.orig_path);
                }
            }
        }
        let _restore = Restore {
            orig_path: orig_path.clone(),
            saved_stdin,
        };

        let out = dir.join("out.png");
        let _ = run_ffmpegthumbnailer(Path::new("/dev/null"), &out);
        let _ = run_ffmpeg(Path::new("/dev/null"), &out);

        let captured_thumbnailer =
            std::fs::read(dir.join("captured_ffmpegthumbnailer.txt")).unwrap_or_default();
        let captured_ffmpeg = std::fs::read(dir.join("captured_ffmpeg.txt")).unwrap_or_default();

        assert!(
            captured_thumbnailer.is_empty(),
            "run_ffmpegthumbnailer が親プロセスの stdin を子に継承している(sentinel を読めてしまった): {:?}",
            String::from_utf8_lossy(&captured_thumbnailer)
        );
        assert!(
            captured_ffmpeg.is_empty(),
            "run_ffmpeg が親プロセスの stdin を子に継承している(sentinel を読めてしまった): {:?}",
            String::from_utf8_lossy(&captured_ffmpeg)
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Root cause: `temp_png_path` built its path directly under `std::env::temp_dir()`, which is
    /// world-writable/world-readable on Linux (`/tmp` is mode `1777`) — the extracted video frame
    /// briefly sits at a pid-and-counter-predictable path, readable by any other local user on a
    /// shared box, since the PNG itself is written by `ffmpeg`/`ffmpegthumbnailer` (an external
    /// process we don't control) under whatever mode their own default umask picks (typically
    /// `0644`). Restricting the **directory** (not the file) is what actually closes this — POSIX
    /// requires execute/search permission on every ancestor directory to open a file by path, so a
    /// `0700` directory blocks other users regardless of the mode the external tool used for the
    /// file itself.
    #[cfg(unix)]
    #[test]
    fn temp_png_path_lives_in_an_owner_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_png_path();
        let dir = path.parent().expect("temp_png_path has a parent dir");
        // The discriminating check: this must be *our own* dedicated subdirectory, not the shared
        // system temp dir directly. Checking only the resulting mode below isn't enough to prove
        // our own code sets it — on macOS, `std::env::temp_dir()` itself already happens to be
        // `0700` (a per-user `/var/folders/.../T/`), which would make a mode-only assertion pass
        // by coincidence even without this fix. Linux's `/tmp` (mode `1777`, world-writable) has no
        // such luck, which is exactly the platform this bug targets.
        assert_ne!(
            dir,
            std::env::temp_dir(),
            "システム共有の一時ディレクトリ直下に出力している(専用サブディレクトリを持っていない)"
        );
        let meta = std::fs::metadata(dir).expect("private temp dir should exist by now");
        assert!(meta.is_dir());
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "抽出フレームの置き場が owner-only(0700) になっていない: {dir:?} mode={mode:#o}"
        );
    }

    /// Root cause: `run_ffmpegthumbnailer`/`run_ffmpeg` used a blocking `Command::status()` with
    /// no upper bound at all — a hung external tool (a pathological input that makes it loop
    /// forever, say) blocked the media worker thread permanently, leaking both the thread and the
    /// child process, and left the busy-indicator spinner stuck spinning.
    ///
    /// Confirmed for real before this fix existed (pasted into the PR/commit description, not kept
    /// as a permanent test): `Command::new("tail").arg("-f").arg("/dev/null").stdout(Stdio::null())
    /// .stderr(Stdio::null()).status()` — the exact shape `run_ffmpeg` and every runner in
    /// `preview::pdf` used before this fix — was still blocked after 3 real seconds waiting on a
    /// `recv_timeout`.
    ///
    /// This permanent test instead proves it deterministically and fast, via `spawn_and_wait_
    /// with_timeout` directly with a short injected timeout (production always uses the real,
    /// generous `TOOL_TIMEOUT`). The fixture (`tail -f /dev/null`) genuinely never exits on its
    /// own — unlike e.g. `sleep N`, which eventually completes regardless of whether the timeout
    /// mechanism works, and so wouldn't actually prove anything. This project avoids asserting
    /// *exact* wall-clock durations (a prior CI break — see `docs/STATUS.md`), so the bound below
    /// is a large, deliberately loose multiple of the injected timeout: it exists only so a real
    /// regression (no timeout at all) fails this test instead of hanging the whole test run.
    #[cfg(unix)]
    #[test]
    fn spawn_and_wait_with_timeout_kills_a_command_that_never_exits() {
        let mut cmd = Command::new("tail");
        cmd.arg("-f")
            .arg("/dev/null")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let started = Instant::now();
        let status = spawn_and_wait_with_timeout(&mut cmd, Duration::from_millis(200));
        let elapsed = started.elapsed();
        assert!(
            status.is_none(),
            "ハングするコマンドが None(タイムアウト)を返さなかった: {status:?}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "タイムアウトが機能せずブロックし続けた: elapsed={elapsed:?}"
        );
    }
}
