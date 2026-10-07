//! Mostly `App::compute_media_diff` in isolation (no run loop) — the same style
//! `md_diff.rs`'s own tests use, plus a handful of `App::poll_media_diff`/`apply_media_diff`
//! tests that need a real (if tiny) `App` for the cache-insertion/eviction behavior.

use super::*;
use crate::config::Config;
#[cfg(feature = "git")]
use crate::test_support::{init_git_repo, jj_scratch_bare, run_git};
use crate::test_support::{sample_path_or_skip, unique_tmp};

fn req(path: PathBuf, root: PathBuf, baseline: DiffBaseline) -> MediaDiffRequest {
    MediaDiffRequest {
        gen: 1,
        path,
        root,
        baseline,
        page: 1,
        raster_px: (800, 600),
        preview_rules: Config::default().preview.rules,
        preview_commands: true,
    }
}

// ---- kind classification / summary vs. ready ----

#[test]
fn a_new_untracked_image_is_ready_with_old_absent() {
    let dir = unique_tmp("konoma_media_diff_new_untracked");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("new.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
        .save(&png)
        .unwrap();
    let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { kind, old, new, .. } => {
            assert_eq!(kind, MediaDiffKind::Image);
            assert!(
                matches!(old, MediaDiffSideDecoded::Absent),
                "旧版は無いはず"
            );
            assert!(
                matches!(new, MediaDiffSideDecoded::Picture(_)),
                "新版はデコードされるはず"
            );
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

#[test]
fn a_deleted_image_is_ready_with_new_absent_and_old_a_picture() {
    let dir = unique_tmp("konoma_media_diff_deleted");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("was.png"); // never actually written — "deleted" = no file on disk
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(5, 5, image::Rgb([9, 8, 7])))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let r = req(png, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { kind, old, new, .. } => {
            assert_eq!(
                kind,
                MediaDiffKind::Image,
                "削除された PNG も旧バイト列から判定できるはず"
            );
            assert!(matches!(old, MediaDiffSideDecoded::Picture(_)));
            assert!(matches!(new, MediaDiffSideDecoded::Absent));
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

#[test]
fn identical_bytes_report_same_bytes_true() {
    let dir = unique_tmp("konoma_media_diff_identical");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("same.png");
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([1, 1, 1])))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    std::fs::write(&png, &bytes).unwrap();
    let r = req(png, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { same_bytes, .. } => {
            assert!(same_bytes, "同一バイト列なら same_bytes のはず");
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

#[test]
fn a_corrupt_image_fails_that_side_without_taking_the_other_down() {
    let dir = unique_tmp("konoma_media_diff_corrupt");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("corrupt.png");
    // Real PNG magic bytes (`infer` only checks the first 4 — `docs/FEATURE-MEDIA-DIFF.md` §2's
    // MIME-sniff classification will therefore still resolve this to `Image`), followed by
    // garbage that isn't a valid PNG stream at all — the decode itself must fail, not the kind
    // classification (a plain-text "not a png at all" would instead sniff as `Text` and the
    // whole diff would degrade to `Summary` before ever reaching a per-side decode, which is a
    // different code path than the one this test means to exercise).
    std::fs::write(&png, b"\x89PNG\r\n\x1a\ngarbage, not a real PNG stream").unwrap();
    let good_old = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([0, 0, 0])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    let r = req(
        png,
        dir.to_path_buf(),
        DiffBaseline::FollowSnapshot(good_old),
    );
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { old, new, .. } => {
            assert!(
                matches!(old, MediaDiffSideDecoded::Picture(_)),
                "旧版は正常"
            );
            assert!(
                matches!(new, MediaDiffSideDecoded::Failed { .. }),
                "新版は壊れているので Failed のはず: {new:?}"
            );
        }
        other => {
            panic!("Ready のはず(kind は new が .png 拡張子なので Image に解決される): {other:?}")
        }
    }
}

#[test]
fn a_non_picture_kind_degrades_to_summary_with_sizes() {
    let dir = unique_tmp("konoma_media_diff_summary_kind");
    std::fs::create_dir_all(&dir).unwrap();
    let mp4 = dir.join("clip.mp4");
    std::fs::write(&mp4, b"0123456789").unwrap();
    let r = req(
        mp4,
        dir.to_path_buf(),
        DiffBaseline::FollowSnapshot(b"01234".to_vec()),
    );
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Summary {
            old_len, new_len, ..
        } => {
            assert_eq!(old_len, Some(5));
            assert_eq!(new_len, Some(10));
        }
        other => panic!("動画は Summary のはず: {other:?}"),
    }
}

// ---- decode_svg_side: rasterizes to the LARGER axis of raster_px ----

/// `decode_svg_side`'s `max_px = raster_px.0.max(raster_px.1)` — the rasterized picture's own
/// pixel size must actually reflect the **larger** of the two `raster_px` components, not the
/// smaller one and not, say, the width component alone. A deliberately non-square, asymmetric
/// `raster_px` (100×50) against a small (40×20, 2:1) SVG — small enough that any target here is
/// an upscale, never `rasterize_trusted`'s own "shrink to fit `HARD_MAX_PX`" branch — makes the
/// three candidate target values (100, 50, and "width alone" = 100 too, so also cross-checked
/// against a second, width-larger fixture below) produce three genuinely different pixel sizes,
/// discriminating a `.max` from a `.min` or an accidental "just use one axis" mutant.
#[test]
fn decode_svg_side_rasterizes_to_the_larger_axis_of_raster_px() {
    let dir = unique_tmp("konoma_media_diff_svg_raster_axis");
    std::fs::create_dir_all(&dir).unwrap();
    let svg = dir.join("icon.svg");
    // A 40x20 (2:1) viewBox — small enough that `rasterize_trusted` always upscales for any
    // `raster_px` used below (never shrinks below 1:1 scale under `HARD_MAX_PX`).
    std::fs::write(
        &svg,
        "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='20' viewBox='0 0 40 20'></svg>",
    )
    .unwrap();
    let r = MediaDiffRequest {
        gen: 1,
        path: svg.clone(),
        root: dir.to_path_buf(),
        baseline: DiffBaseline::Empty,
        page: 1,
        raster_px: (100, 50), // height (50) is the *smaller* component — must not be used alone.
        preview_rules: Config::default().preview.rules,
        preview_commands: true,
    };
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { new, .. } => match new {
            MediaDiffSideDecoded::Picture(p) => {
                // scale = max(100,50) / max(40,20) = 100/40 = 2.5 -> (40*2.5, 20*2.5) = (100, 50).
                assert_eq!(
                    (p.image.width(), p.image.height()),
                    (100, 50),
                    "raster_px の大きい方(100)が長辺のターゲットになるはず"
                );
            }
            other => panic!("SVG はデコードされるはず: {other:?}"),
        },
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// The mirror of the test above with the **width** as the larger component instead of the
/// height — together the two pin that it is genuinely `max(w, h)`, not "whichever axis happens
/// to be listed first" or a hardcoded preference for one axis.
#[test]
fn decode_svg_side_rasterizes_to_the_larger_axis_of_raster_px_when_width_is_larger() {
    let dir = unique_tmp("konoma_media_diff_svg_raster_axis_w");
    std::fs::create_dir_all(&dir).unwrap();
    let svg = dir.join("icon.svg");
    std::fs::write(
        &svg,
        "<svg xmlns='http://www.w3.org/2000/svg' width='20' height='40' viewBox='0 0 20 40'></svg>",
    )
    .unwrap();
    let r = MediaDiffRequest {
        gen: 1,
        path: svg.clone(),
        root: dir.to_path_buf(),
        baseline: DiffBaseline::Empty,
        page: 1,
        raster_px: (50, 100), // width (50) is the *smaller* component this time.
        preview_rules: Config::default().preview.rules,
        preview_commands: true,
    };
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { new, .. } => match new {
            MediaDiffSideDecoded::Picture(p) => {
                // scale = max(50,100) / max(20,40) = 100/40 = 2.5 -> (20*2.5, 40*2.5) = (50, 100).
                assert_eq!(
                    (p.image.width(), p.image.height()),
                    (50, 100),
                    "raster_px の大きい方(100)が長辺のターゲットになるはず"
                );
            }
            other => panic!("SVG はデコードされるはず: {other:?}"),
        },
        other => panic!("Ready のはず: {other:?}"),
    }
}

// ---- compute_media_diff_with_cap: cap-before-reading (over cap, `||` not `&&`) ----

/// The new side alone over cap (the old side is entirely absent — an untracked/new file) still
/// degrades to `Summary`, proving `over_cap = old_over_cap || new_over_cap` — a mutation to `&&`
/// would require *both* sides over cap, and this fixture's old side isn't even present to be
/// "over" anything, so a `&&` mutant would wrongly reach the `Ready`/decode path instead.
#[test]
fn new_side_over_cap_alone_degrades_to_summary() {
    let dir = unique_tmp("konoma_media_diff_cap_new_over");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("big.png");
    // Content is irrelevant — once metadata alone says it's over cap, the bytes are never read
    // at all (`compute_media_diff_with_cap`'s own doc comment on the new side).
    std::fs::write(&png, vec![0u8; 100]).unwrap();
    let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
    match App::compute_media_diff_with_cap(&r, 50) {
        MediaDiffComputed::Summary {
            old_len, new_len, ..
        } => {
            assert_eq!(old_len, None, "旧版は無い(新規/未追跡)");
            assert_eq!(
                new_len,
                Some(100),
                "新版のサイズは metadata から取れているはず(読まずに)"
            );
        }
        other => panic!("cap 超過は Summary のはず: {other:?}"),
    }
}

/// The **old** side alone over cap, while the new side is tiny — the mirror of the test above,
/// completing the `||` pin (a `&&` mutant would also fail to degrade here, since only one side
/// is over cap).
#[test]
fn old_side_over_cap_alone_degrades_to_summary_even_when_new_is_tiny() {
    let dir = unique_tmp("konoma_media_diff_cap_old_over");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("small.png");
    std::fs::write(&png, vec![1u8; 10]).unwrap();
    // No size-only API for the old side (`resolve_old_bytes`'s own doc comment) — a follow
    // snapshot is always fully in hand once resolved, so its length is only known this way.
    let r = req(
        png,
        dir.to_path_buf(),
        DiffBaseline::FollowSnapshot(vec![2u8; 100]),
    );
    match App::compute_media_diff_with_cap(&r, 50) {
        MediaDiffComputed::Summary {
            old_len, new_len, ..
        } => {
            assert_eq!(old_len, Some(100));
            assert_eq!(
                new_len,
                Some(10),
                "新版は cap 未満なので実際に読まれているはず"
            );
        }
        other => {
            panic!("cap 超過(旧版のみ)は Summary のはず(|| であって && ではない): {other:?}")
        }
    }
}

/// `n == cap` is **not** over cap (the comparison is `>`, not `>=`) — the new side decodes for
/// real at exactly the cap's own byte count.
#[test]
fn exact_cap_size_boundary_is_not_over_cap() {
    let dir = unique_tmp("konoma_media_diff_cap_boundary");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("exact.png");
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3])))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    std::fs::write(&png, &bytes).unwrap();
    let cap = bytes.len() as u64; // n == cap, exactly.
    let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
    match App::compute_media_diff_with_cap(&r, cap) {
        MediaDiffComputed::Ready { new, .. } => {
            assert!(
                matches!(new, MediaDiffSideDecoded::Picture(_)),
                "n == cap は over cap ではないので実際にデコードされるはず"
            );
        }
        other => panic!("境界(n==cap)で Summary に落ちてはいけない: {other:?}"),
    }
}

