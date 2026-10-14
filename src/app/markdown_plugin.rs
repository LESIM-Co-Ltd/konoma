//! The `markdown` plugin on the host side: the document display type.
//!
//! The plugin's `view` is a [`DocumentView`](konoma_plugin::view::DocumentView): the Markdown
//! text, its resources and the options it is drawn with. The host's document widget owns
//! everything that works on it (`md_render.rs`, `md_items.rs`, `md_media.rs`, `md_tasks.rs`, ...):
//! it draws the text with the options, not with the `[ui] md_*` settings, so a plugin that takes
//! the content type `markdown` in place of the standard one changes how Markdown files and Word
//! documents look.
//!
//! Two roads reach the plugin:
//!
//! - a `.md` file: the plugin is opened on the file (`plugin_host.rs`, `Input::File`) and answers
//!   with the document;
//! - a converted document (Word): the `document` plugin answers with a `Reopen` ("open this
//!   again as `markdown`"). The worker looks the content type up in the registry
//!   ([`App::content_target`]), opens that plugin on the text with `Input::Content` and asks it for
//!   the document, all in the Word job. The session that answers **replaces** the Word plugin's
//!   in the plugin show, so the other representation (`R`) is the markdown plugin's too.
//!
//! The representations are the plugin's (`Layout.reprs`): `rendered` (the document) and `source`
//! (a text view the host shows in its windowed reader). `R` is one request for the other
//! representation ([`App::choose_source_repr`]) followed by the one rule every change of
//! representation follows (`reset_view_for_repr_change`).

use konoma_plugin::view::{DocOptions, TextSource, View};
use konoma_plugin::{Plugin, Settings};

use super::plugin_host::MadeText;
use super::*;

/// The name of the source representation.
pub(super) const REPR_SOURCE: &str = konoma_markdown::REPR_SOURCE;

#[cfg(test)]
thread_local! {
    /// Test-only: a plugin that stands in for whatever the registry would name for a content type,
    /// on this thread (the plugin work is built on the thread that asks for it).
    pub(super) static CONTENT_STAND_IN: std::cell::RefCell<Option<Arc<dyn Plugin>>> =
        const { std::cell::RefCell::new(None) };
}

/// The plugin that takes a content type, with the settings it is opened with (owned: it moves to
/// a worker thread).
#[derive(Clone)]
pub(super) struct ContentTarget {
    pub(super) plugin: Arc<dyn Plugin>,
    pub(super) settings: Settings,
}

impl App {
    /// The plugin that takes the content type `content_type` (`Registry::resolve_content`: a user
    /// plugin before a standard one), or `None` when no enabled plugin does (the standard one is
    /// switched off with `[plugins.markdown] enabled = false`).
    pub(super) fn content_target(&self, content_type: &str) -> Option<ContentTarget> {
        #[cfg(test)]
        if let Some(plugin) = CONTENT_STAND_IN.with(|s| s.borrow().clone()) {
            let id = plugin.manifest().id;
            return Some(ContentTarget {
                settings: crate::plugins::effective_settings(&self.cfg, &id),
                plugin,
            });
        }
        let registry = crate::plugins::shared_registry(&self.cfg.plugin_disabled());
        let id = registry.resolve_content(content_type)?;
        let plugin = registry.plugin(&id)?;
        Some(ContentTarget {
            settings: crate::plugins::effective_settings(&self.cfg, &id),
            plugin,
        })
    }

    /// The options a Markdown document is drawn with when no document says otherwise: the
    /// `[plugins.markdown]` settings, with the old `[ui] md_*` values behind them.
    pub(super) fn cfg_doc_options(&self) -> DocOptions {
        DocOptions::from_settings(&crate::plugins::effective_settings(
            &self.cfg,
            konoma_markdown::MARKDOWN_ID,
        ))
    }

    /// The options in force for what is on screen: the open document's own, else the settings'
    /// (a rendered diff, a document not loaded yet).
    pub(super) fn doc_options(&self) -> DocOptions {
        match &self.document {
            Some(d) if self.holds_document() => d.options.clone(),
            _ => self.cfg_doc_options(),
        }
    }

    /// `doc_options`' table and image alignment as the renderer's type.
    pub(crate) fn doc_block_aligns(&self) -> crate::preview::markdown::BlockAligns {
        block_aligns(&self.doc_options())
    }

    /// Whether a checkbox of the document on screen may be written back to its file: the
    /// document came from the file (its options say so). A converted document, and a preview with
    /// no document yet, cannot.
    pub(super) fn doc_tasks_writable(&self) -> bool {
        match &self.document {
            Some(d) if self.holds_document() => d.options.tasks_writable,
            // No document: the widget has no checkbox to write (and a rendered diff has no items).
            _ => true,
        }
    }

