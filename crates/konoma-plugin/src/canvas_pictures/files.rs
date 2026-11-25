//! Where a plugin's picture file is, whether the read scope lets the host open it, and how it is
//! opened: a regular file only, decided on the handle that was opened.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use konoma_svg::image::ImageFailure;

use crate::view::ImageData;

/// The most bytes of a picture file the host reads for a plugin (`ImageData::File`).
pub const MAX_PICTURE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Where the picture file `p` of a plugin is: an absolute path is itself, a relative one is
/// relative to the directory of the file being previewed (`shown`), never to the directory
/// konoma happens to run in.
pub fn resolve_picture_path(shown: &Path, p: &Path) -> PathBuf {
    // The previewed file itself (the standard image plugin hands its own path back) is not
    // joined onto its own folder.
    if p.is_absolute() || p == shown {
        return p.to_path_buf();
    }
    shown
        .parent()
        .map_or_else(|| p.to_path_buf(), |d| d.join(p))
}

/// Open the picture file at `abs` for reading, refusing anything but a regular file **without
/// opening it first** where that can be told from the path (`metadata`: a FIFO, a device or a
/// directory is never opened, so opening cannot have a side effect such as raising DTR on a
/// serial line or waking a FIFO's writer), and then deciding again on the handle that was
/// opened (`fstat`: a path checked first and opened afterwards could be swapped in between). The
/// file is opened without blocking (`O_NONBLOCK`) and without becoming a controlling terminal
/// (`O_NOCTTY`). The one door every path that opens a plugin's (or the previewed) picture file
/// goes through.
pub fn open_regular_picture(abs: &Path) -> Result<std::fs::File, ImageFailure> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::metadata(abs) {
        Ok(m) if m.is_file() => {}
        _ => return Err(ImageFailure::Corrupt),
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(abs)
        .map_err(|_| ImageFailure::Corrupt)?;
    match file.metadata() {
        Ok(m) if m.is_file() => Ok(file),
        _ => Err(ImageFailure::Corrupt),
    }
}

/// [`open_regular_picture`] for the file `p` a plugin named, resolved against `shown`
/// ([`resolve_picture_path`]): the handle and the resolved path.
///
/// **The read scope** (`scope`, see [`crate::ReadScope`]): for a plugin that is not a
/// standard one the file must be inside the folders the host may read for it. It is judged on the
/// real path (symbolic links and `..` resolved), and **the real path is what is opened**, so the
/// file judged is the file read; the regular-file / no-blocking rules of
/// [`open_regular_picture`] still apply to it. The previewed file itself (`shown`, however it is
/// spelled) is always allowed: the user chose it, and the standard image plugin hands its own path
/// back. A file outside the scope is refused with [`ImageFailure::OutsideReadDirs`] and not opened.
pub fn open_picture_file(
    shown: &Path,
    p: &Path,
    scope: &crate::ReadScope,
) -> Result<(std::fs::File, PathBuf), ImageFailure> {
    let real = judge_picture_file(shown, p, scope)?;
    let file = open_regular_picture(&real)?;
    Ok((file, real))
}

/// The scope's verdict on the file `p` a plugin named (resolved against `shown`): the path to
/// open (the real path when the scope limits, the resolved spelling otherwise), or the refusal.
/// **Nothing is opened or read** -- this is the one place the verdict is made, for the opening
/// ([`open_picture_file`]) and for the budget ([`refused_by_scope`]) alike.
pub(super) fn judge_picture_file(
    shown: &Path,
    p: &Path,
    scope: &crate::ReadScope,
) -> Result<PathBuf, ImageFailure> {
    use crate::ScopeError;
    let abs = resolve_picture_path(shown, p);
    if !scope.is_restricted() {
        return Ok(abs);
    }
    let is_shown = abs == shown
        || matches!(
            (std::fs::canonicalize(&abs), std::fs::canonicalize(shown)),
            (Ok(a), Ok(b)) if a == b
        );
    if is_shown {
        return std::fs::canonicalize(&abs).map_err(|_| ImageFailure::Corrupt);
    }
    match scope.resolve(&abs) {
        Ok(real) => Ok(real),
        Err(ScopeError::Outside) => Err(ImageFailure::OutsideReadDirs),
        Err(_) => Err(ImageFailure::Corrupt),
    }
}

/// Whether the scope refuses `data` (a file outside it, or an animation with such a file among
/// its frames). Such a picture is refused **before** any budget is looked at: it will never be
/// opened, so it costs nothing -- not a decode, not input, not header pixels -- and cannot make
/// the in-scope pictures after it wait.
pub(super) fn refused_by_scope(data: &ImageData, shown: &Path, scope: &crate::ReadScope) -> bool {
    match data {
        ImageData::File(p) => matches!(
            judge_picture_file(shown, p, scope),
            Err(ImageFailure::OutsideReadDirs)
        ),
        ImageData::Frames(f) => f.iter().any(|(d, _)| refused_by_scope(d, shown, scope)),
        _ => false,
    }
}

/// Read the picture file `p` (resolved against `shown`, see [`resolve_picture_path`]) that a plugin
/// named, and say what it holds as data the decoders take: an SVG (by its first bytes) drawn
/// from the directory it is in, anything else as encoded bytes.
///
/// **Only a regular file is read, and it is decided on the file that was opened**, not on the path:
/// the file is opened without blocking (`O_NONBLOCK`: opening a FIFO that nobody writes to would
/// hold the worker for good, and a cancellation cannot reach it) and without becoming a
/// controlling terminal, and `fstat` of the opened handle must say "regular file" (a device such
/// as `/dev/tty` would read the user's keystrokes). A path checked first and opened afterwards
/// could be swapped for a FIFO in between. The bytes are read from the handle and decoded from
/// memory, so nothing opens the path a second time. Any file the user can read is read -- konoma
/// shows what the user could `cat` -- up to [`MAX_PICTURE_FILE_BYTES`].
pub(super) fn read_picture_file(
    shown: &Path,
    p: &Path,
    scope: &crate::ReadScope,
) -> Result<ImageData, ImageFailure> {
    use std::io::Read;
    let (file, abs) = open_picture_file(shown, p, scope)?;
    let meta = file.metadata().map_err(|_| ImageFailure::Corrupt)?;
    if meta.len() > MAX_PICTURE_FILE_BYTES {
        return Err(ImageFailure::TooLarge);
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_PICTURE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ImageFailure::Corrupt)?;
    if bytes.len() as u64 > MAX_PICTURE_FILE_BYTES {
        return Err(ImageFailure::TooLarge);
    }
    if konoma_svg::svg::can_begin_svg(&bytes[..bytes.len().min(512)]) {
        return Ok(ImageData::Svg {
            data: Arc::from(bytes),
            base_dir: Some(
                abs.parent()
                    .map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
            ),
            trusted: false,
        });
    }
    Ok(ImageData::Encoded(Arc::from(bytes)))
}