/// Unlike `new_side_over_cap_alone_degrades_to_summary` above (whose garbage-bytes fixture
/// fails `classify_kind` regardless of the cap check — `rule_matches`' mime branch sniffs the
/// *real* file at `path` via `infer::get_from_path` even when `sniff` is `None`, so invalid
/// magic bytes alone already forces `Summary`, independent of `over_cap` — this uses a **real,
/// decodable** PNG whose actual byte length exceeds `cap`: genuinely discriminating, since a
/// mutant that skips the metadata-based skip-the-read step would classify it as `Image` and
/// actually decode it (`Ready`), not degrade to `Summary`.
#[test]
fn new_side_real_decodable_png_over_cap_still_degrades_to_summary() {
    let dir = unique_tmp("konoma_media_diff_cap_new_over_real_png");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("big.png");
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(20, 20, image::Rgb([9, 9, 9])))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    std::fs::write(&png, &bytes).unwrap();
    let cap = (bytes.len() as u64) - 1; // strictly under the file's real size.
    let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
    match App::compute_media_diff_with_cap(&r, cap) {
        MediaDiffComputed::Summary { new_len, .. } => {
            assert_eq!(new_len, Some(bytes.len() as u64));
        }
        other => panic!(
            "実在する有効な PNG でも cap 超過なら Summary のはず(実際にデコードされてはいけない): {other:?}"
        ),
    }
}

