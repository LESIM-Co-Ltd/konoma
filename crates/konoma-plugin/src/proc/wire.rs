//! The wire types and their conversions.
//!
//! The in-memory API types ([`View`], [`Input`], ...) carry no serde: they hold `Arc`s, trait
//! objects and private fields, and a derive on them would become a promise about the bytes. These
//! mirror types are the bytes. Each conversion is a pair:
//!
//! - `*_to_wire` runs on the sender and fails with a [`WireError`] for what cannot cross (a lazy
//!   picture, a canvas that fails its own check, a table over the cell cap);
//! - `*_from_wire` runs on the receiver and **assumes the sender is hostile**: it never panics,
//!   never allocates by a number it has not checked against the data it holds, and sends what it
//!   builds through the same checks an in-process plugin goes through ([`CanvasView::check`],
//!   [`Tr::clipped`], [`Layout::clipped`]).
//!
//! What does not survive the crossing (also in the crate docs of [`crate::proc`]):
//!
//! - `Arc` sharing: two `Arc`s of one buffer arrive as two buffers.
//! - [`Resource::Lazy`]: the child sees the resource as absent, and a lazy resource a child puts in
//!   a document is dropped. A [`ImageData::Lazy`] in a view is refused.
//! - A nested animation (`Frames` inside `Frames`) is refused.
//! - A table's per-cell `detail`, and a text view's styler (its lines are styled in the child).

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::input::{Input, Origin, Resource, Resources};
use crate::key::{Key, KeyCode, KeyMods};
use crate::manifest::{
    Act, ApiVersion, Bind, LaunchMode, Launcher, Manifest, Priority, SettingDecl, Target,
    Unavailable, Verb, VerbDecl, ViewKind,
};
use crate::scope::ReadScope;
use crate::session::{ColorDepth, Failure, FailureKind, Outcome, TermCaps};
use crate::settings::{Settings, Value};
use crate::style::{Color, Line, Modifier, Span, Style};
use crate::text::Tr;
use crate::view::{
    CanvasCell, CanvasImage, CanvasView, DiffHunk, DiffKind, DiffLine, DiffView, DocOptions,
    DocumentView, Grid, ImageData, ImageView, Layout, LineStyler, ListRow, ListView, MessageView,
    PageLayout, Reopen, ReprDecl, TableView, TextSource, TextView, View, ViewReq,
    CANVAS_MAX_FRAMES,
};

/// A byte string longer than this goes through a file in the handoff directory, not the frame.
pub const SIDE_THRESHOLD: usize = 1 << 20;
/// The largest side file either end writes or accepts. (A frame is 64 MiB; a picture of 8192 x 8192
/// pixels is 256 MiB.)
pub const SIDE_FILE_MAX: usize = 256 << 20;
/// The most side-file bytes one message from the host to the child may carry.
pub const SIDE_TOTAL_MAX: usize = 512 << 20;
/// The most side-file bytes one message from a child to the host may carry, and the most the host
/// takes. 256 MiB, the same as the canvas pictures the host keeps (`MAX_CANVAS_HELD_BYTES` and
/// `MAX_CANVAS_KEPT_BYTES`): the host holds a whole answer in memory at once, so it accepts no more
/// than it would keep.
pub const SIDE_TOTAL_MAX_FROM_CHILD: usize = 256 << 20;
/// The largest encoded size of a styled text (its lines with their styles) a child sends. Past
/// it the text is sent plain, so the answer stays well inside a frame.
pub const STYLED_MAX_ENCODED: usize = 40 << 20;
/// The cells (all columns of all rows) of a table a child sends. The same number as the Office
/// reader's per-sheet cell budget, so a sheet the reader keeps whole also crosses whole.
pub const TABLE_MAX_CELLS: usize = 4_000_000;
/// The columns of a table a child sends (Excel's own limit is 16384).
pub const TABLE_MAX_COLS: usize = 1 << 16;
/// The text a child styles itself (a `TextView` whose styler is run in the child). A longer file is
/// sent as the file, unstyled.
pub const STYLE_MAX_BYTES: u64 = 8 << 20;
/// The deepest settings value the sender builds.
pub const VALUE_MAX_DEPTH: usize = 32;
/// The most distinct side files and resources in one message (a bound on the loop, not a design
/// limit).
pub const MAX_RESOURCES: usize = 100_000;

/// Why a value could not be converted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    /// The value is not valid (a relative path, a broken canvas, a bad length).
    Bad(String),
    /// The value is over a size limit.
    TooLarge(String),
}

impl WireError {
    pub fn bad(m: impl Into<String>) -> WireError {
        WireError::Bad(m.into())
    }

    pub fn too_large(m: impl Into<String>) -> WireError {
        WireError::TooLarge(m.into())
    }

    pub fn message(&self) -> &str {
        match self {
            WireError::Bad(m) | WireError::TooLarge(m) => m,
        }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for WireError {}

impl From<WireError> for Failure {
    fn from(e: WireError) -> Failure {
        match &e {
            WireError::Bad(m) => Failure::corrupt(crate::t!(
                en: "the plugin sent data that is not valid",
                ja: "プラグインが不正なデータを返しました"
            ))
            .with_detail(m.clone()),
            WireError::TooLarge(m) => Failure::too_large(crate::t!(
                en: "the plugin sent more than the host accepts",
                ja: "プラグインの返したデータが大きすぎます"
            ))
            .with_detail(m.clone()),
        }
    }
}

impl From<std::io::Error> for WireError {
    fn from(e: std::io::Error) -> WireError {
        WireError::Bad(format!("side file: {e}"))
    }
}

// ---------------------------------------------------------------------------------------------
// Byte strings and side files
// ---------------------------------------------------------------------------------------------

/// Serde for a byte string: MessagePack `bin` (a plain `Vec<u8>` would be an array of integers).
mod bin {
    use serde::de::{self, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }

    struct Bytes;

    impl<'de> Visitor<'de> for Bytes {
        type Value = Vec<u8>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a byte string")
        }

        fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
            Ok(v.to_vec())
        }

        fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
            Ok(v)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut out = Vec::new();
            while let Some(b) = seq.next_element::<u8>()? {
                out.push(b);
            }
            Ok(out)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        d.deserialize_bytes(Bytes)
    }
}

/// A byte string on the wire: in the frame, or (over [`SIDE_THRESHOLD`]) in a file of the handoff
/// directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WBin {
    Inline(#[serde(with = "bin")] Vec<u8>),
    File { name: String, len: u64 },
}

/// A path on the wire: its bytes (a Unix path is bytes, not text).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WPath(#[serde(with = "bin")] Vec<u8>);

/// The handoff directory of one connection, and this end's side of it.
///
/// The host creates the directory (mode 0700, a fresh name) and tells the child in the handshake.
/// Each end names its files `<prefix>-<n>` (`h` host, `c` child) and creates them exclusively, so
/// the two ends never meet in a name. The receiver takes a file with [`Dec::bin`]: it checks the
/// name is one plain component, opens it without following a link, checks it is a regular file of
/// the announced length, reads it, then deletes it.
#[derive(Debug)]
pub struct Side {
    dir: PathBuf,
    prefix: char,
    threshold: usize,
    seq: AtomicU64,
}

impl Side {
    pub fn new(dir: impl Into<PathBuf>, prefix: char) -> Side {
        Side {
            dir: dir.into(),
            prefix,
            threshold: SIDE_THRESHOLD,
            seq: AtomicU64::new(0),
        }
    }

    /// Change the size over which a byte string goes through a file (tests use a small one).
    pub fn with_threshold(mut self, threshold: usize) -> Side {
        self.threshold = threshold;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn put(&self, bytes: &[u8]) -> Result<WBin, WireError> {
        if bytes.len() > SIDE_FILE_MAX {
            return Err(WireError::too_large(format!(
                "a byte string of {} bytes is over the {} byte limit",
                bytes.len(),
                SIDE_FILE_MAX
            )));
        }
        let name = format!(
            "{}-{}",
            self.prefix,
            self.seq.fetch_add(1, Ordering::Relaxed)
        );
        let path = self.dir.join(&name);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        if let Err(e) = f.write_all(bytes) {
            drop(f);
            let _ = std::fs::remove_file(&path);
            return Err(e.into());
        }
        Ok(WBin::File {
            name,
            len: bytes.len() as u64,
        })
    }

    /// Whether `name` is one plain file name (what a side file is called), so joining it to the
    /// directory cannot leave the directory.
    fn plain(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name != "."
            && name != ".."
            && !name.bytes().any(|b| b == b'/' || b == 0)
    }

    /// Delete a side file the receiver has not read (the message it came with failed). A name that
    /// is not a plain one is ignored: it names nothing in the directory.
    pub fn discard(&self, name: &str) {
        if Side::plain(name) {
            let _ = std::fs::remove_file(self.dir.join(name));
        }
    }

    /// Open a side file for reading, checked: a plain name, no link followed, a regular file of
    /// exactly the announced length. Returns the file and its path.
    fn open_checked(&self, name: &str, len: u64) -> Result<(std::fs::File, PathBuf), WireError> {
        if !Side::plain(name) {
            return Err(WireError::bad(format!(
                "side file name {name:?} is not a plain name"
            )));
        }
        if len > SIDE_FILE_MAX as u64 {
            return Err(WireError::too_large(format!(
                "side file of {len} bytes is over the limit"
            )));
        }
        let path = self.dir.join(name);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|e| WireError::bad(format!("side file {name:?}: {e}")))?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(WireError::bad(format!(
                "side file {name:?} is not a regular file"
            )));
        }
        if meta.len() != len {
            return Err(WireError::bad(format!(
                "side file {name:?} has {} bytes, announced {len}",
                meta.len()
            )));
        }
        Ok((file, path))
    }

    /// Read a side file into a buffer made at its size (the length was checked against the file
    /// itself, so the allocation is for bytes that exist), then delete it.
    fn take(&self, name: &str, len: u64) -> Result<Vec<u8>, WireError> {
        let (file, path) = self.open_checked(name, len)?;
        let mut out = Vec::with_capacity(len as usize);
        let read = file.take(len).read_to_end(&mut out);
        // Deleted once read, whether or not the rest of the message is good.
        let _ = std::fs::remove_file(&path);
        read?;
        if out.len() as u64 != len {
            return Err(WireError::bad(format!("side file {name:?} was cut short")));
        }
        Ok(out)
    }

    /// [`Side::take`] into an `Arc<[u8]>` directly (a `Vec` converted to an `Arc` is copied).
    fn take_arc(&self, name: &str, len: u64) -> Result<Arc<[u8]>, WireError> {
        let (file, path) = self.open_checked(name, len)?;
        // SAFETY: zeroed bytes are a valid `u8`, so the slice is initialised.
        let mut arc: Arc<[u8]> =
            unsafe { Arc::<[u8]>::new_zeroed_slice(len as usize).assume_init() };
        let read = match Arc::get_mut(&mut arc) {
            Some(buf) => file.take(len).read_exact(buf),
            None => Err(std::io::Error::other("a new Arc is shared")),
        };
        let _ = std::fs::remove_file(&path);
        read.map_err(|e| WireError::bad(format!("side file {name:?} was cut short: {e}")))?;
        Ok(arc)
    }
}

