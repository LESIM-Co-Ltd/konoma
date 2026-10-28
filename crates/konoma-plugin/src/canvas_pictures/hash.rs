//! The identity of a picture's content: a hash that follows the bytes or pixels (for a file: its
//! path, size, times and inode), computed once per distinct buffer and cancellable while a large
//! buffer is read, and the test for "the very same buffers" that skips hashing altogether.

use std::collections::HashMap;
use std::path::Path;

use super::files::resolve_picture_path;
use crate::view::ImageData;

/// FNV-1a 64 over several pieces (the standard offset basis and prime).
pub(super) struct Fnv(pub(super) u64);

impl Fnv {
    pub(super) fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    pub(super) fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub(super) fn num(&mut self, n: u64) {
        self.bytes(&n.to_le_bytes());
    }
}

/// The identity of a buffer a plugin handed over: where its bytes are and how many.
pub(super) type BufId = (usize, usize);

pub(super) fn buf_id(b: &[u8]) -> BufId {
    (b.as_ptr() as usize, b.len())
}

/// How many bytes are hashed between two looks at the cancellation.
pub(super) const HASH_CHUNK: usize = 1 << 20;

/// The hashing of one answer: each distinct buffer (one `Arc` cloned into many places, as an
/// animation of one frame repeated does) is hashed once, the cancellation is looked at while a
/// large buffer is hashed, and the bytes hashed are counted (the per-answer input budget is
/// applied before a picture is hashed).
pub(super) struct Hasher<'a> {
    /// The hash of the bytes of each buffer hashed in this answer.
    pub(super) memo: HashMap<BufId, u64>,
    /// The bytes of the distinct buffers hashed so far.
    pub(super) bytes: u64,
    cancelled: &'a dyn Fn() -> bool,
    /// The file being previewed: a relative picture path is relative to its directory.
    shown: Option<&'a Path>,
}

impl<'a> Hasher<'a> {
    pub(super) fn new(cancelled: &'a dyn Fn() -> bool) -> Hasher<'a> {
        Hasher {
            memo: HashMap::new(),
            bytes: 0,
            cancelled,
            shown: None,
        }
    }

    /// Resolve the relative picture files of a plugin against `shown`'s directory.
    pub(super) fn relative_to(mut self, shown: &'a Path) -> Hasher<'a> {
        self.shown = Some(shown);
        self
    }

    /// The hash of the bytes of `b`; `None` when the job was cancelled meanwhile.
    pub(super) fn buffer(&mut self, b: &[u8]) -> Option<u64> {
        let id = buf_id(b);
        if let Some(h) = self.memo.get(&id) {
            return Some(*h);
        }
        let mut f = Fnv::new();
        for chunk in b.chunks(HASH_CHUNK) {
            if (self.cancelled)() {
                return None;
            }
            f.bytes(chunk);
        }
        self.memo.insert(id, f.0);
        self.bytes += b.len() as u64;
        Some(f.0)
    }

    /// A hash of what the picture is: its bytes or pixels, or for a file its path, size and
    /// modification time (a file that changed under the same id is another picture). `None` when
    /// the job was cancelled meanwhile.
    pub(super) fn content(&mut self, data: &ImageData) -> Option<u64> {
        let mut h = Fnv::new();
        self.feed(&mut h, data)?;
        Some(h.0)
    }

    fn feed(&mut self, h: &mut Fnv, data: &ImageData) -> Option<()> {
        match data {
            ImageData::File(p) => {
                h.num(1);
                h.bytes(p.as_os_str().as_encoded_bytes());
                let abs = self
                    .shown
                    .map_or_else(|| p.to_path_buf(), |shown| resolve_picture_path(shown, p));
                if let Ok(m) = std::fs::metadata(abs) {
                    use std::os::unix::fs::MetadataExt;
                    h.num(m.len());
                    let t = m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos() as u64);
                    h.num(t);
                    // The same material as the stamp of the shown file (`FileStamp`): a file
                    // replaced with its size and modification time kept (`cp -p`, `rsync -t`, a
                    // rename) differs in inode, and one rewritten in place with the time put
                    // back differs in change time.
                    h.num(m.dev());
                    h.num(m.ino());
                    h.num(m.ctime() as u64);
                    h.num(m.ctime_nsec() as u64);
                }
            }
            ImageData::Encoded(b) => {
                h.num(2);
                h.num(self.buffer(b)?);
                h.num(b.len() as u64);
            }
            ImageData::Svg {
                data,
                base_dir,
                trusted,
            } => {
                h.num(3);
                h.num(self.buffer(data)?);
                h.num(data.len() as u64);
                h.num(*trusted as u64);
                if let Some(d) = base_dir {
                    h.bytes(d.as_os_str().as_encoded_bytes());
                }
            }
            ImageData::Rgba {
                width,
                height,
                pixels,
            } => {
                h.num(4);
                h.num(*width as u64);
                h.num(*height as u64);
                h.num(self.buffer(pixels)?);
                h.num(pixels.len() as u64);
            }
            ImageData::Frames(frames) => {
                h.num(5);
                for (f, d) in frames {
                    self.feed(h, f)?;
                    h.num(d.as_millis() as u64);
                }
            }
            ImageData::Lazy(_) => h.num(6),
            // A kind this host does not know (`ImageData` is non-exhaustive for other crates, so
            // the arm is dead here): hashed as one value; decoding refuses it.
            #[allow(unreachable_patterns)]
            _ => h.num(7),
        }
        Some(())
    }
}

/// A hash of what the picture is (see [`Hasher::content`]); every buffer is hashed in full.
#[cfg(any(test, feature = "test-hooks"))]
pub fn content_hash(data: &ImageData) -> u64 {
    Hasher::new(&never).content(data).unwrap_or_default()
}

/// Whether `a` and `b` are the very same buffers (the same allocation, so the same bytes): a
/// plugin that keeps one `Arc` and clones it into each answer is recognised without hashing. A
/// file is never "the same" (it may have changed on disk).
pub(super) fn same_source(a: &ImageData, b: &ImageData) -> bool {
    match (a, b) {
        (ImageData::Encoded(x), ImageData::Encoded(y)) => buf_id(x) == buf_id(y),
        (
            ImageData::Svg {
                data: x,
                base_dir: bx,
                trusted: tx,
            },
            ImageData::Svg {
                data: y,
                base_dir: by,
                trusted: ty,
            },
        ) => buf_id(x) == buf_id(y) && bx == by && tx == ty,
        (
            ImageData::Rgba {
                width: wx,
                height: hx,
                pixels: x,
            },
            ImageData::Rgba {
                width: wy,
                height: hy,
                pixels: y,
            },
        ) => wx == wy && hx == hy && buf_id(x) == buf_id(y),
        (ImageData::Frames(x), ImageData::Frames(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((fx, dx), (fy, dy))| dx == dy && same_source(fx, fy))
        }
        _ => false,
    }
}

/// Never cancelled.
#[cfg(any(test, feature = "test-hooks"))]
fn never() -> bool {
    false
}