/// Mutation-proving: if `classify_kind` degenerated into "always picture-capable", this would
/// come back `Ready` instead — pins the actual branch fires.
#[test]
fn classify_kind_actually_gates_the_ready_vs_summary_branch() {
    assert_eq!(
        classify_kind(&PreviewKind::Video(PathBuf::from("x.mp4"))),
        None
    );
    assert_eq!(
        classify_kind(&PreviewKind::Image(PathBuf::from("x.png"))),
        Some(MediaDiffKind::Image)
    );
}

#[test]
fn over_the_cap_degrades_to_summary_via_the_test_seam() {
    let dir = unique_tmp("konoma_media_diff_cap");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("big.png");
    std::fs::write(&png, vec![0u8; 100]).unwrap(); // "big" only relative to the tiny test cap below
    let r = req(
        png,
        dir.to_path_buf(),
        DiffBaseline::FollowSnapshot(vec![0u8; 100]),
    );
    // Sanity: under a real-sized cap this would be a Ready (Image, by content sniff/extension).
    match App::compute_media_diff_with_cap(&r, 1_000_000) {
        MediaDiffComputed::Ready { .. } | MediaDiffComputed::Summary { .. } => {}
        MediaDiffComputed::Unavailable => panic!("前提が崩れている"),
    }
    // A cap smaller than either side's byte length forces the Summary (not Ready) branch.
    match App::compute_media_diff_with_cap(&r, 10) {
        MediaDiffComputed::Summary {
            old_len, new_len, ..
        } => {
            assert_eq!(old_len, Some(100));
            assert_eq!(new_len, Some(100));
        }
        other => panic!("上限超は Summary のはず: {other:?}"),
    }
}

