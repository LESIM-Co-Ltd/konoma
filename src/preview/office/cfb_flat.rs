//! Reads the workbook stream of an xls the way `calamine` 0.37 does, so that the format pass scans
//! exactly the bytes `calamine` will parse.
//!
//! `calamine`'s `Cfb::get_stream` takes the **first entry of the flat directory array** whose name
//! matches exactly (the 64-byte name up to its first NUL), whatever its type, whatever storage it
//! sits in, reachable from the tree or not. A reader that walks the tree (the `cfb` crate) can pick
//! another stream than `calamine` does, and then the memory checks vouch for bytes `calamine` never
//! reads. So this module reads the header, the FAT and the directory chain itself, and every
//! irregularity (a loop, a sector past the end, a stream shorter than declared in the mini FAT) is
//! a refusal rather than something to be lenient about.
//!
//! Spec: <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cfb/>

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use super::OfficeError;

const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const DIFSECT: u32 = 0xFFFF_FFFC;
const RESERVED_SECTORS: u32 = 0xFFFF_FFFA;
const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// The order in which `calamine` tries the stream names.
const NAMES: [&str; 4] = ["Workbook", "Book", "WORKBOOK", "BOOK"];

fn bad(what: &str) -> OfficeError {
    OfficeError::Corrupt(format!("compound file: {what}"))
}

fn u16_le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u32s(b: &[u8]) -> impl Iterator<Item = u32> + '_ {
    b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c))
}

struct Reader {
    file: File,
    file_len: u64,
    sector_size: u64,
    /// `calamine`'s bound on a sector id: the file length in sectors.
    max_id: u64,
    fats: Vec<u32>,
}

impl Reader {
    /// Sector `id` (a short last sector is returned short, as `calamine` does).
    fn sector(&self, id: u32) -> Result<Vec<u8>, OfficeError> {
        if u64::from(id) > self.max_id {
            return Err(bad("sector past the end of the file"));
        }
        // The header takes the first sector-size bytes, sector 0 follows it.
        let start = self.sector_size + u64::from(id) * self.sector_size;
        if start >= self.file_len {
            return Err(bad("sector past the end of the file"));
        }
        let n = (self.file_len - start).min(self.sector_size) as usize;
        let mut buf = vec![0u8; n];
        self.file
            .read_exact_at(&mut buf, start)
            .map_err(|e| bad(&e.to_string()))?;
        Ok(buf)
    }

    /// The bytes of the chain starting at `start` through `fat`, up to `len` of them. A chain that
    /// ends early returns what it has (as `calamine` does); a loop is an error.
    fn chain(&self, start: u32, fat: &[u32], len: u64) -> Result<Vec<u8>, OfficeError> {
        let mut out: Vec<u8> = Vec::new();
        let mut id = start;
        let mut steps = 0u64;
        while id != ENDOFCHAIN {
            steps += 1;
            if steps > self.max_id + 2 {
                return Err(bad("sector chain loops"));
            }
            let sector = self.sector(id)?;
            let remaining = len - out.len() as u64;
            let take = (sector.len() as u64).min(remaining) as usize;
            out.extend_from_slice(&sector[..take]);
            if out.len() as u64 >= len {
                break;
            }
            id = *fat
                .get(id as usize)
                .ok_or_else(|| bad("sector chain leaves the FAT"))?;
        }
        Ok(out)
    }
}

/// One entry of the directory array: name (up to its first NUL, as UTF-16 units), start sector and
/// declared length.
struct Entry {
    name: Vec<u16>,
    start: u32,
    len: u64,
}

fn entries(dir: &[u8], large_sectors: bool) -> Result<Vec<Entry>, OfficeError> {
    let (chunks, rest) = dir.as_chunks::<128>();
    if !rest.is_empty() {
        return Err(bad("directory is not a whole number of entries"));
    }
    Ok(chunks
        .iter()
        .map(|c| {
            let name: Vec<u16> = c[..64]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes(*p))
                .take_while(|&u| u != 0)
                .collect();
            let len = if large_sectors {
                u64::from(u32_le(c, 120)) | (u64::from(u32_le(c, 124)) << 32)
            } else {
                u64::from(u32_le(c, 120))
            };
            Entry {
                name,
                start: u32_le(c, 116),
                len,
            }
        })
        .collect())
}

