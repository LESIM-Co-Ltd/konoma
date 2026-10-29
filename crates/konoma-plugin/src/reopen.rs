//! "Open this again as `content_type`" (a [`View::Reopen`](crate::View)): the one rule both hosts
//! (konoma and the standalone app) follow.
//!
//! A plugin that converts a file into text (Word hands over Markdown) answers its `view` with a
//! `Reopen`. The host looks the content type up in the registry
//! ([`Registry::resolve_content`](crate::registry::Registry::resolve_content)), opens the plugin it
//! names on the text with [`Input::Content`] and asks it for the document. The session that
//! answers replaces the converting plugin's from then on. Looking the target up is the host's
//! (it knows its registry and settings); everything after that is [`reopen_document`].

use std::sync::Arc;

use crate::session::{Failure, OpenCx, Plugin, Session, TermCaps, ViewCx};
use crate::view::{DocumentView, Reopen, View, ViewReq};
use crate::{t, Cancel, Input, Settings};

/// The plugin that takes a content type, with the settings it is opened with (owned: it may move
/// to a worker thread).
#[derive(Clone)]
pub struct ContentTarget {
    pub plugin: Arc<dyn Plugin>,
    pub settings: Settings,
}

/// The document a [`Reopen`] was opened into, and the session that answered (the one the show
/// keeps from here on).
pub struct Reopened {
    pub session: Box<dyn Session>,
    /// A conversion cut short stays cut short: `truncated` of the document includes the
    /// `Reopen`'s own.
    pub document: DocumentView,
}

/// Open `target` on the text of `r` and ask it for the document. `accepts` is the one content
/// type the host can show (`markdown`): another one, or no `target` (no enabled plugin takes the
/// content type), is "no plugin is available to show this document".
pub fn reopen_document(
    target: Option<&ContentTarget>,
    accepts: &str,
    r: Reopen,
    lang: &str,
    cancel: &Cancel,
    term: TermCaps,
) -> Result<Reopened, Failure> {
    let Some(target) = target.filter(|_| r.content_type == accepts) else {
        return Err(Failure::unsupported(t!(
            en: "no plugin is available to show this document",
            ja: "この文書を表示するプラグインを使えません",
        )));
    };
    let truncated = r.truncated;
    let input = Input::Content {
        content_type: r.content_type,
        text: r.text,
        resources: r.resources,
        origin: r.origin,
    };
    let open_cx = OpenCx::new(&target.settings, lang)
        .with_cancel(cancel.clone())
        .with_term(term);
    let mut session = target.plugin.open(input, &open_cx)?;
    let view_cx = ViewCx::new(&target.settings, lang)
        .with_cancel(cancel.clone())
        .with_term(term);
    let View::Document(mut document) = session.view(&ViewReq::default(), &view_cx)? else {
        return Err(Failure::unsupported(t!(
            en: "the plugin did not return a document",
            ja: "プラグインが文書を返しませんでした",
        )));
    };
    document.truncated |= truncated;
    Ok(Reopened { session, document })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Origin;
    use crate::view::DocOptions;
    use crate::{Manifest, Settings};

    /// Takes markdown content; its document is `truncated` as given.
    struct Target {
        document_truncated: bool,
    }

    struct TargetSession {
        text: String,
        truncated: bool,
    }

    impl Plugin for Target {
        fn manifest(&self) -> Manifest {
            Manifest::new("target", "0.0.1")
        }
        fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            let Input::Content { text, .. } = input else {
                return Err(Failure::unsupported(t!(en: "content only")));
            };
            Ok(Box::new(TargetSession {
                text,
                truncated: self.document_truncated,
            }))
        }
    }

    impl Session for TargetSession {
        fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
            Ok(View::Document(DocumentView {
                markdown: self.text.clone(),
                origin: Origin::File(std::path::PathBuf::from("x.md")),
                resources: Default::default(),
                options: DocOptions::default(),
                title: None,
                truncated: self.truncated,
            }))
        }
    }

    fn reopen(content_type: &str, truncated: bool) -> Reopen {
        Reopen {
            content_type: content_type.into(),
            text: "text\n".into(),
            resources: Default::default(),
            origin: Origin::Converted {
                from: std::path::PathBuf::from("a.conv"),
            },
            truncated,
        }
    }

    fn run(
        document_truncated: bool,
        content_type: &str,
        truncated: bool,
    ) -> Result<Reopened, Failure> {
        let target = ContentTarget {
            plugin: Arc::new(Target { document_truncated }),
            settings: Settings::empty().clone(),
        };
        reopen_document(
            Some(&target),
            "markdown",
            reopen(content_type, truncated),
            "en",
            &Cancel::never(),
            TermCaps::new(),
        )
    }

    #[test]
    fn a_conversion_cut_short_stays_cut_short() {
        let r = run(false, "markdown", true).expect("opens");
        assert!(r.document.truncated, "the Reopen's own flag is kept");
        assert_eq!(r.document.markdown, "text\n");
    }

    #[test]
    fn a_whole_conversion_is_not_marked_cut() {
        let r = run(false, "markdown", false).expect("opens");
        assert!(!r.document.truncated);
    }

    #[test]
    fn a_document_the_target_itself_cut_stays_cut_whatever_the_reopen_says() {
        assert!(
            run(true, "markdown", false)
                .ok()
                .unwrap()
                .document
                .truncated
        );
        assert!(run(true, "markdown", true).unwrap().document.truncated);
    }

    #[test]
    fn another_content_type_or_no_target_is_no_plugin_available() {
        assert!(run(false, "html", false).is_err());
        let none = reopen_document(
            None,
            "markdown",
            reopen("markdown", false),
            "en",
            &Cancel::never(),
            TermCaps::new(),
        );
        assert!(none.is_err());
    }
}
