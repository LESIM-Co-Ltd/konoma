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
// - `footer` and `help` list, per context, which operations appear and in what order; the key
//   notation is generated from `keys`.

pub mod decl;
pub mod footer;
pub mod gate;
pub mod help;
pub mod keys;

#[cfg(test)]
mod tests;

pub use gate::{action_enabled, gate, Gate};

/// Which backend's screens are open (git or jj): the half of a `git.changes` context name. jj when
/// the background scan has settled on jj (`App::git_vcs`) or when the root itself is a jj
/// workspace: the first frames of a screen opened before the scan lands must name jj's keys too.
#[cfg(feature = "git")]
pub fn backend(app: &crate::app::App) -> crate::keymap::Backend {
    if app.git_vcs == crate::vcs::VcsKind::Jj
        || crate::vcs::detect(&app.tab.root) == crate::vcs::VcsKind::Jj
    {
        crate::keymap::Backend::Jj
    } else {
        crate::keymap::Backend::Git
    }
}
