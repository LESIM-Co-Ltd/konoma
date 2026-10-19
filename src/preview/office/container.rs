//! Safety boundary in front of the spreadsheet readers: size limits, container sniffing and
//! encryption detection.
//!
//! Everything here runs **before** any reader sees the file (`calamine`, which reads xlsb / xls /
//! ods, has no limits of its own), and the readers that open zip parts themselves (the xlsx
//! reader and the format passes, `fmt_*.rs`) read them only through [`part_reader`], so one place
//! decides what is "too large".
//!
//! **Declared zip sizes.** `zip` 9 enforces the `uncompressed_size` written in the central
//! directory while reading an entry: the `Crc32Reader` around the decompressor fails with "File is
//! larger than its declared uncompressed size" as soon as more bytes come out (the test
//! `zip_stops_at_the_declared_size` pins this against the real crate; zip 8.6 inflated such a
//! stream in full). A forged size therefore cannot hide a bomb: an entry that *declares* more than
//! the part limit is refused up front, and one that declares less but inflates past it is
//! `Corrupt`. [`scan_zip`] still inflates every entry once through [`count_real_bytes`] and counts
//! the bytes that actually come out, as a backstop that does not depend on the zip crate keeping
//! that behaviour (with zip 9 the count can only equal the declared size, so it is unreachable
//! from a file and is covered by a unit test on the counting function). The cost is one extra
//! inflate pass over the package (a worker thread, ~1 s per GiB); in exchange the readers only
//! ever see packages whose real expanded size is bounded.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use zip::result::ZipError;
use zip::ZipArchive;

use super::OfficeError;

/// Hard limits applied before and while reading a workbook. Every value has a reason; tests build
/// small `Limits` so the boundaries are cheap to hit.
///
/// Two kinds of limit, because the readers are of two kinds. **xlsx / xlsb are streamed**: the
/// per-sheet budgets (`max_rows`, `max_sheet_cells`, `max_sheet_text_bytes`) *stop the reading*, so
/// a sheet of any size shows its beginning, marked as capped. **ods / xls cannot be streamed**
/// (`calamine` builds a dense matrix of every sheet while opening the file), so their size is
/// checked *before* opening (`max_dense_cells`, `max_text_bytes`) and a file over it is refused.
/// Memory figures below are measured on a release build (see `docs/FEATURE-OFFICE-PREVIEW.md` §7-3a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest file we open. Beyond this "can not preview" is the honest answer: opening such a
    /// package already means decompressing gigabytes (`max_total_bytes`).
    pub max_file_bytes: u64,
    /// Most zip entries. Real packages hold tens (one XML per sheet, a few dozen parts); 10,000
    /// leaves room for workbooks with thousands of images/charts, and bounds the central
    /// directory a crafted file can make us index.
    pub max_entries: usize,
    /// Largest expanded size of one zip entry (counted while inflating, not as declared). One
    /// sheet XML of 256 MiB is already ~10M cells; the streaming readers never hold it, but a
    /// single `<v>` / `formatCode` can be as long as the part, so it bounds those too.
    pub max_part_bytes: u64,
    /// Largest total expanded size over all entries (a zip bomb with many medium parts).
    pub max_total_bytes: u64,
    /// Rows kept per sheet. Same cap as the CSV table (`table::MAX_ROWS`) so both table previews
    /// truncate alike.
    pub max_rows: usize,
    /// Columns kept per sheet: Excel's own maximum (XFD = 16,384).
    pub max_cols: usize,
    /// Cells kept per sheet (non-empty cells; **per sheet**, not per workbook: only the sheet on
    /// screen is held). A kept cell is 56 bytes (48 for the cell, 8 more for its column in the
    /// row) plus its strings (`max_sheet_text_bytes`), so 4M cells are ~225 MB of cells. The
    /// largest sheet that fits whole is 100,000 rows x 40 columns; a 10 x 100,000-row sheet of
    /// 20 columns (2M cells) is far inside it.
    pub max_sheet_cells: u64,
    /// Bytes of strings kept per sheet: text values, displayed (formatted) strings and formulas,
    /// each counted at its length plus a fixed overhead per string. Counted at the moment a cell
    /// is kept, so a shared string used by a million cells is counted a million times — which is
    /// what it costs once copied into each. 128 MiB on top of the cells' 225 MB keeps the
    /// ceiling for one sheet near 400 MB (+ the shared string table, below).
    pub max_sheet_text_bytes: u64,
    /// ods / xls only: largest `rows x columns` *bounding box* of a sheet's values that we let
    /// `calamine` build. It fills a dense matrix over the box (a 32-byte `Data` per cell) and
    /// parses through a per-cell list of its own (~40 bytes) and, with formulas, a second matrix
    /// of `String`s (24 bytes): about 100 bytes per cell at the peak. A sheet with values only at
    /// A1 and XFD1048576 would ask for ~550 GB, and an OOM abort is not a catchable panic, so the
    /// box is checked first. 8M cells is ~800 MB at the peak; xlsx/xlsb do not use this limit.
    pub max_dense_cells: u64,
    /// Bytes of text held *once the file is open*, which is read whole: for xlsx / xlsb the
    /// shared-string table (each string plus a fixed per-string overhead; a table over this is
    /// refused when the file is opened; konoma's xlsx reader keeps it in one buffer, `calamine`
    /// copies each string of an xlsb table); for ods / xls all text of every sheet (a shared string
    /// counted per cell that uses it, ods text times its repeat counts, plus the overhead).
    /// 256 MiB is about 1 GB of resident memory once copied, well above any real workbook (a
    /// 4M-cell sheet of 30-byte strings is 120 MB).
    pub max_text_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        Limits {
            max_file_bytes: 256 * MIB,
            max_entries: 10_000,
            max_part_bytes: 256 * MIB,
            max_total_bytes: 1024 * MIB,
            max_rows: crate::preview::table::MAX_ROWS,
            max_cols: 16_384,
            max_sheet_cells: 4_000_000,
            max_sheet_text_bytes: 128 * MIB,
            max_dense_cells: 8_000_000,
            max_text_bytes: 256 * MIB,
        }
    }
}

