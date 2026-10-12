// Image decoding, limits and the decode-memory gate: live in the `konoma-svg` crate; re-exported
// so `crate::preview::image::...` paths are unchanged. What stays here is the translated message of
// an `ImageFailure` (the catalog, `i18n`, is the host's).

use crate::i18n::{tr, Lang, Msg};
use crate::preview::svg_guard::SvgFailMsg;

pub use konoma_svg::image::*;

/// The translated reason shown in place of a picture that could not be decoded. In scope wherever
/// `ImageFailure::message(lang)` is called.
pub trait ImageFailureMessage {
    /// None for `Cancelled`, which is never shown.
    fn message(self, lang: Lang) -> Option<&'static str>;
}

impl ImageFailureMessage for ImageFailure {
    fn message(self, lang: Lang) -> Option<&'static str> {
        Some(tr(
            lang,
            match self {
                Self::TooLarge => Msg::ImageReasonTooLarge,
                Self::Corrupt => Msg::ImageReasonCorrupt,
                Self::UnsupportedFormat => Msg::ImageReasonUnsupportedFormat,
                Self::Cancelled => return None,
                Self::Svg(f) => f.msg(),
            },
        ))
    }
}
