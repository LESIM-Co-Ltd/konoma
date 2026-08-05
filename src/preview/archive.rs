//! Archive (zip / tar / tar.gz) listing preview: reads only entry **metadata** (name, size,
//! modified time) and never extracts/decompresses content. The listing is handed back as a
//! `TableData` and rendered through the exact same grid as CSV/TSV (`ui/table.rs`) — Name / Size /
//! Modified — so cell navigation, in-preview search, and cell/row/column copy all come for free.
//!
//! **Security boundary (read this before touching this file).** The historical `tar` crate
//! advisories (RUSTSEC-2018-0002 / 2021-0080 / 2026-0067 / 2026-0068) are all about
//! `Archive::unpack`'s symlink / path-traversal handling. This module:
//! - never calls `unpack`/`unpack_in`, or any zip API that reads an entry's *content*
//!   (`by_index_raw` returns a raw, non-decompressing reader; we only call `.name()`/`.size()`/
//!   `.is_dir()`/`.last_modified()` on it, never `.read()`);
//! - never joins an in-archive entry name onto a filesystem path — that join is the precondition
//!   for both Zip Slip and the tar advisories above. Entry names are opaque display strings that
//!   flow straight into `TableData` cells (the same table renderer already flattens/escapes
//!   embedded newlines and CJK-width-truncates arbitrary cell text, so a hostile name is just an
//!   ugly row, never a filesystem operation);
//! - never pre-allocates based on a declared/uncompressed size (`TableData` only ever stores the
//!   formatted string `human_size(declared_size)`, never a buffer of that size) — no zip-bomb
//!   amplification is possible because nothing is decompressed in the first place;
//! - caps the number of listed entries (`MAX_ENTRIES`), so a central directory / tar stream that
//!   claims (truthfully or not) an enormous entry count cannot keep the UI thread busy forever.
//!
//! **Dependency footprint.** zip's default features pull optional decompression codecs (bzip2
//! among them, which links a C library) that this module has no use for: `by_index_raw` returns
//! metadata via a raw reader regardless of the entry's compression method, so
//! `default-features = false` is sufficient (verified against Deflate-compressed fixtures — see
//! the tests below and the real `samples/sample.zip`, which a plain `zip` CLI also compresses with
//! Deflate). Likewise tar's `default-features = false` only drops its `xattr` default feature
//! (extended-attribute *restore*, an unpack-only concern); listing entries via `Archive::entries`
//! never touches it.
//!
//! Every parse failure returns `Err` (never panics — the call site in `app/table_actions.rs`
//! additionally wraps this in `catch_silent` as a second line of defense against a bug in the
//! `zip`/`tar` crates themselves), so the caller degrades to the `[can not preview: <ext>]`-style
//! hint rather than showing anything (design principle #3, "unsupported is handled safely").

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::fileops::{format_epoch_utc, human_size};
use crate::preview::table::TableData;

/// Cap on the number of entries listed. Mirrors CSV's `MAX_ROWS` (same rationale: bound
/// memory/parse time on an arbitrarily large input and mark the table `truncated` so the UI can
/// say so) — it also happens to bound how long a crafted central directory / tar stream that
/// claims a huge entry count can keep us busy, which CSV rows don't need to worry about.
pub const MAX_ENTRIES: usize = 100_000;

/// Which archive format `path` resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
}

impl ArchiveKind {
    /// Infer the format from `path`'s file name (case-insensitive). `.tar.gz` / `.tgz` are
    /// checked before the bare `.tar` suffix: `Path::extension()` on `foo.tar.gz` is just `"gz"`,
    /// so the compound extension needs a whole-name suffix check rather than `Path::extension()`.
    /// `None` for anything else (an unrecognized extension degrades to `CanNotPreview` rather than
    /// guessing a format — see `PreviewKind::from_rule`).
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(ArchiveKind::TarGz)
        } else if name.ends_with(".tar") {
            Some(ArchiveKind::Tar)
        } else if name.ends_with(".zip") {
            Some(ArchiveKind::Zip)
        } else {
            None
        }
    }
}

/// One listed entry, before flattening into `TableData`'s string cells.
struct Entry {
    name: String,
    size: u64,
    is_dir: bool,
    modified: String,
}

