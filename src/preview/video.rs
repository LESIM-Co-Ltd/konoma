// Video thumbnail preview: extract a single representative frame and return it as a DynamicImage.
// The result is put onto the app's image_src just like SVG, and from there flows through the normal
// image path (prepare_image → worker re-encode → kitty graphics) unchanged.
// **We do not "play" video inside the terminal (thumbnail only)** — real-time playback via kitty
// graphics would be CPU-prohibitive and is parser-bound to the point of breaking down in Ghostty
// (investigated 2026-06-27; see docs/AUDIT).
//
// **Extractor priority order** (the same shape `preview::pdf` uses for hayro → qlmanage/sips):
//   1. Pure Rust, in-process, no external tool at all — `re_mp4` reads the container index and
//      `rust_h264` decodes one keyframe. This covers **H.264 (AVC) video inside mp4/m4v/mov**,
//      which is what a screen recording, a `git`-tracked demo clip, or anything a browser can play
//      almost always is.
//   2. `ffmpegthumbnailer`, then `ffmpeg`. Still the only thing that can read **HEVC (iPhone's
//      default recording format), VP9, AV1**, mkv/webm/avi containers, and the H.264 profiles step 1
//      deliberately refuses (below). Both remain strictly optional: if neither is installed this
//      returns None and the caller degrades to a hint message (PRD §5 ease of distribution,
//      principle #3 "unsupported must fail safely").
//
// **Why the pure-Rust path is narrow on purpose.**
//   * *Container*: only `mp4`/`m4v`/`mov` are even opened, because that is what `re_mp4` indexes.
//     Anything else returns None before a single byte is read.
//   * *Codec family*: the track's `codec_string` must start with `avc1`/`avc3`. HEVC reports `hvc1…`
//     and is rejected by name rather than being left to fail somewhere deeper. This is a cheap early
//     filter, **not** a second opinion on the one below: both come from the same `avcC` record.
//   * *Stream properties*: `rust_h264` decodes Baseline/Main/High **8-bit 4:2:0** only, and —
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
//     So the refusal happens *before* decoding and reads the **bitstream's own sequence parameter
//     set**, not the container's summary of it — see `parse_sps_facts` / `sps_is_decodable`.
//   * *Size*: the SPS's coded dimensions, again rather than the container's `tkhd`, because the
//     decoder sizes its buffers from those. An absurd sample size is refused rather than allocated too.
// Everything refused this way simply falls through to step 2, so the user-visible effect of the
// narrowness is "a tool is needed for that file", never a wrong picture.
//
// **What "supported" does and does not promise.** For the streams that pass the gate above, the
// decoded frame matched ffmpeg byte for byte on the bundled `samples/sample.mp4` (all 115,200 Y/U/V
// samples identical). That is not a guarantee for every such stream: a clip carrying custom
// quantization matrices (SPS/PPS scaling lists) decoded with **2.6% of samples differing, by up to
// 50 levels** — visually the same picture, but not bit-identical. Acceptable for a thumbnail, and
// worth knowing before anyone writes a test that asserts equality with ffmpeg in general.
//
// The whole pure-Rust path runs inside `catch_silent`, like every other pre-1.0 decoder konoma
// drives (resvg, hayro, mermaid-rs-renderer): a hostile or malformed file must degrade to the
// external chain, not take down the preview. It genuinely earns its keep here — both `rust_h264`
// (an index-out-of-bounds on interlaced H.264, which is `profile_idc` 100 and so passes every guard
// above) and `re_mp4` (a capacity overflow parsing a fuzzed `avcC`) were observed panicking on real
// inputs during review.
//
// One honest caveat on the ordering: the guards above are konoma's, but `re_mp4::Mp4::read` runs
// *before* them, so "refused rather than allocated" describes what this module decides, not the
// container parse it has to do first to decide anything. That parse is bounded by the index rather
// than the file (`mdat` is skipped, not read — a 1 GB clip costs 19 MB of RSS, measured), and its
// failures land in `catch_silent` like everything else.
//
// `allow_external` (= `[external] video`) gates **step 2 only**. Step 1 never launches a process, so
// H.264 mp4/mov thumbnails keep working with the flag off — exactly the relationship `[external] pdf`
// has with hayro. Tool execution runs on the media worker thread, so a blocking child process never
// blocks the UI.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::DynamicImage;

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