#[test]
fn pdf_page_beyond_that_sides_own_count_is_page_missing() {
    let Some(p) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let bytes = std::fs::read(&p).unwrap();
    let dir = unique_tmp("konoma_media_diff_pdf_page_missing");
    std::fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("doc.pdf");
    std::fs::write(&pdf, &bytes).unwrap(); // sample.pdf is a known 3-page document
    let mut r = req(pdf, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
    r.page = 999;
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { old, new, .. } => {
            assert!(matches!(old, MediaDiffSideDecoded::PageMissing));
            assert!(matches!(new, MediaDiffSideDecoded::PageMissing));
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

// ---- baseline / base selection ----

#[cfg(feature = "git")]
#[test]
fn git_baseline_reports_head_and_reads_the_committed_blob() {
    let dir = unique_tmp("konoma_media_diff_git_head");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let png = dir.join("pic.png");
    let old_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &old_bytes).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    // Modify on disk without committing.
    let new_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([2, 2, 2])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &new_bytes).unwrap();

    let r = req(png, dir.to_path_buf(), DiffBaseline::Vcs);
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, old, .. } => {
            assert_eq!(base, MediaBase::Head);
            match old {
                MediaDiffSideDecoded::Picture(p) => assert_eq!(p.bytes, old_bytes.len() as u64),
                other => panic!("旧版はコミット済み画像のはず: {other:?}"),
            }
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

#[cfg(feature = "git")]
#[test]
fn follow_snapshot_baseline_reports_follow_start() {
    let dir = unique_tmp("konoma_media_diff_follow_snapshot");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    let bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &bytes).unwrap();
    let r = req(png, dir.to_path_buf(), DiffBaseline::FollowSnapshot(bytes));
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, .. } => assert_eq!(base, MediaBase::FollowStart),
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// A follow session with **no** usable snapshot for this file (`DiffBaseline::Empty` — dirty at
/// follow-start but too large to snapshot, or genuinely no baseline) falls back to the committed
/// baseline (`docs/FEATURE-MEDIA-DIFF.md` §7), and the reported base names that fallback
/// (`Head`), not `FollowStart` — unlike the Markdown block-diff, which treats this as "no old
/// side" instead (`App::compute_md_diff`'s own doc comment).
#[cfg(feature = "git")]
#[test]
fn follow_without_a_snapshot_falls_back_to_head_and_reports_head() {
    let dir = unique_tmp("konoma_media_diff_follow_no_snapshot");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let png = dir.join("pic.png");
    let bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &bytes).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);

    let r = req(png, dir.to_path_buf(), DiffBaseline::Empty);
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, old, .. } => {
            assert_eq!(
                base,
                MediaBase::Head,
                "スナップショット無し = HEAD に落ちて基準名も HEAD のはず"
            );
            assert!(
                matches!(old, MediaDiffSideDecoded::Picture(_)),
                "HEAD には実体があるので Absent ではないはず: {old:?}"
            );
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// `FollowHead` (clean at follow-start, pinned to the HEAD sha) for a file that did **not exist**
/// at that sha — created after following began. `crate::git::blob_at` returns `None` for this
/// (not `Some(empty vec)`), and `resolve_old_bytes` must fall back to the committed baseline
/// exactly like `DiffBaseline::Empty` does, landing on `Absent`/`old_len == None`/`base == Head`
/// — not `Failed` with a spurious 0-byte "旧版" and not mislabeled `FollowStart`. Regression: an
/// earlier version used `blob_at(..).unwrap_or_default()`, decoding the empty vec as a corrupt
/// image (`Failed`) instead of recognizing the file simply didn't exist yet.
#[cfg(feature = "git")]
#[test]
fn follow_head_for_a_file_created_after_follow_start_falls_back_to_head_and_is_absent() {
    let dir = unique_tmp("konoma_media_diff_follow_head_created_after");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    // A first commit that does NOT include the image at all — this is the sha follow-start pins.
    std::fs::write(dir.join("placeholder.txt"), b"seed\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "seed"]);
    let sha = crate::git::head_commit_id(&dir).expect("HEAD sha が取れるはず");

    // The image is created only *after* that commit (untracked at follow-start).
    let png = dir.join("new-since-follow.png");
    let bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([9, 9, 9])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &bytes).unwrap();

    // Direct, precise check on the function under review: `blob_at` returns `None` here (the
    // path isn't in that commit at all), so this must resolve to `(None, Head)` — never
    // `(Some(vec![]), FollowStart)`.
    let (old_bytes, base) =
        resolve_old_bytes(&DiffBaseline::FollowHead { sha: sha.clone() }, &dir, &png);
    assert_eq!(
        old_bytes, None,
        "blob_at が None ⟹ 旧版は None のはず(空 Vec ではない)"
    );
    assert_eq!(
        base,
        MediaBase::Head,
        "blob_at が None ⟹ HEAD への構成的フォールバック、FollowStart と偽らない"
    );

    // Integration-level check: the full computation must therefore treat the old side as
    // genuinely absent, not as a corrupt/undecodable 0-byte image.
    let r = req(png, dir.to_path_buf(), DiffBaseline::FollowHead { sha });
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, old, .. } => {
            assert_eq!(base, MediaBase::Head);
            assert!(
                matches!(old, MediaDiffSideDecoded::Absent),
                "follow 開始時点に存在しないファイルは Absent のはず(0 バイトの Failed ではない): {old:?}"
            );
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// `FollowHead` for a file created *after* the pinned follow-start sha, same as the test above —
/// but this time it has **since been committed** (unlike that test's still-untracked fixture).
/// `blob_at(root, sha, path)` still returns `None` (the path genuinely isn't in the pinned
/// commit), so this falls back to `base_contents` exactly as before — but `base_contents` reads
/// against the **current** HEAD, not the pinned sha, and the file *is* there now: the old side
/// must be the real committed bytes (`Head`, matching `Some`), not `Absent`. A version of
/// `resolve_old_bytes` that conflated "not in the pinned commit" with "not in the repo at all"
/// (e.g. by short-circuiting straight to `Absent` on a `blob_at` miss, instead of actually
/// falling through to `base_contents`) would still pass the sibling test above — both fixtures
/// hit the identical `blob_at → None` branch — but only this one has a real committed blob on
/// the other side of that fallback to notice going missing.
#[cfg(feature = "git")]
#[test]
fn follow_head_for_a_file_created_after_follow_start_and_since_committed_reads_head_not_absent() {
    let dir = unique_tmp("konoma_media_diff_follow_head_created_then_committed");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    std::fs::write(dir.join("placeholder.txt"), b"seed\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "seed"]);
    let sha = crate::git::head_commit_id(&dir).expect("HEAD sha が取れるはず"); // follow-start pin

    // Created after follow-start (not in `sha` at all)...
    let png = dir.join("new-since-follow.png");
    let committed_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(5, 5, image::Rgb([3, 3, 3])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &committed_bytes).unwrap();
    // ...but, unlike the sibling test, it IS committed since (advancing HEAD past `sha`).
    run_git(&dir, &["add", "-A"]);
    run_git(
        &dir,
        &["commit", "-q", "-m", "add the png after follow-start"],
    );

    let (old_bytes, base) =
        resolve_old_bytes(&DiffBaseline::FollowHead { sha: sha.clone() }, &dir, &png);
    assert_eq!(
        old_bytes,
        Some(committed_bytes.clone()),
        "follow 開始後に作成されても、以後コミットされていれば旧版は現在の HEAD の実バイト列のはず(Absent ではない)"
    );
    assert_eq!(
        base,
        MediaBase::Head,
        "blob_at は None(follow 開始時点には無い)だが、現在の HEAD にはあるので Head 扱い"
    );

    let r = req(png, dir.to_path_buf(), DiffBaseline::FollowHead { sha });
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, old, .. } => {
            assert_eq!(base, MediaBase::Head);
            assert!(
                matches!(old, MediaDiffSideDecoded::Picture(_)),
                "以後コミットされているので旧版はデコードされた絵のはず(Absent ではない): {old:?}"
            );
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// `FollowHead` for a file that **did** exist at the pinned sha (clean at follow-start) and was
/// then modified — `blob_at` finds real bytes, so the base stays `FollowStart` (unlike the
/// "created after" case above, which falls back to `Head`).
#[cfg(feature = "git")]
#[test]
fn follow_head_for_a_file_modified_since_follow_start_reads_the_committed_blob() {
    let dir = unique_tmp("konoma_media_diff_follow_head_modified");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let png = dir.join("pic.png");
    let old_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &old_bytes).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    let sha = crate::git::head_commit_id(&dir).expect("HEAD sha が取れるはず");

    // Modified on disk after the pinned sha (still clean/committed at follow-start itself).
    let new_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([2, 2, 2])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &new_bytes).unwrap();

    let r = req(png, dir.to_path_buf(), DiffBaseline::FollowHead { sha });
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, old, .. } => {
            assert_eq!(
                base,
                MediaBase::FollowStart,
                "sha に実体があるので FollowStart のはず(HEAD へのフォールバックではない)"
            );
            match old {
                MediaDiffSideDecoded::Picture(p) => {
                    assert_eq!(
                        p.bytes,
                        old_bytes.len() as u64,
                        "旧版はピン留めされた sha のバイト列のはず"
                    );
                }
                other => panic!("旧版はコミット済み画像のはず: {other:?}"),
            }
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

#[cfg(feature = "git")]
#[test]
fn jj_baseline_reports_jj_parent() {
    let Some(dir) = jj_scratch_bare("konoma_media_diff_jj_parent") else {
        return;
    };
    let png = dir.join("pic.png");
    let old_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([3, 3, 3])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &old_bytes).unwrap();
    let ok = std::process::Command::new("jj")
        .current_dir(&dir)
        .env("HOME", &dir)
        .env("JJ_USER", "konoma test")
        .env("JJ_EMAIL", "test@example.invalid")
        .args(["commit", "-m", "seed"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("SKIP: jj commit failed in the scratch workspace");
        std::fs::remove_dir_all(&dir).ok();
        return;
    }
    // Modify on disk without committing (the new working-copy state).
    let new_bytes = {
        let mut b = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([4, 4, 4])))
            .write_to(&mut std::io::Cursor::new(&mut b), image::ImageFormat::Png)
            .unwrap();
        b
    };
    std::fs::write(&png, &new_bytes).unwrap();

    let r = req(png, dir.to_path_buf(), DiffBaseline::Vcs);
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { base, .. } => assert_eq!(base, MediaBase::JjParent),
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// Mutation-proving: `is_jj` must actually consult the real backend, not hardcode `false` — a
/// jj-only workspace (no `.git`) must answer `true`.
#[cfg(feature = "git")]
#[test]
fn is_jj_actually_detects_a_jj_only_workspace() {
    let Some(dir) = jj_scratch_bare("konoma_media_diff_is_jj") else {
        return;
    };
    assert!(is_jj(&dir), "jj のみのワークスペースは jj と判定されるはず");
    let git_dir = unique_tmp("konoma_media_diff_is_jj_git_control");
    std::fs::create_dir_all(&git_dir).unwrap();
    init_git_repo(&git_dir);
    assert!(!is_jj(&git_dir), "git リポジトリは jj と判定されないはず");
}

// ---- App-level: poll/apply, cache insertion, staleness, eviction ----

#[test]
fn poll_media_diff_sync_fallback_returns_ready_on_first_call() {
    let dir = unique_tmp("konoma_media_diff_poll_sync");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([5, 5, 5])))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let outcome = app.poll_media_diff(&png, 1, (400, 300));
    assert!(
        outcome.is_some(),
        "tx 未 attach = 同期フォールバックで初回から結果が返るはず"
    );
}

