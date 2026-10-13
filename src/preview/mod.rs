// Resolution of preview kind and renderer selection.
// Config rules are resolved into a PreviewKind and dispatched to each renderer (builtin / external
// delegation). M0 implements only the kind resolution. Actual rendering is added incrementally in
// each submodule (M2: image / M3: markdown / later: code, command).

pub mod archive;
pub mod code;
pub mod command;
pub mod gitdiff;
pub mod image;
pub mod kitty;
pub mod markdown;
#[cfg(test)]
pub mod math;
pub mod media_diff;
pub mod mermaid;
pub mod office;
#[cfg(test)]
mod office_host_tests;
pub mod pdf;
pub(crate) use konoma_base::private_dir;
pub mod svg;
pub(crate) mod svg_guard;
pub(crate) mod svg_proc;
#[cfg(test)]
mod svg_proc_tests;
#[cfg(test)]
mod svg_size;
pub mod table;
pub mod text;
pub mod window;

use std::path::{Path, PathBuf};

/// A resolved preview kind. Determined from the config rule (the first one that matched).
#[derive(Debug, Clone)]
pub enum PreviewKind {
    /// A file shown by a plugin through the plugin API (`docs/FEATURE-PLUGIN-ARCHITECTURE.md`): the
    /// plugin's `Session` returns a display type and the host draws it. The plugins wired so far
    /// are `csv` (CSV/TSV as a grid; `hint` is `"csv"` or `"tsv"`, the old `builtin` name) and
    /// `archive` (zip / tar / tar.gz listing as a grid), both opened by `app/plugin_host.rs`,
    /// `code` (source and config files as colored text: the host reads the file in windows and the
    /// plugin's styler colors the visible lines), and
    /// the picture plugins `image`, `svg`, `pdf`, `video` and `mermaid`, whose `ImageView` goes
    /// through the host's image pipeline (`app/media_plugin.rs`; a `.mmd` file shown as text is
    /// drawn by the host's decorated path from the plugin's text view). Transitional: it grows as the other kinds
    /// move onto plugins, and the enum goes when the last one has.
    Plugin {
        path: PathBuf,
        /// The plugin id.
        id: String,
        /// The note the binding rule handed over.
        hint: Option<String>,
    },
    /// Built-in plain-text display (extension not registered but judged to be text).
    Text(PathBuf),
    /// Git diff preview (opened with Enter in the Git view; unified display, Zed-style coloring).
    /// Not produced from config rules; `open_git_diff` sets it directly.
    GitDiff(PathBuf),
    /// Full-screen view of the Nth ```mermaid fence of the current Markdown preview (Enter on a
    /// Tab-focused inline diagram). Not produced from config rules; `open_mermaid_fence` sets it.
    /// The fence source is re-extracted by ordinal on load (count-guarded like code-block copy).
    MermaidFence(usize),
    /// External command delegation. Expands {path}/{out} and runs a child process.
    Command {
        path: PathBuf,
        /// Command string with the template not yet expanded (e.g. "mpv {path}").
        template: String,
        /// How to treat the output: if Some("image"), display the produced file full-screen as an image.
        render_as: Option<String>,
        /// If true, open in a separate process and do not block the TUI (videos, etc.).
        detached: bool,
    },
    /// Matches no rule / unsupported. Displays `[can not preview: <ext>]` full-screen.
    CanNotPreview { ext: String },
}

impl PreviewKind {
    /// Whether this is a file shown by the plugin `id`.
    pub fn is_plugin(&self, id: &str) -> bool {
        matches!(self, PreviewKind::Plugin { id: own, .. } if own == id)
    }

    /// A file shown by the plugin `id` (no note).
    pub fn plugin(id: &str, path: PathBuf) -> Self {
        PreviewKind::Plugin {
            path,
            id: id.to_string(),
            hint: None,
        }
    }

    /// Whether this is a file the `markdown` plugin shows (its document is drawn by the host's
    /// decorated-document widget).
    pub fn is_markdown(&self) -> bool {
        self.is_plugin(konoma_markdown::MARKDOWN_ID)
    }

    /// Whether the host's document container (`App::document`) belongs to this kind: a Markdown
    /// file, or a Word document or presentation (`is_office_document`).
    pub fn is_document_kind(&self) -> bool {
        self.is_markdown() || self.is_office_document()
    }

    /// A Markdown file shown by the `markdown` plugin.
    #[cfg(test)]
    pub fn markdown(path: PathBuf) -> Self {
        Self::plugin(konoma_markdown::MARKDOWN_ID, path)
    }

    /// Whether this is a file the `code` plugin shows.
    pub fn is_code_plugin(&self) -> bool {
        self.is_plugin(konoma_code::CODE_ID)
    }

    /// A picture file shown by the `image` plugin.
    #[cfg(test)]
    pub fn image(path: PathBuf) -> Self {
        Self::plugin(konoma_media::IMAGE_ID, path)
    }

    /// An SVG file shown by the `svg` plugin.
    #[cfg(test)]
    pub fn svg(path: PathBuf) -> Self {
        Self::plugin(konoma_media::SVG_ID, path)
    }

