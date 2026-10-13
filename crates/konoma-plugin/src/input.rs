//! What a plugin is asked to open.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::session::{Cancel, Failure};
use crate::text::Tr;
use crate::view::ImageData;

/// The thing to open.
#[derive(Clone, Debug)]
pub enum Input {
    /// A file on disk. `hint` is a note from the binding rule (for example `"tsv"` from an old
    /// `builtin = "tsv"`, or a command template).
    File { path: PathBuf, hint: Option<String> },
    /// Content handed over by another plugin (a converted document, say).
    Content {
        content_type: String,
        text: String,
        resources: Resources,
        origin: Origin,
    },
    /// An embedded thing: a fenced block or an inline formula. `display` is true for a block.
    Embed {
        kind: String,
        source: String,
        display: bool,
    },
}

impl Input {
    /// Read the whole file. Only a [`Input::File`] can be read.
    pub fn read_all(&self) -> Result<Vec<u8>, Failure> {
        match self {
            Input::File { path, .. } => Ok(std::fs::read(path)?),
            _ => Err(Failure::unsupported(Tr::new(
                "only a file input can be read as bytes",
            ))),
        }
    }
}

/// Where a document came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Read from this file as is.
    File(PathBuf),
    /// Converted from this file (cannot be written back).
    Converted { from: PathBuf },
}

/// Named resources (images and the like) that a document refers to.
#[derive(Clone, Debug, Default)]
pub struct Resources(BTreeMap<String, Resource>);

impl Resources {
    pub fn new() -> Resources {
        Resources::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, r: Resource) {
        self.0.insert(name.into(), r);
    }

    pub fn get(&self, name: &str) -> Option<&Resource> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Resource)> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One resource: bytes now, or something made when it becomes visible.
#[derive(Clone, Debug)]
pub enum Resource {
    Bytes(Arc<[u8]>),
    Lazy(Arc<dyn LazyResource>),
}

/// A resource made on demand, at the size the host asks for, with cancellation (a slide drawn
/// when it scrolls into view). A failure kind lets the host decide to ask again smaller:
/// [`crate::FailureKind::TooHeavy`] and [`crate::FailureKind::Timeout`].
pub trait LazyResource: Send + Sync + std::fmt::Debug {
    /// The natural size in pixels, if known without drawing.
    fn natural_size(&self) -> Option<(u32, u32)>;
    /// Draw it. Check `cancel` between steps.
    fn render(&self, req: &ResourceReq, cancel: &Cancel) -> Result<ImageData, Failure>;
}

/// What the host asks of a [`LazyResource`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceReq {
    /// The longest side, in pixels.
    pub max_px: u32,
}
