//! The words for a picture that could not be decoded, shared by konoma and the standalone app: the
//! sentence says what the file did (never blames the terminal), in the language asked for.

use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::lang::Lang;
use konoma_svg::image::ImageFailure;
use konoma_svg::svg_guard::SvgFail;

/// The message of an SVG that was refused or stopped.
pub fn svg_fail_msg(f: SvgFail) -> CoreMsg {
    match f {
        SvgFail::TooDeep => CoreMsg::SvgTooDeep,
        SvgFail::TooLarge => CoreMsg::SvgTooLarge,
        SvgFail::TooComplex => CoreMsg::SvgTooComplex,
        SvgFail::TooHeavy => CoreMsg::SvgTooHeavy,
        SvgFail::Timeout => CoreMsg::SvgTimeout,
        SvgFail::Memory => CoreMsg::SvgMemory,
        SvgFail::Crashed => CoreMsg::SvgCrashed,
        SvgFail::Invalid => CoreMsg::SvgInvalid,
    }
}

/// The message of a failure; `None` for `Cancelled`, which is never shown.
pub fn image_failure_msg(f: ImageFailure) -> Option<CoreMsg> {
    Some(match f {
        ImageFailure::TooLarge => CoreMsg::ImageReasonTooLarge,
        ImageFailure::Corrupt => CoreMsg::ImageReasonCorrupt,
        ImageFailure::UnsupportedFormat => CoreMsg::ImageReasonUnsupportedFormat,
        ImageFailure::OutsideReadDirs => CoreMsg::ImageReasonOutsideReadDirs,
        ImageFailure::Cancelled => return None,
        ImageFailure::Svg(f) => svg_fail_msg(f),
    })
}

/// The translated reason shown in place of a picture that could not be decoded (`None` for
/// `Cancelled`). `plugin_id` is the plugin whose document the picture belongs to (`""` for
/// konoma's own previews): the read-scope refusal names it in the `[plugins.<id>] read_dirs` it
/// tells the user to change, so no display path can show the raw `<id>` placeholder.
pub fn image_failure_message(f: ImageFailure, lang: Lang, plugin_id: &str) -> Option<String> {
    image_failure_msg(f).map(|m| m.tr(lang).replace("<id>", plugin_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every failure the decoders can name has a sentence in both languages, and only `Cancelled`
    /// has none.
    #[test]
    fn every_failure_has_a_sentence_but_cancelled() {
        let all: Vec<ImageFailure> = [
            ImageFailure::TooLarge,
            ImageFailure::Corrupt,
            ImageFailure::UnsupportedFormat,
            ImageFailure::Cancelled,
            ImageFailure::OutsideReadDirs,
        ]
        .into_iter()
        .chain(SvgFail::ALL.into_iter().map(ImageFailure::Svg))
        .collect();
        assert!(
            all.len() >= 13,
            "the walk must see every failure: {}",
            all.len()
        );
        for f in all {
            for lang in [Lang::En, Lang::Jp] {
                let m = image_failure_message(f, lang, "p");
                match f {
                    ImageFailure::Cancelled => assert!(m.is_none()),
                    _ => assert!(!m.expect("a sentence").is_empty(), "{f:?}"),
                }
            }
        }
    }

    #[test]
    fn the_plugin_id_replaces_the_placeholder() {
        let m = image_failure_message(ImageFailure::OutsideReadDirs, Lang::En, "foo").unwrap();
        assert!(
            m.contains("plugin foo") && m.contains("[plugins.foo]"),
            "{m}"
        );
        assert!(!m.contains("<id>"));
    }
}