    /// A standalone mermaid file shown by the `mermaid` plugin.
    #[cfg(test)]
    pub fn mermaid(path: PathBuf) -> Self {
        Self::plugin(konoma_mermaid::MERMAID_ID, path)
    }

    /// A video file shown by the `video` plugin.
    #[cfg(test)]
    pub fn video(path: PathBuf) -> Self {
        Self::plugin(konoma_media::VIDEO_ID, path)
    }

    /// A PDF file shown by the `pdf` plugin.
    #[cfg(test)]
    pub fn pdf(path: PathBuf) -> Self {
        Self::plugin(konoma_media::PDF_ID, path)
    }

    /// A workbook shown by the `spreadsheet` plugin.
    #[cfg(test)]
    pub fn spreadsheet(path: PathBuf) -> Self {
        Self::plugin(konoma_office::SPREADSHEET_ID, path)
    }

    /// A Word document shown by the `document` plugin.
    #[cfg(test)]
    pub fn document(path: PathBuf) -> Self {
        Self::plugin(konoma_office::DOCUMENT_ID, path)
    }

    /// A presentation shown by the `presentation` plugin.
    #[cfg(test)]
    pub fn presentation(path: PathBuf) -> Self {
        Self::plugin(konoma_office::PRESENTATION_ID, path)
    }

    /// The Word or presentation plugin's kind for `path`, by its extension (what the registry
    /// settles for the default rules).
    #[cfg(test)]
    pub fn office_document(path: PathBuf) -> Self {
        if crate::app::is_presentation_path(&path) {
            Self::presentation(path)
        } else {
            Self::document(path)
        }
    }

    /// Whether this is a file the `document` (Word) or `presentation` plugin shows: the ones whose
    /// result is a converted Markdown document with pictures (`app/office_doc.rs`).
    pub fn is_office_document(&self) -> bool {
        matches!(self, PreviewKind::Plugin { id, .. } if Self::is_office_document_id(id))
    }

    /// Whether `id` names the Word or the presentation plugin.
    pub fn is_office_document_id(id: &str) -> bool {
        id == konoma_office::DOCUMENT_ID || id == konoma_office::PRESENTATION_ID
    }

    /// Whether this is a workbook the `spreadsheet` plugin shows.
    pub fn is_spreadsheet(&self) -> bool {
        self.is_plugin(konoma_office::SPREADSHEET_ID)
    }

    /// Whether this is a file the picture plugins show (`image`, `svg`, `pdf`, `video`, `mermaid`):
    /// the ones whose result is an image in the host's image pipeline. A `mermaid` file shown as
    /// text (`mode = "text"`, or a terminal with no image backend) never loads a picture; the
    /// callers that start a load check the mode.
    pub fn is_media_plugin(&self) -> bool {
        matches!(self, PreviewKind::Plugin { id, .. } if Self::is_media_plugin_id(id))
    }

    /// Whether `id` names one of the picture plugins (`image`, `svg`, `pdf`, `video`, `mermaid`).
    pub fn is_media_plugin_id(id: &str) -> bool {
        matches!(
            id,
            konoma_media::IMAGE_ID
                | konoma_media::SVG_ID
                | konoma_media::PDF_ID
                | konoma_media::VIDEO_ID
                | konoma_mermaid::MERMAID_ID
        )
    }

    /// Whether this is a standalone `.mmd` / `.mermaid` file (the `mermaid` plugin's).
    pub fn is_mermaid_file(&self) -> bool {
        self.is_plugin(konoma_mermaid::MERMAID_ID)
    }

    /// Unsupported fallback. Displays safely with the extension attached.
    pub fn can_not_preview(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        PreviewKind::CanNotPreview { ext }
    }
}

/// Whether `path` is safe to hand to a preview opener: it exists and is a **regular file**
/// (symlinks are followed, so a symlink to a regular file still previews normally — same as
/// before this check existed). A directory, FIFO, character/block device, socket, or missing path
/// all fail this and must degrade to `CanNotPreview` instead of ever reaching `File::open`.
///
/// This is a `stat`-only check (`std::fs::metadata`), which never blocks on any file type —
/// unlike `File::open`, which for a FIFO with no writer on the other end blocks the calling
/// thread until one shows up (POSIX `open(2)`). `enter_preview` calls this **before** resolving
/// the preview kind (`Config::resolve_preview`), because the kind resolution itself can already
/// open the file to sniff it (the mime-glob rule match via `infer::get_from_path`, and the
/// no-rule-matched fallback via `text::is_probably_text`) — so the hang could happen before a
/// `PreviewKind` was even chosen, not only inside the Text/Code windowed reader. Since every
/// downstream opener (`start_media_load` / `setup_windowed` / `load_plugin`, and the render
/// side's raw-text fallbacks) dispatches purely on the resolved `PreviewKind`, forcing it to
/// `CanNotPreview` here closes every `File::open` path in the one place all of them share —
/// entering preview on a FIFO or device (`konoma /dev`, Enter on a named pipe, following a
/// pasted/bookmarked/followed path onto one, paging `Ctrl-n`/`Ctrl-p` past one, restoring a
/// session that pointed at one, ...) can no longer hang the key-processing thread.
pub fn is_previewable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod hostile_tests;

