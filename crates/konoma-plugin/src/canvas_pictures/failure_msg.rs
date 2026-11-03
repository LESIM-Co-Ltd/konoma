//! The words for a picture that could not be decoded, shared by konoma and the standalone app: the
//! sentence says what the file did (never blames the terminal), in the language asked for.

use std::path::Path;

use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::lang::Lang;
use konoma_svg::image::ImageFailure;
use konoma_svg::svg_guard::SvgFail;

use crate::manifest::Unavailable;
use crate::session::{Failure, FailureKind};

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

/// The reason a picture-showing plugin's failure is explained by, from what the plugin declared
/// it shows when it shows nothing (`unavailable`), never from its id. `None` for a plugin whose
/// words come from elsewhere (a PDF, a video, one a user installed: it has its own sentence or the
/// failure's own message).
///
/// A plugin whose file is still text when it cannot be drawn (`RawText { with_reason: true }`,
/// the SVG one) gets the vector-file reasons, one that names the host's picture frame
/// (`Picture`, the image one) gets the picture reasons. A dropped stale answer is `Cancelled`.
pub fn image_failure_for(unavailable: &Unavailable, f: &Failure) -> Option<ImageFailure> {
    use konoma_svg::svg_guard::SvgFail;
    match (unavailable, f.kind) {
        (_, FailureKind::Cancelled) => Some(ImageFailure::Cancelled),
        (Unavailable::RawText { with_reason: true }, FailureKind::TooLarge) => {
            Some(ImageFailure::Svg(SvgFail::TooLarge))
        }
        (Unavailable::RawText { with_reason: true }, _) => {
            Some(ImageFailure::Svg(SvgFail::Invalid))
        }
        (Unavailable::Picture, FailureKind::TooLarge | FailureKind::TooHeavy) => {
            Some(ImageFailure::TooLarge)
        }
        (Unavailable::Picture, FailureKind::Unsupported) => Some(ImageFailure::UnsupportedFormat),
        (Unavailable::Picture, _) => Some(ImageFailure::Corrupt),
        _ => None,
    }
}

/// The words of a failed plugin that are not in the picture frame, as konoma and the standalone
/// app both draw them: the file as text with the reason above it (`RawText`), the plugin's
/// sentence and the path (`Notice`), or the failure's own message and the path (`Message`).
/// `None` for `Picture`: that one is the frame with `[image] <reason>` ([`failure_line`]-style,
/// drawn by the host).
///
/// `why` is [`image_failure_for`]'s answer (the reason a vector file failed with), `body` makes the
/// file's text for `RawText` (only called then), `lang_code` is `"ja"` or `"en"`.
pub fn unavailable_text(
    unavailable: &Unavailable,
    failure_message: &str,
    why: Option<ImageFailure>,
    (lang_code, lang): (&str, Lang),
    plugin_id: &str,
    path: &Path,
    body: &dyn Fn() -> String,
) -> Option<String> {
    match unavailable {
        Unavailable::RawText { with_reason } => {
            let body = body();
            // When the cause is known (too deep, too heavy, took too long, ...) a line says so first.
            match why
                .and_then(|f| image_failure_message(f, lang, plugin_id))
                .filter(|_| *with_reason)
            {
                Some(why) => Some(format!("{why}\n\n{body}")),
                None => Some(body),
            }
        }
        Unavailable::Notice(words) => Some(format!("{}\n{}", words.get(lang_code), path.display())),
        Unavailable::Message => Some(plain_failure_text(failure_message, path)),
        Unavailable::Picture => None,
    }
}

/// The file as text for a failed vector file (`RawText`), the one body konoma and the standalone
/// app both show under the reason: the whole file within the preview's size and line caps, ending
/// with a note when it was cut, or the read error. The caller scrolls and wraps it.
pub fn raw_file_body(path: &Path, lang: Lang) -> String {
    match konoma_widgets::text::load(path) {
        Ok(content) => {
            let mut s = content.lines.join("\n");
            if content.truncated {
                s.push_str(CoreMsg::PreviewTruncated.tr(lang));
            }
            s
        }
        // The `[can not preview: ...]` marker is a fixed English string by spec.
        Err(e) => format!("[can not preview: load failed] {e}"),
    }
}

