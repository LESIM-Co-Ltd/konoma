//! What a plugin author imports: `use konoma_plugin::prelude::*;`.

pub use crate::session::{ActCx, Failure, OpenCx, Outcome, Plugin, Session, ViewCx};
pub use crate::style::{Color, Line, Span, Style};
pub use crate::t;
pub use crate::text::Tr;
pub use crate::view::{View, ViewReq};
pub use crate::{Act, Bind, Input, Manifest};
