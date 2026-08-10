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
pub mod math;
pub mod pdf;
pub mod svg;
pub mod table;
pub mod text;
pub mod video;
pub mod window;

use std::path::{Path, PathBuf};

use crate::config::Rule;

/// A resolved preview kind. Determined from the config rule (the first one that matched).
#[derive(Debug, Clone)]
pub enum PreviewKind {
    /// Built-in Markdown renderer (decorated by tui-markdown; mermaid fences inside the md are composited via mermaid-text).
    Markdown(PathBuf),
    /// Built-in Mermaid renderer (standalone .mmd/.mermaid files; draws Unicode box lines via mermaid-text).
    Mermaid(PathBuf),
    /// Built-in image renderer (ratatui-image / kitty graphics). GIFs land here and are expanded into animation on the app side.
    Image(PathBuf),
    /// Built-in SVG renderer (rasterizes via resvg/usvg/tiny-skia, then flows into the image path).
    Svg(PathBuf),
    /// Built-in video thumbnail (extracts one representative frame via ffmpegthumbnailer/ffmpeg, then flows into the image path).
    /// Does not play inside the terminal. Missing/failed external tools fall back safely (hint display).
    Video(PathBuf),
    /// Built-in PDF preview: renders the current page natively in Rust (`hayro`, no external tool
    /// needed), then flows into the image path. `J`/`K` turn any page. Falls back — on macOS, for
    /// page 1 only — to the bundled `qlmanage`/`sips` if `hayro` can't render a given PDF
    /// (encrypted/corrupt/unsupported), and to a hint display if nothing can render it (`preview::pdf`).
    Pdf(PathBuf),
    /// Built-in code highlighting (syntect).
    Code(PathBuf),
    /// Built-in CSV/TSV table preview: aligned grid with rainbow columns and a movable cell cursor.
    /// `delimiter` is the field separator byte (`b','` for csv, `b'\t'` for tsv).
    Table { path: PathBuf, delimiter: u8 },
    /// Built-in archive listing (zip / tar / tar.gz / tgz): entries (name/size/modified) rendered
    /// through the same table grid as CSV/TSV. Metadata only — never extracts/decompresses entry
    /// content (see `preview/archive.rs`'s module docs for the security boundary).
    Archive {
        path: PathBuf,
        kind: archive::ArchiveKind,
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
    /// Determine the kind from the matched rule. Prefer builtin, falling back to command.
    /// If neither is present, or the builtin name is unknown, fall back to the safe CanNotPreview.
    pub fn from_rule(rule: &Rule, path: &Path) -> Self {
        let p = path.to_path_buf();
        if let Some(builtin) = rule.builtin.as_deref() {
            return match builtin {
                "markdown" => PreviewKind::Markdown(p),
                "mermaid" => PreviewKind::Mermaid(p),
                "image" => PreviewKind::Image(p),
                "svg" => PreviewKind::Svg(p),
                "video" => PreviewKind::Video(p),
                "pdf" => PreviewKind::Pdf(p),
                "code" => PreviewKind::Code(p),
                // csv = comma / tsv = tab-delimited. Both go to the same table renderer (only the delimiter differs).
                "csv" => PreviewKind::Table {
                    path: p,
                    delimiter: b',',
                },
                "tsv" => PreviewKind::Table {
                    path: p,
                    delimiter: b'\t',
                },
                "archive" => match archive::ArchiveKind::from_path(path) {
                    Some(kind) => PreviewKind::Archive { path: p, kind },
                    None => PreviewKind::can_not_preview(path),
                },
                "text" => PreviewKind::Text(p),
                _ => PreviewKind::can_not_preview(path),
            };
        }
        if let Some(template) = rule.command.as_deref() {
            return PreviewKind::Command {
                path: p,
                template: template.to_string(),
                render_as: rule.render_as.clone(),
                detached: rule.detached,
            };
        }
        PreviewKind::can_not_preview(path)
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
/// downstream opener (`start_media_load` / `setup_windowed` / `load_table`, and the render
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
mod tests {
    use super::*;

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

        std::fs::remove_dir_all(&dir).ok();
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
        std::fs::remove_dir_all(&dir).ok();
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

        std::fs::remove_dir_all(&dir).ok();
    }
}