/// The container format actually found in the file (the extension is only a hint: a `.xlsx` that
/// holds an `.xls` is read as what it is).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detected {
    /// OOXML spreadsheet (`xl/workbook.xml`): xlsx, xlsm, xltx, xltm.
    Xlsx,
    /// Binary OOXML spreadsheet (`xl/workbook.bin`).
    Xlsb,
    /// OpenDocument spreadsheet (`mimetype` + `content.xml`).
    Ods,
    /// BIFF workbook inside a Compound File (`Workbook` / `Book` stream).
    Xls,
}

const ZIP_MAGIC: [u8; 2] = *b"PK";
const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Opens the file, enforces the file-size limit, identifies the container and (for zip) scans every
/// entry against the limits. Returns the detected spreadsheet format; password-protected OOXML
/// (a CFB holding `/EncryptedPackage`) is [`OfficeError::Encrypted`].
pub fn inspect(path: &Path, limits: &Limits) -> Result<Detected, OfficeError> {
    let mut f = File::open(path).map_err(io_err)?;
    let len = f.metadata().map_err(io_err)?.len();
    if len > limits.max_file_bytes {
        return Err(OfficeError::TooLarge { what: "file" });
    }
    let mut head = [0u8; 8];
    let n = read_up_to(&mut f, &mut head).map_err(io_err)?;
    if n == 0 {
        return Err(OfficeError::Corrupt("empty file".into()));
    }
    if n >= 2 && head[..2] == ZIP_MAGIC {
        let names = scan_zip(path, limits)?;
        return match detect_zip(&names) {
            // Not a spreadsheet *we can read*: but an ods whose content is encrypted is one.
            Err(OfficeError::Unsupported) if is_encrypted_ods(path, &names, limits) => {
                Err(OfficeError::Encrypted)
            }
            r => r,
        };
    }
    if n == 8 && head == CFB_MAGIC {
        return inspect_cfb(path);
    }
    Err(OfficeError::Corrupt("not a zip or compound file".into()))
}