/// List `path`'s entries as a `TableData` (headers: Name / Size / Modified, one row per entry, in
/// the archive's own order — this is a listing, not a sort). See the module docs for the security
/// boundary this stays inside. Returns `Err` on any read/parse failure (missing file, wrong
/// format, corrupt archive, empty file) rather than panicking.
pub fn list(path: &Path, kind: ArchiveKind) -> Result<TableData> {
    // A literal empty file is never a valid archive of any of the three formats. zip already
    // errors on this naturally (no end-of-central-directory record to find); tar's reader is
    // permissive about a zero-byte stream (it reads as "zero entries" rather than erroring), so
    // this explicit check makes the three formats behave identically on this input.
    let len = std::fs::metadata(path)
        .with_context(|| format!("stat: {}", path.display()))?
        .len();
    if len == 0 {
        bail!("empty file: {}", path.display());
    }
    let (entries, truncated) = match kind {
        ArchiveKind::Zip => list_zip(path)?,
        ArchiveKind::Tar => {
            let f = File::open(path).with_context(|| format!("open: {}", path.display()))?;
            list_tar(f)?
        }
        ArchiveKind::TarGz => {
            let f = File::open(path).with_context(|| format!("open: {}", path.display()))?;
            list_tar(flate2::read::GzDecoder::new(f))?
        }
    };
    let rows: Vec<Vec<String>> = entries
        .into_iter()
        .map(|e| {
            vec![
                e.name,
                // Directories don't have a meaningful size of their own (mirrors the tree's "--"
                // for the same case in `fileops::detail_cell`).
                if e.is_dir {
                    "--".to_string()
                } else {
                    human_size(e.size)
                },
                e.modified,
            ]
        })
        .collect();
    Ok(TableData {
        headers: vec!["Name".into(), "Size".into(), "Modified".into()],
        rows,
        ncols: 3,
        truncated,
    })
}

fn list_zip(path: &Path) -> Result<(Vec<Entry>, bool)> {
    let f = File::open(path).with_context(|| format!("open: {}", path.display()))?;
    let mut archive = zip::ZipArchive::new(f).context("parse zip central directory")?;
    let total = archive.len();
    let n = total.min(MAX_ENTRIES);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // `by_index_raw`: metadata via a **non-decompressing** reader (see module docs) — content
        // is never read regardless of the entry's declared compression method, and we only call
        // metadata accessors on the result below, never `.read()`.
        let file = archive
            .by_index_raw(i)
            .with_context(|| format!("zip entry {i}"))?;
        out.push(Entry {
            name: file.name().to_string(),
            size: file.size(),
            is_dir: file.is_dir(),
            // zip's DOS-epoch timestamp has no timezone (ambiguous by spec — whatever the
            // archiver's local clock said), so this intentionally has no "UTC" suffix (unlike
            // tar's `format_epoch_utc` below, which is a true Unix epoch).
            modified: file
                .last_modified()
                .map(|dt| dt.to_string())
                .unwrap_or_default(),
        });
    }
    Ok((out, total > MAX_ENTRIES))
}

