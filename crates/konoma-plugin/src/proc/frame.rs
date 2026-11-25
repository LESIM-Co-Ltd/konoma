//! Framing: a 4-byte big-endian length, then a MessagePack body.
//!
//! The length is checked **before** the body is read, so a peer that announces a huge frame costs
//! nothing. The body is read with a growing buffer (not one allocation of the announced size), so a
//! peer that announces 64 MiB and sends 3 bytes costs 3 bytes.

use std::fmt;
use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// The largest frame body either side accepts or sends. A canvas of the largest allowed size is
/// well under it (one run per row); anything bigger travels as a side file (see `wire::Side`) or is
/// refused. 64 MiB keeps a frame that a hostile peer can make the other side hold in memory small
/// against the 1 GiB the host allows a child as a whole.
pub const MAX_FRAME: usize = 64 << 20;

/// The deepest nesting the decoder follows. A settings table nests, and `rmp-serde` recurses on the
/// stack, so a frame of a million nested arrays must stop here, not at the end of the stack.
pub const MAX_DEPTH: usize = 128;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    /// The announced body is over [`MAX_FRAME`] (the announced size).
    TooLarge(u64),
    /// The stream ended inside a frame.
    Truncated,
    /// The body is not a valid message.
    Codec(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "I/O error on the plugin pipe: {e}"),
            FrameError::TooLarge(n) => {
                write!(f, "frame of {n} bytes is over the limit of {MAX_FRAME}")
            }
            FrameError::Truncated => write!(f, "the plugin pipe ended inside a frame"),
            FrameError::Codec(m) => write!(f, "malformed frame: {m}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> FrameError {
        FrameError::Io(e)
    }
}

/// Encode one message as a MessagePack body (struct fields by name, so a field can be added).
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = rmp_serde::to_vec_named(msg).map_err(|e| FrameError::Codec(e.to_string()))?;
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len() as u64));
    }
    Ok(body)
}

/// Decode a body. Trailing bytes are an error (a frame is exactly one message).
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    let mut rd = body;
    let mut de = rmp_serde::Deserializer::new(&mut rd);
    de.set_max_depth(MAX_DEPTH);
    let v = T::deserialize(&mut de).map_err(|e| FrameError::Codec(e.to_string()))?;
    if !rd.is_empty() {
        return Err(FrameError::Codec(format!(
            "{} bytes after the message",
            rd.len()
        )));
    }
    Ok(v)
}

/// Write one frame (and flush).
pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> Result<(), FrameError> {
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len() as u64));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(body)?;
    w.flush()?;
    Ok(())
}

/// Read one frame body. `Ok(None)` is a clean end of stream (before any byte of a frame).
pub fn read_frame<R: Read>(r: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = u32::from_be_bytes(head) as u64;
    if len > MAX_FRAME as u64 {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = Vec::new();
    let n = r.take(len).read_to_end(&mut body)?;
    if (n as u64) < len {
        return Err(FrameError::Truncated);
    }
    Ok(Some(body))
}

/// Encode and write a message.
pub fn send<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    write_frame(w, &encode(msg)?)
}

/// Read and decode a message. `Ok(None)` is a clean end of stream.
pub fn recv<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>, FrameError> {
    match read_frame(r)? {
        None => Ok(None),
        Some(body) => decode(&body).map(Some),
    }
}
