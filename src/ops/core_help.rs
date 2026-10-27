// The glue between the host and the shared help / footer builders (`konoma_host_core::help`).
//
// The builders write the lines of key layers 1 to 3 from the actual bindings; this module tells
// them what only the host knows: where the keys come from (`HostLookup`), which operations do not
// act right now, and the words that change with the open file (`facts_for`).

use konoma_host_core::help::{HelpFacts, HelpOp, KeyLayer, KeyLookup};
use konoma_host_core::keymap::{CoreAction, CoreContext, CoreMenu, KeyPress};

use crate::app::App;
use crate::i18n::Msg;
use crate::keymap::{Action, Context, LeaderId};
use crate::ops::decl::text;
use crate::ops::gate::action_enabled;
use crate::ops::keys::keys_of;

/// The shared context of a host context, if the standalone app shows it too.
pub fn core_ctx(ctx: Context) -> Option<CoreContext> {
    Some(match ctx {
        Context::Text => CoreContext::Text,
        Context::Document => CoreContext::Document,
        Context::Image => CoreContext::Image,
        Context::Table => CoreContext::Table,
        Context::Canvas => CoreContext::Canvas,
        _ => return None,
    })
}

/// The host operation a shared one is (`None` for a menu key, which is no operation).
pub fn action_of(op: HelpOp) -> Option<Action> {
    match op {
        HelpOp::Act(a) => Some(Action::from(a)),
        HelpOp::Cancel => Some(Action::Cancel),
        HelpOp::Menu(_) => None,
    }
}

/// Where the keys of one host context come from: the host's keymap and its fixed keys.
pub struct HostLookup<'a> {
    pub app: &'a App,
    pub ctx: Context,
}

impl KeyLookup for HostLookup<'_> {
    fn keys_of(&self, op: HelpOp) -> Vec<KeyPress> {
        match op {
            HelpOp::Menu(m) => {
                let id = match m {
                    CoreMenu::Options => LeaderId::Options,
                    CoreMenu::Actions => LeaderId::File,
                    CoreMenu::Copy => LeaderId::Copy,
                    CoreMenu::TableCopy => LeaderId::TableCopy,
                };
                self.app
                    .keymaps
                    .leader_keys_of(self.ctx)
                    .into_iter()
                    .filter(|(_, l)| *l == id)
                    .map(|(k, _)| k)
                    .collect()
            }
            op => action_of(op)
                .map(|a| keys_of(self.app, self.ctx, a))
                .unwrap_or_default(),
        }
    }

    fn layer_of(&self, key: KeyPress) -> Option<KeyLayer> {
        self.app.keymaps.layer_of_key(self.ctx, key)
    }
}

/// What the shared builders need to know about the state of `app` in `ctx`: which of the shared
/// operations act, and the words they have right now.
pub fn facts_for(app: &App, ctx: Context) -> HelpFacts {
    let mut facts = HelpFacts::default();
    let Some(cctx) = core_ctx(ctx) else {
        return facts;
    };
    let mut ops: Vec<HelpOp> = Vec::new();
    for row in konoma_host_core::help::rows_of(cctx) {
        for op in row.ops() {
            if !ops.contains(op) {
                ops.push(*op);
            }
        }
    }
    for op in ops {
        let Some(action) = action_of(op) else {
            continue;
        };
        if !action_enabled(app, ctx, action) {
            facts.off.push(op);
        }
        if let HelpOp::Act(a) = op {
            let t = text(app, ctx, Action::from(a));
            facts.label.push((a, t.label.and_then(Msg::core)));
            facts.help.push((a, t.help.and_then(Msg::core)));
        }
    }
    facts
}

/// The shared operation of a host one, when it is one.
#[allow(dead_code)]
pub fn core_of(a: Action) -> Option<CoreAction> {
    a.core()
}