#[test]
fn apply_media_diff_drops_a_stale_generation() {
    let dir = unique_tmp("konoma_media_diff_stale_gen");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1])))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    // Kick once for real (bumps media_diff_gen to 1 and lands a result via the sync fallback).
    let first = app.poll_media_diff(&png, 1, (400, 300));
    assert!(first.is_some());
    let current_gen = app.media_diff_gen;
    // A result tagged with an older generation must be rejected.
    let stale = MediaDiffResult {
        gen: current_gen.wrapping_sub(1),
        path: png.clone(),
        page: 1,
        raster_px: (400, 300),
        computed: MediaDiffComputed::Unavailable,
    };
    assert!(!app.apply_media_diff(stale), "古い gen は false を返すはず");
    // The landed outcome is still the earlier (real) one, not clobbered by the stale apply.
    assert!(app.poll_media_diff(&png, 1, (400, 300)).is_some());
}

#[test]
fn applied_pictures_land_in_md_image_cache_under_media_diff_keys() {
    let dir = unique_tmp("konoma_media_diff_cache_insert");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([7, 7, 7])))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let outcome = app
        .poll_media_diff(&png, 1, (400, 300))
        .expect("同期で返るはず");
    match outcome {
        MediaDiffOutcome::Ready {
            kind,
            base,
            same_bytes,
            old,
            new,
        } => {
            assert_eq!(kind, MediaDiffKind::Image);
            assert!(
                matches!(base, MediaBase::Head),
                "リポジトリ外なので HEAD 扱い"
            );
            assert!(!same_bytes, "旧版が無いので同一ではない");
            assert!(matches!(old, MediaDiffSide::Absent), "旧版は無いはず");
            match new {
                MediaDiffSide::Picture(p) => {
                    assert!(
                        crate::preview::media_diff::is_media_diff_url(
                            &p.cache_key.to_string_lossy()
                        ),
                        "cache_key は media-diff:// キーのはず: {:?}",
                        p.cache_key
                    );
                    assert!(
                        app.md_image_cache.contains_key(&p.cache_key),
                        "md_image_cache に入っているはず"
                    );
                    assert_eq!(p.natural_px, (4, 4), "自然寸法が保存されているはず");
                    assert!(p.bytes > 0, "バイト数が保存されているはず");
                    assert_eq!(p.page_count, None, "画像は PDF ではないので None のはず");
                }
                other => panic!("新版はデコードされるはず: {other:?}"),
            }
        }
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// `App::ensure_md_cache`'s own Markdown-doc prune (`md_render.rs`'s `md_image_cache.retain`,
/// keyed off `is_mermaid_fence_url`/`is_math_url`) must leave a `media-diff://` key alone — it
/// isn't a mermaid/math key, so the predicate's `!(mermaid || math)` arm already keeps it
/// unconditionally, but this pins that behavior against the real production rebuild path (not
/// just by inspection) rather than assuming the `||` never grows a third disjunct that would
/// change that.
#[test]
fn markdown_prune_on_rebuild_leaves_a_media_diff_key_alone() {
    let dir = unique_tmp("konoma_media_diff_markdown_prune");
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("doc.md");
    // Deliberately no mermaid fence / math expression in this document — isolates the prune's
    // treatment of a media-diff key from its own, separately-tested mermaid/math eviction.
    std::fs::write(&md, "# Title\n\nJust an ordinary paragraph.\n").unwrap();
    let media_key = PathBuf::from(crate::preview::media_diff::media_diff_url(
        KeySide::New,
        crate::preview::media_diff::fnv1a64(b"whatever bytes"),
        1,
        None,
    ));
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    // Open the document first (`enter_preview` clears the *whole* `md_image_cache` on a file
    // switch — a different, already-expected mechanism from the rebuild-time prune this test
    // means to isolate), then insert the media-diff key and force a second `ensure_md_cache`
    // pass (a width/content rebuild of the *same, already-open* file) to exercise the prune.
    app.enter_preview(&md);
    app.ensure_md_cache(100);
    app.md_image_cache
        .insert(media_key.clone(), MdImgEntry::default());
    app.md_cache = None; // force ensure_md_cache to actually rebuild (and prune) again
    app.ensure_md_cache(100);
    assert!(
        app.md_image_cache.contains_key(&media_key),
        "media-diff:// キーは Markdown の mermaid/math prune に巻き込まれないはず"
    );
}

/// The landed (App-level) `MediaDiffSide::Failed` — not just `compute_media_diff`'s own
/// `MediaDiffSideDecoded::Failed` — carries a non-empty reason through `apply_media_diff`'s
/// pixel-stripping step (`materialize_side`), so a caption can eventually say *why* a side
/// couldn't be shown rather than just that it couldn't.
#[test]
fn a_failed_side_keeps_its_reason_through_to_the_landed_outcome() {
    let dir = unique_tmp("konoma_media_diff_landed_failed_reason");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("corrupt.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\ngarbage, not a real PNG stream").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let outcome = app
        .poll_media_diff(&png, 1, (400, 300))
        .expect("同期で返るはず");
    match outcome {
        MediaDiffOutcome::Ready { new, .. } => match new {
            MediaDiffSide::Failed { reason } => {
                assert!(!reason.is_empty(), "理由が空でないはず");
            }
            other => panic!("壊れた PNG は Failed のはず: {other:?}"),
        },
        other => panic!("Ready のはず: {other:?}"),
    }
}

/// The landed `MediaDiffOutcome::Summary` (not just `MediaDiffComputed::Summary`) carries its
/// sizes/`same_bytes`/`base` all the way through `apply_media_diff` unchanged (that variant has
/// no pixel data to strip, unlike `Ready`).
#[test]
fn a_non_picture_kind_lands_as_summary_with_its_fields_intact() {
    let dir = unique_tmp("konoma_media_diff_landed_summary");
    std::fs::create_dir_all(&dir).unwrap();
    let mp4 = dir.join("clip.mp4");
    std::fs::write(&mp4, b"0123456789").unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let outcome = app
        .poll_media_diff(&mp4, 1, (400, 300))
        .expect("同期で返るはず");
    match outcome {
        MediaDiffOutcome::Summary {
            base,
            same_bytes,
            old_len,
            new_len,
        } => {
            assert!(matches!(base, MediaBase::Head));
            assert!(!same_bytes, "旧版が無いので同一ではない");
            assert_eq!(old_len, None, "旧版は無いはず");
            assert_eq!(new_len, Some(10));
        }
        other => panic!("動画は Summary のはず: {other:?}"),
    }
}

#[test]
fn switching_target_drops_the_old_media_diff_keys() {
    let dir = unique_tmp("konoma_media_diff_switch_target");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.png");
    let b = dir.join("b.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
        .save(&a)
        .unwrap();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
        .save(&b)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();

    let outcome_a = app.poll_media_diff(&a, 1, (400, 300)).unwrap();
    let key_a = match outcome_a {
        MediaDiffOutcome::Ready {
            new: MediaDiffSide::Picture(p),
            ..
        } => p.cache_key,
        other => panic!("a: Ready のはず: {other:?}"),
    };
    assert!(app.md_image_cache.contains_key(&key_a));

    // Switching to a different target/page/raster is itself a new request — invalidate first
    // (mirrors what `App::invalidate_diff_caches` does on a real file/target switch), then poll.
    app.invalidate_media_diff();
    let outcome_b = app.poll_media_diff(&b, 1, (400, 300)).unwrap();
    let key_b = match outcome_b {
        MediaDiffOutcome::Ready {
            new: MediaDiffSide::Picture(p),
            ..
        } => p.cache_key,
        other => panic!("b: Ready のはず: {other:?}"),
    };
    assert_ne!(key_a, key_b, "別内容なのでキーも違うはず");
    assert!(
        !app.md_image_cache.contains_key(&key_a),
        "旧ターゲットのキーは prune されるはず"
    );
    assert!(app.md_image_cache.contains_key(&key_b));
}

/// Mutation-proving: if the eviction predicate in `apply_media_diff` degenerated into a no-op
/// (never pruning), `switching_target_drops_the_old_media_diff_keys` above already fails — this
/// test additionally pins that a **mermaid** key untouched by any media diff survives the same
/// `retain` call (the predicate must gate on `is_media_diff_url`, not evict everything).
#[test]
fn eviction_never_touches_a_mermaid_key() {
    let dir = unique_tmp("konoma_media_diff_eviction_leaves_mermaid");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.png");
    let b = dir.join("b.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
        .save(&a)
        .unwrap();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
        .save(&b)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let mermaid_key = PathBuf::from(crate::preview::markdown::mermaid_fence_url(
        "graph LR\nA-->B",
    ));
    app.md_image_cache
        .insert(mermaid_key.clone(), MdImgEntry::default());

    app.poll_media_diff(&a, 1, (400, 300));
    app.invalidate_media_diff();
    app.poll_media_diff(&b, 1, (400, 300));

    assert!(
        app.md_image_cache.contains_key(&mermaid_key),
        "mermaid キーは media-diff の prune に巻き込まれないはず"
    );
}

// ---- worker dispatch: dedup / coalescing / stale-gen (real channel, `attach_media_diff_loader`) ----
//
// Every test above either calls `App::compute_media_diff[_with_cap]` directly (no dispatch
// machinery involved at all) or drives `poll_media_diff`/`apply_media_diff` through the
// synchronous no-`Sender` fallback (`spawn_or_sync_media_diff`'s own doc comment) — which
// bypasses `App::kick_media_diff`'s dedup/coalesce branch and `App::dispatch_media_diff`'s real
// thread spawn entirely, since the sync path never leaves a request "in flight" for a second
// call to observe. These tests attach a **real** `mpsc::channel` (`App::attach_media_diff_loader`,
// the exact production wiring `main.rs` uses) so a request genuinely stays in flight between two
// calls, and use `test_support::count_media_diff_dispatch_calls` to observe how many worker
// threads were actually spawned — the only way to exercise `kick_media_diff`'s dedup-vs-coalesce
// branch and `apply_media_diff`'s "dispatch the coalesced want" step at all.

/// Two `poll_media_diff` calls for the identical `(path, page, raster_px)`, before anything has
/// landed, dispatch only **one** worker — `media_diff_pending`'s own dedup check in
/// `poll_media_diff`.
#[test]
fn poll_media_diff_dedups_two_identical_polls_into_one_dispatch() {
    let dir = unique_tmp("konoma_media_diff_dedup");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_diff_loader(tx);

    let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
        let _ = app.poll_media_diff(&png, 1, (400, 300));
        let _ = app.poll_media_diff(&png, 1, (400, 300));
    });
    assert_eq!(
        dispatches, 1,
        "同一 want を2回 poll しても dispatch は1回のはず"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("worker が結果を返す");
    assert!(app.apply_media_diff(res), "現世代の結果は適用されるはず");
}

/// A raw raster image (PNG) doesn't depend on `raster_px` at all (`normalize_raster_px`'s own
/// doc comment) — two polls for the same `(path, page)` but two different `raster_px` boxes
/// (standing in for a terminal resize) still dispatch only **one** worker, and both requested
/// boxes are answered by the single landed result once it arrives.
#[test]
fn two_polls_with_different_raster_px_for_a_png_dispatch_only_once() {
    let dir = unique_tmp("konoma_media_diff_raster_normalize");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_diff_loader(tx);

    let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
        let _ = app.poll_media_diff(&png, 1, (100, 100));
        let _ = app.poll_media_diff(&png, 1, (900, 700));
    });
    assert_eq!(
        dispatches, 1,
        "ラスタ画像は raster_px に依存しないので、異なる箱を求めても再 dispatch しないはず"
    );

    let res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("worker が結果を返す");
    assert!(app.apply_media_diff(res));
    assert!(
        app.poll_media_diff(&png, 1, (100, 100)).is_some(),
        "どちらの raster_px でも同じ(正規化された)着地を引けるはず"
    );
    assert!(app.poll_media_diff(&png, 1, (900, 700)).is_some());
}

