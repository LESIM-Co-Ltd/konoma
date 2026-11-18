// Operations: what each key *does*, declared once, and the help and the footer built from it
// (plugin stage 2 / B2; `docs/FEATURE-PLUGIN-ARCHITECTURE.md` §4-4 c and §10-3-2).
//
// Before this module the key notation of the `?` help and of the footer was written by hand in
// about 290 places and did not follow `[keys]`. Now:
//
// - `decl` declares every `Action` once: a short label for the footer, a description for the help,
//   and the fact that it is a "launch" operation (layer 3).
// - `text` is the same declaration as it reads *now* in a context: the labels that change with the
//   state (`e` is "edit" or "open", `R` names whatever it switches to) are functions here.
// - `gate` is the one place that says whether an operation acts now. The key handler, the footer
//   and the help all ask it, so a hint is shown exactly when its key does what the label says
//   ("the hint is shown iff the key acts").
// - `keys` knows which keys are bound to an operation in a context (the layers of the keymap plus
//   the fixed keys `Enter`, `Tab`, `Esc`, ... which now also run through `dispatch_action`).
// - `menu` builds the menus behind `y`, `Space` and `,` for a context.
// - `footer` and `help` list, per context, which operations appear and in what order; the key
//   notation is generated from `keys`.

pub mod core_help;
pub mod decl;
pub mod footer;
pub mod gate;
pub mod help;
pub mod keys;
pub mod menu;
pub mod preview_plan;
pub mod write_guard;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_guard_tests;

pub use gate::{action_enabled, gate, Gate};
