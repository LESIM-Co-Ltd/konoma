//! The `command` plugin: a `[[preview.rules]] command = "..."` delegation, as a plugin that is
//! built into the host (`docs/FEATURE-PLUGIN-ARCHITECTURE.md` §3-2).
//!
//! One instance is made per rule that matched (`CommandPlugin::new`): the rule's template, its
//! `render_as` and its `detached` flag. The host opens it like any other plugin, on a worker:
//!
//! - **Not detached.** `view` runs the command (30 s limit, output capped, no shell: the same
//!   `preview::command::run_capture` as ever) and answers with a picture (`render_as = "image"`:
//!   the produced file decoded under the image limits) or with a text view whose source is the
//!   output file (the host reads it in windows, like any text). A failed run is a `Failure` whose
//!   English message is the reason the `[can not preview: <ext>]` screen shows.
//! - **Detached** (a player, an app: a side effect). The first `view` starts the program once and
//!   answers with a message ("opened externally"); a later `view` of the same session answers
//!   with the message again and starts nothing. Whether a preview may be opened again (a tab
//!   switch, an outside change to the file) is the host's question and it is answered before any
//!   session exists: a detached rule is opened only when the preview is entered
//!   (`App::kind_loads_media`).

use konoma_plugin::view::{ImageData, ImageView, MessageView, TextSource, TextView};
use konoma_plugin::{
    t, Failure, Input, Line, Manifest, OpenCx, Plugin, Session, Tr, View, ViewCx, ViewReq,
};

use super::{standard, COMMAND_ID};
use crate::app::{run_command_job, MediaPayload};
use crate::i18n::{tr, Lang, Msg};
use crate::preview::command;

/// The `command` plugin for one rule.
pub(crate) struct CommandPlugin {
    template: String,
    as_image: bool,
    detached: bool,
}

impl CommandPlugin {
    /// The plugin for a rule with this template, `render_as` and `detached`.
    pub(crate) fn new(template: &str, render_as: Option<&str>, detached: bool) -> CommandPlugin {
        CommandPlugin {
            template: template.to_string(),
            as_image: render_as == Some("image"),
            detached,
        }
    }

    /// The instance the registry holds: bound to no rule (an empty template), so opening it fails
    /// the way an empty `command = ""` always did. The host opens the instance of the rule.
    pub(crate) fn unbound() -> CommandPlugin {
        CommandPlugin::new("", None, false)
    }
}

pub(crate) fn manifest() -> Manifest {
    standard(COMMAND_ID, t!(en: "External command", ja: "外部コマンド"))
}

impl Plugin for CommandPlugin {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn open(&self, input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        let Input::File { path, .. } = input else {
            return Err(Failure::unsupported(t!(
                en: "the command plugin takes a file",
                ja: "コマンドプラグインはファイルを受け取ります",
            )));
        };
        Ok(Box::new(CommandSession {
            path,
            template: self.template.clone(),
            as_image: self.as_image,
            detached: self.detached,
            launched: false,
        }))
    }
}

struct CommandSession {
    path: std::path::PathBuf,
    template: String,
    as_image: bool,
    detached: bool,
    /// Whether the detached program has been started by this session.
    launched: bool,
}

impl CommandSession {
    /// The message of a detached launch, in the user's language.
    fn opened_message(&self, lang: &str) -> View {
        let lang = if lang == "ja" { Lang::Jp } else { Lang::En };
        View::Message(MessageView {
            lines: vec![Line::raw(format!(
                "{}{}",
                tr(lang, Msg::CommandOpenedExternally),
                self.template
            ))],
        })
    }
}

impl Session for CommandSession {
    fn view(&mut self, _req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        if self.detached && self.launched {
            return Ok(self.opened_message(cx.lang));
        }
        let out = command::temp_out_path();
        let argv = command::build_argv(&self.template, &self.path, &out);
        if self.detached {
            command::run_detached(&argv).map_err(|e| Failure::external(Tr::new(e.to_string())))?;
            self.launched = true;
            return Ok(self.opened_message(cx.lang));
        }
        let uses_out = self.template.contains("{out}");
        match run_command_job(argv, out, uses_out, self.as_image) {
            Some(MediaPayload::CommandText(p)) => Ok(View::Text(TextView {
                source: TextSource::File(p),
                styler: None,
                file_lines: false,
                syntax_name: None,
            })),
            Some(MediaPayload::Static(img)) => {
                let rgba = img.into_rgba8();
                let (width, height) = rgba.dimensions();
                Ok(View::Image(ImageView::new(ImageData::Rgba {
                    width,
                    height,
                    pixels: std::sync::Arc::new(rgba.into_raw()),
                })))
            }
            Some(MediaPayload::CommandFailed(why)) => Err(Failure::external(Tr::new(why))),
            _ => Err(Failure::external(t!(
                en: "the command produced nothing to show",
                ja: "コマンドは表示できるものを出しませんでした",
            ))),
        }
    }
}