fn list_tar<R: Read>(reader: R) -> Result<(Vec<Entry>, bool)> {
    let mut archive = tar::Archive::new(reader);
    let mut out = Vec::new();
    let mut truncated = false;
    for entry in archive.entries().context("read tar entries")? {
        let entry = entry.context("read tar entry")?;
        if out.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        // Raw bytes, lossily decoded — never a `Path`/filesystem join (see module docs). A tar
        // entry name is not guaranteed to be valid UTF-8; lossy decoding degrades gracefully
        // (principle #3) instead of erroring the whole listing over one odd entry.
        let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let is_dir = entry.header().entry_type().is_dir();
        let size = entry.header().size().unwrap_or(0);
        let modified = entry
            .header()
            .mtime()
            .ok()
            .filter(|&t| t > 0)
            .map(format_epoch_utc)
            .unwrap_or_default();
        out.push(Entry {
            name,
            size,
            is_dir,
            modified,
        });
    }
    Ok((out, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use std::io::Write;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = unique_tmp(&format!("konoma_archive_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8], bool)]) {
        let f = File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        // Deflate needs no codec feature to *read* back (see module docs), but writing it back out
        // does need one; tests therefore write Stored (uncompressed) fixtures. Real-world zips
        // (created by any tool) are still exercised via `samples/sample.zip` below.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content, is_dir) in entries {
            if *is_dir {
                zw.add_directory(*name, opts).unwrap();
            } else {
                zw.start_file(*name, opts).unwrap();
                zw.write_all(content).unwrap();
            }
        }
        zw.finish().unwrap();
    }

    fn write_tar(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut b = tar::Builder::new(f);
        for (name, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(1_700_000_000);
            header.set_cksum();
            b.append_data(&mut header, *name, *content).unwrap();
        }
        b.finish().unwrap();
    }

    fn write_tar_gz(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut b = tar::Builder::new(gz);
        for (name, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(1_700_000_000);
            header.set_cksum();
            b.append_data(&mut header, *name, *content).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn from_path_recognizes_all_kinds_case_insensitively() {
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.zip")),
            Some(ArchiveKind::Zip)
        );
        assert_eq!(
            ArchiveKind::from_path(Path::new("A.ZIP")),
            Some(ArchiveKind::Zip)
        );
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.tar")),
            Some(ArchiveKind::Tar)
        );
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.TAR")),
            Some(ArchiveKind::Tar)
        );
        // Compound extension: Path::extension() only returns "gz", so we need a whole-name suffix check.
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.tar.gz")),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.TGZ")),
            Some(ArchiveKind::TarGz)
        );
        assert_eq!(ArchiveKind::from_path(Path::new("a.txt")), None);
        assert_eq!(ArchiveKind::from_path(Path::new("Makefile")), None);
    }

    #[test]
    fn lists_zip_entries_with_name_size_dir_and_order_preserved() {
        let dir = tmp_dir("zip_basic");
        let p = dir.join("t.zip");
        write_zip(
            &p,
            &[
                ("hello.txt", b"hello world!", false),
                ("dir/", b"", true),
                ("dir/nested.txt", b"nested", false),
            ],
        );
        let t = list(&p, ArchiveKind::Zip).unwrap();
        assert_eq!(t.headers, vec!["Name", "Size", "Modified"]);
        assert_eq!(t.ncols, 3);
        assert!(!t.truncated);
        assert_eq!(t.rows.len(), 3);
        // Kept in storage order (no reordering).
        assert_eq!(t.rows[0][0], "hello.txt");
        assert_eq!(t.rows[0][1], "12 B");
        assert_eq!(t.rows[1][0], "dir/");
        assert_eq!(
            t.rows[1][1], "--",
            "ディレクトリのサイズは -- (tree 詳細列と同流儀)"
        );
        assert_eq!(t.rows[2][0], "dir/nested.txt");
        assert_eq!(t.rows[2][1], "6 B");
        // Modified time is non-empty (even the 1980-01-01 default fills in something).
        assert!(!t.rows[0][2].is_empty());
    }

    #[test]
    fn lists_tar_entries() {
        let dir = tmp_dir("tar_basic");
        let p = dir.join("t.tar");
        write_tar(&p, &[("a.txt", b"hello"), ("b.txt", b"xy")]);
        let t = list(&p, ArchiveKind::Tar).unwrap();
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0][0], "a.txt");
        assert_eq!(t.rows[0][1], "5 B");
        assert!(
            t.rows[0][2].ends_with("UTC"),
            "tar の mtime は真の UTC epoch"
        );
        assert_eq!(t.rows[1][0], "b.txt");
        assert_eq!(t.rows[1][1], "2 B");
    }

    #[test]
    fn lists_tar_gz_entries() {
        let dir = tmp_dir("targz_basic");
        let p = dir.join("t.tar.gz");
        write_tar_gz(&p, &[("c.txt", b"z")]);
        let t = list(&p, ArchiveKind::TarGz).unwrap();
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0][0], "c.txt");
        assert_eq!(t.rows[0][1], "1 B");
    }

    #[test]
    fn empty_file_is_err_for_all_three_formats() {
        let dir = tmp_dir("empty");
        for (name, kind) in [
            ("e.zip", ArchiveKind::Zip),
            ("e.tar", ArchiveKind::Tar),
            ("e.tar.gz", ArchiveKind::TarGz),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, b"").unwrap();
            assert!(
                list(&p, kind).is_err(),
                "空ファイル({name})は Err(→ ヒント降格)、panic しない"
            );
        }
    }

    #[test]
    fn garbage_bytes_are_err_not_panic_for_all_three_formats() {
        // Not zero-length, but doesn't carry any format's signature (a stand-in for a corrupt file / mislabeled extension).
        let dir = tmp_dir("garbage");
        let junk = vec![0x41u8; 4096];
        for (name, kind) in [
            ("g.zip", ArchiveKind::Zip),
            ("g.tar", ArchiveKind::Tar),
            ("g.tar.gz", ArchiveKind::TarGz),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, &junk).unwrap();
            assert!(list(&p, kind).is_err(), "{name}: 壊れたファイルは Err");
        }
    }

    #[test]
    fn truncated_real_zip_is_err_not_panic() {
        let dir = tmp_dir("truncated");
        let full = dir.join("full.zip");
        write_zip(
            &full,
            &[("a.txt", b"hello world, this is some content", false)],
        );
        let bytes = std::fs::read(&full).unwrap();
        let cut = dir.join("cut.zip");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        assert!(list(&cut, ArchiveKind::Zip).is_err());
    }

    #[test]
    fn nonexistent_file_is_err_not_panic() {
        let p = Path::new("/no/such/archive.zip");
        assert!(list(p, ArchiveKind::Zip).is_err());
        assert!(list(Path::new("/no/such/archive.tar"), ArchiveKind::Tar).is_err());
        assert!(list(Path::new("/no/such/archive.tar.gz"), ArchiveKind::TarGz).is_err());
    }

    #[test]
    fn traversal_and_absolute_looking_names_are_display_strings_only() {
        // Even when a Zip-Slip-style name (../ or an absolute path) is present, konoma never joins
        // it onto a filesystem path (no extraction — listing only). The name flows straight through
        // as an opaque display string, and this test's whole point is that path is never touched.
        let dir = tmp_dir("evil");
        let p = dir.join("evil.zip");
        write_zip(
            &p,
            &[
                ("../../../tmp/konoma_zip_slip_probe", b"pwned?", false),
                ("/absolute/looking/evil", b"pwned2?", false),
            ],
        );
        let t = list(&p, ArchiveKind::Zip).unwrap();
        assert!(t
            .rows
            .iter()
            .any(|r| r[0] == "../../../tmp/konoma_zip_slip_probe"));
        assert!(t.rows.iter().any(|r| r[0] == "/absolute/looking/evil"));
        assert!(
            !Path::new("/absolute/looking/evil").exists(),
            "一覧化だけでファイルシステムへ書き込まれてはいけない"
        );
    }

    #[test]
    fn entry_count_is_capped_and_marks_truncated() {
        let dir = tmp_dir("cap");
        let p = dir.join("many.zip");
        let names: Vec<String> = (0..(MAX_ENTRIES + 10)).map(|i| format!("f{i}")).collect();
        let f = File::create(&p).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for n in &names {
            zw.start_file(n, opts).unwrap();
        }
        zw.finish().unwrap();

        let t = list(&p, ArchiveKind::Zip).unwrap();
        assert_eq!(t.rows.len(), MAX_ENTRIES, "MAX_ENTRIES で打ち切る");
        assert!(t.truncated, "打ち切ったら truncated=true");
    }

    /// The repo's own bundled fixture (built by simply zipping a few other sample files with the
    /// system `zip` tool — see `samples/README.md`). Skipped when `samples/` is absent (excluded
    /// from the published crate; see `Cargo.toml`'s `exclude`), mirroring `preview/pdf.rs`'s tests.
    #[test]
    fn lists_the_bundled_sample_zip() {
        let p = Path::new("samples/sample.zip");
        if !p.exists() {
            return;
        }
        let t = list(p, ArchiveKind::Zip).expect("bundled sample.zip should list cleanly");
        assert!(!t.rows.is_empty());
        assert!(t.rows.iter().any(|r| r[0].ends_with("hello.rs")));
    }

    /// See `lists_the_bundled_sample_zip`.
    #[test]
    fn lists_the_bundled_sample_tar_gz() {
        let p = Path::new("samples/sample.tar.gz");
        if !p.exists() {
            return;
        }
        let t = list(p, ArchiveKind::TarGz).expect("bundled sample.tar.gz should list cleanly");
        assert!(!t.rows.is_empty());
        assert!(t.rows.iter().any(|r| r[0] == "sample.csv"));
    }
}
