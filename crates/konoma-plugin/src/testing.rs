//! Checks that do not start the host.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::hints::hints;
use crate::input::Input;
use crate::keys::{self, KeyIssue};
use crate::session::{Failure, OpenCx, Plugin, Session, ViewCx};
use crate::text::Tr;
use crate::view::{DocumentView, ImageView, TextSource, View, ViewReq};

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

/// One problem found in a manifest by [`check_manifest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestIssue {
    /// A key problem (see [`keys::validate`]).
    Key(KeyIssue),
    /// The plugin is given files by `binds` but declares no `shows`: the host would attach no
    /// screen to them (the same finding the registry reports at startup).
    ShowsNotDeclared,
}

/// The problems of a plugin's manifest: the key problems (see [`keys::validate`]) and files
/// assigned without a declared `shows`. Empty means the host reads the manifest as intended.
pub fn check_manifest(plugin: &dyn Plugin) -> Vec<ManifestIssue> {
    let manifest = plugin.manifest();
    let mut out: Vec<ManifestIssue> = keys::validate(&manifest)
        .into_iter()
        .map(ManifestIssue::Key)
        .collect();
    if manifest.assigns_files_without_shows() {
        out.push(ManifestIssue::ShowsNotDeclared);
    }
    out
}

/// The table view of a session as text: the header (column titles; empty strings when the grid has
/// none) and every cell by row. A view of another type is an error. At most `max_rows` rows are
/// read, so a test never walks a huge grid.
pub fn view_table(
    session: &mut dyn Session,
    req: &ViewReq,
    max_rows: usize,
) -> Result<TableText, Failure> {
    match session.view(req, &ViewCx::bare())? {
        View::Table(t) => {
            let g = &t.grid;
            let header = (0..g.ncols())
                .map(|c| g.header(c).unwrap_or_default())
                .collect();
            let rows = (0..g.nrows().min(max_rows))
                .map(|r| (0..g.ncols()).map(|c| g.cell(r, c)).collect())
                .collect();
            Ok(TableText {
                header,
                rows,
                title: t.title,
            })
        }
        other => {
            Err(Failure::unsupported(Tr::new("not a table view")).with_detail(format!("{other:?}")))
        }
    }
}

/// What [`view_table`] reads from a table view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableText {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub title: Option<String>,
}

/// The document view of a session (Markdown plus resources). A view of another type is an error.
pub fn view_document(session: &mut dyn Session, req: &ViewReq) -> Result<DocumentView, Failure> {
    match session.view(req, &ViewCx::bare())? {
        View::Document(d) => Ok(d),
        other => {
            Err(Failure::unsupported(Tr::new("not a document view"))
                .with_detail(format!("{other:?}")))
        }
    }
}

/// The image view of a session. A view of another type is an error.
pub fn view_image(session: &mut dyn Session, req: &ViewReq) -> Result<ImageView, Failure> {
    match session.view(req, &ViewCx::bare())? {
        View::Image(i) => Ok(i),
        other => {
            Err(Failure::unsupported(Tr::new("not an image view"))
                .with_detail(format!("{other:?}")))
        }
    }
}

/// A file made from bytes held in memory, removed when dropped. Keep it alive as long as the
/// session that was opened on it.
#[derive(Debug)]
pub struct Scratch {
    dir: PathBuf,
    file: PathBuf,
}

impl Scratch {
    /// The path of the file.
    pub fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Open a plugin on bytes held in memory, as if they were a file called `file_name` (the name
/// decides nothing for the plugin itself, but plugins read their settings from it). Returns the
/// session and the [`Scratch`] file that backs it.
pub fn open_bytes(
    plugin: &dyn Plugin,
    file_name: &str,
    bytes: &[u8],
) -> Result<(Box<dyn Session>, Scratch), Failure> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "konoma_plugin_scratch_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir)?;
    // The name is a file name, never a path out of the directory.
    let leaf = Path::new(file_name)
        .file_name()
        .map_or_else(|| "input".to_string(), |n| n.to_string_lossy().into_owned());
    let file = dir.join(leaf);
    let scratch = Scratch { dir, file };
    std::fs::write(&scratch.file, bytes)?;
    let session = open_file(plugin, &scratch.file)?;
    Ok((session, scratch))
}

/// What [`check_hints`] found wrong with the hints of a plugin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HintReport {
    /// Keys that collide with a reserved key, with another action, or that cannot be parsed.
    pub key_issues: Vec<KeyIssue>,
    /// Actions the session says are not enabled that the host would still show.
    pub shown_while_disabled: Vec<String>,
    /// Actions that are enabled but would be shown without a label or help text.
    pub unlabelled: Vec<String>,
}

impl HintReport {
    /// Whether nothing is wrong.
    pub fn is_clean(&self) -> bool {
        self.key_issues.is_empty()
            && self.shown_while_disabled.is_empty()
            && self.unlabelled.is_empty()
    }
}

/// Check the hints the host would build from a plugin's declarations for an open session, with
/// the **same rules the host uses**: the keys are checked against the layers it reserves
/// ([`keys::validate`]), and what is shown is what [`hints`] answers, which is every action the
/// session says is [`enabled`](Session::enabled) and nothing else.
pub fn check_hints(plugin: &dyn Plugin, session: &dyn Session) -> HintReport {
    let manifest = plugin.manifest();
    let key_issues = keys::validate(&manifest);
    let shown = hints(&manifest, &|id| session.enabled(id));
    let (clean, _) = keys::sanitize(&manifest);
    let mut report = HintReport {
        key_issues,
        ..HintReport::default()
    };
    for act in &clean.actions {
        let is_shown = shown.iter().any(|h| h.act == act.id);
        if !session.enabled(&act.id) && is_shown {
            report.shown_while_disabled.push(act.id.clone());
        }
        if is_shown && (act.label.en().is_empty() || act.help.en().is_empty()) {
            report.unlabelled.push(act.id.clone());
        }
    }
    report
}
