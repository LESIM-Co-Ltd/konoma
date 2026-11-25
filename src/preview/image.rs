// Image decoding, limits and the decode-memory gate: live in the `konoma-svg` crate; re-exported
// so `crate::preview::image::...` paths are unchanged. What stays here is the translated message of
// an `ImageFailure` (the catalog, `i18n`, is the host's).

use crate::i18n::Lang;

pub use konoma_svg::image::*;

/// The translated reason shown in place of a picture that could not be decoded. In scope wherever
/// `ImageFailure::message(lang)` is called.
pub trait ImageFailureMessage {
    /// None for `Cancelled`, which is never shown. `plugin_id` is the plugin whose document the
    /// picture belongs to (`""` for konoma's own previews): the read-scope refusal names it in
    /// the `[plugins.<id>] read_dirs` it tells the user to change, so no display path can show
    /// the raw `<id>` placeholder.
    fn message(self, lang: Lang, plugin_id: &str) -> Option<String>;
}

impl ImageFailureMessage for ImageFailure {
    fn message(self, lang: Lang, plugin_id: &str) -> Option<String> {
        konoma_plugin::canvas_pictures::image_failure_message(self, lang, plugin_id)
    }
}
