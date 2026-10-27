//! The messages: a handshake, then calls and answers.

use serde::{Deserialize, Serialize};

use super::wire::{
    WCx, WFailure, WInput, WKey, WLayout, WManifest, WOutcome, WPath, WView, WViewReq,
};

/// The protocol version this build speaks. Versions start at 1; a change that an older end cannot
/// read bumps it.
pub const PROTOCOL_VERSION: u32 = 1;

/// The versions a host accepts: the current one and the one before (none before 1 yet).
pub fn host_versions() -> Vec<u32> {
    (PROTOCOL_VERSION.saturating_sub(1).max(1)..=PROTOCOL_VERSION).collect()
}

/// The first frame, host to child.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Every version the host can speak; the child picks one it speaks.
    pub versions: Vec<u32>,
    pub host_version: String,
    /// The directory for files too big for a frame (see `wire::Side`).
    pub side_dir: WPath,
    pub lang: String,
}

/// The answer to [`Hello`], child to host.
// A message value that lives for one send or one receive; boxing would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HelloReply {
    Ack {
        version: u32,
        manifest: WManifest,
    },
    /// The child speaks none of the host's versions.
    Refused {
        supported: Vec<u32>,
        reason: String,
    },
}

/// The state the host reads synchronously on the UI thread, copied into every answer that can
/// change it: the session's `layout()` and `enabled()` of every action the manifest declares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WState {
    pub layout: WLayout,
    pub enabled: Vec<(String, bool)>,
}

/// A call or a notice, host to child. `sid` is a session id (chosen by the host), `rid` a request
/// id (chosen by the host, unique over the connection).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToChild {
    Open {
        sid: u64,
        rid: u64,
        input: WInput,
        cx: WCx,
    },
    View {
        sid: u64,
        rid: u64,
        req: WViewReq,
        cx: WCx,
    },
    Act {
        sid: u64,
        rid: u64,
        action: String,
        cx: WCx,
    },
    Key {
        sid: u64,
        rid: u64,
        key: WKey,
        cx: WCx,
    },
    /// Drop the session. No answer.
    Close { sid: u64 },
    /// Ask the call `rid` to stop (raises its `Cancel`). No answer of its own: the call answers.
    Cancel { rid: u64 },
    /// Finish and exit. No answer.
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Opened {
        state: WState,
    },
    Viewed {
        view: WView,
        state: WState,
    },
    /// The answer of both `Act` and `Key`.
    Acted {
        outcome: WOutcome,
        state: WState,
    },
}

// See `HelloReply`.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Answer {
    Ok(Reply),
    Err(WFailure),
}

/// An answer, child to host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FromChild {
    pub rid: u64,
    pub answer: Answer,
}
