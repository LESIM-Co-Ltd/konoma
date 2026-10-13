//! What a plugin author imports: `use konoma_plugin::prelude::*;`.

pub use crate::manifest::ViewKind;
pub use crate::session::{ActCx, Failure, OpenCx, Outcome, Plugin, Session, ViewCx};
pub use crate::settings::{Settings, Value};
pub use crate::style::{Color, Line, Modifier, Span, Style};
pub use crate::t;
pub use crate::text::Tr;
pub use crate::view::{Layout, ReprDecl, View, ViewReq};
pub use crate::{Act, Bind, Input, Manifest, SettingDecl};
