//! Opt-in file-diff keys. Ordinary file apps retain the unextended core tables.
use konoma_host_core::{
    keymap::{
        CoreBinding, CoreContext, CoreKeys, CoreTable, CoreViewType, HostOp, KeyPress, KeyScheme,
    },
    keys::KeysConfig,
};

const OPERATIONS: &[HostOp] = &[
    HostOp::CycleDiffView,
    HostOp::DiffNextChange,
    HostOp::DiffPrevChange,
    HostOp::JumpNextChange,
    HostOp::JumpPrevChange,
];

/// The representation switch belongs to the diff type layer, as in the full host.
pub fn file_diff_type_keys() -> CoreTable {
    [(KeyPress::ch('R'), CoreBinding::Host(HostOp::CycleDiffView))].into()
}

/// File-diff operations in the context's own layer, before user overrides.
pub fn file_diff_own_keys() -> CoreTable {
    [
        (KeyPress::ch('!'), HostOp::GitLaunchTool),
        (KeyPress::ch('n'), HostOp::DiffNextChange),
        (KeyPress::ch('N'), HostOp::DiffPrevChange),
        (KeyPress::ctrl_ch('n'), HostOp::JumpNextChange),
        (KeyPress::ctrl_ch('p'), HostOp::JumpPrevChange),
    ]
    .into_iter()
    .map(|(key, op)| (key, CoreBinding::Host(op)))
    .collect()
}

/// Read file-diff defaults and config in the same order as the full host. This does not
/// enable these operations in `CoreKeys::from_config` or ordinary standalone file apps.
pub fn file_diff_keys(scheme: KeyScheme, config: &KeysConfig) -> CoreKeys {
    let mut keys = CoreKeys::defaults(scheme);
    keys.types
        .entry(CoreViewType::Diff)
        .or_default()
        .extend(file_diff_type_keys());
    keys.contexts
        .entry(CoreContext::Diff)
        .or_default()
        .extend(file_diff_own_keys());
    keys.with_config(
        &config.to_keymap_config(),
        OPERATIONS,
        &[("preview_git_diff", CoreContext::Diff)],
    )
}