/// A burst of (invalidate + poll) cycles while a worker is already busy — an AI rewriting the
/// same file repeatedly, or a rapid string of page turns — coalesces into the single
/// `media_diff_queued` slot instead of piling up concurrent decodes: no extra dispatch happens
/// until the busy worker's result lands, and then exactly **one** follow-up dispatch fires, for
/// the *newest* want only (earlier coalesced wants are overwritten, never accumulated). The busy
/// worker's own result, once it does land, is itself stale by then (three invalidations bumped
/// `media_diff_gen` out from under it) — `apply_media_diff` returns `false` for it, proving the
/// stale-gen discard still holds even while this coalescing machinery is what freed the slot.
#[cfg(feature = "git")]
#[test]
fn coalesces_a_burst_of_invalidate_and_poll_into_one_follow_up_dispatch_for_the_newest_want() {
    let Some(pdf) = sample_path_or_skip("sample.pdf") else {
        return;
    };
    let bytes = std::fs::read(&pdf).unwrap();
    let dir = unique_tmp("konoma_media_diff_coalesce");
    std::fs::create_dir_all(&dir).unwrap();
    init_git_repo(&dir);
    let doc = dir.join("doc.pdf");
    std::fs::write(&doc, &bytes).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-q", "-m", "init"]);
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_diff_loader(tx);

    let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
        // The first poll dispatches — the one worker slot is now busy.
        let _ = app.poll_media_diff(&doc, 1, (800, 600));
        // Three (invalidate + poll) bursts while that worker is still busy, each wanting a
        // different page — PDF pages, unlike a raster image, genuinely depend on the request
        // identity (`normalize_raster_px` leaves `Pdf`/`Svg` alone).
        for pg in [3u32, 4, 2] {
            app.invalidate_media_diff();
            let _ = app.poll_media_diff(&doc, pg, (800, 600));
        }
    });
    assert_eq!(
        dispatches, 1,
        "busy な間の invalidate+poll バーストは新規 dispatch を増やさないはず"
    );
    assert_eq!(
        app.media_diff_queued,
        Some((doc.clone(), 2, (800, 600))),
        "coalesce された want は最新のものだけ(上書き、蓄積ではない)のはず"
    );

    // The busy worker's own result lands — its generation was superseded by the three
    // invalidations above, so it's discarded...
    let stale_res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("busy だったワーカーが結果を返す");
    let (applied, dispatches) =
        crate::test_support::count_media_diff_dispatch_calls(|| app.apply_media_diff(stale_res));
    assert!(
        !applied,
        "3回 invalidate 済みなので gen が古く、この結果自体は捨てられるはず"
    );
    // ...but the slot is freed and the coalesced want (page 2) is dispatched immediately, in the
    // very same call.
    assert_eq!(
        dispatches, 1,
        "着地の瞬間に coalesce されていた want が1回だけ dispatch されるはず"
    );
    assert!(
        app.media_diff_queued.is_none(),
        "dispatch 後は queued スロットが空になるはず"
    );

    let res2 = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("coalesce された want のワーカーが結果を返す");
    assert!(app.apply_media_diff(res2), "最新世代の結果は適用されるはず");
    let outcome = app.poll_media_diff(&doc, 2, (800, 600));
    assert!(
        matches!(outcome, Some(MediaDiffOutcome::Ready { .. })),
        "最新の want(page 2) の結果が着地しているはず: {outcome:?}"
    );
}