/// The workbook stream of the xls at `path`, chosen and read like `calamine` 0.37 does. A stream
/// longer than `max_bytes` is [`OfficeError::TooLarge`].
pub(crate) fn read_workbook_stream(path: &Path, max_bytes: u64) -> Result<Vec<u8>, OfficeError> {
    let file = File::open(path).map_err(|e| bad(&e.to_string()))?;
    let file_len = file.metadata().map_err(|e| bad(&e.to_string()))?.len();
    let mut head = [0u8; 512];
    file.read_exact_at(&mut head, 0)
        .map_err(|_| bad("shorter than a header"))?;
    if head[..8] != SIGNATURE {
        return Err(bad("not a compound file"));
    }
    let sector_size: u64 = match u16_le(&head, 30) {
        0x0009 => 512,
        0x000C => 4096,
        _ => return Err(bad("unsupported sector size")),
    };
    if u16_le(&head, 32) != 0x0006 {
        return Err(bad("unsupported mini sector size"));
    }
    let dir_start = u32_le(&head, 48);
    let mini_fat_start = u32_le(&head, 60);
    let mini_fat_len = u64::from(u32_le(&head, 64));
    let difat_start = u32_le(&head, 68);

    let mut r = Reader {
        file,
        file_len,
        sector_size,
        max_id: file_len / sector_size,
        fats: Vec::new(),
    };

    // The DIFAT: 109 entries in the header, then a chain of sectors whose last entry links on.
    let mut difat: Vec<u32> = u32s(&head[76..512]).collect();
    let mut id = difat_start;
    let mut steps = 0u64;
    while id < RESERVED_SECTORS {
        steps += 1;
        if steps > r.max_id + 2 {
            return Err(bad("DIFAT chain loops"));
        }
        difat.extend(u32s(&r.sector(id)?));
        id = difat.pop().ok_or_else(|| bad("empty DIFAT sector"))?;
    }
    // The FAT sectors. A real file lists each once; a repeated one would multiply the memory.
    let mut seen_fat = std::collections::HashSet::new();
    let mut fats: Vec<u32> = Vec::new();
    for id in difat.into_iter().filter(|&id| id < DIFSECT) {
        if !seen_fat.insert(id) {
            return Err(bad("a FAT sector is listed twice"));
        }
        fats.extend(u32s(&r.sector(id)?));
    }
    r.fats = fats;

    let dir = r.chain(dir_start, &r.fats, u64::MAX)?;
    let dirs = entries(&dir, sector_size == 4096)?;
    drop(dir);
    // (`calamine` also refuses a version 4 file whose root has no stream; that is its own
    // refusal to make, after this pass has had its say.)
    if dirs.is_empty() {
        return Err(bad("empty root directory"));
    }

    for name in NAMES {
        let want: Vec<u16> = name.encode_utf16().collect();
        let Some(e) = dirs.iter().find(|d| d.name == want) else {
            continue;
        };
        if e.len > max_bytes {
            return Err(OfficeError::TooLarge { what: "entry" });
        }
        if e.len == 0 {
            return Ok(Vec::new());
        }
        if e.len < 4096 {
            // A mini stream: the root entry's stream cut into 64-byte pieces, chained by the mini FAT.
            if mini_fat_len == 0 {
                return Err(bad("mini stream without a mini FAT"));
            }
            let mini = r.chain(dirs[0].start, &r.fats, dirs[0].len)?;
            let mini_fat: Vec<u32> =
                u32s(&r.chain(mini_fat_start, &r.fats, mini_fat_len * sector_size)?).collect();
            let mut out: Vec<u8> = Vec::new();
            let mut id = e.start;
            let mut steps = 0u64;
            while id != ENDOFCHAIN {
                steps += 1;
                if steps > mini.len() as u64 / 64 + 2 {
                    return Err(bad("mini sector chain loops"));
                }
                let at = id as usize * 64;
                let piece = mini
                    .get(at..at + 64)
                    .ok_or_else(|| bad("mini sector past the mini stream"))?;
                let take = (e.len - out.len() as u64).min(64) as usize;
                out.extend_from_slice(&piece[..take]);
                if out.len() as u64 >= e.len {
                    break;
                }
                id = *mini_fat
                    .get(id as usize)
                    .ok_or_else(|| bad("mini sector chain leaves the mini FAT"))?;
            }
            return Ok(out);
        }
        return r.chain(e.start, &r.fats, e.len);
    }
    Err(OfficeError::Corrupt("missing Workbook stream".into()))
}
