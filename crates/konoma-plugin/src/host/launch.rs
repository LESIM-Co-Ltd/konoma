//! What `e` and `o` say and ask, written once for every host (konoma and the standalone app):
//! the notice of a finished or refused launch, how `e` is named from where it goes, and the line
//! the editor is asked to open at. The way itself is [`crate::launch::resolve_launch`].

use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::lang::Lang;

use crate::launch::{Disabled, Launch, OfficeKind, Opener};

/// How `e` is named where it goes now (the footer, the help row and the gate read this one answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditWord {
    /// It opens the external editor.
    Editor,
    /// It opens an application (an Office app, or the GUI chain the plugin declares).
    App,
    /// It only explains that opening is switched off: nothing is named.
    Off,
}

/// The word of `e` for `launch`. A terminal chain is handed the terminal like the editor is, so it
/// is named as the editor.
pub fn edit_word(launch: &Launch) -> EditWord {
    match launch {
        Launch::Editor { .. } | Launch::Terminal(_) => EditWord::Editor,
        Launch::Gui(_) => EditWord::App,
        Launch::Disabled(_) => EditWord::Off,
    }
}

/// The line the editor is asked to open at: `caret` (what is on screen) unless the declaration
/// of the verb says the editor takes no line.
pub fn edit_line(launch: &Launch, caret: Option<usize>) -> Option<usize> {
    match launch {
        Launch::Editor { line: false } => None,
        _ => caret,
    }
}

/// The notice of a launch that was refused because it is switched off in `[external]`.
pub fn disabled_notice(lang: Lang, d: Disabled) -> String {
    match d {
        Disabled::OfficeApps => CoreMsg::OfficeAppsDisabled,
        Disabled::OpenLinks => CoreMsg::ExternalOpenLinksDisabled,
    }
    .tr(lang)
    .to_string()
}

/// The notice of a chain that opened the file with `opener`.
pub fn opened_notice(lang: Lang, opener: Opener) -> String {
    match opener {
        Opener::Microsoft(k) => CoreMsg::OfficeOpenedIn.tr(lang).replace(
            "{app}",
            match k {
                OfficeKind::Word => "Microsoft Word",
                OfficeKind::Excel => "Microsoft Excel",
                OfficeKind::PowerPoint => "Microsoft PowerPoint",
            },
        ),
        Opener::LibreOffice => CoreMsg::OfficeOpenedIn
            .tr(lang)
            .replace("{app}", "LibreOffice"),
        Opener::SystemDefault => CoreMsg::OfficeOpenedDefault.tr(lang).to_string(),
        Opener::Declared => CoreMsg::Opened
            .tr(lang)
            .trim_end_matches([':', ' '])
            .to_string(),
    }
}

/// The notice of a chain that could not open the file (`reason` is the last failure).
pub fn open_failed_notice(lang: Lang, reason: &str) -> String {
    format!("{}{reason}", CoreMsg::OfficeOpenFailed.tr(lang))
}

/// The notice of an editor that could not be started or failed.
pub fn editor_failed_notice(lang: Lang, reason: &str) -> String {
    format!("{}{reason}", CoreMsg::EditorFailed.tr(lang))
}

/// The outcome of a chain as the notice shows it.
pub fn chain_notice(lang: Lang, outcome: &Result<Opener, String>) -> String {
    match outcome {
        Ok(o) => opened_notice(lang, *o),
        Err(e) => open_failed_notice(lang, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Attempt;

    #[test]
    fn the_word_of_e_follows_where_it_goes() {
        let a = vec![Attempt {
            opener: Opener::SystemDefault,
            prog: "x".into(),
            args: vec![],
        }];
        assert_eq!(edit_word(&Launch::Editor { line: true }), EditWord::Editor);
        assert_eq!(edit_word(&Launch::Terminal(a.clone())), EditWord::Editor);
        assert_eq!(edit_word(&Launch::Gui(a)), EditWord::App);
        assert_eq!(
            edit_word(&Launch::Disabled(Disabled::OfficeApps)),
            EditWord::Off
        );
    }

    #[test]
    fn the_editor_is_given_no_line_when_the_declaration_says_so() {
        assert_eq!(edit_line(&Launch::Editor { line: false }, Some(7)), None);
        assert_eq!(edit_line(&Launch::Editor { line: true }, Some(7)), Some(7));
        assert_eq!(edit_line(&Launch::Editor { line: true }, None), None);
    }

    #[test]
    fn the_notices_are_localised() {
        assert_eq!(
            opened_notice(Lang::En, Opener::Microsoft(OfficeKind::Word)),
            "opened in Microsoft Word"
        );
        assert!(opened_notice(Lang::Jp, Opener::LibreOffice).contains("LibreOffice"));
        assert_eq!(opened_notice(Lang::En, Opener::Declared), "opened");
        assert_eq!(
            chain_notice(Lang::En, &Err("boom".into())),
            "could not open: boom"
        );
        assert!(disabled_notice(Lang::En, Disabled::OpenLinks).contains("open_links"));
        assert!(editor_failed_notice(Lang::En, "x").starts_with("editor failed: "));
    }
}