/// The words of a document (Word, a presentation) that did not load, as konoma and the standalone
/// app both draw them: the failure's message (or `[can not preview]` with none), then, only when
/// the failure is `Unsupported` and `e` really opens an application (`edit_opens_app`, the same
/// predicate as the footer's `e` hint), the line that says so, then the path.
pub fn document_failure_text(
    failure: Option<&Failure>,
    (lang_code, lang): (&str, Lang),
    edit_opens_app: bool,
    path: &Path,
) -> String {
    let mut body = failure.map_or_else(
        || "[can not preview]".to_string(),
        |f| f.message.get(lang_code).to_string(),
    );
    if edit_opens_app && failure.is_some_and(|f| f.kind == FailureKind::Unsupported) {
        body.push('\n');
        body.push_str(CoreMsg::DocErrUnsupportedOpen.tr(lang));
    }
    plain_failure_text(&body, path)
}

/// The failure's message, then the file's path: what a plugin with nothing declared shows.
pub fn plain_failure_text(message: &str, path: &Path) -> String {
    format!("{message}\n{}", path.display())
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

    fn failure(kind: FailureKind) -> Failure {
        Failure::new(kind, crate::t!(en: "nope", ja: "だめ"))
    }

    /// What a failure is explained as follows from the declared `Unavailable`, for every kind.
    #[test]
    fn the_reason_follows_the_declared_unavailable_for_every_kind() {
        use FailureKind::*;
        let kinds = [Corrupt, TooLarge, TooHeavy, Unsupported, Cancelled];
        for k in kinds {
            let f = failure(k);
            let svg = image_failure_for(&Unavailable::RawText { with_reason: true }, &f);
            let pic = image_failure_for(&Unavailable::Picture, &f);
            if k == Cancelled {
                assert_eq!(svg, Some(ImageFailure::Cancelled));
                assert_eq!(pic, Some(ImageFailure::Cancelled));
            } else {
                assert!(matches!(svg, Some(ImageFailure::Svg(_))), "{k:?}");
                assert!(
                    matches!(
                        pic,
                        Some(
                            ImageFailure::TooLarge
                                | ImageFailure::Corrupt
                                | ImageFailure::UnsupportedFormat
                        )
                    ),
                    "{k:?}"
                );
            }
            // The ones that say nothing about the picture give no reason.
            assert_eq!(
                image_failure_for(&Unavailable::Message, &f)
                    .filter(|r| *r != ImageFailure::Cancelled),
                None
            );
            assert_eq!(
                image_failure_for(&Unavailable::RawText { with_reason: false }, &f)
                    .filter(|r| *r != ImageFailure::Cancelled),
                None
            );
        }
        // The exact reasons (a picture's kinds, a vector file's two).
        let pic = |k| image_failure_for(&Unavailable::Picture, &failure(k));
        assert_eq!(pic(Corrupt), Some(ImageFailure::Corrupt));
        assert_eq!(pic(TooLarge), Some(ImageFailure::TooLarge));
        let svg = |k| image_failure_for(&Unavailable::RawText { with_reason: true }, &failure(k));
        assert_eq!(
            svg(TooLarge),
            Some(ImageFailure::Svg(konoma_svg::svg_guard::SvgFail::TooLarge))
        );
        assert_eq!(
            svg(Corrupt),
            Some(ImageFailure::Svg(konoma_svg::svg_guard::SvgFail::Invalid))
        );
        assert_eq!(
            image_failure_for(&Unavailable::Picture, &failure(TooHeavy)),
            Some(ImageFailure::TooLarge)
        );
        assert_eq!(
            image_failure_for(&Unavailable::Picture, &failure(Unsupported)),
            Some(ImageFailure::UnsupportedFormat)
        );
    }

    #[test]
    fn the_words_of_each_unavailable() {
        let p = Path::new("/d/f.x");
        let body = || "BODY".to_string();
        let say = |u: Unavailable, why| {
            unavailable_text(&u, "nope", why, ("en", Lang::En), "p", p, &body)
        };
        assert_eq!(say(Unavailable::Message, None).unwrap(), "nope\n/d/f.x");
        assert_eq!(
            say(Unavailable::Notice(crate::t!(en: "note")), None).unwrap(),
            "note\n/d/f.x"
        );
        assert_eq!(say(Unavailable::Picture, None), None);
        assert_eq!(
            say(Unavailable::RawText { with_reason: true }, None).unwrap(),
            "BODY"
        );
        let with = say(
            Unavailable::RawText { with_reason: true },
            Some(ImageFailure::Svg(SvgFail::Invalid)),
        )
        .unwrap();
        assert!(
            with.ends_with("\n\nBODY") && with.len() > "\n\nBODY".len(),
            "{with}"
        );
        // A plugin that did not ask for the reason shows the text alone.
        assert_eq!(
            say(
                Unavailable::RawText { with_reason: false },
                Some(ImageFailure::Svg(SvgFail::Invalid))
            )
            .unwrap(),
            "BODY"
        );
    }
}
