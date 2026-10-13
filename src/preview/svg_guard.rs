// Guard rails for SVG that konoma did not write: they live in the `konoma-svg` crate. This module
// keeps the paths `crate::preview::svg_guard::...` working and holds the one thing that cannot move
// with the crate: the mapping from a failure reason to the user's (translated) message.

pub use konoma_svg::svg_guard::*;

/// The message the user sees for an SVG that was refused or stopped (a catalog entry: the sentence
/// says what the file did, never blames the terminal). Lives here because the catalog (`i18n`) is
/// the host's; `konoma-svg` only knows the reason.
pub trait SvgFailMsg {
    fn msg(self) -> crate::i18n::Msg;
}

impl SvgFailMsg for SvgFail {
    fn msg(self) -> crate::i18n::Msg {
        use crate::i18n::Msg;
        match self {
            SvgFail::TooDeep => Msg::SvgTooDeep,
            SvgFail::TooLarge => Msg::SvgTooLarge,
            SvgFail::TooComplex => Msg::SvgTooComplex,
            SvgFail::TooHeavy => Msg::SvgTooHeavy,
            SvgFail::Timeout => Msg::SvgTimeout,
            SvgFail::Memory => Msg::SvgMemory,
            SvgFail::Crashed => Msg::SvgCrashed,
            SvgFail::Invalid => Msg::SvgInvalid,
        }
    }
}