/// The sending state of one message.
pub struct Enc<'a> {
    side: &'a Side,
    sent: usize,
    total_max: usize,
    written: Vec<String>,
}

impl<'a> Enc<'a> {
    /// A message to the child (up to [`SIDE_TOTAL_MAX`] bytes of side files).
    pub fn new(side: &'a Side) -> Enc<'a> {
        Enc::with_total_max(side, SIDE_TOTAL_MAX)
    }

    /// A message with another side-file total (a child uses [`SIDE_TOTAL_MAX_FROM_CHILD`], what
    /// the host accepts).
    pub fn with_total_max(side: &'a Side, total_max: usize) -> Enc<'a> {
        Enc {
            side,
            sent: 0,
            total_max,
            written: Vec::new(),
        }
    }

    fn bin(&mut self, bytes: &[u8]) -> Result<WBin, WireError> {
        if bytes.len() <= self.side.threshold {
            return Ok(WBin::Inline(bytes.to_vec()));
        }
        self.sent = self.sent.saturating_add(bytes.len());
        if self.sent > self.total_max {
            return Err(WireError::too_large("one message carries too many bytes"));
        }
        let bin = self.side.put(bytes)?;
        if let WBin::File { name, .. } = &bin {
            self.written.push(name.clone());
        }
        Ok(bin)
    }

    /// The side files this message wrote (the caller deletes them if it does not send it).
    pub fn into_written(self) -> Vec<String> {
        self.written
    }

    /// Delete every side file this message wrote (it will not be sent).
    pub fn discard(&mut self) {
        for name in self.written.drain(..) {
            self.side.discard(&name);
        }
    }
}

/// The receiving state of one message.
pub struct Dec<'a> {
    side: &'a Side,
    taken: usize,
    total_max: usize,
}

impl<'a> Dec<'a> {
    /// A message from the host (up to [`SIDE_TOTAL_MAX`] bytes of side files).
    pub fn new(side: &'a Side) -> Dec<'a> {
        Dec::with_total_max(side, SIDE_TOTAL_MAX)
    }

    /// A message with another side-file total (the host uses [`SIDE_TOTAL_MAX_FROM_CHILD`]).
    pub fn with_total_max(side: &'a Side, total_max: usize) -> Dec<'a> {
        Dec {
            side,
            taken: 0,
            total_max,
        }
    }

    fn count(&mut self, len: u64) -> Result<(), WireError> {
        self.taken = self
            .taken
            .saturating_add(len.min(usize::MAX as u64) as usize);
        if self.taken > self.total_max {
            return Err(WireError::too_large("one message carries too many bytes"));
        }
        Ok(())
    }

    fn bin(&mut self, b: WBin) -> Result<Vec<u8>, WireError> {
        match b {
            WBin::Inline(v) => Ok(v),
            WBin::File { name, len } => {
                self.count(len)?;
                self.side.take(&name, len)
            }
        }
    }

    fn arc(&mut self, b: WBin) -> Result<Arc<[u8]>, WireError> {
        match b {
            WBin::Inline(v) => Ok(Arc::from(v)),
            WBin::File { name, len } => {
                self.count(len)?;
                self.side.take_arc(&name, len)
            }
        }
    }

    /// Delete the side files of a message that could not be converted (the ones already read are
    /// gone; the rest would stay until the connection ends).
    pub fn discard(&self, names: &[String]) {
        for n in names {
            self.side.discard(n);
        }
    }
}

pub fn path_to_wire(p: &Path) -> WPath {
    WPath(p.as_os_str().as_bytes().to_vec())
}

pub fn path_from_wire(p: WPath) -> Result<PathBuf, WireError> {
    if p.0.contains(&0) {
        return Err(WireError::bad("a path holds a NUL byte"));
    }
    if p.0.len() > 4096 * 4 {
        return Err(WireError::too_large("a path is too long"));
    }
    Ok(PathBuf::from(OsStr::from_bytes(&p.0)))
}

/// A path the receiver will read: it must be absolute, because a relative path would mean "relative
/// to the working directory of the other process", which differs between the two ends.
pub fn abs_path_from_wire(p: WPath, what: &str) -> Result<PathBuf, WireError> {
    let path = path_from_wire(p)?;
    if !path.is_absolute() {
        return Err(WireError::bad(format!(
            "{what} {} is not an absolute path",
            path.display()
        )));
    }
    Ok(path)
}

/// A path the host's own input carries to the child: made absolute against the host's working
/// directory, because the child's working directory is `/`.
fn path_made_absolute(p: &Path, what: &str) -> Result<WPath, WireError> {
    std::path::absolute(p)
        .map(|abs| path_to_wire(&abs))
        .map_err(|e| WireError::bad(format!("{what} {}: {e}", p.display())))
}

/// A path a child's view carries to the host: it must already be absolute. A relative path would
/// mean the child's working directory (`/`), which is not where the plugin author thinks it is,
/// so it is refused instead of silently pointing somewhere else.
fn path_required_absolute(p: &Path, what: &str) -> Result<WPath, WireError> {
    if !p.is_absolute() {
        return Err(WireError::bad(format!(
            "{what} {} is not an absolute path",
            p.display()
        )));
    }
    Ok(path_to_wire(p))
}

fn index_from_wire(n: u64, what: &str) -> Result<usize, WireError> {
    usize::try_from(n).map_err(|_| WireError::bad(format!("{what} {n} does not fit")))
}

// ---------------------------------------------------------------------------------------------
// Text, style, settings
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WTr {
    pub en: String,
    pub others: Vec<(String, String)>,
}

pub fn tr_to_wire(t: &Tr) -> WTr {
    WTr {
        en: t.en().to_string(),
        others: t
            .translations()
            .map(|(l, s)| (l.to_string(), s.to_string()))
            .collect(),
    }
}

pub fn tr_from_wire(t: WTr) -> Tr {
    let mut out = Tr::new(t.en);
    for (l, s) in t.others.into_iter().take(crate::text::MAX_TR_LANGUAGES + 1) {
        out = out.with(l, s);
    }
    out.clipped()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WColor {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

fn color_to_wire(c: Color) -> WColor {
    match c {
        Color::Reset => WColor::Reset,
        Color::Black => WColor::Black,
        Color::Red => WColor::Red,
        Color::Green => WColor::Green,
        Color::Yellow => WColor::Yellow,
        Color::Blue => WColor::Blue,
        Color::Magenta => WColor::Magenta,
        Color::Cyan => WColor::Cyan,
        Color::Gray => WColor::Gray,
        Color::DarkGray => WColor::DarkGray,
        Color::LightRed => WColor::LightRed,
        Color::LightGreen => WColor::LightGreen,
        Color::LightYellow => WColor::LightYellow,
        Color::LightBlue => WColor::LightBlue,
        Color::LightMagenta => WColor::LightMagenta,
        Color::LightCyan => WColor::LightCyan,
        Color::White => WColor::White,
        Color::Indexed(i) => WColor::Indexed(i),
        Color::Rgb(r, g, b) => WColor::Rgb(r, g, b),
    }
}

fn color_from_wire(c: WColor) -> Color {
    match c {
        WColor::Reset => Color::Reset,
        WColor::Black => Color::Black,
        WColor::Red => Color::Red,
        WColor::Green => Color::Green,
        WColor::Yellow => Color::Yellow,
        WColor::Blue => Color::Blue,
        WColor::Magenta => Color::Magenta,
        WColor::Cyan => Color::Cyan,
        WColor::Gray => Color::Gray,
        WColor::DarkGray => Color::DarkGray,
        WColor::LightRed => Color::LightRed,
        WColor::LightGreen => Color::LightGreen,
        WColor::LightYellow => Color::LightYellow,
        WColor::LightBlue => Color::LightBlue,
        WColor::LightMagenta => Color::LightMagenta,
        WColor::LightCyan => Color::LightCyan,
        WColor::White => Color::White,
        WColor::Indexed(i) => Color::Indexed(i),
        WColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WStyle {
    pub fg: Option<WColor>,
    pub bg: Option<WColor>,
    pub add: u16,
    pub sub: u16,
}

pub fn style_to_wire(s: &Style) -> WStyle {
    WStyle {
        fg: s.fg.map(color_to_wire),
        bg: s.bg.map(color_to_wire),
        add: s.add.bits(),
        sub: s.sub.bits(),
    }
}

pub fn style_from_wire(s: WStyle) -> Style {
    Style::from_parts(
        s.fg.map(color_from_wire),
        s.bg.map(color_from_wire),
        Modifier::from_bits_truncate(s.add),
        Modifier::from_bits_truncate(s.sub),
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WLine {
    pub spans: Vec<(String, WStyle)>,
    pub style: WStyle,
}

pub fn line_to_wire(l: &Line) -> WLine {
    WLine {
        spans: l
            .spans
            .iter()
            .map(|s| (s.text.clone(), style_to_wire(&s.style)))
            .collect(),
        style: style_to_wire(&l.style),
    }
}

pub fn line_from_wire(l: WLine) -> Line {
    Line {
        spans: l
            .spans
            .into_iter()
            .map(|(text, st)| Span::styled(text, style_from_wire(st)))
            .collect(),
        style: style_from_wire(l.style),
    }
}

fn lines_to_wire(ls: &[Line]) -> Vec<WLine> {
    ls.iter().map(line_to_wire).collect()
}

fn lines_from_wire(ls: Vec<WLine>) -> Vec<Line> {
    ls.into_iter().map(line_from_wire).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<WValue>),
    Table(Vec<(String, WValue)>),
}

pub fn value_to_wire(v: &Value, depth: usize) -> Result<WValue, WireError> {
    if depth > VALUE_MAX_DEPTH {
        return Err(WireError::too_large("a settings value nests too deeply"));
    }
    Ok(match v {
        Value::Bool(b) => WValue::Bool(*b),
        Value::Int(i) => WValue::Int(*i),
        Value::Float(f) => WValue::Float(*f),
        Value::Str(s) => WValue::Str(s.clone()),
        Value::List(l) => WValue::List(
            l.iter()
                .map(|x| value_to_wire(x, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Table(t) => WValue::Table(
            t.iter()
                .map(|(k, x)| Ok((k.clone(), value_to_wire(x, depth + 1)?)))
                .collect::<Result<_, WireError>>()?,
        ),
    })
}

pub fn value_from_wire(v: WValue, depth: usize) -> Result<Value, WireError> {
    if depth > VALUE_MAX_DEPTH {
        return Err(WireError::too_large("a settings value nests too deeply"));
    }
    Ok(match v {
        WValue::Bool(b) => Value::Bool(b),
        WValue::Int(i) => Value::Int(i),
        WValue::Float(f) => Value::Float(f),
        WValue::Str(s) => Value::Str(s),
        WValue::List(l) => Value::List(
            l.into_iter()
                .map(|x| value_from_wire(x, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        WValue::Table(t) => {
            let mut m = BTreeMap::new();
            for (k, x) in t {
                m.insert(k, value_from_wire(x, depth + 1)?);
            }
            Value::Table(m)
        }
    })
}

// ---------------------------------------------------------------------------------------------
// The context of a call
// ---------------------------------------------------------------------------------------------

/// The parts of `OpenCx` / `ViewCx` / `ActCx` that cross (the cancel flag does not: it is the
/// request id and a `Cancel` message).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WCx {
    pub settings: Vec<(String, WValue)>,
    pub lang: String,
    pub pixels: bool,
    pub colors: u8,
    pub external: bool,
    /// `None` is unrestricted.
    pub read_dirs: Option<Vec<WPath>>,
}

/// An owned context, decoded; the child borrows from it to build the `*Cx`.
#[derive(Clone, Debug)]
pub struct CxData {
    pub settings: Settings,
    pub lang: String,
    pub term: TermCaps,
    pub external: bool,
    pub read_scope: ReadScope,
}

/// What the host puts in a call's context.
#[derive(Clone, Debug)]
pub struct CxSource<'a> {
    pub settings: &'a Settings,
    pub lang: &'a str,
    pub term: TermCaps,
    pub external: bool,
    pub read_scope: &'a ReadScope,
}

pub fn cx_to_wire(cx: &CxSource<'_>) -> Result<WCx, WireError> {
    let mut settings = Vec::new();
    for (k, v) in cx.settings.iter() {
        settings.push((k.clone(), value_to_wire(v, 0)?));
    }
    Ok(WCx {
        settings,
        lang: cx.lang.to_string(),
        pixels: cx.term.pixels,
        colors: match cx.term.colors {
            ColorDepth::Ansi16 => 0,
            ColorDepth::Ansi256 => 1,
            ColorDepth::TrueColor => 2,
            #[allow(unreachable_patterns)]
            _ => 0,
        },
        external: cx.external,
        read_dirs: cx
            .read_scope
            .dirs()
            .map(|d| d.iter().map(|p| path_to_wire(p)).collect()),
    })
}

pub fn cx_from_wire(c: WCx) -> Result<CxData, WireError> {
    let mut settings = Settings::new();
    for (k, v) in c.settings {
        settings.set(k, value_from_wire(v, 0)?);
    }
    let read_scope = match c.read_dirs {
        None => ReadScope::unrestricted(),
        Some(dirs) => {
            let mut paths = Vec::new();
            for d in dirs {
                paths.push(abs_path_from_wire(d, "a read scope directory")?);
            }
            ReadScope::within(paths)
        }
    };
    Ok(CxData {
        settings,
        lang: c.lang,
        term: TermCaps::new().pixels(c.pixels).colors(match c.colors {
            2 => ColorDepth::TrueColor,
            1 => ColorDepth::Ansi256,
            _ => ColorDepth::Ansi16,
        }),
        external: c.external,
        read_scope,
    })
}

// ---------------------------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WOrigin {
    File(WPath),
    Converted { from: WPath },
}

/// `from_host`: the origin is the host's own input (made absolute); otherwise it is a child's
/// view (must be absolute).
fn origin_to_wire(o: &Origin, from_host: bool) -> Result<WOrigin, WireError> {
    let conv = |p: &Path| {
        if from_host {
            path_made_absolute(p, "origin")
        } else {
            path_required_absolute(p, "origin")
        }
    };
    Ok(match o {
        Origin::File(p) => WOrigin::File(conv(p)?),
        Origin::Converted { from } => WOrigin::Converted { from: conv(from)? },
    })
}

fn origin_from_wire(o: WOrigin) -> Result<Origin, WireError> {
    Ok(match o {
        WOrigin::File(p) => Origin::File(abs_path_from_wire(p, "origin")?),
        WOrigin::Converted { from } => Origin::Converted {
            from: abs_path_from_wire(from, "origin")?,
        },
    })
}

/// A named byte resource. A lazy resource has no bytes to send and is left out.
fn resources_to_wire(r: &Resources, enc: &mut Enc<'_>) -> Result<Vec<(String, WBin)>, WireError> {
    let mut out = Vec::new();
    for (name, res) in r.iter() {
        if let Resource::Bytes(b) = res {
            out.push((name.clone(), enc.bin(b)?));
        }
    }
    Ok(out)
}

fn resources_from_wire(r: Vec<(String, WBin)>, dec: &mut Dec<'_>) -> Result<Resources, WireError> {
    if r.len() > MAX_RESOURCES {
        return Err(WireError::too_large("too many resources"));
    }
    let mut out = Resources::new();
    for (name, b) in r {
        out.insert(name, Resource::Bytes(dec.arc(b)?));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WInput {
    File {
        path: WPath,
        hint: Option<String>,
    },
    Content {
        content_type: String,
        text: String,
        resources: Vec<(String, WBin)>,
        origin: WOrigin,
    },
    Embed {
        kind: String,
        source: String,
        display: bool,
    },
}

/// A file input's path is made absolute (against the host's working directory) here, because the
/// child's working directory is `/`.
pub fn input_to_wire(i: &Input, enc: &mut Enc<'_>) -> Result<WInput, WireError> {
    Ok(match i {
        Input::File { path, hint } => WInput::File {
            path: path_made_absolute(path, "path")?,
            hint: hint.clone(),
        },
        Input::Content {
            content_type,
            text,
            resources,
            origin,
        } => WInput::Content {
            content_type: content_type.clone(),
            text: text.clone(),
            resources: resources_to_wire(resources, enc)?,
            origin: origin_to_wire(origin, true)?,
        },
        Input::Embed {
            kind,
            source,
            display,
        } => WInput::Embed {
            kind: kind.clone(),
            source: source.clone(),
            display: *display,
        },
    })
}

pub fn input_from_wire(i: WInput, dec: &mut Dec<'_>) -> Result<Input, WireError> {
    Ok(match i {
        WInput::File { path, hint } => Input::File {
            path: abs_path_from_wire(path, "input file")?,
            hint,
        },
        WInput::Content {
            content_type,
            text,
            resources,
            origin,
        } => Input::Content {
            content_type,
            text,
            resources: resources_from_wire(resources, dec)?,
            origin: origin_from_wire(origin)?,
        },
        WInput::Embed {
            kind,
            source,
            display,
        } => Input::Embed {
            kind,
            source,
            display,
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Key, request, outcome, failure, layout
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WKeyCode {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Esc,
    F(u8),
    Paste(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WKey {
    pub code: WKeyCode,
    pub mods: u8,
}

pub fn key_to_wire(k: &Key) -> WKey {
    WKey {
        code: match &k.code {
            KeyCode::Char(c) => WKeyCode::Char(*c),
            KeyCode::Enter => WKeyCode::Enter,
            KeyCode::Backspace => WKeyCode::Backspace,
            KeyCode::Delete => WKeyCode::Delete,
            KeyCode::Tab => WKeyCode::Tab,
            KeyCode::BackTab => WKeyCode::BackTab,
            KeyCode::Left => WKeyCode::Left,
            KeyCode::Right => WKeyCode::Right,
            KeyCode::Up => WKeyCode::Up,
            KeyCode::Down => WKeyCode::Down,
            KeyCode::Home => WKeyCode::Home,
            KeyCode::End => WKeyCode::End,
            KeyCode::PageUp => WKeyCode::PageUp,
            KeyCode::PageDown => WKeyCode::PageDown,
            KeyCode::Esc => WKeyCode::Esc,
            KeyCode::F(n) => WKeyCode::F(*n),
            KeyCode::Paste(s) => WKeyCode::Paste(s.clone()),
        },
        mods: k.mods.bits(),
    }
}

pub fn key_from_wire(k: WKey) -> Key {
    let code = match k.code {
        WKeyCode::Char(c) => KeyCode::Char(c),
        WKeyCode::Enter => KeyCode::Enter,
        WKeyCode::Backspace => KeyCode::Backspace,
        WKeyCode::Delete => KeyCode::Delete,
        WKeyCode::Tab => KeyCode::Tab,
        WKeyCode::BackTab => KeyCode::BackTab,
        WKeyCode::Left => KeyCode::Left,
        WKeyCode::Right => KeyCode::Right,
        WKeyCode::Up => KeyCode::Up,
        WKeyCode::Down => KeyCode::Down,
        WKeyCode::Home => KeyCode::Home,
        WKeyCode::End => KeyCode::End,
        WKeyCode::PageUp => KeyCode::PageUp,
        WKeyCode::PageDown => KeyCode::PageDown,
        WKeyCode::Esc => KeyCode::Esc,
        WKeyCode::F(n) => KeyCode::F(n),
        WKeyCode::Paste(s) => KeyCode::Paste(s),
    };
    Key::new(code).with_mods(KeyMods::from_bits_truncate(k.mods))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WViewReq {
    pub repr: Option<String>,
    pub page: Option<u64>,
    pub cols: u16,
    pub rows: u16,
    pub cell_px: Option<(u16, u16)>,
}

pub fn req_to_wire(r: &ViewReq) -> WViewReq {
    WViewReq {
        repr: r.repr.clone(),
        page: r.page.map(|p| p as u64),
        cols: r.cols,
        rows: r.rows,
        cell_px: r.cell_px,
    }
}

pub fn req_from_wire(r: WViewReq) -> Result<ViewReq, WireError> {
    let mut out = ViewReq::new(r.cols, r.rows);
    out.repr = r.repr;
    out.page = r.page.map(|p| index_from_wire(p, "page")).transpose()?;
    out.cell_px = r.cell_px;
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WOutcome {
    Unhandled,
    Redraw,
    Flash(WTr),
    Reload,
}

pub fn outcome_to_wire(o: &Outcome) -> WOutcome {
    match o {
        Outcome::Unhandled => WOutcome::Unhandled,
        Outcome::Redraw => WOutcome::Redraw,
        Outcome::Flash(t) => WOutcome::Flash(tr_to_wire(t)),
        Outcome::Reload => WOutcome::Reload,
    }
}

pub fn outcome_from_wire(o: WOutcome) -> Outcome {
    match o {
        WOutcome::Unhandled => Outcome::Unhandled,
        WOutcome::Redraw => Outcome::Redraw,
        WOutcome::Flash(t) => Outcome::Flash(tr_from_wire(t)),
        WOutcome::Reload => Outcome::Reload,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WFailureKind {
    Unsupported,
    Encrypted,
    Corrupt,
    TooLarge,
    TooHeavy,
    Timeout,
    Cancelled,
    Io,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WFailure {
    pub kind: WFailureKind,
    pub message: WTr,
    pub detail: Option<String>,
}

pub fn failure_to_wire(f: &Failure) -> WFailure {
    WFailure {
        kind: match f.kind {
            FailureKind::Unsupported => WFailureKind::Unsupported,
            FailureKind::Encrypted => WFailureKind::Encrypted,
            FailureKind::Corrupt => WFailureKind::Corrupt,
            FailureKind::TooLarge => WFailureKind::TooLarge,
            FailureKind::TooHeavy => WFailureKind::TooHeavy,
            FailureKind::Timeout => WFailureKind::Timeout,
            FailureKind::Cancelled => WFailureKind::Cancelled,
            FailureKind::Io => WFailureKind::Io,
            FailureKind::External => WFailureKind::External,
        },
        message: tr_to_wire(&f.message),
        detail: f.detail.clone(),
    }
}

pub fn failure_from_wire(f: WFailure) -> Failure {
    let kind = match f.kind {
        WFailureKind::Unsupported => FailureKind::Unsupported,
        WFailureKind::Encrypted => FailureKind::Encrypted,
        WFailureKind::Corrupt => FailureKind::Corrupt,
        WFailureKind::TooLarge => FailureKind::TooLarge,
        WFailureKind::TooHeavy => FailureKind::TooHeavy,
        WFailureKind::Timeout => FailureKind::Timeout,
        WFailureKind::Cancelled => FailureKind::Cancelled,
        WFailureKind::Io => FailureKind::Io,
        WFailureKind::External => FailureKind::External,
    };
    Failure {
        kind,
        message: tr_from_wire(f.message),
        detail: f.detail,
    }
    .clipped()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WPages {
    Separate {
        count: Option<u64>,
        titles: Vec<String>,
    },
    Anchored {
        heading_level: u8,
        count: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WLayout {
    pub reprs: Vec<(String, WTr)>,
    pub default_repr: Option<String>,
    pub default_repr_without_pixels: Option<String>,
    pub pages: Option<WPages>,
}

pub fn layout_to_wire(l: &Layout) -> WLayout {
    WLayout {
        reprs: l
            .reprs
            .iter()
            .map(|r| (r.id.clone(), tr_to_wire(&r.label)))
            .collect(),
        default_repr: l.default_repr.clone(),
        default_repr_without_pixels: l.default_repr_without_pixels.clone(),
        pages: l.pages.as_ref().map(|p| match p {
            PageLayout::Separate { count, titles } => WPages::Separate {
                count: count.map(|c| c as u64),
                titles: titles.clone(),
            },
            PageLayout::Anchored {
                heading_level,
                count,
            } => WPages::Anchored {
                heading_level: *heading_level,
                count: *count as u64,
            },
        }),
    }
}

/// The layout is clipped the way the host clips an in-process plugin's (`Layout::clipped`).
pub fn layout_from_wire(l: WLayout) -> Layout {
    let to_usize = |n: u64| usize::try_from(n).unwrap_or(usize::MAX);
    Layout {
        reprs: l
            .reprs
            .into_iter()
            .map(|(id, label)| ReprDecl::new(id, tr_from_wire(label)))
            .collect(),
        default_repr: l.default_repr,
        default_repr_without_pixels: l.default_repr_without_pixels,
        pages: l.pages.map(|p| match p {
            WPages::Separate { count, titles } => PageLayout::Separate {
                count: count.map(to_usize),
                titles,
            },
            WPages::Anchored {
                heading_level,
                count,
            } => PageLayout::Anchored {
                heading_level,
                count: to_usize(count),
            },
        }),
    }
    .clipped()
}

// ---------------------------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------------------------

/// One picture (no animation).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WLeaf {
    File(WPath),
    Encoded(WBin),
    Svg {
        data: WBin,
        base_dir: Option<WPath>,
        trusted: bool,
    },
    Rgba {
        width: u32,
        height: u32,
        pixels: WBin,
    },
}

/// A picture, or an animation of pictures with their delays in microseconds. There is no
/// `Lazy` here: a lazy picture holds a callback into the plugin's own memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WImage {
    One(WLeaf),
    Frames(Vec<(WLeaf, u64)>),
}

fn leaf_to_wire(d: &ImageData, enc: &mut Enc<'_>) -> Result<WLeaf, WireError> {
    Ok(match d {
        ImageData::File(p) => WLeaf::File(path_required_absolute(p, "image path")?),
        ImageData::Encoded(b) => WLeaf::Encoded(enc.bin(b)?),
        ImageData::Svg {
            data,
            base_dir,
            trusted,
        } => WLeaf::Svg {
            data: enc.bin(data)?,
            base_dir: base_dir
                .as_deref()
                .map(|p| path_required_absolute(p, "svg base directory"))
                .transpose()?,
            trusted: *trusted,
        },
        ImageData::Rgba {
            width,
            height,
            pixels,
        } => WLeaf::Rgba {
            width: *width,
            height: *height,
            pixels: enc.bin(pixels)?,
        },
        ImageData::Frames(_) => return Err(WireError::bad("an animation inside an animation")),
        ImageData::Lazy(_) => {
            return Err(WireError::bad(
                "a lazy picture cannot cross the process boundary",
            ))
        }
    })
}

pub fn image_to_wire(d: &ImageData, enc: &mut Enc<'_>) -> Result<WImage, WireError> {
    match d {
        ImageData::Frames(frames) => {
            if frames.len() > CANVAS_MAX_FRAMES {
                return Err(WireError::too_large("an animation has too many frames"));
            }
            let mut out = Vec::with_capacity(frames.len());
            for (f, delay) in frames {
                let micros = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
                out.push((leaf_to_wire(f, enc)?, micros));
            }
            Ok(WImage::Frames(out))
        }
        other => Ok(WImage::One(leaf_to_wire(other, enc)?)),
    }
}

fn leaf_from_wire(l: WLeaf, dec: &mut Dec<'_>) -> Result<ImageData, WireError> {
    Ok(match l {
        WLeaf::File(p) => ImageData::File(abs_path_from_wire(p, "image file")?),
        WLeaf::Encoded(b) => ImageData::Encoded(dec.arc(b)?),
        WLeaf::Svg {
            data,
            base_dir,
            trusted: _,
        } => ImageData::Svg {
            data: dec.arc(data)?,
            base_dir: base_dir
                .map(|p| abs_path_from_wire(p, "svg base directory"))
                .transpose()?,
            // What the child claims is a ceiling, not a grant: whether an SVG is trusted is the
            // host's decision from where it came from, and a program the host starts as a child
            // is not a standard plugin. So a picture from a child is never trusted.
            trusted: false,
        },
        WLeaf::Rgba {
            width,
            height,
            pixels,
        } => {
            let pixels = dec.bin(pixels)?;
            let want = (width as u64)
                .checked_mul(height as u64)
                .and_then(|n| n.checked_mul(4));
            if width == 0 || height == 0 || want != Some(pixels.len() as u64) {
                return Err(WireError::bad(format!(
                    "a {width}x{height} picture with {} pixel bytes",
                    pixels.len()
                )));
            }
            ImageData::Rgba {
                width,
                height,
                pixels: Arc::new(pixels),
            }
        }
    })
}

pub fn image_from_wire(i: WImage, dec: &mut Dec<'_>) -> Result<ImageData, WireError> {
    match i {
        WImage::One(l) => leaf_from_wire(l, dec),
        WImage::Frames(frames) => {
            if frames.len() > CANVAS_MAX_FRAMES {
                return Err(WireError::too_large("an animation has too many frames"));
            }
            let mut out = Vec::with_capacity(frames.len());
            for (l, micros) in frames {
                out.push((leaf_from_wire(l, dec)?, Duration::from_micros(micros)));
            }
            Ok(ImageData::Frames(out))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WTextSource {
    File(WPath),
    Owned(String),
    Lines(Vec<WLine>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WText {
    pub source: WTextSource,
    pub file_lines: bool,
    pub syntax_name: Option<String>,
    /// Why the text was sent plain although the plugin styled it (too long to style, or the styled
    /// form would not fit in a frame). The host does not show it; it is for the log of the child.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WDocOptions {
    pub front_matter: bool,
    pub footnotes: bool,
    pub inline_html: bool,
    pub autolink: bool,
    pub emoji: bool,
    pub alerts: bool,
    pub details: String,
    pub task_states: Vec<char>,
    pub tasks_writable: bool,
    pub table_align: String,
    pub image_align: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WDocument {
    pub markdown: String,
    pub origin: WOrigin,
    pub resources: Vec<(String, WBin)>,
    pub options: WDocOptions,
    pub title: Option<String>,
    pub truncated: bool,
}

/// A table as data: the child reads every cell of the grid. Rows may be shorter than `ncols`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WTable {
    pub title: Option<String>,
    pub rainbow: bool,
    pub addressed: bool,
    pub ncols: u64,
    pub header: Vec<Option<String>>,
    pub truncated: bool,
    pub rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WImageView {
    pub data: WImage,
    pub sharp_zoom: bool,
    pub raster_max_px: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WListRow {
    pub cells: Vec<WLine>,
    pub kind: String,
    pub target: Option<WPath>,
    pub checked: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WDiffKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WDiffLine {
    pub kind: WDiffKind,
    pub old_no: Option<u64>,
    pub new_no: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WReopen {
    pub content_type: String,
    pub text: String,
    pub resources: Vec<(String, WBin)>,
    pub origin: WOrigin,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WCanvasImage {
    pub id: u64,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub image: Option<WImage>,
}

/// A canvas as rows of style runs. A run is the number of a style and the symbols of its cells
/// written one after the other; the empty cell after a wide character is written as U+0000 (a
/// control character, which a cell symbol may not hold, so it is never a real symbol).
///
/// The styles are a table of this answer (`styles`, [`STYLE_RECORD`] bytes each, see
/// [`pack_styles`]) that the runs refer to by number, so a style is written once however many runs
/// use it. The table is a byte string, so a canvas whose every cell has a style of its own (up to
/// four million) is a table of tens of megabytes that goes through a side file, not the frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WCanvas {
    pub cols: u16,
    pub rows: u16,
    pub styles: WBin,
    pub runs: Vec<Vec<(u32, String)>>,
    pub images: Vec<WCanvasImage>,
    /// Milliseconds, rounded up; `None` is "no timed redraw".
    pub redraw_after_ms: Option<u32>,
    pub text_input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WView {
    Text(WText),
    Document(WDocument),
    Table(WTable),
    Image(WImageView),
    List(Vec<WListRow>),
    Diff(Vec<Vec<WDiffLine>>),
    Canvas(WCanvas),
    Reopen(WReopen),
    Message(Vec<WLine>),
}

const CONTINUATION: &str = "\u{0}";

/// A table the child keeps whole: every cell of a grid, in row order, no more than the existing
/// cap ([`TABLE_MAX_CELLS`]).
struct DataGrid {
    ncols: usize,
    header: Vec<Option<String>>,
    truncated: bool,
    rows: Vec<Vec<String>>,
}

impl Grid for DataGrid {
    fn nrows(&self) -> usize {
        self.rows.len()
    }

    fn ncols(&self) -> usize {
        self.ncols
    }

    fn header(&self, col: usize) -> Option<String> {
        self.header.get(col).cloned().flatten()
    }

    fn cell(&self, row: usize, col: usize) -> String {
        self.cell_str(row, col).unwrap_or("").to_string()
    }

    fn cell_str(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row)?.get(col).map(String::as_str)
    }

    fn row_len(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, Vec::len)
    }

    fn truncated(&self) -> bool {
        self.truncated
    }

    fn visit_row(&self, row: usize, f: &mut dyn FnMut(usize, &str)) {
        if let Some(r) = self.rows.get(row) {
            for (col, text) in r.iter().enumerate() {
                f(col, text);
            }
        }
    }
}

fn doc_options_to_wire(o: &DocOptions) -> WDocOptions {
    WDocOptions {
        front_matter: o.front_matter,
        footnotes: o.footnotes,
        inline_html: o.inline_html,
        autolink: o.autolink,
        emoji: o.emoji,
        alerts: o.alerts,
        details: o.details.clone(),
        task_states: o.task_states.clone(),
        tasks_writable: o.tasks_writable,
        table_align: o.table_align.clone(),
        image_align: o.image_align.clone(),
    }
}

fn doc_options_from_wire(o: WDocOptions) -> DocOptions {
    DocOptions {
        front_matter: o.front_matter,
        footnotes: o.footnotes,
        inline_html: o.inline_html,
        autolink: o.autolink,
        emoji: o.emoji,
        alerts: o.alerts,
        details: o.details,
        task_states: o.task_states,
        tasks_writable: o.tasks_writable,
        table_align: o.table_align,
        image_align: o.image_align,
    }
}

/// Style `text` with `styler` (the lines of a file whose styler lives in the child).
fn styled_lines(styler: &mut dyn LineStyler, text: &str) -> Vec<Line> {
    styler.style_block(0, text)
}

/// An upper bound of the encoded size of styled lines: every span is its text, the map keys of a
/// style, two colours and the framing. Cheap to compute, so the exact size is only measured for
/// the rare text that comes near the limit.
fn styled_size_bound(lines: &[WLine]) -> usize {
    lines
        .iter()
        .map(|l| 80 + l.spans.iter().map(|(t, _)| t.len() + 80).sum::<usize>())
        .sum()
}

/// Styled lines as a source, or `Err(reason)` when their encoded form is over
/// [`STYLED_MAX_ENCODED`] (the caller then sends the text plain).
fn styled_source(lines: Vec<Line>) -> Result<WTextSource, String> {
    let wl = lines_to_wire(&lines);
    if styled_size_bound(&wl) > STYLED_MAX_ENCODED {
        let exact = rmp_serde::to_vec_named(&wl)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if exact > STYLED_MAX_ENCODED {
            return Err(format!(
                "the styled text is {exact} bytes encoded, over the limit of {STYLED_MAX_ENCODED}"
            ));
        }
    }
    Ok(WTextSource::Lines(wl))
}

fn text_to_wire(t: &mut TextView) -> Result<WText, WireError> {
    let styler_ready = t.styler.as_ref().is_some_and(|s| s.ready());
    let mut note: Option<String> = None;
    let source = match (&t.source, styler_ready) {
        (TextSource::Lines(lines), _) => match styled_source(lines.clone()) {
            Ok(src) => src,
            Err(why) => {
                // The same fallback as for text this side styles: the words, without the colours.
                note = Some(format!("{why}; sent plain"));
                WTextSource::Owned(lines.iter().map(Line::plain).collect::<Vec<_>>().join("\n"))
            }
        },
        (TextSource::Owned(s), false) => WTextSource::Owned(s.clone()),
        (TextSource::Owned(s), true) => {
            if s.len() as u64 > STYLE_MAX_BYTES {
                note = Some(format!(
                    "the text is {} bytes, over the {STYLE_MAX_BYTES} that are styled; sent plain",
                    s.len()
                ));
                WTextSource::Owned(s.clone())
            } else {
                let styler = t
                    .styler
                    .as_mut()
                    .ok_or_else(|| WireError::bad("no styler"))?;
                match styled_source(styled_lines(&mut **styler, s)) {
                    Ok(src) => src,
                    Err(why) => {
                        note = Some(format!("{why}; sent plain"));
                        WTextSource::Owned(s.clone())
                    }
                }
            }
        }
        (TextSource::File(p), false) => WTextSource::File(path_required_absolute(p, "text file")?),
        (TextSource::File(p), true) => {
            let wp = path_required_absolute(p, "text file")?;
            let small = std::fs::metadata(p).is_ok_and(|m| m.len() <= STYLE_MAX_BYTES);
            let text = if small {
                std::fs::read(p)
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
            } else {
                note = Some(format!(
                    "the file is over the {STYLE_MAX_BYTES} bytes that are styled; sent as the file"
                ));
                None
            };
            match (text, t.styler.as_mut()) {
                (Some(text), Some(styler)) => {
                    match styled_source(styled_lines(&mut **styler, &text)) {
                        Ok(src) => src,
                        Err(why) => {
                            note = Some(format!("{why}; sent as the file"));
                            WTextSource::File(wp)
                        }
                    }
                }
                _ => WTextSource::File(wp),
            }
        }
    };
    Ok(WText {
        source,
        file_lines: t.file_lines,
        syntax_name: t.syntax_name.clone(),
        note,
    })
}

fn table_to_wire(t: &TableView) -> Result<WTable, WireError> {
    let g = &*t.grid;
    let ncols = g.ncols();
    if ncols > TABLE_MAX_COLS {
        return Err(WireError::too_large(format!("a table of {ncols} columns")));
    }
    let nrows = g.nrows();
    if nrows > TABLE_MAX_CELLS {
        return Err(WireError::too_large(format!("a table of {nrows} rows")));
    }
    let mut cells = 0usize;
    let mut rows = Vec::new();
    for r in 0..nrows {
        let len = g.row_len(r).min(ncols);
        cells = cells.saturating_add(len.max(1));
        if cells > TABLE_MAX_CELLS {
            return Err(WireError::too_large(format!(
                "a table of more than {TABLE_MAX_CELLS} cells"
            )));
        }
        let mut row = vec![String::new(); len];
        g.visit_row(r, &mut |c, text| {
            if let Some(slot) = row.get_mut(c) {
                *slot = text.to_string();
            }
        });
        rows.push(row);
    }
    Ok(WTable {
        title: t.title.clone(),
        rainbow: t.rainbow,
        addressed: t.addressed,
        ncols: ncols as u64,
        header: (0..ncols).map(|c| g.header(c)).collect(),
        truncated: g.truncated(),
        rows,
    })
}

fn table_from_wire(t: WTable) -> Result<TableView, WireError> {
    let ncols = index_from_wire(t.ncols, "column count")?;
    if ncols > TABLE_MAX_COLS {
        return Err(WireError::too_large(format!("a table of {ncols} columns")));
    }
    if t.header.len() > ncols {
        return Err(WireError::bad("a table header is wider than the table"));
    }
    let mut cells = 0usize;
    for r in &t.rows {
        if r.len() > ncols {
            return Err(WireError::bad("a table row is wider than the table"));
        }
        cells = cells.saturating_add(r.len().max(1));
        if cells > TABLE_MAX_CELLS {
            return Err(WireError::too_large(format!(
                "a table of more than {TABLE_MAX_CELLS} cells"
            )));
        }
    }
    Ok(TableView {
        grid: Arc::new(DataGrid {
            ncols,
            header: t.header,
            truncated: t.truncated,
            rows: t.rows,
        }),
        title: t.title,
        rainbow: t.rainbow,
        addressed: t.addressed,
    })
}

/// The bytes of one packed style: foreground (4), background (4), added modifiers (2, little
/// endian), removed modifiers (2).
pub const STYLE_RECORD: usize = 12;

const NAMED_COLORS: [WColor; 17] = [
    WColor::Reset,
    WColor::Black,
    WColor::Red,
    WColor::Green,
    WColor::Yellow,
    WColor::Blue,
    WColor::Magenta,
    WColor::Cyan,
    WColor::Gray,
    WColor::DarkGray,
    WColor::LightRed,
    WColor::LightGreen,
    WColor::LightYellow,
    WColor::LightBlue,
    WColor::LightMagenta,
    WColor::LightCyan,
    WColor::White,
];
const TAG_INDEXED: u8 = 18;
const TAG_RGB: u8 = 19;

fn pack_color(c: Option<WColor>, out: &mut Vec<u8>) {
    // Tag 0 is "no colour"; 1..=17 the named colours; then indexed and RGB with their bytes.
    let rec = match c {
        None => [0, 0, 0, 0],
        Some(WColor::Indexed(i)) => [TAG_INDEXED, i, 0, 0],
        Some(WColor::Rgb(r, g, b)) => [TAG_RGB, r, g, b],
        Some(named) => {
            let tag = NAMED_COLORS
                .iter()
                .position(|n| *n == named)
                .map_or(0, |i| i as u8 + 1);
            [tag, 0, 0, 0]
        }
    };
    out.extend_from_slice(&rec);
}

fn unpack_color(rec: &[u8]) -> Result<Option<WColor>, WireError> {
    Ok(match rec[0] {
        0 => None,
        t @ 1..=17 => Some(NAMED_COLORS[t as usize - 1]),
        TAG_INDEXED => Some(WColor::Indexed(rec[1])),
        TAG_RGB => Some(WColor::Rgb(rec[1], rec[2], rec[3])),
        t => return Err(WireError::bad(format!("canvas style with colour tag {t}"))),
    })
}

/// The style table as bytes.
pub fn pack_styles(styles: &[WStyle]) -> Vec<u8> {
    let mut out = Vec::with_capacity(styles.len() * STYLE_RECORD);
    for s in styles {
        pack_color(s.fg, &mut out);
        pack_color(s.bg, &mut out);
        out.extend_from_slice(&s.add.to_le_bytes());
        out.extend_from_slice(&s.sub.to_le_bytes());
    }
    out
}

/// The style table from its bytes; at most `max` styles (a canvas cannot use more styles than it
/// has cells).
fn unpack_styles(bytes: &[u8], max: usize) -> Result<Vec<Style>, WireError> {
    if !bytes.len().is_multiple_of(STYLE_RECORD) {
        return Err(WireError::bad(format!(
            "canvas style table of {} bytes is not whole records",
            bytes.len()
        )));
    }
    if bytes.len() / STYLE_RECORD > max {
        return Err(WireError::bad("canvas has more styles than cells"));
    }
    bytes
        .as_chunks::<STYLE_RECORD>()
        .0
        .iter()
        .map(|r| {
            Ok(style_from_wire(WStyle {
                fg: unpack_color(&r[0..4])?,
                bg: unpack_color(&r[4..8])?,
                add: u16::from_le_bytes([r[8], r[9]]),
                sub: u16::from_le_bytes([r[10], r[11]]),
            }))
        })
        .collect()
}

fn canvas_to_wire(c: &CanvasView, enc: &mut Enc<'_>) -> Result<WCanvas, WireError> {
    c.check()
        .map_err(|e| WireError::bad(format!("canvas: {e}")))?;
    let cols = c.cols as usize;
    let mut table: Vec<WStyle> = Vec::new();
    let mut numbers: HashMap<WStyle, u32> = HashMap::new();
    let mut runs: Vec<Vec<(u32, String)>> = Vec::with_capacity(c.rows as usize);
    for y in 0..c.rows as usize {
        let mut row: Vec<(u32, String)> = Vec::new();
        let mut last: Option<&str> = None;
        for cell in &c.cells[y * cols..(y + 1) * cols] {
            let ws = style_to_wire(&cell.style);
            let st = *numbers.entry(ws).or_insert_with(|| {
                table.push(ws);
                (table.len() - 1) as u32
            });
            let sym: &str = if cell.symbol.is_empty() {
                CONTINUATION
            } else {
                &cell.symbol
            };
            // Two cells whose symbols would fuse into one grapheme when written one after the
            // other (two regional indicators) start a new run, so a run reads back cell for cell.
            let fuses = last.is_some_and(|l| {
                !(l.is_ascii() && sym.is_ascii()) && {
                    let mut joined = String::with_capacity(l.len() + sym.len());
                    joined.push_str(l);
                    joined.push_str(sym);
                    joined.graphemes(true).count() < 2
                }
            });
            match row.last_mut() {
                Some((s, text)) if *s == st && !fuses => text.push_str(sym),
                _ => row.push((st, sym.to_string())),
            }
            last = Some(sym);
        }
        runs.push(row);
    }
    let styles = enc.bin(&pack_styles(&table))?;
    let images = c
        .images
        .iter()
        .map(|im| {
            Ok(WCanvasImage {
                id: im.id,
                x: im.x,
                y: im.y,
                cols: im.cols,
                rows: im.rows,
                image: im
                    .image
                    .as_ref()
                    .map(|d| image_to_wire(d, enc))
                    .transpose()?,
            })
        })
        .collect::<Result<_, WireError>>()?;
    Ok(WCanvas {
        cols: c.cols,
        rows: c.rows,
        styles,
        runs,
        images,
        redraw_after_ms: c
            .redraw_after
            .map(|d| u32::try_from(d.as_nanos().div_ceil(1_000_000)).unwrap_or(u32::MAX)),
        text_input: c.text_input,
    })
}

fn canvas_from_wire(c: WCanvas, dec: &mut Dec<'_>) -> Result<CanvasView, WireError> {
    // The sizes are checked against the limits before anything is built from them, and the rows
    // against the data actually sent: nothing is allocated for a size the data does not fill.
    if c.cols > crate::view::CANVAS_MAX_SIDE
        || c.rows > crate::view::CANVAS_MAX_SIDE
        || c.cols as usize * c.rows as usize > crate::view::CANVAS_MAX_CELLS
    {
        return Err(WireError::bad(format!(
            "canvas of {}x{} cells is too large",
            c.cols, c.rows
        )));
    }
    if c.runs.len() != c.rows as usize {
        return Err(WireError::bad(format!(
            "canvas has {} rows of cells, expected {}",
            c.runs.len(),
            c.rows
        )));
    }
    let cols = c.cols as usize;
    let styles = unpack_styles(
        &dec.bin(c.styles)?,
        (c.cols as usize * c.rows as usize).max(1),
    )?;
    let mut cells: Vec<CanvasCell> = Vec::new();
    for (y, row) in c.runs.into_iter().enumerate() {
        let before = cells.len();
        for (st, text) in row {
            let style = *styles.get(st as usize).ok_or_else(|| {
                WireError::bad(format!("canvas run names style {st}, not in the table"))
            })?;
            for g in text.graphemes(true) {
                if cells.len() - before >= cols {
                    return Err(WireError::bad(format!("canvas row {y} has too many cells")));
                }
                let symbol = if g == CONTINUATION { "" } else { g };
                cells.push(CanvasCell::new(symbol, style));
            }
        }
        if cells.len() - before != cols {
            return Err(WireError::bad(format!(
                "canvas row {y} has {} cells, expected {cols}",
                cells.len() - before
            )));
        }
    }
    let mut images = Vec::new();
    for im in c.images {
        // Bounded by the data: each entry is already in memory.
        let mut out = CanvasImage::unchanged(
            im.id,
            crate::view::CellRect::new(im.x, im.y, im.cols, im.rows),
        );
        out.image = im.image.map(|i| image_from_wire(i, dec)).transpose()?;
        images.push(out);
    }
    let mut canvas = CanvasView::new(0, 0);
    canvas.cols = c.cols;
    canvas.rows = c.rows;
    canvas.cells = cells;
    canvas.images = images;
    canvas.redraw_after = c
        .redraw_after_ms
        .map(|ms| Duration::from_millis(u64::from(ms)));
    canvas.text_input = c.text_input;
    canvas
        .check()
        .map_err(|e| WireError::bad(format!("canvas: {e}")))?;
    Ok(canvas)
}

/// Convert a view for sending. Takes the view by mutable reference because a text view's styler
/// is run (it is `&mut`).
pub fn view_to_wire(v: &mut View, enc: &mut Enc<'_>) -> Result<WView, WireError> {
    Ok(match v {
        View::Text(t) => WView::Text(text_to_wire(t)?),
        View::Document(d) => WView::Document(WDocument {
            markdown: d.markdown.clone(),
            origin: origin_to_wire(&d.origin, false)?,
            resources: resources_to_wire(&d.resources, enc)?,
            options: doc_options_to_wire(&d.options),
            title: d.title.clone(),
            truncated: d.truncated,
        }),
        View::Table(t) => WView::Table(table_to_wire(t)?),
        View::Image(i) => WView::Image(WImageView {
            data: image_to_wire(&i.data, enc)?,
            sharp_zoom: i.sharp_zoom,
            raster_max_px: i.raster_max_px,
        }),
        View::List(l) => WView::List(
            l.rows
                .iter()
                .map(|r| {
                    Ok(WListRow {
                        cells: lines_to_wire(&r.cells),
                        kind: r.kind.clone(),
                        target: r
                            .target
                            .as_deref()
                            .map(|p| path_required_absolute(p, "list target"))
                            .transpose()?,
                        checked: r.checked,
                    })
                })
                .collect::<Result<_, WireError>>()?,
        ),
        View::Diff(d) => WView::Diff(
            d.hunks
                .iter()
                .map(|h| {
                    h.lines
                        .iter()
                        .map(|l| WDiffLine {
                            kind: match l.kind {
                                DiffKind::Context => WDiffKind::Context,
                                DiffKind::Added => WDiffKind::Added,
                                DiffKind::Removed => WDiffKind::Removed,
                            },
                            old_no: l.old_no.map(|n| n as u64),
                            new_no: l.new_no.map(|n| n as u64),
                            text: l.text.clone(),
                        })
                        .collect()
                })
                .collect(),
        ),
        View::Canvas(c) => WView::Canvas(canvas_to_wire(c, enc)?),
        View::Reopen(r) => WView::Reopen(WReopen {
            content_type: r.content_type.clone(),
            text: r.text.clone(),
            resources: resources_to_wire(&r.resources, enc)?,
            origin: origin_to_wire(&r.origin, false)?,
            truncated: r.truncated,
        }),
        View::Message(m) => WView::Message(lines_to_wire(&m.lines)),
    })
}

// ---------------------------------------------------------------------------------------------
// The side files a message names
// ---------------------------------------------------------------------------------------------

fn bin_name(b: &WBin, out: &mut Vec<String>) {
    if let WBin::File { name, .. } = b {
        out.push(name.clone());
    }
}

fn resource_names(r: &[(String, WBin)], out: &mut Vec<String>) {
    for (_, b) in r {
        bin_name(b, out);
    }
}

fn leaf_names(l: &WLeaf, out: &mut Vec<String>) {
    match l {
        WLeaf::File(_) => {}
        WLeaf::Encoded(b) => bin_name(b, out),
        WLeaf::Svg { data, .. } => bin_name(data, out),
        WLeaf::Rgba { pixels, .. } => bin_name(pixels, out),
    }
}

fn image_names(i: &WImage, out: &mut Vec<String>) {
    match i {
        WImage::One(l) => leaf_names(l, out),
        WImage::Frames(fs) => fs.iter().for_each(|(l, _)| leaf_names(l, out)),
    }
}

/// The names of the side files a received view announces. The receiver deletes the ones it did not
/// get to read when the conversion of the message fails.
pub fn view_side_names(v: &WView) -> Vec<String> {
    let mut out = Vec::new();
    match v {
        WView::Document(d) => resource_names(&d.resources, &mut out),
        WView::Reopen(r) => resource_names(&r.resources, &mut out),
        WView::Image(i) => image_names(&i.data, &mut out),
        WView::Canvas(c) => {
            bin_name(&c.styles, &mut out);
            for im in &c.images {
                if let Some(i) = &im.image {
                    image_names(i, &mut out);
                }
            }
        }
        WView::Text(_) | WView::Table(_) | WView::List(_) | WView::Diff(_) | WView::Message(_) => {}
    }
    out
}

/// The names of the side files a received input announces (see [`view_side_names`]).
pub fn input_side_names(i: &WInput) -> Vec<String> {
    let mut out = Vec::new();
    if let WInput::Content { resources, .. } = i {
        resource_names(resources, &mut out);
    }
    out
}

/// Convert a received view. The sender is untrusted: see the module docs.
pub fn view_from_wire(v: WView, dec: &mut Dec<'_>) -> Result<View, WireError> {
    Ok(match v {
        WView::Text(t) => View::Text(TextView {
            source: match t.source {
                WTextSource::File(p) => TextSource::File(abs_path_from_wire(p, "text file")?),
                WTextSource::Owned(s) => TextSource::Owned(s),
                WTextSource::Lines(l) => TextSource::Lines(lines_from_wire(l)),
            },
            styler: None,
            file_lines: t.file_lines,
            syntax_name: t.syntax_name,
        }),
        WView::Document(d) => View::Document(DocumentView {
            markdown: d.markdown,
            origin: origin_from_wire(d.origin)?,
            resources: resources_from_wire(d.resources, dec)?,
            options: doc_options_from_wire(d.options),
            title: d.title,
            truncated: d.truncated,
        }),
        WView::Table(t) => View::Table(table_from_wire(t)?),
        WView::Image(i) => View::Image(ImageView {
            data: image_from_wire(i.data, dec)?,
            sharp_zoom: i.sharp_zoom,
            raster_max_px: i.raster_max_px,
        }),
        WView::List(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                out.push(ListRow {
                    cells: lines_from_wire(r.cells),
                    kind: r.kind,
                    target: r
                        .target
                        .map(|p| abs_path_from_wire(p, "list target"))
                        .transpose()?,
                    checked: r.checked,
                });
            }
            View::List(ListView { rows: out })
        }
        WView::Diff(hunks) => {
            let mut out = Vec::with_capacity(hunks.len());
            for h in hunks {
                let mut lines = Vec::with_capacity(h.len());
                for l in h {
                    lines.push(DiffLine {
                        kind: match l.kind {
                            WDiffKind::Context => DiffKind::Context,
                            WDiffKind::Added => DiffKind::Added,
                            WDiffKind::Removed => DiffKind::Removed,
                        },
                        old_no: l
                            .old_no
                            .map(|n| index_from_wire(n, "line number"))
                            .transpose()?,
                        new_no: l
                            .new_no
                            .map(|n| index_from_wire(n, "line number"))
                            .transpose()?,
                        text: l.text,
                    });
                }
                out.push(DiffHunk { lines });
            }
            View::Diff(DiffView { hunks: out })
        }
        WView::Canvas(c) => View::Canvas(canvas_from_wire(c, dec)?),
        WView::Reopen(r) => View::Reopen(Reopen {
            content_type: r.content_type,
            text: r.text,
            resources: resources_from_wire(r.resources, dec)?,
            origin: origin_from_wire(r.origin)?,
            truncated: r.truncated,
        }),
        WView::Message(l) => View::Message(MessageView {
            lines: lines_from_wire(l),
        }),
    })
}

// ---------------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WTarget {
    Glob(String),
    FileName(String),
    Mime(String),
    Content(String),
    Embed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WBind {
    pub targets: Vec<WTarget>,
    /// `true` for `Priority::Option`.
    pub option: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WAct {
    pub id: String,
    pub key: Option<String>,
    pub label: WTr,
    pub help: WTr,
    pub footer: Option<u8>,
    pub screen: Option<String>,
    pub writes: bool,
    pub destructive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WLauncher {
    Editor,
    MacApp { bundle_id: String },
    Command(Vec<String>),
    OsDefault,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WVerb {
    /// `true` for `Verb::Open`, `false` for `Verb::Edit`.
    pub open: bool,
    pub chain: Vec<WLauncher>,
    /// `true` for `LaunchMode::Gui`.
    pub gui: bool,
    pub line_token: bool,
    pub label: Option<WTr>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WSetting {
    pub name: String,
    pub default: WValue,
    pub help: WTr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WViewKind {
    Text,
    Document,
    Table,
    Image,
    Diff,
    List,
    Canvas,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WUnavailable {
    Message,
    RawText { with_reason: bool },
    Notice(WTr),
    Picture,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WManifest {
    pub id: String,
    pub version: String,
    pub api: (u16, u16),
    pub name: WTr,
    pub binds: Vec<WBind>,
    pub actions: Vec<WAct>,
    pub verbs: Vec<WVerb>,
    pub settings: Vec<WSetting>,
    pub screens: Vec<String>,
    pub shows: Vec<WViewKind>,
    pub work_group: Option<String>,
    pub converts: bool,
    pub file_text: bool,
    pub sized: bool,
    pub unavailable: WUnavailable,
}

pub fn manifest_to_wire(m: &Manifest) -> Result<WManifest, WireError> {
    Ok(WManifest {
        id: m.id.clone(),
        version: m.version.clone(),
        api: (m.api.major, m.api.minor),
        name: tr_to_wire(&m.name),
        binds: m
            .binds
            .iter()
            .map(|b| WBind {
                targets: b
                    .targets
                    .iter()
                    .map(|t| match t {
                        Target::Glob(s) => WTarget::Glob(s.clone()),
                        Target::FileName(s) => WTarget::FileName(s.clone()),
                        Target::Mime(s) => WTarget::Mime(s.clone()),
                        Target::Content(s) => WTarget::Content(s.clone()),
                        Target::Embed(s) => WTarget::Embed(s.clone()),
                    })
                    .collect(),
                option: b.priority == Priority::Option,
            })
            .collect(),
        actions: m
            .actions
            .iter()
            .map(|a| WAct {
                id: a.id.clone(),
                key: a.key.clone(),
                label: tr_to_wire(&a.label),
                help: tr_to_wire(&a.help),
                footer: a.footer,
                screen: a.screen.clone(),
                writes: a.writes,
                destructive: a.destructive,
            })
            .collect(),
        verbs: m
            .verbs
            .iter()
            .map(|v| WVerb {
                open: v.verb == Verb::Open,
                chain: v
                    .chain
                    .iter()
                    .map(|l| match l {
                        Launcher::Editor => WLauncher::Editor,
                        Launcher::MacApp { bundle_id } => WLauncher::MacApp {
                            bundle_id: bundle_id.clone(),
                        },
                        Launcher::Command(c) => WLauncher::Command(c.clone()),
                        Launcher::OsDefault => WLauncher::OsDefault,
                    })
                    .collect(),
                gui: v.mode == LaunchMode::Gui,
                line_token: v.line_token,
                label: v.label.as_ref().map(tr_to_wire),
            })
            .collect(),
        settings: m
            .settings
            .iter()
            .map(|s| {
                Ok(WSetting {
                    name: s.name.clone(),
                    default: value_to_wire(&s.default, 0)?,
                    help: tr_to_wire(&s.help),
                })
            })
            .collect::<Result<_, WireError>>()?,
        screens: m.screens.clone(),
        shows: m
            .shows
            .iter()
            .map(|k| match k {
                ViewKind::Text => WViewKind::Text,
                ViewKind::Document => WViewKind::Document,
                ViewKind::Table => WViewKind::Table,
                ViewKind::Image => WViewKind::Image,
                ViewKind::Diff => WViewKind::Diff,
                ViewKind::List => WViewKind::List,
                ViewKind::Canvas => WViewKind::Canvas,
            })
            .collect(),
        work_group: m.work_group.clone(),
        converts: m.converts,
        file_text: m.file_text,
        sized: m.sized,
        unavailable: match &m.unavailable {
            Unavailable::Message => WUnavailable::Message,
            Unavailable::RawText { with_reason } => WUnavailable::RawText {
                with_reason: *with_reason,
            },
            Unavailable::Notice(t) => WUnavailable::Notice(tr_to_wire(t)),
            Unavailable::Picture => WUnavailable::Picture,
        },
    })
}

/// A manifest from the wire, or from the host's cache file. Texts are clipped; the declarations
/// themselves (keys, globs, ids) are validated by the registry as for any plugin, not here.
pub fn manifest_from_wire(m: WManifest) -> Result<Manifest, WireError> {
    let mut out = Manifest::new(m.id, m.version);
    out.api = ApiVersion {
        major: m.api.0,
        minor: m.api.1,
    };
    out.name = tr_from_wire(m.name);
    out.binds = m
        .binds
        .into_iter()
        .map(|b| Bind {
            targets: b
                .targets
                .into_iter()
                .map(|t| match t {
                    WTarget::Glob(s) => Target::Glob(s),
                    WTarget::FileName(s) => Target::FileName(s),
                    WTarget::Mime(s) => Target::Mime(s),
                    WTarget::Content(s) => Target::Content(s),
                    WTarget::Embed(s) => Target::Embed(s),
                })
                .collect(),
            priority: if b.option {
                Priority::Option
            } else {
                Priority::Default
            },
        })
        .collect();
    out.actions = m
        .actions
        .into_iter()
        .map(|a| Act {
            id: a.id,
            key: a.key,
            label: tr_from_wire(a.label),
            help: tr_from_wire(a.help),
            footer: a.footer,
            screen: a.screen,
            writes: a.writes,
            destructive: a.destructive,
        })
        .collect();
    out.verbs = m
        .verbs
        .into_iter()
        .map(|v| VerbDecl {
            verb: if v.open { Verb::Open } else { Verb::Edit },
            chain: v
                .chain
                .into_iter()
                .map(|l| match l {
                    WLauncher::Editor => Launcher::Editor,
                    WLauncher::MacApp { bundle_id } => Launcher::MacApp { bundle_id },
                    WLauncher::Command(c) => Launcher::Command(c),
                    WLauncher::OsDefault => Launcher::OsDefault,
                })
                .collect(),
            mode: if v.gui {
                LaunchMode::Gui
            } else {
                LaunchMode::Terminal
            },
            line_token: v.line_token,
            label: v.label.map(tr_from_wire),
        })
        .collect();
    out.settings = m
        .settings
        .into_iter()
        .map(|s| {
            Ok(SettingDecl::new(
                s.name,
                value_from_wire(s.default, 0)?.cleaned(),
                tr_from_wire(s.help),
            ))
        })
        .collect::<Result<_, WireError>>()?;
    out.screens = m.screens;
    out.shows = m
        .shows
        .into_iter()
        .map(|k| match k {
            WViewKind::Text => ViewKind::Text,
            WViewKind::Document => ViewKind::Document,
            WViewKind::Table => ViewKind::Table,
            WViewKind::Image => ViewKind::Image,
            WViewKind::Diff => ViewKind::Diff,
            WViewKind::List => ViewKind::List,
            WViewKind::Canvas => ViewKind::Canvas,
        })
        .collect();
    out.work_group = m.work_group;
    out.converts = m.converts;
    out.file_text = m.file_text;
    out.sized = m.sized;
    out.unavailable = match m.unavailable {
        WUnavailable::Message => Unavailable::Message,
        WUnavailable::RawText { with_reason } => Unavailable::RawText { with_reason },
        WUnavailable::Notice(t) => Unavailable::Notice(tr_from_wire(t)),
        WUnavailable::Picture => Unavailable::Picture,
    };
    Ok(out)
}
