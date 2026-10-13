//! konoma's `markdown` plugin: Markdown files, and every document another plugin converts to
//! Markdown (a Word document), shown as a **document**.
//!
//! The plugin returns a [`DocumentView`]: the Markdown text (CommonMark and GFM with konoma's
//! extensions), the resources it refers to (pictures of a converted document) and the options it
//! is drawn with. It never draws. The host's document widget parses and draws the text, and owns
//! everything that works on it: the decoration, inline pictures and the embedded diagrams and
//! formulas, `Tab` focus on links, checkboxes and code blocks, the outline, folding, search, the
//! rendered diff, and writing a checkbox back to the file. Only a document that came from a file
//! can be written back ([`Origin::File`]); the options of a converted one say so.
//!
//! What the plugin owns is therefore small: which files are Markdown, how a file is read (the
//! preview limits), the mapping of `[plugins.markdown]` to the options, and the two
//! representations of the same text (`R`): the rendered document and its source.
//!
//! - **Rendered** (the default): a [`View::Document`].
//! - **Source**: a [`View::Text`] the host shows in its windowed text reader. For a file it is the
//!   file itself ([`TextSource::File`], line numbers are the file's); for a converted document it
//!   is the converted text ([`TextSource::Owned`]; there is no file with those lines).
//!
//! Two inputs open it: `Input::File` (a `.md` file) and `Input::Content` of the content type
//! `markdown` (what a Word plugin hands over to be shown). Because the host looks the content
//! type up in the registry, a plugin that takes `markdown` in place of this one changes how Word
//! documents look as well.
//!
//! Settings (`[plugins.markdown]`, or the old `[ui] md_*` names the host fills in; the new name
//! wins): `task_states`, `autolink`, `alerts`, `emoji`, `frontmatter`, `footnotes`, `inline_html`,
//! `details`, `table_align`, `image_align`.

use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;

use konoma_plugin::view::{
    DocOptions, DocumentView, Layout, ReprDecl, TextSource, TextView, View, ViewReq,
};
use konoma_plugin::{
    t, Bind, Failure, Input, LaunchMode, Launcher, Manifest, OpenCx, Origin, Plugin, Resources,
    Session, SettingDecl, Settings, Value, Verb, VerbDecl, ViewCx,
};

/// The id of the plugin.
pub const MARKDOWN_ID: &str = "markdown";

/// The content type the plugin takes from other plugins.
pub const CONTENT_TYPE: &str = "markdown";

/// The representation that draws the text as a document (the default).
pub const REPR_RENDERED: &str = "rendered";

/// The representation that shows the text as it is written.
pub const REPR_SOURCE: &str = "source";

/// The most bytes of a file the preview reads. A longer file is cut and says so.
pub const MAX_BYTES: usize = 1024 * 1024;

/// The most lines of a file the preview shows. A longer file is cut and says so.
pub const MAX_LINES: usize = 5000;

/// The size of one read.
const CHUNK: usize = 8 * 1024;

/// The declarations of the `markdown` plugin.
pub fn markdown_manifest() -> Manifest {
    let s = |v: &str| Value::Str(v.to_string());
    Manifest::new(MARKDOWN_ID, env!("CARGO_PKG_VERSION"))
        .name(t!(en: "Markdown", ja: "Markdown"))
        .bind(Bind::files(["*.md"]))
        .bind(Bind::content(CONTENT_TYPE))
        .setting(SettingDecl::new(
            "task_states",
            Value::List(vec![s(" "), s("x")]),
            t!(en: "Checkbox states Space cycles through", ja: "Space で巡回するチェックボックスの状態"),
        ))
        .setting(SettingDecl::new(
            "autolink",
            Value::Bool(true),
            t!(en: "Turn bare URLs into links", ja: "裸の URL をリンクにする"),
        ))
        .setting(SettingDecl::new(
            "alerts",
            Value::Bool(true),
            t!(en: "Draw GitHub alerts (> [!NOTE])", ja: "GitHub のアラート(> [!NOTE])を描く"),
        ))
        .setting(SettingDecl::new(
            "emoji",
            Value::Bool(true),
            t!(en: "Replace :shortcode: emoji", ja: ":shortcode: の絵文字を置き換える"),
        ))
        .setting(SettingDecl::new(
            "frontmatter",
            Value::Bool(true),
            t!(en: "Show front matter as a block", ja: "front matter をブロックとして表示する"),
        ))
        .setting(SettingDecl::new(
            "footnotes",
            Value::Bool(true),
            t!(en: "Draw footnotes", ja: "脚注を描く"),
        ))
        .setting(SettingDecl::new(
            "inline_html",
            Value::Bool(true),
            t!(en: "Render inline HTML", ja: "インライン HTML を描く"),
        ))
        .setting(SettingDecl::new(
            "details",
            s("auto"),
            t!(en: "<details> folding: auto, open or closed", ja: "<details> の折りたたみ: auto・open・closed"),
        ))
        .setting(SettingDecl::new(
            "table_align",
            s("left"),
            t!(en: "Table alignment: left, center or right", ja: "表の寄せ: left・center・right"),
        ))
        .setting(SettingDecl::new(
            "image_align",
            s("center"),
            t!(en: "Image alignment: left, center or right", ja: "画像の寄せ: left・center・right"),
        ))
        // `edit` opens the user's editor in the terminal; `open` hands the file to the OS default
        // application as a GUI program.
        .verb(
            VerbDecl::new(Verb::Edit)
                .launcher(Launcher::Editor)
                .mode(LaunchMode::Terminal)
                .line_token(),
        )
        .verb(
            VerbDecl::new(Verb::Open)
                .launcher(Launcher::OsDefault)
                .mode(LaunchMode::Gui),
        )
}

