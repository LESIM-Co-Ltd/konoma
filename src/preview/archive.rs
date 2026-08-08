//! Archive (zip / tar / tar.gz) listing preview: reads only entry **metadata** (name, size,
//! modified time). The listing is handed back as a `TableData` and rendered through the exact same
//! grid as CSV/TSV (`ui/table.rs`) — Name / Size / Modified — so cell navigation, in-preview
//! search, and cell/row/column copy all come for free.
//!
//! **What "metadata only" actually means per format (read this before touching this file).**
//! zip's central directory holds every entry's metadata up front, so `list_zip` never reads an
//! entry's compressed data stream at all — see the zip-specific bullet below. **tar has no central
//! directory**: metadata for entry N+1 only becomes readable after the file-format offset of entry
//! N's body has been passed, so listing a tar stream necessarily walks past (`Seek`s past, for a
//! plain `.tar`; otherwise reads-and-discards, for `.tar.gz`) every byte of every entry's content up
//! to wherever the listing stops — "never extracts/decompresses content" is true in the sense that
//! entry bodies are never handed to a decoder/written anywhere, but every tar-family byte up to that
//! point is still read (and, for `.tar.gz`, gzip-decompressed) off the underlying reader. The two
//! caps below (`MAX_ENTRIES` and, for `.tar.gz` only, `MAX_TAR_GZ_READ_BYTES`) exist specifically to
//! bound that walk.
//!
//! **Security boundary.** The historical `tar` crate advisories (RUSTSEC-2018-0002 / 2021-0080 /
//! 2026-0067 / 2026-0068) are all about `Archive::unpack`'s symlink / path-traversal handling. This
//! module:
//! - never calls `unpack`/`unpack_in`, or any zip API that reads an entry's *content*
//!   (`by_index_raw` returns a raw, non-decompressing reader; we only call `.name()`/`.size()`/
//!   `.is_dir()`/`.last_modified()` on it, never `.read()`);
//! - never joins an in-archive entry name onto a filesystem path — that join is the precondition
//!   for both Zip Slip and the tar advisories above. Entry names are opaque display strings that
//!   flow straight into `TableData` cells (the same table renderer already flattens/escapes
//!   embedded newlines and CJK-width-truncates arbitrary cell text, so a hostile name is just an
//!   ugly row, never a filesystem operation);
//! - never pre-allocates based on a declared/uncompressed size (`TableData` only ever stores the
//!   formatted string `human_size(declared_size)`, never a buffer of that size) — a listing can
//!   never itself amplify a declared size into a large in-memory buffer;
//! - caps the number of listed **entries** (`MAX_ENTRIES`, all three formats) and, for `.tar.gz`
//!   specifically, the total **decompressed bytes read** while walking to find them
//!   (`MAX_TAR_GZ_READ_BYTES` — see `list_tar_gz`'s doc comment for why tar's other two formats
//!   don't need a separate byte cap), so neither a central directory / tar stream that claims
//!   (truthfully or not) an enormous entry count, nor one whose early entries carry enormous bodies,
//!   can keep the caller (synchronous, on the UI thread — see `app/table_actions.rs::load_table`,
//!   which runs this on every tab switch back to an open archive tab) busy for an unbounded amount
//!   of time.
//!
//! **Dependency footprint.** zip's default features pull optional decompression codecs (bzip2
//! among them, which links a C library) that this module has no use for: `by_index_raw` returns
//! metadata via a raw reader regardless of the entry's compression method, so
//! `default-features = false` is sufficient (verified against Deflate-compressed fixtures — see
//! the tests below and the real `samples/sample.zip`, which a plain `zip` CLI also compresses with
//! Deflate). Likewise tar's `default-features = false` only drops its `xattr` default feature
//! (extended-attribute *restore*, an unpack-only concern); listing entries via `Archive::entries`/
//! `Archive::entries_with_seek` never touches it.
//!
//! Every parse failure returns `Err` (never panics — the call site in `app/table_actions.rs`
//! additionally wraps this in `catch_silent` as a second line of defense against a bug in the
//! `zip`/`tar` crates themselves), so the caller degrades to the `[can not preview: <ext>]`-style
//! hint rather than showing anything (design principle #3, "unsupported is handled safely").

