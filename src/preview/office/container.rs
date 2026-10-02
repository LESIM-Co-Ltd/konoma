//! Safety boundary in front of the spreadsheet readers: size limits, container sniffing and
//! encryption detection.
//!
//! Everything here runs **before** `calamine` (which has no limits of its own) sees the file, and
//! the format passes (`fmt_*.rs`) read zip parts only through [`part_reader`], so one place decides
//! what is "too large".
//!
//! **Why declared zip sizes are not trusted.** `zip` 8.6 does *not* stop a deflate stream at the
//! `uncompressed_size` written in the central directory: `Decompressor::Deflated` is a bare
//! `flate2::bufread::DeflateDecoder` and the `Crc32Reader` around it only compares a checksum at
//! EOF. A forged zip that declares 100 bytes and inflates to gigabytes therefore decompresses in
//! full (the test `zip_does_not_stop_at_declared_size` below pins this against the real crate).
//! So [`scan_zip`] decompresses every entry once through `Read::take(limit + 1)` and counts the
//! bytes that actually come out. The cost is one extra inflate pass over the package (a worker
//! thread, ~1 s per GiB); in exchange `calamine` and the format passes only ever see packages
//! whose real expanded size is bounded.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use zip::result::ZipError;
use zip::ZipArchive;

use super::OfficeError;

/// Hard limits applied before and while reading a workbook. Every value has a reason; tests build
/// small `Limits` so the boundaries are cheap to hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest file we open. A real workbook of this size already costs a few GiB of RAM once a
    /// reader densifies it; beyond that "can not preview" is the honest answer.
    pub max_file_bytes: u64,
    /// Most zip entries. Real packages hold tens (one XML per sheet, a few dozen parts); 10,000
    /// leaves room for workbooks with thousands of images/charts, and bounds the central
    /// directory a crafted file can make us index.
    pub max_entries: usize,
    /// Largest expanded size of one zip entry (counted while inflating, not as declared). A single
    /// sheet XML of 256 MiB is already ~10M cells.
    pub max_part_bytes: u64,
    /// Largest total expanded size over all entries (a zip bomb with many medium parts).
    pub max_total_bytes: u64,
    /// Budget for cells kept in the grid (`rows x columns`) over the **whole workbook** (handed
    /// out to the sheets in order; rows beyond it are cut off). Measured cost of a loaded cell is
    /// about 105-115 bytes (the kept cell plus the reader's dense copy of the sheet being built),
    /// so 4M cells is roughly 450 MB worst case, plus the text budget below.
    pub max_grid_cells: u64,
    /// Rows kept per sheet. Same cap as the CSV table (`table::MAX_ROWS`) so both table previews
    /// truncate alike.
    pub max_rows: usize,
    /// Columns kept per sheet: Excel's own maximum (XFD = 16,384).
    pub max_cols: usize,
    /// Largest `rows x columns` *bounding box* of a sheet's values that we let `calamine` build.
    /// `calamine` fills a dense matrix over the bounding box (32 bytes per cell), so a sheet with
    /// values only at A1 and XFD1048576 would ask for ~550 GB and abort the process (an OOM abort
    /// is not a catchable panic). 16M cells is ~512 MB worst case.
    pub max_dense_cells: u64,
    /// Budget for text held in memory, counted per *reference*: a shared string used by a million
    /// cells is copied into each of them by the reader and again by us, so 10 KB x 50,000 cells
    /// is 500 MB from a 100 KB file. Counted before `calamine` runs, over the whole workbook:
    /// the shared-string table itself (each string plus a fixed per-string overhead), every cell
    /// that refers to a shared string (its length again), inline and formula strings, and ods
    /// text times its repeat counts. 256 MiB of text is about 1 GB peak once copied by the reader
    /// and by us, well above any real workbook (a 4M-cell sheet of 30-byte strings is 120 MB).
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
            max_grid_cells: 4_000_000,
            max_rows: crate::preview::table::MAX_ROWS,
            max_cols: 16_384,
            max_dense_cells: 16_000_000,
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
        return detect_zip(&names);
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
            names.push(raw.name().to_string());
            if raw.is_dir() {
                continue;
            }
            if raw.encrypted() {
                return Err(OfficeError::Encrypted);
            }
            // The declared size is only an early exit; the real count below is what is enforced.
            if raw.size() > limits.max_part_bytes {
                return Err(OfficeError::TooLarge { what: "entry" });
            }
        }
        let entry = zip.by_index(i).map_err(zip_err)?;
        let mut limited = entry.take(limits.max_part_bytes + 1);
        let n = io::copy(&mut limited, &mut io::sink())
            .map_err(|e| OfficeError::Corrupt(format!("zip entry: {e}")))?;
        if n > limits.max_part_bytes {
            return Err(OfficeError::TooLarge { what: "entry" });
        }
        total = total.saturating_add(n);
        if total > limits.max_total_bytes {
            return Err(OfficeError::TooLarge { what: "package" });
        }
    }
    Ok(names)
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