/// `media_diff_landed_for` must reject an old-generation landing: switching from A to B and
/// back to A, with A having **changed on disk** while B was on screen, must re-kick a fresh
/// computation for A rather than silently serving the stale (pre-change) landed result — the
/// same `(path, page, raster_px)` key would otherwise still "match" if generation weren't part
/// of the check.
#[test]
fn switching_a_to_b_to_a_with_a_changed_on_disk_re_kicks_rather_than_serving_stale_a() {
    let dir = unique_tmp("konoma_media_diff_a_b_a_stale");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.png");
    let b = dir.join("b.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
        .save(&a)
        .unwrap();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
        .save(&b)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();

    let outcome_a1 = app.poll_media_diff(&a, 1, (400, 300)).unwrap();
    let key_a1 = match outcome_a1 {
        MediaDiffOutcome::Ready {
            new: MediaDiffSide::Picture(p),
            ..
        } => p.cache_key,
        other => panic!("A(1回目): Ready のはず: {other:?}"),
    };

    app.invalidate_media_diff();
    let _ = app.poll_media_diff(&b, 1, (400, 300)).unwrap();

    // A changes on disk while B is on screen (e.g. an AI rewriting the file).
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9])))
        .save(&a)
        .unwrap();

    app.invalidate_media_diff();
    let outcome_a2 = app.poll_media_diff(&a, 1, (400, 300)).unwrap();
    let (key_a2, natural_a2) = match outcome_a2 {
        MediaDiffOutcome::Ready {
            new: MediaDiffSide::Picture(p),
            ..
        } => (p.cache_key, p.natural_px),
        other => panic!("A(2回目): Ready のはず: {other:?}"),
    };
    assert_ne!(
        key_a1, key_a2,
        "内容が変わったのでキーも変わるはず(古い A の着地を使い回していない)"
    );
    assert_eq!(
        natural_a2,
        (8, 8),
        "変更後の A の実寸が反映されているはず(re-kick された証拠)"
    );
}