use std::cell::Cell;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;
use std::rc::Rc;

use anyhow::{bail, Context, Result};

use crate::fileops::{format_epoch_utc, human_size};
use crate::preview::table::TableData;

/// Cap on the number of entries listed. Mirrors CSV's `MAX_ROWS` (same rationale: bound
/// memory/parse time on an arbitrarily large input and mark the table `truncated` so the UI can
/// say so) — it also happens to bound how long a crafted central directory / tar stream that
/// claims a huge entry count can keep us busy, which CSV rows don't need to worry about.
pub const MAX_ENTRIES: usize = 100_000;

/// Cap on how many **decompressed bytes** `list_tar_gz` will read while walking a `.tar.gz`'s
/// entries, independent of `MAX_ENTRIES`. A gzip stream isn't `Seek`, so — unlike a plain `.tar`
/// (`list_tar_seek`, which jumps straight to each header without reading the bytes in between) —
/// getting from entry N's header to entry N+1's means reading and gzip-decompressing every byte of
/// entry N's declared body via tar's own non-seek `skip()` (a 32KB-buffered read loop,
/// `tar-0.4.46/src/archive.rs:555`). Without this, a handful of entries with enormous declared
/// bodies could make a listing decompress an unbounded amount of data well before `MAX_ENTRIES`
/// entries had even been seen (measured: a 3.0MB tar.gz that decompresses to 1GB took 289ms to
/// list — and this runs synchronously on the UI thread on every tab switch back to an open archive
/// tab, per `app/table_actions.rs::load_table`).
///
/// 64 MiB, matching this codebase's existing precedent for the same kind of bound —
/// `preview::pdf::PAGE_COUNT_MAX_BYTES`, "how much may a synchronous, UI-thread-adjacent read
/// process before giving up and calling the rest unknown/truncated." At the throughput measured
/// above (1GB / 289ms ≈ 3.6 GB/s, admittedly on trivially-compressible filler), 64 MiB lands
/// around ~18ms; even at a far less favorable ~200 MB/s (real-world, less-compressible content),
/// it's still a hard ~320ms ceiling for a single archive-tab open, not the previous unbounded read.
/// Once crossed, listing stops and the table is marked `truncated` — same UX as hitting
/// `MAX_ENTRIES`, just triggered by bytes instead of row count.
const MAX_TAR_GZ_READ_BYTES: u64 = 64 * 1024 * 1024;

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
            // `File` is `Seek`, so a plain `.tar` walks its entries via `entries_with_seek` —
            // getting from one header to the next is a single `seek()` past the declared body
            // size, never a read of the body itself. See `list_tar_seek`'s doc comment.
            let f = File::open(path).with_context(|| format!("open: {}", path.display()))?;
            list_tar_seek(f)?
        }
        ArchiveKind::TarGz => {
            // A gzip stream can't `Seek`, so this instead bounds the walk by total bytes read.
            // See `list_tar_gz`'s doc comment.
            let f = File::open(path).with_context(|| format!("open: {}", path.display()))?;
            list_tar_gz(flate2::read::GzDecoder::new(f))?
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

/// Extracts the display fields konoma cares about from a tar entry's header — shared between
/// `list_tar_seek` and `list_tar_gz`, which differ only in how they get from one entry to the next,
/// never in what they read off an entry once they have it. Generic over `R` (this reads only
/// `entry.header()`/`entry.path_bytes()`, neither of which touch the entry's content reader, so it
/// works the same whether `R` is the seekable `File` `list_tar_seek` uses or the
/// budget-tracking wrapper `list_tar_gz` uses).
fn entry_from_tar<R: Read>(entry: &tar::Entry<'_, R>) -> Entry {
    // Raw bytes, lossily decoded — never a `Path`/filesystem join (see module docs). A tar entry
    // name is not guaranteed to be valid UTF-8; lossy decoding degrades gracefully (principle #3)
    // instead of erroring the whole listing over one odd entry.
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
    Entry {
        name,
        size,
        is_dir,
        modified,
    }
}

/// Lists a plain (uncompressed) `.tar`'s entries via `Archive::entries_with_seek` — requires
/// `R: Seek` (a plain `File`, which every real `.tar` on disk is opened as). Getting from entry N's
/// header to entry N+1's is then a single `seek()` past N's declared body size (tar's own
/// `Archive::skip`, `tar-0.4.46/src/archive.rs:555`, prefers `Seek` when the archive was opened
/// this way — see that function's `if let Some(seekable_archive) = ...` branch), never a read of
/// N's body: unlike `list_tar_gz` below, this needs no separate byte cap, since walking past even
/// an enormous entry costs the same as walking past a tiny one.
fn list_tar_seek<R: Read + Seek>(reader: R) -> Result<(Vec<Entry>, bool)> {
    let mut archive = tar::Archive::new(reader);
    let mut out = Vec::new();
    let mut truncated = false;
    for entry in archive.entries_with_seek().context("read tar entries")? {
        let entry = entry.context("read tar entry")?;
        if out.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        out.push(entry_from_tar(&entry));
    }
    Ok((out, truncated))
}

/// A `Read` wrapper that tallies bytes passed through it into a shared counter — the mechanism
/// `list_tar_gz` uses to bound its total decompressed-byte read (see `MAX_TAR_GZ_READ_BYTES`).
/// `Rc<Cell<_>>` rather than a plain field: the counter needs to be readable from `list_tar_gz`'s
/// loop while the reader itself is owned by the `tar::Archive` it was handed to. `list_tar_gz` runs
/// synchronously on whatever thread calls it (see `app/table_actions.rs::load_table`), so this
/// never needs to be `Send`/`Sync`.
struct CountingRead<R> {
    inner: R,
    total: Rc<Cell<u64>>,
}

impl<R: Read> Read for CountingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.total.set(self.total.get() + n as u64);
        Ok(n)
    }
}

