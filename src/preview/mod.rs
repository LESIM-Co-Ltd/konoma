// Resolution of the preview kind: which display a file gets.
// The config's `[[preview.rules]]` and the standard plugins settle the *who* (`crate::plugins`); the
// answer is a `PreviewKind`, which says how the host draws it. Rendering itself lives in each
// submodule.

pub mod archive;
pub mod code;
pub mod command;
pub mod gitdiff;
pub mod image;
#[cfg(test)]
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

/// What the preview shows, as the host sees it. Four of the variants are chosen for a file by the
/// rules (`Plugin`, `Text`, `Command`, `CanNotPreview`: `Config::resolve_preview`); the other two
/// (`GitDiff`, `MermaidFence`) are modes the host enters by itself. Every `match` that tells the variants apart names all of them (no `_`
/// arm), so a new one cannot be added without the compiler pointing at each decision it changes.
///
/// What a *file* is, beyond this, is the plugin's: a Markdown file, a picture, a workbook and a
/// Word document are all `Plugin`, told apart by the plugin id and by what the plugin's session
/// answered (its display type and layout), never by a variant of their own.
#[derive(Debug, Clone)]
pub enum PreviewKind {
    /// A file shown by a plugin through the plugin API (`docs/FEATURE-PLUGIN-ARCHITECTURE.md`): the
    /// plugin's `Session` returns a display type and the host draws it. The standard plugins
    /// (`markdown`, `mermaid`, `code`, `image`, `svg`, `pdf`, `video`, `csv`, `archive`,
    /// `spreadsheet`, `document`, `presentation`) are all opened this way by
    /// `app/plugin_host.rs`; a picture goes through the host's image pipeline
    /// (`app/media_plugin.rs`) and a document through the host's document container.
    Plugin {
        path: PathBuf,
        /// The plugin id.
        id: String,
        /// The note the binding rule handed over (`"csv"` / `"tsv"` for the table plugin).
        hint: Option<String>,
    },
    /// Built-in plain-text display (extension not registered but judged to be text). Read in
    /// windows by the host; colored by the `code` plugin's styler when the file name gives a
    /// grammar.
    Text(PathBuf),
    /// Git diff preview (opened with Enter in the Git view; unified display, Zed-style coloring).
    /// A mode of the host: not produced from config rules; `open_git_diff` sets it directly.
    GitDiff(PathBuf),
    /// Full-screen view of the Nth ```mermaid fence of the current Markdown preview (Enter on a
    /// Tab-focused inline diagram). A mode of the host: not produced from config rules;
    /// `open_mermaid_fence` sets it. The fence source is re-extracted by ordinal on load
    /// (count-guarded like code-block copy).
    MermaidFence(usize),
    /// External command delegation (`[[preview.rules]] command = "..."`): the built-in `command`
    /// plugin (`plugins/command.rs`) runs it on a worker, bound to this rule. The fields are the
    /// rule's.
    Command {
        path: PathBuf,
        /// Command string with the template not yet expanded (e.g. "mpv {path}").
        template: String,
        /// How to treat the output: if Some("image"), display the produced file full-screen as an image.
        render_as: Option<String>,
        /// If true, open in a separate process and do not block the TUI (videos, etc.). Opened
        /// once, when the preview is entered; never again by a tab switch or a change to the file.
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
    #[cfg(test)]
    pub fn plugin(id: &str, path: PathBuf) -> Self {
        PreviewKind::Plugin {
            path,
            id: id.to_string(),
            hint: None,
        }
    }

    /// What the plugin of this kind declares (all false for any other kind).
    pub fn profile(&self) -> crate::plugins::Profile {
        match self {
            PreviewKind::Plugin { id, .. } => crate::plugins::profile_of(id),
            _ => crate::plugins::Profile::default(),
        }
    }

    /// Whether this is a document the plugin reads from the file as is (a Markdown file): the
    /// host's decorated-document widget draws it, its source view is the file itself, and a
    /// checkbox can be written back to it. Decided by what the plugin declares (it shows a
    /// document and does not convert), never by its id.
    pub fn is_markdown(&self) -> bool {
        let p = self.profile();
        p.document && !p.converts
    }

    /// Whether the host's document container (`App::document`) belongs to this kind: any plugin
    /// that shows a document (a Markdown file, a converted Word document or presentation).
    pub fn is_document_kind(&self) -> bool {
        self.profile().document
    }

    /// Whether the main view of this kind's plugin is text (the host's windowed reader): the
    /// `code` plugin, and any plugin that declares `shows(Text)`.
    pub fn is_text_plugin(&self) -> bool {
        self.profile().text
    }

    /// [`PreviewKind::is_text_plugin`] under its old name, which the existing tests use.
    #[cfg(test)]
    pub fn is_code_plugin(&self) -> bool {
        self.is_text_plugin()
    }

    /// Whether the text view of this kind's plugin is the file itself (read in windows at once).
    pub fn is_file_text(&self) -> bool {
        let p = self.profile();
        p.text && p.file_text
    }

    /// A Markdown file shown by the `markdown` plugin.
    #[cfg(test)]
    pub fn markdown(path: PathBuf) -> Self {
        Self::plugin(konoma_markdown::MARKDOWN_ID, path)
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

    /// Whether this is a document of a plugin that converts the file (Word, a presentation): the
    /// result is Markdown with pictures that no file has the lines of (`app/office_doc.rs`).
    pub fn is_office_document(&self) -> bool {
        let p = self.profile();
        p.document && p.converts
    }

    /// Whether this is a table of a plugin that converts the file (a workbook): it has pages
    /// (sheets) read one at a time, addressed cells, and takes long enough to read for a
    /// "Loading..." screen.
    pub fn is_spreadsheet(&self) -> bool {
        let p = self.profile();
        p.table && p.converts
    }

    /// Whether this is a file the picture plugins show: the ones whose result is an image in the
    /// host's image pipeline (`app/media_plugin.rs`). A `mermaid` file shown as text
    /// (`mode = "text"`, or a terminal with no image backend) never loads a picture; the callers
    /// that start a load check the mode.
    pub fn is_media_plugin(&self) -> bool {
        self.profile().image
    }

    /// Whether the plugin `id` declares that it shows pictures.
    pub fn is_media_plugin_id(id: &str) -> bool {
        crate::plugins::profile_of(id).image
    }

    /// Whether this is a standalone `.mmd` / `.mermaid` file of the standard `mermaid` plugin. The
    /// one place the host knows a diagram plugin by name: its text diagram (`mode = "text"`) and
    /// source view are drawn by the host from the plugin's `fence:mermaid` representation.
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

    /// The kinds are these six, and nothing about a *file type* is among them: a Markdown file, a
    /// picture, a workbook and a Word document are all `Plugin`, told apart by the plugin id. The
    /// `match` has no `_` arm, so a variant added (or one removed) stops this from compiling and
    /// the author has to decide what it means here and at every other exhaustive `match`.
    #[test]
    fn the_kinds_are_the_plugin_the_text_the_command_and_the_hosts_modes() {
        let p = PathBuf::from("/x/f");
        let all = [
            PreviewKind::plugin("markdown", p.clone()),
            PreviewKind::Text(p.clone()),
            PreviewKind::GitDiff(p.clone()),
            PreviewKind::MermaidFence(0),
            PreviewKind::Command {
                path: p.clone(),
                template: "t".into(),
                render_as: None,
                detached: false,
            },
            PreviewKind::can_not_preview(&p),
        ];
        let mut names = Vec::new();
        for k in &all {
            names.push(match k {
                PreviewKind::Plugin { .. } => "plugin",
                PreviewKind::Text(_) => "text",
                PreviewKind::GitDiff(_) => "git-diff",
                PreviewKind::MermaidFence(_) => "mermaid-fence",
                PreviewKind::Command { .. } => "command",
                PreviewKind::CanNotPreview { .. } => "can-not-preview",
            });
        }
        assert_eq!(
            names,
            [
                "plugin",
                "text",
                "git-diff",
                "mermaid-fence",
                "command",
                "can-not-preview"
            ]
        );
    }

    /// What rules can hand out are the four; the two modes never come out of `resolve_preview`.
    #[test]
    fn rules_only_hand_out_the_four_kinds_never_a_mode() {
        let dir = crate::test_support::unique_tmp("konoma_kinds_of_rules");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.preview.rules = vec![crate::config::Rule {
            glob: Some("*.cmd".into()),
            command: Some("true".into()),
            ..Default::default()
        }];
        for (name, body) in [
            ("a.md", "# x\n"),
            ("a.png", "not really"),
            ("a.txt", "text\n"),
            ("a.cmd", "x"),
            ("a.bin", "\0\0\0\0"),
        ] {
            let f = dir.join(name);
            std::fs::write(&f, body).unwrap();
            let kind = cfg.resolve_preview(&f);
            assert!(
                matches!(
                    kind,
                    PreviewKind::Plugin { .. }
                        | PreviewKind::Text(_)
                        | PreviewKind::Command { .. }
                        | PreviewKind::CanNotPreview { .. }
                ),
                "{name}: {kind:?}"
            );
        }
    }

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
