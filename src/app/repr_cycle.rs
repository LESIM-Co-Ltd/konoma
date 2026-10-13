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
    /// Whether the representations of the open plugin are the host's own to walk, or one of the
    /// standard shapes whose `R` has its own code and wording.
    fn repr_cycle_is_standard(&self, reprs: &[konoma_plugin::view::ReprDecl]) -> bool {
        // A presentation: slides / text, keeping the slide.
        if self.deck_has_pictures() {
            return true;
        }
        // A document or diagram whose `R` is the rendered / source (or picture / text) switch.
        if self.is_decorated_kind() {
            if self.shows_plugin(konoma_mermaid::MERMAID_ID) {
                return true;
            }
            let ids: Vec<&str> = reprs.iter().map(|r| r.id.as_str()).collect();
            return ids == [konoma_markdown::REPR_RENDERED, konoma_markdown::REPR_SOURCE];
        }
        false
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
        if reprs.len() < 2 || self.repr_cycle_is_standard(reprs) {
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
            if !self.plugin_view_now(self.plugin_view_req()) {
                self.tab.repr = before;
                return false;
            }
            self.reset_view_for_repr_change();
        } else if self.shows_media_plugin() {
            // A picture: shown whole again, drawn on the worker like a turned page.
            self.tab.image_zoom = 1.0;
            self.tab.image_center = (0.5, 0.5);
            self.image_crop = None;
            if let Some(PreviewKind::Plugin { id, path, .. }) = self.tab.preview_kind.clone() {
                self.start_media_plugin(&id, &path);
            }
        } else if self.is_table_preview() {
            self.turn_table_page();
        } else {
            if !self.plugin_view_now(self.plugin_view_req()) {
                self.tab.repr = before;
                return false;
            }
            self.reset_view_for_repr_change();
        }
        true
    }
}