/// Lists a `.tar.gz`'s entries via `Archive::entries` (tar's non-seek path — a gzip decompression
/// stream can't `Seek`). Getting from entry N's header to entry N+1's means reading and
/// gzip-decompressing every byte of N's declared body through tar's own 32KB-buffered `skip()`, so
/// this wraps the reader in [`CountingRead`] and checks the running total after every entry
/// retrieved: once it crosses [`MAX_TAR_GZ_READ_BYTES`], the listing stops and is marked
/// `truncated`, the same as crossing `MAX_ENTRIES`. The check happens *after* an entry's header is
/// already in hand (mirroring how the `MAX_ENTRIES` check above always has), so the walk may
/// overshoot the byte cap by up to one entry's worth of skip cost — the point is bounding the total
/// to a small multiple of the cap, not hitting it exactly.
fn list_tar_gz<R: Read>(reader: R) -> Result<(Vec<Entry>, bool)> {
    let total = Rc::new(Cell::new(0u64));
    let counted = CountingRead {
        inner: reader,
        total: Rc::clone(&total),
    };
    let mut archive = tar::Archive::new(counted);
    let mut out = Vec::new();
    let mut truncated = false;
    for entry in archive.entries().context("read tar entries")? {
        let entry = entry.context("read tar entry")?;
        if out.len() >= MAX_ENTRIES || total.get() >= MAX_TAR_GZ_READ_BYTES {
            truncated = true;
            break;
        }
        out.push(entry_from_tar(&entry));
    }
    Ok((out, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp;
    use std::io::{Cursor, SeekFrom, Write};

    /// A `Read`(+`Seek`) wrapper that counts how many `.read()` calls pass through it. Test-only —
    /// used solely by `seeking_past_a_tar_entrys_body_never_reads_it` below to make the
    /// seek-vs-buffered-read difference in `tar-0.4.46/src/archive.rs:555`'s `skip()` directly
    /// observable by call count, without timing.
    struct CountingReadSeek<R> {
        inner: R,
        reads: usize,
    }

    impl<R: Read> Read for CountingReadSeek<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            self.inner.read(buf)
        }
    }

    impl<R: Seek> Seek for CountingReadSeek<R> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    /// Root cause proof for the tar half of this module's tar/tar.gz split: a plain `.tar` is
    /// `Seek` (any real `.tar` on disk, opened as a `File`, is), so `list_tar_seek`
    /// (`Archive::entries_with_seek`) should get from one entry's header to the next with a single
    /// `seek()` — never reading the entry's body at all — while the non-seek path
    /// (`Archive::entries`, which `list_tar_gz` must fall back to, since a gzip decompression
    /// stream isn't `Seek`) has to `read()` its way through the body in 32KB-buffered chunks
    /// (`tar-0.4.46/src/archive.rs:555`'s `skip()`).
    ///
    /// Built directly against the `tar` crate's two entry-point APIs (not through this module's
    /// `list_tar_seek`/`list_tar_gz` wrappers), so the comparison isolates exactly the mechanism
    /// the tar/tar.gz split relies on — with a single large (5MB) entry followed by a small one,
    /// large enough that a 32KB-buffered skip needs on the order of 150 read calls, versus
    /// effectively none via seek. Both halves also assert the resulting entry listing is
    /// unchanged (the same content konoma's `entry_from_tar` reads off either path), so this
    /// doubles as the non-regression check for the seek switch.
    #[test]
    fn seeking_past_a_tar_entrys_body_never_reads_it() {
        let big = vec![0x5Au8; 5 * 1024 * 1024];
        let mut bytes = Vec::new();
        {
            let mut b = tar::Builder::new(&mut bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(big.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            b.append_data(&mut header, "big.bin", big.as_slice())
                .unwrap();
            let mut header2 = tar::Header::new_gnu();
            header2.set_size(3);
            header2.set_mode(0o644);
            header2.set_cksum();
            b.append_data(&mut header2, "small.bin", &b"abc"[..])
                .unwrap();
            b.finish().unwrap();
        }

        // Seek path (what `list_tar_seek` uses): entries_with_seek.
        let mut seek_reader = CountingReadSeek {
            inner: Cursor::new(bytes.clone()),
            reads: 0,
        };
        let names: Vec<String> = tar::Archive::new(&mut seek_reader)
            .entries_with_seek()
            .unwrap()
            .map(|e| String::from_utf8_lossy(&e.unwrap().path_bytes()).into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["big.bin", "small.bin"],
            "seek 経路でも一覧の内容は(順序含め)変わらない"
        );
        assert!(
            seek_reader.reads < 20,
            "entries_with_seek がヘッダ以外に大量の read を発行している(skip が seek でなく read になっている疑い): reads={}",
            seek_reader.reads
        );

        // Non-seek path (what `list_tar_gz` must use instead for a real, non-seekable gzip
        // stream): same bytes, fresh counter — the contrast that proves the assertion above is
        // actually measuring something, not just "any correct implementation has few reads."
        let mut buffered_reader = CountingReadSeek {
            inner: Cursor::new(bytes),
            reads: 0,
        };
        let names2: Vec<String> = tar::Archive::new(&mut buffered_reader)
            .entries()
            .unwrap()
            .map(|e| String::from_utf8_lossy(&e.unwrap().path_bytes()).into_owned())
            .collect();
        assert_eq!(
            names2,
            vec!["big.bin", "small.bin"],
            "非 seek 経路でも一覧の内容は(順序含め)変わらない(対照実験)"
        );
        assert!(
            buffered_reader.reads > 100,
            "非 seek 経路は 5MB のボディを 32KB ずつ読み飛ばすので read が100回を超えるはず\
             (対照になっていない=このテストが何も証明していない): reads={}",
            buffered_reader.reads
        );
    }

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = unique_tmp(&format!("konoma_archive_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Resolves a fixture bundled under the repo's `samples/` directory, anchored at
    /// `CARGO_MANIFEST_DIR` (baked in at compile time) rather than a bare relative path — a plain
    /// `Path::new("samples/…")` resolves against the test binary's **cwd**, which is only the
    /// crate root by convention (`cargo test` run from elsewhere, e.g. `cd /tmp && cargo test
    /// --manifest-path …`, is a real, supported invocation), so it silently missed the fixture and
    /// silently skipped every assertion in every test that used it. Tolerant of the one case where
    /// the fixture is legitimately absent — `samples/` is excluded from the published crate
    /// (`Cargo.toml`'s `exclude`) — by returning `None` (same early-return as before) but saying so
    /// loudly (`eprintln!`, visible with `--nocapture` or in the captured-output dump whenever the
    /// process later exits non-zero for any reason) instead of silently passing zero assertions.
    fn sample_path_or_skip(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

    /// Root cause: `.tar.gz` can't `Seek` (a gzip stream), so listing it walks `tar::Archive::
    /// entries()`'s **non-seek** skip path (`tar-0.4.46/src/archive.rs:555`'s `skip()`, a 32KB-
    /// buffered read loop over an entry's *entire* body) to get from one header to the next — and
    /// the only existing bound (`MAX_ENTRIES`) counts *rows*, not the bytes skipped to reach them,
    /// so a small compressed file whose entries carry large bodies could make a listing read an
    /// unbounded amount of decompressed data before ever hitting `MAX_ENTRIES` (measured: a 3.0MB
    /// tar.gz that decompresses to 1GB took 289ms to list — and `load_table` calls this
    /// synchronously on the UI thread on every tab switch back to an open archive tab).
    ///
    /// 40 entries of 5MB each (200MB decompressed, comfortably compressible filler so the fixture
    /// itself is fast to build) — any read-byte cap sized to keep this operation fast must truncate
    /// well before all 40 are listed. Deliberately asserted by **count**, not by wall-clock time (a
    /// prior CI break from a timing assertion — see `docs/STATUS.md`): `truncated` must be set, and
    /// strictly fewer than all 40 entries must have made it into the table.
    #[test]
    fn targz_read_is_bounded_by_bytes_not_just_entry_count() {
        let dir = tmp_dir("targz_byte_cap");
        let p = dir.join("big.tar.gz");
        let filler = vec![0x5Au8; 5 * 1024 * 1024]; // 5MB, highly compressible (repeated byte)
        let entries: Vec<(&str, &[u8])> = (0..40)
            .map(|i| -> (&str, &[u8]) {
                // `i` doesn't need to be part of the name for this test, so a fixed name is fine —
                // tar allows duplicate names (this is a listing, not a filesystem).
                let _ = i;
                ("f", filler.as_slice())
            })
            .collect();
        write_tar_gz(&p, &entries);

        let t = list(&p, ArchiveKind::TarGz).unwrap();
        assert!(
            t.truncated,
            "200MB 分の tar.gz を全部読み切ってしまっている(バイト上限が効いていない)"
        );
        assert!(
            t.rows.len() < entries.len(),
            "40件全部が一覧化されている(バイト上限が効いていない): rows={}",
            t.rows.len()
        );
        assert!(!t.rows.is_empty(), "上限が厳しすぎて1件も読めていない");
    }

    /// The repo's own bundled fixture (built by simply zipping a few other sample files with the
    /// system `zip` tool — see `samples/README.md`). Skipped when `samples/` is absent (excluded
    /// from the published crate; see `Cargo.toml`'s `exclude`), mirroring `preview/pdf.rs`'s tests.
    #[test]
    fn lists_the_bundled_sample_zip() {
        let Some(p) = sample_path_or_skip("sample.zip") else {
            return;
        };
        let t = list(&p, ArchiveKind::Zip).expect("bundled sample.zip should list cleanly");
        assert!(!t.rows.is_empty());
        assert!(t.rows.iter().any(|r| r[0].ends_with("hello.rs")));
    }

    /// See `lists_the_bundled_sample_zip`.
    #[test]
    fn lists_the_bundled_sample_tar_gz() {
        let Some(p) = sample_path_or_skip("sample.tar.gz") else {
            return;
        };
        let t = list(&p, ArchiveKind::TarGz).expect("bundled sample.tar.gz should list cleanly");
        assert!(!t.rows.is_empty());
        assert!(t.rows.iter().any(|r| r[0] == "sample.csv"));
    }
}