fn detect_zip(names: &[String]) -> Result<Detected, OfficeError> {
    let has = |n: &str| names.iter().any(|x| x == n);
    if has("xl/workbook.xml") {
        Ok(Detected::Xlsx)
    } else if has("xl/workbook.bin") {
        Ok(Detected::Xlsb)
    } else if has("content.xml") && has("mimetype") {
        Ok(Detected::Ods)
    } else {
        // A valid zip that is something else (docx, pptx, jar, ...): not ours to read.
        Err(OfficeError::Unsupported)
    }
}

/// An OpenDocument spreadsheet whose `content.xml` is not there to read because the package is
/// encrypted: LibreOffice 25+ stores the whole package as one `encrypted-package` entry (AES-GCM),
/// and the manifest says so with `encryption-data`. The `mimetype` entry stays in the clear and
/// says what the document is. (Older packages keep `content.xml` with encrypted bytes and are
/// recognised by `fmt_ods::read`, which looks at the manifest.)
fn is_encrypted_ods(path: &Path, names: &[String], limits: &Limits) -> bool {
    const ODS_MIME: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
    odf_encrypted_as(path, names, limits, Some(ODS_MIME))
}

/// An OpenDocument package of any kind (text, spreadsheet, presentation ...) whose content is
/// encrypted: see [`is_encrypted_ods`] for the two ways LibreOffice stores it.
pub(crate) fn odf_encrypted(path: &Path, names: &[String], limits: &Limits) -> bool {
    odf_encrypted_as(path, names, limits, None)
}

/// [`odf_encrypted`], optionally only when the `mimetype` starts with `mime`.
fn odf_encrypted_as(
    path: &Path,
    names: &[String],
    limits: &Limits,
    mime_want: Option<&[u8]>,
) -> bool {
    let Ok(mut zip) = open_zip(path) else {
        return false;
    };
    let mime = match part_reader(&mut zip, "mimetype", 256) {
        Ok(Some(mut r)) => {
            let mut buf = Vec::new();
            if r.read_to_end(&mut buf).is_err() {
                return false;
            }
            buf
        }
        _ => return false,
    };
    let mime = mime.trim_ascii();
    match mime_want {
        Some(want) if !mime.starts_with(want) => return false,
        None if !mime.starts_with(b"application/vnd.oasis.opendocument.") => return false,
        _ => {}
    }
    if names.iter().any(|n| n == "encrypted-package") {
        return true;
    }
    let encrypted = match part_reader(&mut zip, "META-INF/manifest.xml", limits.max_part_bytes) {
        Ok(Some(r)) => super::fmt_ods::manifest_is_encrypted(BufReader::new(r)).unwrap_or(false),
        _ => false,
    };
    encrypted
}

/// The Word counterpart of [`inspect`]: opens the file, enforces the file-size limit, scans every
/// zip entry against the limits (really inflating each) and returns the entry names. A
/// password-protected OOXML wrapper (a CFB with `/EncryptedPackage`) is
/// [`OfficeError::Encrypted`]; any other CFB (a legacy `.doc`) and a zip that is not a package are
/// [`OfficeError::Unsupported`]. Whether the package holds a Word document is the caller's check
/// (the main part is named by `_rels/.rels`).
pub(crate) fn inspect_word_package(
    path: &Path,
    limits: &Limits,
) -> Result<Vec<String>, OfficeError> {
    let mut f = File::open(path).map_err(io_err)?;
    let len = f.metadata().map_err(io_err)?.len();
    if len > limits.max_file_bytes {
        return Err(OfficeError::TooLarge { what: "file" });
    }
    let mut head = [0u8; 8];
    let n = read_up_to(&mut f, &mut head).map_err(io_err)?;
    if n == 0 {
        return Err(OfficeError::Corrupt("empty file".into()));
    }
    if n >= 2 && head[..2] == ZIP_MAGIC {
        return scan_zip(path, limits);
    }
    if n == 8 && head == CFB_MAGIC {
        return match inspect_cfb(path) {
            Err(e) => Err(e),
            // A BIFF workbook, or a legacy document: not a Word package.
            Ok(_) => Err(OfficeError::Unsupported),
        };
    }
    Err(OfficeError::Corrupt("not a zip or compound file".into()))
}