/// The options a document is drawn with, from the plugin's settings and where the text came from.
/// A converted document has no front matter of its own to show and cannot be written back (there
/// is no file with those lines).
pub fn doc_options(settings: &Settings, origin: &Origin) -> DocOptions {
    let mut options = DocOptions::from_settings(settings);
    if matches!(origin, Origin::Converted { .. }) {
        options.front_matter = false;
        options.tasks_writable = false;
    }
    options
}

/// Read a file the way the preview does: at most [`MAX_BYTES`] bytes, at most [`MAX_LINES`] lines,
/// bytes that are not UTF-8 replaced. Returns the lines joined with `\n` (the line ends of the file
/// are not kept) and whether the file was cut.
pub fn read_limited(path: &Path) -> std::io::Result<(String, bool)> {
    let mut f = std::fs::File::open(path)?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; CHUNK];
    let mut cut = false;
    loop {
        let n = f.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len() + n > MAX_BYTES {
            let take = MAX_BYTES - buf.len();
            buf.extend_from_slice(&chunk[..take]);
            cut = true;
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        cut = true;
    }
    Ok((lines.join("\n"), cut))
}

/// The Markdown plugin.
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownPlugin;

impl Plugin for MarkdownPlugin {
    fn manifest(&self) -> Manifest {
        markdown_manifest()
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        match input {
            Input::File { path, .. } => {
                let (markdown, truncated) = read_limited(&path).map_err(|e| {
                    // The words the preview has always shown for a file it could not read.
                    Failure::io(t!(
                        en: format!("[can not preview: could not read it] {e}"),
                        ja: format!("[can not preview: 読み込み失敗] {e}"),
                    ))
                    .with_detail(e.to_string())
                })?;
                let origin = Origin::File(path);
                Ok(Box::new(MarkdownSession {
                    options: doc_options(cx.settings, &origin),
                    markdown,
                    resources: Resources::new(),
                    origin,
                    truncated,
                }))
            }
            Input::Content {
                content_type,
                text,
                resources,
                origin,
            } if content_type == CONTENT_TYPE => Ok(Box::new(MarkdownSession {
                options: doc_options(cx.settings, &origin),
                markdown: text,
                resources,
                origin,
                truncated: false,
            })),
            _ => Err(Failure::unsupported(t!(
                en: "this plugin opens Markdown files and Markdown content only",
                ja: "このプラグインは Markdown のファイルと Markdown の内容だけを開きます",
            ))),
        }
    }
}

/// One opened Markdown text. It keeps the text, so the other representation (and a return to the
/// rendered one) is answered without reading the file again.
pub struct MarkdownSession {
    markdown: String,
    resources: Resources,
    origin: Origin,
    options: DocOptions,
    truncated: bool,
}

impl MarkdownSession {
    /// The file the text came from, if it is one.
    fn file(&self) -> Option<PathBuf> {
        match &self.origin {
            Origin::File(p) => Some(p.clone()),
            Origin::Converted { .. } => None,
        }
    }
}

impl Session for MarkdownSession {
    fn layout(&self) -> Layout {
        Layout {
            reprs: vec![
                ReprDecl::new(REPR_RENDERED, t!(en: "Rendered", ja: "整形表示")),
                ReprDecl::new(REPR_SOURCE, t!(en: "Source", ja: "ソース")),
            ],
            default_repr: Some(REPR_RENDERED.to_string()),
            default_repr_without_pixels: None,
            pages: None,
        }
    }

    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        if req.repr.as_deref() == Some(REPR_SOURCE) {
            let (source, file_lines) = match self.file() {
                Some(path) => (TextSource::File(path), true),
                None => (TextSource::Owned(self.markdown.clone()), false),
            };
            return Ok(View::Text(TextView {
                source,
                styler: None,
                file_lines,
                syntax_name: Some("markdown".to_string()),
            }));
        }
        Ok(View::Document(DocumentView {
            markdown: self.markdown.clone(),
            origin: self.origin.clone(),
            resources: self.resources.clone(),
            options: self.options.clone(),
            title: None,
            truncated: self.truncated,
        }))
    }
}

#[cfg(test)]
mod tests;