    /// The text the decorated view parses: the document's Markdown. A Markdown file cut at the
    /// display cap says so at its end (its own paragraph); a converted document says so in the
    /// title instead.
    pub(super) fn document_source_text(&self) -> String {
        match &self.document {
            Some(d) if self.holds_document() => {
                let mut s = d.markdown.clone();
                if d.truncated && !d.is_converted() {
                    s.push_str(super::md_render::TRUNCATED_NOTE);
                }
                s
            }
            _ => String::new(),
        }
    }

    /// Test-only: a Markdown file put straight into the preview: the preview kind is set to the
    /// `markdown` plugin and the plugin is opened on the file through the same road a tree
    /// `Enter` takes (a session over the file, the host's container built from its view), without
    /// the other things `enter_preview` resets.
    #[cfg(test)]
    pub fn show_markdown_for_test(&mut self, path: PathBuf) {
        self.tab.preview_kind = Some(PreviewKind::markdown(path.clone()));
        self.tab.preview_path = Some(path);
        self.tab.mode = Mode::Preview;
        self.load_plugin();
    }

    /// Whether the host's source representation (`R`) is the one chosen: the tab's choice is
    /// `source` **and** `source` is the host's own (`App::host_source_view`; a plugin that merely
    /// names one of its representations `source` is not).
    pub fn is_md_raw(&self) -> bool {
        self.tab.repr.as_deref() == Some(REPR_SOURCE) && self.host_source_view()
    }

    /// Whether the plugin handed its source over as a string (a converted document has no file
    /// with those lines): the host then windows a temp file it wrote the string to.
    pub(super) fn source_view_is_owned(&self) -> bool {
        self.is_md_raw()
            && matches!(
                self.plugin.as_ref().and_then(|p| p.view.as_ref()),
                Some(View::Text(t)) if matches!(t.source, TextSource::Owned(_))
            )
    }

    /// Switch to the source representation (`source`) or back to the rendered one. A Markdown file
    /// or a Word document asks its session for the representation and applies the answer at once
    /// (the session holds the text: nothing is read again); a standalone mermaid file's source is
    /// the host's own (no plugin round trip). Returns false when the switch could not be made (the
    /// session is busy): nothing changed, and the key is ignored.
    pub(super) fn choose_source_repr(&mut self, source: bool) -> bool {
        let next = source.then(|| REPR_SOURCE.to_string());
        if !self.holds_document() {
            self.tab.repr = next;
            return true;
        }
        let before = std::mem::replace(&mut self.tab.repr, next);
        if !self.plugin_view_now(self.plugin_view_req()) {
            self.tab.repr = before;
            return false;
        }
        if !source {
            // Leaving the source view: the temp file holding a converted document's text is done for.
            self.clear_command_out();
        }
        true
    }

    /// The document is on screen and the source is the chosen representation (a tab that came
    /// back in it): the document was asked for first so that `R` has something to return to; ask
    /// for the source now.
    pub(super) fn show_source_view(&mut self) {
        self.clear_command_out();
        if !self.plugin_view_now(self.plugin_view_req()) {
            // The session is busy: the rendered document is what is shown.
            self.tab.repr = None;
        }
    }

    /// The source view landed (`R`, or a tab that came back in it): a string source was written to
    /// a private temp file by the worker (`made`), and the windowed reader is opened on the file
    /// (the file itself, or the temp file). Nothing is written here: the UI thread only takes the
    /// path.
    pub(super) fn source_view_landed(&mut self, made: Option<std::io::Result<MadeText>>) {
        if !self.is_md_raw() {
            return;
        }
        let owned = match self.plugin.as_ref().and_then(|p| p.view.as_ref()) {
            Some(View::Text(t)) => matches!(t.source, TextSource::Owned(_)),
            _ => return,
        };
        if owned {
            self.clear_command_out();
            match made.unwrap_or_else(|| Err(std::io::Error::other("no text was written"))) {
                Ok(file) => self.tab.command_out = file.take(),
                Err(e) => {
                    self.flash = Some(format!(
                        "{}{e}",
                        tr(self.lang, crate::i18n::Msg::OperationFailed)
                    ));
                    // No reader to open: back to the rendered document.
                    self.tab.repr = None;
                    self.md_cache = None;
                    self.setup_windowed();
                    return;
                }
            }
        }
        self.setup_windowed();
    }
}

/// A document's table and image alignment options as the renderer's type. An unrecognized value
/// is the option's default.
pub(super) fn block_aligns(o: &DocOptions) -> crate::preview::markdown::BlockAligns {
    use crate::preview::markdown::{BlockAlign, BlockAligns};
    let d = BlockAligns::default();
    BlockAligns {
        table: BlockAlign::from_config(&o.table_align, d.table),
        image: BlockAlign::from_config(&o.image_align, d.image),
    }
}
