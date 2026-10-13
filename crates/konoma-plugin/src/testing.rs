//! Checks that do not start the host.

use std::path::Path;

use crate::input::Input;
use crate::keys::{self, KeyIssue};
use crate::session::{Failure, OpenCx, Plugin, Session, ViewCx};
use crate::text::Tr;
use crate::view::{TextSource, View, ViewReq};

/// Open a file with a plugin using default (English, no settings, no pixels) contexts.
pub fn open_file(plugin: &dyn Plugin, path: &Path) -> Result<Box<dyn Session>, Failure> {
    plugin.open(
        Input::File {
            path: path.to_path_buf(),
            hint: None,
        },
        &OpenCx::bare(),
    )
}

/// The text view of a session as plain lines (styles dropped). A view of another type is an
/// error, so a test notices a plugin that returns something else.
pub fn view_text(session: &mut dyn Session, req: &ViewReq) -> Result<Vec<String>, Failure> {
    match session.view(req, &ViewCx::bare())? {
        View::Text(t) => match t.source {
            TextSource::Lines(lines) => Ok(lines.iter().map(|l| l.plain()).collect()),
            TextSource::Owned(s) => Ok(s.lines().map(str::to_string).collect()),
            TextSource::File(p) => Ok(std::fs::read_to_string(p)?
                .lines()
                .map(str::to_string)
                .collect()),
        },
        other => {
            Err(Failure::unsupported(Tr::new("not a text view")).with_detail(format!("{other:?}")))
        }
    }
}

/// The key problems of a plugin's manifest (see [`keys::validate`]).
pub fn check_manifest(plugin: &dyn Plugin) -> Vec<KeyIssue> {
    keys::validate(&plugin.manifest())
}