#[cfg(test)]
mod raster_limits_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_send_every_spreadsheet_extension_to_the_spreadsheet_viewer() {
        // The rule is a glob (not a mime), so the kind resolves from the *name* — a deleted file
        // (diff of a removed workbook) still gets it. None of these paths exist.
        let cfg = crate::config::Config::default();
        for name in [
            "a.xlsx",
            "a.xlsm",
            "a.xltx",
            "a.xltm",
            "a.xlsb",
            "a.xls",
            "a.ods",
            "A.XLSX",
            "dir.v2/b.Ods",
        ] {
            match cfg.resolve_preview(Path::new(&format!("/nonexistent/{name}"))) {
                PreviewKind::Plugin { id, path: p, .. } if id == "spreadsheet" => {
                    assert!(p.ends_with(name), "{p:?}")
                }
                other => panic!("{name}: Spreadsheet を期待: {other:?}"),
            }
        }
        // Not spreadsheets: a near-miss extension and the csv/zip rules keep their own kinds.
        assert!(!cfg
            .resolve_preview(Path::new("/nonexistent/a.xlsxx"))
            .is_spreadsheet());
        assert!(cfg
            .resolve_preview(Path::new("/nonexistent/a.csv"))
            .is_plugin("csv"));
        assert!(cfg
            .resolve_preview(Path::new("/nonexistent/a.zip"))
            .is_plugin("archive"));
    }

    #[test]
    fn spreadsheet_builtin_name_resolves_and_a_user_rule_can_override_it() {
        use crate::config::Rule;
        // `builtin = "spreadsheet"` names the viewer for whatever the rule's glob catches.
        let mut named = crate::config::Config::default();
        named.preview.rules.push(Rule {
            glob: Some("*.zzz".into()),
            builtin: Some("spreadsheet".into()),
            ..Rule::default()
        });
        assert!(named
            .resolve_preview(Path::new("/x/a.zzz"))
            .is_spreadsheet());
        // A user rule placed first wins (rules are evaluated top to bottom): delegate .xlsx instead.
        let mut cfg = crate::config::Config::default();
        cfg.preview.rules.insert(
            0,
            Rule {
                glob: Some("*.xlsx".into()),
                command: Some("echo {path}".into()),
                ..Rule::default()
            },
        );
        assert!(matches!(
            cfg.resolve_preview(Path::new("/nonexistent/a.xlsx")),
            PreviewKind::Command { .. }
        ));
    }

    #[test]
    fn can_not_preview_captures_extension_or_empty() {
        // With an extension: keeps ext.
        match PreviewKind::can_not_preview(Path::new("/x/foo.xyz")) {
            PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "xyz"),
            other => panic!("CanNotPreview を期待: {other:?}"),
        }
        // Without an extension: empty string (does not crash).
        match PreviewKind::can_not_preview(Path::new("/x/Makefile")) {
            PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "", "拡張子なしは空"),
            other => panic!("CanNotPreview を期待: {other:?}"),
        }
    }

    #[test]
    fn is_previewable_accepts_regular_files_and_symlinks_to_them() {
        use crate::test_support::unique_tmp;
        let dir = unique_tmp("konoma_is_previewable_regular");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("real.txt");
        std::fs::write(&file, b"hello").unwrap();
        assert!(is_previewable(&file), "通常ファイルは true");

        #[cfg(unix)]
        {
            let link = dir.join("link.txt");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert!(
                is_previewable(&link),
                "通常ファイルへのシンボリックリンクは追従して true (既存の挙動を壊さない)"
            );
        }
    }

    #[test]
    fn is_previewable_rejects_directories_and_missing_paths() {
        use crate::test_support::unique_tmp;
        let dir = unique_tmp("konoma_is_previewable_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_previewable(&dir), "ディレクトリは false");
        assert!(
            !is_previewable(&dir.join("does_not_exist")),
            "存在しないパスは false"
        );
    }

    /// A FIFO is not a regular file, so this must return `false` — and it must do so
    /// **instantly**: `std::fs::metadata` is `stat(2)`, which (unlike `File::open`'s `open(2)`
    /// for a read-only FIFO) never blocks waiting for a writer, even though nothing ever writes
    /// to this pipe in the test. If this ever regressed to opening the path, the test itself
    /// would hang here.
    #[cfg(unix)]
    #[test]
    fn is_previewable_rejects_a_fifo_without_blocking() {
        use crate::test_support::unique_tmp;
        let dir = unique_tmp("konoma_is_previewable_fifo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo コマンドを起動できない");
        assert!(status.success(), "mkfifo に失敗");

        assert!(!is_previewable(&fifo), "FIFO(通常ファイルでない)は false");

        // A symlink to the FIFO must also be rejected (following the link doesn't launder its type).
        let link = dir.join("link_to_pipe");
        std::os::unix::fs::symlink(&fifo, &link).unwrap();
        assert!(
            !is_previewable(&link),
            "FIFO へのシンボリックリンクも追従した先の種別で false"
        );
    }
}