/// Container extensions the pure-Rust path will even open. `re_mp4` indexes the ISO-BMFF family;
/// everything else (mkv/webm/avi/…) is the external chain's job, and is rejected here without
/// reading the file at all.
const NATIVE_CONTAINER_EXTS: [&str; 3] = ["mp4", "m4v", "mov"];

/// `profile_idc` values `rust_h264` actually decodes correctly: Baseline (66), Main (77),
/// High (100). Necessary but **not sufficient** — the profile says nothing about chroma format or
/// bit depth, which is why [`sps_is_decodable`] checks those separately. See the module doc comment
/// for why all of this must be refused *before* decoding rather than checked afterwards.
const DECODABLE_PROFILE_IDCS: [u8; 3] = [66, 77, 100];

/// The `profile_idc` values whose SPS carries the `chroma_format_idc` / `bit_depth_*` block
/// (H.264 §7.3.2.1.1). For every other profile those fields are absent and 8-bit 4:2:0 is implied.
const PROFILES_WITH_CHROMA_BLOCK: [u8; 13] =
    [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// `chroma_format_idc` for 4:2:0 — the only sampling `rust_h264` implements. 0 is monochrome
/// (4:0:0), 2 is 4:2:2, 3 is 4:4:4.
const CHROMA_FORMAT_420: u32 = 1;

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

/// Extract and return one representative frame from the video at `path`.
///
/// Tries the pure-Rust H.264/mp4 path first (no external process, works regardless of
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

/// Extract one keyframe entirely in-process (`re_mp4` + `rust_h264`). `None` means "this file isn't
/// something the built-in decoder handles" (wrong container/codec/profile, corrupt index, or a
/// caught panic) — never a crash, and never a wrong picture: [`thumbnail`] then tries the external
/// tools. Wrapped in `catch_silent` for the same reason `preview::pdf::render_page_native` is: these
/// are pre-1.0 decoders being pointed at arbitrary user files.
fn thumbnail_native(path: &Path) -> Option<DynamicImage> {
    crate::preview::markdown::catch_silent(|| thumbnail_native_inner(path)).flatten()
}

fn thumbnail_native_inner(path: &Path) -> Option<DynamicImage> {
    if !has_native_container_ext(path) {
        return None;
    }
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

    // Guard 1 — codec family, from the container. A cheap early filter that rejects HEVC/VP9/AV1 by
    // name; it is *not* the profile check (both come from the same `avcC` record — see
    // `codec_string_is_avc`), so it stops nothing that guard 2 wouldn't.
    if !codec_string_is_avc(track.codec_string(&mp4).as_deref()) {
        return None;
    }

    let cfg = track.raw_codec_config(&mp4)?;
    let avcc = rust_h264::nal::parse_avcc_config(&cfg).ok()?;

    // Guard 2 — the load-bearing one. Read the **sequence parameter set out of the bitstream**, not
    // the container's summary of it, and require exactly what `rust_h264` implements. See
    // `sps_is_decodable` and the module doc comment.
    let sps = parse_sps_facts(&avcc.sps_nals.first()?.rbsp)?;
    if !sps_is_decodable(&sps) {
        return None;
    }

    // Every failure inside this loop is a `continue`, not an early return: one unreadable or
    // undecodable keyframe should cost us that keyframe, not the whole file.
    let mut last: Option<DynamicImage> = None;
    for sample_idx in pick_keyframes(&track.samples, NATIVE_FLAT_FRAME_RETRIES) {
        let Some(sample) = track.samples.get(sample_idx) else {
            continue;
        };
        if sample.size == 0 || sample.size > NATIVE_MAX_SAMPLE_BYTES {
            continue;
        }
        let Some(buf) = read_sample_bytes(&mut file, sample.offset, sample.size) else {
            continue;
        };
        let Some(frame) = decode_keyframe(&avcc, &buf) else {
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

/// Whether the path's extension is one the pure-Rust path opens (ASCII case-insensitive).
fn has_native_container_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            NATIVE_CONTAINER_EXTS
                .iter()
                .any(|known| e.eq_ignore_ascii_case(known))
        })
        .unwrap_or(false)
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

/// Minimal MSB-first bit reader with the two exp-Golomb codings H.264 headers use. Every read is
/// bounds-checked and returns None past the end, so a truncated parameter set can only end the parse,
/// never read past the buffer or loop forever.
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

/// Feed SPS/PPS and one sample's NAL units to a fresh decoder and take the first picture out of it.
///
/// `Decoder` (not `OrderedDecoder`) on purpose: we want *a* correct picture, not a correctly ordered
/// sequence, and the plain decoder hands back the IDR immediately instead of holding it to resolve
/// B-frame display order. A fresh decoder per attempt keeps a failed attempt from poisoning the next
/// keyframe's state.
fn decode_keyframe(
    avcc: &rust_h264::nal::AvccConfig<'_>,
    sample: &[u8],
) -> Option<rust_h264::decoder::Frame> {
    let mut decoder = rust_h264::decoder::Decoder::new();
    for nal in avcc.sps_nals.iter().chain(avcc.pps_nals.iter()) {
        decoder.decode_nal(nal).ok()?;
    }
    for nal in rust_h264::nal::parse_avcc(sample, avcc.length_size) {
        if let Some(frame) = decoder.decode_nal(&nal).ok()? {
            return Some(frame);
        }
    }
    decoder.flush()
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

/// Convert a decoded 8-bit 4:2:0 frame to RGB.
///
/// Limited ("TV") range, which is what essentially all H.264 in the wild uses. The matrix is picked
/// by height — BT.709 at 720p and above, BT.601 below — which is the same heuristic every player
/// uses as a default. This is an **approximation**: the authoritative answer lives in the SPS's VUI
/// `colour_primaries`/`matrix_coefficients`, which `rust_h264` does not surface. Getting it wrong
/// shifts saturation slightly on an unusually-tagged clip; it never produces the kind of structural
/// garbage the profile guard exists to prevent.
fn frame_to_image(frame: &rust_h264::decoder::Frame) -> Option<DynamicImage> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    // `rust_h264` sizes the chroma planes with truncating division, so an odd display dimension
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
        struct BitWriter {
            bytes: Vec<u8>,
            bits: u32,
        }
        impl BitWriter {
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

        let mut w = BitWriter {
            bytes: Vec::new(),
            bits: 0,
        };
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

    /// Only H.264/AVC gets the pure-Rust path. HEVC (`hvc1…`, iPhone's default recording format),
    /// VP9 and AV1 must be rejected here by name rather than left to fail deeper in the decoder.
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

    /// The container gate is by **extension**, decided before the file is opened, so a `.mkv`/
    /// `.webm`/extensionless path costs nothing at all. Proven discriminatingly: the fixture files
    /// below hold **byte-for-byte valid mp4 data** (a copy of `samples/sample.mp4`, which
    /// `native_path_extracts_a_real_frame_without_any_external_tool` shows really does decode) and
    /// are still refused — so the refusal can only be coming from the name, i.e. nothing read it.
    #[test]
    fn native_path_only_opens_containers_it_indexes() {
        assert!(has_native_container_ext(Path::new("/x/clip.mp4")));
        assert!(
            has_native_container_ext(Path::new("/x/clip.MP4")),
            "大小無視"
        );
        assert!(has_native_container_ext(Path::new("/x/clip.m4v")));
        assert!(has_native_container_ext(Path::new("/x/clip.mov")));
        assert!(!has_native_container_ext(Path::new("/x/clip.mkv")));
        assert!(!has_native_container_ext(Path::new("/x/clip.webm")));
        assert!(!has_native_container_ext(Path::new("/x/clip.avi")));
        assert!(!has_native_container_ext(Path::new("/x/clip")));

        let Some(src) = sample_path_or_skip("sample.mp4") else {
            return;
        };
        let bytes = std::fs::read(&src).unwrap();
        let dir = unique_tmp("konoma_video_ext_gate_test");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["clip.mkv", "clip.webm", "clip"] {
            let disguised = dir.join(name);
            std::fs::write(&disguised, &bytes).unwrap();
            assert!(
                thumbnail_native(&disguised).is_none(),
                "{name}: 拡張子ゲートで即 None(中身が正しい mp4 でも読みに行かない)"
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
