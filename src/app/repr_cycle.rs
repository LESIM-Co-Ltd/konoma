//! `R` for any plugin: the host walks the representations the plugin declares (`Layout.reprs`).
//!
//! A plugin that offers two or more representations of what it shows (a table and its summary,
//! a picture and its text, a document and its outline) gets `R` for free: the key goes to the next
//! one in the declared order and wraps after the last, the page is kept (`ViewReq::page` is the
//! tab's page), and the footer, the `?` help and the `Space` menu name the representation the key
//! goes to, with the plugin's own label.
//!
//! The standard plugins keep their own wording and their own steps: a presentation's slides / text
//! (`office_doc.rs`), a Markdown file's or Word document's rendered / source (`markdown_plugin.rs`)
//! and a diagram's picture / text, plus the diff's `R` (`diff_view.rs`). [`App::repr_step`] is `None`
//! for those, so the key, its hint and its help row come from the code they always came from. One
//! predicate (`repr_step`) answers the gate, the label and the handler, so a hint shows only while
//! the key acts.

use super::plugin_host::PluginPhase;
use super::*;

/// The representation `R` goes to next, and what to call it in the language of the interface.
pub(crate) struct ReprStep {
    /// The id the plugin is asked for (`ViewReq::repr`).
    pub id: String,
    /// The plugin's label for it.
    pub label: String,
}

impl App {
    /// Whether the host's own `rendered` / `source` toggle (the host's wording, the source shown
    /// in its windowed reader, `R` = `toggle_md_raw`) is what `R` does on the open file. **The one
    /// predicate for that**: the request a plugin is asked with (`plugin_view_req`, the first
    /// request of an open), the footer / help / menu gate, the label and the handler all use it.
    ///
    /// It holds for a standalone diagram file (`mermaid`), and for a document (a Markdown file, a
    /// Word document) whose plugin declares **exactly** the representations `rendered` and
    /// `source`, in either order. Every other representation id, `"source"` included, is just a
    /// name to the host, which walks it like any other ([`App::repr_step`]); a document that
    /// declares one representation or none has no `R` at all. Before the plugin has answered (its
    /// layout is not known yet) a document is assumed to be the standard shape.
    pub(crate) fn host_source_view(&self) -> bool {
        if self.shows_plugin(konoma_mermaid::MERMAID_ID) {
            return true;
        }
        if !self.holds_document() {
            return false;
        }
        let Some(show) = self.plugin.as_ref().filter(|p| self.shows_plugin_file(p)) else {
            return true;
        };
        let reprs = &show.layout.reprs;
        if reprs.is_empty() && !matches!(show.phase, PluginPhase::Ready) {
            return true;
        }
        let has = |id: &str| reprs.iter().any(|r| r.id == id);
        reprs.len() == 2 && has(konoma_markdown::REPR_RENDERED) && has(konoma_markdown::REPR_SOURCE)
    }

    /// Whether the representations of the open plugin are the host's own to walk, or one of the
    /// standard shapes whose `R` has its own code and wording: a presentation's slides / text, and
    /// [`App::host_source_view`].
    fn repr_cycle_is_standard(&self) -> bool {
        self.deck_has_pictures() || self.host_source_view()
    }

    /// The representation the open plugin shows now: the tab's choice, else the plugin's default
    /// for this terminal, else the first it declares.
    fn current_repr_index(&self, layout: &konoma_plugin::view::Layout) -> usize {
        let find = |id: &str| layout.reprs.iter().position(|r| r.id == id);
        if let Some(i) = self.tab.repr.as_deref().and_then(find) {
            return i;
        }
        let default = if self.terminal_draws_pixels() {
            layout.default_repr.as_deref()
        } else {
            layout
                .default_repr_without_pixels
                .as_deref()
                .or(layout.default_repr.as_deref())
        };
        default.and_then(find).unwrap_or(0)
    }

    /// The representation `R` goes to, when it walks the plugin's declared list: the open plugin
    /// is ready, declares two or more, and is not one of the standard shapes with their own `R`.
    pub(crate) fn repr_step(&self) -> Option<ReprStep> {
        if self.tab.preview_from_diff {
            return None;
        }
        let show = self.plugin.as_ref()?;
        if !matches!(show.phase, PluginPhase::Ready) {
            return None;
        }
        let reprs = &show.layout.reprs;
        if reprs.len() < 2 || self.repr_cycle_is_standard() {
            return None;
        }
        let next = &reprs[(self.current_repr_index(&show.layout) + 1) % reprs.len()];
        Some(ReprStep {
            id: next.id.clone(),
            label: next.label.get(self.plugin_lang()).to_string(),
        })
    }

    /// `R` on a plugin whose representations the host walks. Returns whether it acted. The page
    /// stays (it is in the request), what depended on the old representation starts over.
    pub(crate) fn cycle_repr(&mut self) -> bool {
        let Some(step) = self.repr_step() else {
            return false;
        };
        let before = self.tab.repr.replace(step.id);
        if self.holds_document() {
            // The session holds the content: the answer lands at once.
            if !self.plugin_view_now_or_queue(self.plugin_view_req()) {
                self.tab.repr = before;
                return false;
            }
            self.reset_view_for_repr_change();
        } else if self.shows_media_plugin() {
            // A picture: shown whole again, drawn on the worker like a turned page.
            self.tab.image_view.zoom = 1.0;
            self.tab.image_view.center = (0.5, 0.5);
            self.image_viewer.set_crop(None);
            if let Some(PreviewKind::Plugin { id, path, .. }) = self.tab.preview_kind.clone() {
                self.start_media_plugin(&id, &path);
            }
        } else if self.is_table_preview() {
            self.turn_table_page();
        } else {
            // Any other plugin (a canvas, a text view a third party made): its `view` is arbitrary
            // code and may be slow, so it is asked on a worker like every first frame, never on
            // this thread. With a job holding the session (a redraw tick) the wish is remembered
            // and asked when the session returns. Only a document's session is cheap enough to ask
            // on the spot (above): the golden snapshots are of those.
            self.plugin_view_when_free();
            self.reset_view_for_repr_change();
        }
        true
    }
}