/// A stale landing (its `gen` superseded) with **nothing** coalesced behind it
/// (`media_diff_queued` empty at that moment) must still free the one worker slot —
/// otherwise `media_diff_worker_busy` is stuck `true` forever (no queued want to dispatch, and
/// no future worker is ever spawned to eventually call `apply_media_diff` again), and every
/// later `poll_media_diff` for anything at all just silently coalesces into `media_diff_queued`
/// without ever dispatching. `apply_media_diff`'s own doc comment says this is unconditional
/// ("Always frees the one worker slot first") — this pins it against a mutant that only frees
/// the slot inside the non-stale branch.
#[test]
fn a_stale_landing_with_nothing_queued_still_frees_the_worker_slot() {
    let dir = unique_tmp("konoma_media_diff_stale_no_queue_frees_slot");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.png");
    let b = dir.join("b.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
        .save(&a)
        .unwrap();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6, 6, image::Rgb([2, 2, 2])))
        .save(&b)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.attach_media_diff_loader(tx);

    // Dispatch for A — the one worker slot is now busy.
    let _ = app.poll_media_diff(&a, 1, (400, 300));
    // Invalidate (no further poll yet) — bumps gen, does NOT touch worker_busy/queued. The
    // in-flight A worker's eventual result is now stale, and nothing is queued behind it.
    app.invalidate_media_diff();
    assert!(
        app.media_diff_queued.is_none(),
        "前提: この時点で何も queue されていない"
    );

    // The busy (now-stale) worker's own result lands.
    let stale_res = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("A のワーカーが結果を返す");
    assert!(
        !app.apply_media_diff(stale_res),
        "gen が古いので適用されないはず"
    );

    // A brand new want (B) must actually dispatch a fresh worker now — the slot must have been
    // freed by the stale landing above, even though nothing was queued to piggyback on.
    let (_, dispatches) = crate::test_support::count_media_diff_dispatch_calls(|| {
        let _ = app.poll_media_diff(&b, 1, (400, 300));
    });
    assert_eq!(
        dispatches, 1,
        "stale landing (キューなし) の後は worker slot が解放され、新規 want は即 dispatch されるはず \
         (解放されないと診断が永久に止まる)"
    );
    let res_b = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("B のワーカーが結果を返す");
    assert!(
        app.apply_media_diff(res_b),
        "最新世代の B の結果は適用されるはず"
    );
}

/// Switching away from a tab showing a landed media diff to a non-media tab, and back, still shows
/// both pictures — the picture cache is pruned on the switch-away (see `App::prune_media_diff_
/// picture_cache`'s doc comment: any moment the active tab's view stops being a media diff frees the
/// pixels), but the switch-back correctly re-kicks a fresh computation rather than leaving a stale
/// placeholder.
#[cfg(feature = "git")]
#[test]
fn switching_to_another_tab_and_back_redraws_both_pictures() {
    let dir = unique_tmp("konoma_media_diff_tab_switch_redraw");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 1, 1])))
        .save(&png)
        .unwrap();
    let mut app = App::new(dir.to_path_buf(), Config::default()).unwrap();

    // Tab 0: open a media diff and let it land.
    app.open_git_diff(&png);
    let outcome = app
        .poll_media_diff(&png, app.diff_media_page(), (400, 300))
        .unwrap();
    let key = match outcome {
        MediaDiffOutcome::Ready {
            new: MediaDiffSide::Picture(p),
            ..
        } => p.cache_key,
        other => panic!("Ready のはず: {other:?}"),
    };
    assert!(
        app.md_image_cache_contains(&key),
        "前提: 着地して cache に乗る"
    );

    // Tab 1: a fresh Tree tab, then switch away and back — a genuine load_active round trip
    // against a non-media target and then back to the still-open media diff.
    app.tab_new().unwrap();
    app.tab_goto(0);
    app.tab_goto(1);
    assert!(
        !app.md_image_cache_contains(&key),
        "非 media タブへ切り替えたら picture は破棄されるはず(メモリは解放される)"
    );

    app.tab_goto(0);
    let outcome2 = app
        .poll_media_diff(&png, app.diff_media_page(), (400, 300))
        .unwrap();
    assert!(
        matches!(
            outcome2,
            MediaDiffOutcome::Ready {
                new: MediaDiffSide::Picture(_),
                ..
            }
        ),
        "タブへ戻ったら re-kick されて絵が再着地するはず: {outcome2:?}"
    );
}

/// A path that's become a **directory** (not a regular file) is treated the same as a deleted
/// new side, not as an existing-but-unreadable file — `new_exists` means "a regular file is
/// there", not merely "something answers `fs::metadata`". Classification for the new side falls
/// back to sniffing the old bytes (as it would for a genuinely deleted path), and the new side
/// itself lands `Absent`, not a spurious `Failed { "image decode failed" }`.
#[test]
fn a_directory_at_the_new_path_is_treated_as_absent_not_an_existing_unreadable_file() {
    let dir = unique_tmp("konoma_media_diff_new_side_is_directory");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("was_a_file.png");
    // The old side has real committed bytes (a valid PNG); the new "file" is actually a
    // directory now — e.g. a rename/rewrite race, or an agent replacing a file with a folder.
    let mut old_bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 3, image::Rgb([7, 7, 7])))
        .write_to(
            &mut std::io::Cursor::new(&mut old_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    std::fs::create_dir_all(&png).unwrap(); // `png` is now a directory, not a file.

    let r = MediaDiffRequest {
        gen: 1,
        path: png,
        root: dir.to_path_buf(),
        baseline: DiffBaseline::FollowSnapshot(old_bytes.clone()),
        page: 1,
        raster_px: (400, 300),
        preview_rules: Config::default().preview.rules,
        preview_commands: true,
    };
    match App::compute_media_diff(&r) {
        MediaDiffComputed::Ready { old, new, .. } => {
            assert!(
                matches!(old, MediaDiffSideDecoded::Picture(_)),
                "旧版は実バイト列からデコードされるはず: {old:?}"
            );
            assert!(
                matches!(new, MediaDiffSideDecoded::Absent),
                "新版はディレクトリなので Absent のはず(Failed ではない): {new:?}"
            );
        }
        other => panic!("旧版が PNG として分類されるので Ready のはず: {other:?}"),
    }
}
