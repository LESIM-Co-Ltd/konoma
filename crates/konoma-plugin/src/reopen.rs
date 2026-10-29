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