fn inspect_cfb(path: &Path) -> Result<Detected, OfficeError> {
    let cf = cfb::open(path).map_err(|e| OfficeError::Corrupt(format!("compound file: {e}")))?;
    // Password-protected OOXML (any of xlsx/docx/pptx) is wrapped in a CFB with these streams.
    if cf.exists("/EncryptedPackage") || cf.exists("/EncryptionInfo") {
        return Err(OfficeError::Encrypted);
    }
    if cf.exists("/Workbook") || cf.exists("/Book") {
        Ok(Detected::Xls)
    } else {
        // doc / ppt / msg / ... — a CFB, but not a spreadsheet.
        Err(OfficeError::Unsupported)
    }
}

/// Opens the zip for the format passes.
pub fn open_zip(path: &Path) -> Result<ZipArchive<BufReader<File>>, OfficeError> {
    let f = File::open(path).map_err(io_err)?;
    ZipArchive::new(BufReader::new(f)).map_err(zip_err)
}

/// A reader over one zip part, capped at `limit` bytes of expanded data (a part that is longer is
/// truncated and the XML parser then reports it as corrupt). `Ok(None)` when the part does not
/// exist. This is the only way the format passes read zip content.
pub fn part_reader<'a>(
    zip: &'a mut ZipArchive<BufReader<File>>,
    name: &str,
    limit: u64,
) -> Result<Option<impl Read + 'a>, OfficeError> {
    match zip.by_name(name) {
        Ok(f) => Ok(Some(f.take(limit))),
        Err(ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(zip_err(e)),
    }
}

/// Checks every entry against the limits by *actually inflating it* and returns the entry names.
fn scan_zip(path: &Path, limits: &Limits) -> Result<Vec<String>, OfficeError> {
    let mut zip = open_zip(path)?;
    if zip.len() > limits.max_entries {
        return Err(OfficeError::TooLarge { what: "entries" });
    }
    let mut names = Vec::with_capacity(zip.len());
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        {
            let raw = zip.by_index_raw(i).map_err(zip_err)?;
            names.push(raw.name().map_or_else(
                |_| String::from_utf8_lossy(raw.name_raw()).into_owned(),
                |n| n.into_owned(),
            ));
            if raw.is_dir() {
                continue;
            }
            if raw.encrypted() {
                return Err(OfficeError::Encrypted);
            }
            // The declared size is an early exit (and what zip 9 itself enforces while reading);
            // the real count below is a backstop that does not rely on that.
            if raw.size() > limits.max_part_bytes {
                return Err(OfficeError::TooLarge { what: "entry" });
            }
        }
        let entry = zip.by_index(i).map_err(zip_err)?;
        let n = count_real_bytes(entry, limits.max_part_bytes)?;
        total = total.saturating_add(n);
        if total > limits.max_total_bytes {
            return Err(OfficeError::TooLarge { what: "package" });
        }
    }
    Ok(names)
}

/// Inflates `entry` into a sink and returns how many bytes it really holds, refusing more than
/// `max` (it never reads past `max + 1`). Errors from the decompressor are `Corrupt`.
pub(crate) fn count_real_bytes(entry: impl Read, max: u64) -> Result<u64, OfficeError> {
    let mut limited = entry.take(max.saturating_add(1));
    let n = io::copy(&mut limited, &mut io::sink())
        .map_err(|e| OfficeError::Corrupt(format!("zip entry: {e}")))?;
    if n > max {
        return Err(OfficeError::TooLarge { what: "entry" });
    }
    Ok(n)
}

pub(crate) fn zip_err(e: ZipError) -> OfficeError {
    match e {
        ZipError::InvalidPassword => OfficeError::Encrypted,
        ZipError::UnsupportedArchive(msg) if msg.to_ascii_lowercase().contains("password") => {
            OfficeError::Encrypted
        }
        ZipError::UnsupportedArchive(_) => OfficeError::Unsupported,
        // A truncated or damaged archive often surfaces as an I/O error from deep inside the reader.
        other => OfficeError::Corrupt(format!("zip: {other}")),
    }
}

/// File-system failures (open / stat / read of the file itself), as opposed to damaged content.
pub(crate) fn io_err(e: io::Error) -> OfficeError {
    OfficeError::Io(e.to_string())
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}
