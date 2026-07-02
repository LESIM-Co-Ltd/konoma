// プレビュー方式の解決とレンダラ選択。
// 設定ルール → PreviewKind に落とし込み、各レンダラ(内蔵/外部委譲)へ振り分ける。
// M0 では種別の解決のみを実装。実描画は各サブモジュールで段階的に実装する
// (M2: image / M3: markdown / 以降: code・command)。

pub mod code;
pub mod command;
pub mod gitdiff;
pub mod image;
pub mod markdown;
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
    /// Built-in PDF preview (rasterizes the first page via pdftocairo/pdftoppm/qlmanage/sips, then flows into the image path).
    /// Missing tools fall back safely (hint display). First page only.
    Pdf(PathBuf),
    /// Built-in code highlighting (syntect).
    Code(PathBuf),
    /// Built-in CSV/TSV table preview: aligned grid with rainbow columns and a movable cell cursor.
    /// `delimiter` is the field separator byte (`b','` for csv, `b'\t'` for tsv).
    Table { path: PathBuf, delimiter: u8 },
    /// Built-in plain-text display (extension not registered but judged to be text).
    Text(PathBuf),
    /// Git diff preview (opened with Enter in the Git view; unified display, Zed-style coloring).
    /// Not produced from config rules; `open_git_diff` sets it directly.
    GitDiff(PathBuf),
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
                // csv=カンマ / tsv=タブ区切り。どちらも同じテーブルレンダラへ(区切り文字だけ違う)。
                "csv" => PreviewKind::Table {
                    path: p,
                    delimiter: b',',
                },
                "tsv" => PreviewKind::Table {
                    path: p,
                    delimiter: b'\t',
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_not_preview_captures_extension_or_empty() {
        // 拡張子つき: ext を保持する。
        match PreviewKind::can_not_preview(Path::new("/x/foo.xyz")) {
            PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "xyz"),
            other => panic!("CanNotPreview を期待: {other:?}"),
        }
        // 拡張子なし: 空文字(クラッシュしない)。
        match PreviewKind::can_not_preview(Path::new("/x/Makefile")) {
            PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "", "拡張子なしは空"),
            other => panic!("CanNotPreview を期待: {other:?}"),
        }
    }
}
