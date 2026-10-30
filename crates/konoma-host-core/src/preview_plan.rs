//! What the preview screens (text, document, image, table, canvas) declare about their
//! operations, with no state of any host in it: which operations the footer plans and in what
//! order, which operations the `?` help declares and in what order, and whether an operation has
//! a name at all (a menu offers only named operations).
//!
//! konoma's footer, its help's order and its `Space` menu are built from this, and so can the
//! standalone app's, which is what keeps the letters and the order of the two menus the same: the
//! menu gives letters over *every* operation of the screen (the ones only konoma runs too) before
//! the ones that do not act are hidden.
//!
//! What changes with the state of the open file is not looked up here. It comes in as a
//! [`PreviewState`], so every function below is pure. The places that read it:
//!
//! - the footer plan: [`PreviewState::text_missing`], [`PreviewState::decorated`],
//!   [`PreviewState::focus`], [`PreviewState::mermaid_pannable`], [`PreviewState::windowed`];
//! - the help's declared rows: [`PreviewState::text_missing`], [`PreviewState::office_document`];
//! - whether an operation is named: [`PreviewState::edit_named`], [`PreviewState::repr_step`],
//!   [`PreviewState::repr_text`], [`PreviewState::returns_to_diff`], [`PreviewState::focus`].
//!
//! The words of the labels that change with the state (`e` is edit or open, `J`/`K` are page,
//! sheet or slide, `R` names what it switches to) stay with the host: only whether there is a
//! label is decided here.

use crate::help::{foot_priority, CoreFoot, HelpFacts, HelpOp};
use crate::help_msg::CoreMsg;
use crate::keymap::{CoreAction, CoreContext, HostOp, Motion};

/// An operation of a preview screen, from any layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewOp {
    /// An operation of the shared layers (the standalone app runs these too).
    Core(CoreAction),
    /// An operation only the full app runs.
    Host(HostOp),
    /// The fixed `Esc`.
    Cancel,
    /// A new tab (the one at the end of the tabs).
    TabNew,
    /// `Enter` over a table: the full-cell popup.
    ToggleTableCell,
    /// `Enter` over the focused link or item.
    LinkOpen,
    /// Follow the files of the diff or of the whole tree (needs konoma's `git` feature).
    ToggleFollowDiffScope,
}

const fn nav(m: Motion) -> PreviewOp {
    PreviewOp::Core(CoreAction::Navigate(m))
}
const fn core(a: CoreAction) -> PreviewOp {
    PreviewOp::Core(a)
}
const fn host(o: HostOp) -> PreviewOp {
    PreviewOp::Host(o)
}

/// The focused item of a decorated document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Task,
    Details,
    CodeBlock,
    MermaidFence,
    LocalLink,
    AnchorLink,
    ExternalLink,
}

/// The state of the open file the plans and names depend on. Every field says where it is read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewState {
    /// A Word / OpenDocument file still converting or failed: it has no text, so only the keys
    /// that still act are planned and declared. Reads: the footer plan, the help rows.
    pub text_missing: bool,
    /// The text is shown as a decorated document (links to follow) rather than as text. Reads:
    /// the footer plan.
    pub decorated: bool,
    /// The focused item of the decorated document. Reads: the footer plan; whether `Enter` and
    /// `Ctrl-t` are named.
    pub focus: Option<Focus>,
    /// A focused diagram is zoomed in and fully on screen, so `hjkl` pans it. Reads: the footer
    /// plan.
    pub mermaid_pannable: bool,
    /// The text is windowed (raw source, plain code or text), so a range can be selected. Reads:
    /// the footer plan.
    pub windowed: bool,
    /// A Word or OpenDocument text: it has no diagrams or checkboxes to document. Reads: the help
    /// rows.
    pub office_document: bool,
    /// `e` has a label: opening Office documents in their app is not switched off. Reads: the
    /// name of `e`.
    pub edit_named: bool,
    /// The open plugin declares a representation to step to. Reads: the name of `R`.
    pub repr_step: bool,
    /// `R` has a label of its own now (a deck, a diff's representation, a source view). Reads:
    /// the name of `R` over a text, a table or a canvas.
    pub repr_text: bool,
    /// The picture is a diff's own representation. Reads: the name of `R` over a picture.
    pub returns_to_diff: bool,
}

/// A group of operations the footer writes under one label (`j/k:scroll`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PvGroup {
    Scroll,
    Ends,
    /// `0`: back to the start of the line (one operation).
    Home,
    Hjkl,
    Tabs,
    FileJump,
    Zoom,
    Select,
    /// `Enter` / `Space` over a task or a `<details>`, and the focused item's open.
    ToggleItem,
}

impl PvGroup {
    /// The operations of the group, in the order the footer writes them.
    pub fn ops(self) -> &'static [PreviewOp] {
        match self {
            PvGroup::Scroll => G_SCROLL,
            PvGroup::Ends => G_ENDS,
            PvGroup::Home => G_HOME,
            PvGroup::Hjkl => G_HJKL,
            PvGroup::Tabs => G_TABS,
            PvGroup::FileJump => G_FILE_JUMP,
            PvGroup::Zoom => G_ZOOM,
            PvGroup::Select => G_SELECT,
            PvGroup::ToggleItem => G_TOGGLE_ITEM,
        }
    }
}

const G_SCROLL: &[PreviewOp] = &[nav(Motion::Down), nav(Motion::Up)];
const G_ENDS: &[PreviewOp] = &[nav(Motion::Top), nav(Motion::Bottom)];
const G_HOME: &[PreviewOp] = &[nav(Motion::LineHome)];
const G_HJKL: &[PreviewOp] = &[
    nav(Motion::Left),
    nav(Motion::Down),
    nav(Motion::Up),
    nav(Motion::Right),
];
const G_TABS: &[PreviewOp] = &[core(CoreAction::TabPrev), core(CoreAction::TabNext)];
const G_FILE_JUMP: &[PreviewOp] = &[host(HostOp::PreviewFileNext), host(HostOp::PreviewFilePrev)];
const G_ZOOM: &[PreviewOp] = &[
    core(CoreAction::ImageZoomIn),
    core(CoreAction::ImageZoomOut),
];
const G_SELECT: &[PreviewOp] = &[
    host(HostOp::PreviewEnterVisual),
    host(HostOp::PreviewEnterVisualLine),
];
const G_TOGGLE_ITEM: &[PreviewOp] = &[host(HostOp::ToggleFocusedItem), PreviewOp::LinkOpen];

/// The footer entries a dynamic token stands for (worked out from the state by the host).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PvDyn {
    /// `n/N:match[3/12]` while a search is active.
    SearchStatus,
    /// `n/N:change` while a file with changes is on screen and nothing is searched.
    ChangeStatus,
    /// `y c:copy code` while a code block is focused.
    CopyCode,
}

/// One entry of a footer plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PvFoot {
    /// One operation, labelled as the host's declaration says it reads now.
    Op(PreviewOp),
    /// One operation under a label of this plan's own.
    As(PreviewOp, CoreMsg),
    /// The keys of a group under one label.
    Group(PvGroup, CoreMsg),
    /// Like `Group`, labelled as the declaration says for the group's first operation.
    GroupOp(PvGroup),
    /// The keys of a group merged.
    Union(PvGroup, CoreMsg),
    /// A token worked out from the state.
    Dyn(PvDyn),
    /// An entry of the shared layers.
    Core(CoreFoot),
}

/// The footer plan of a preview screen while nothing more specific is going on.
pub fn footer_plan(ctx: CoreContext, st: &PreviewState) -> Vec<PvFoot> {
    match ctx {
        CoreContext::Text | CoreContext::Document => text_plan(st),
        CoreContext::Image => vec![
            PvFoot::Core(CoreFoot::Zoom),
            PvFoot::Core(CoreFoot::ZoomReset),
            PvFoot::Core(CoreFoot::Pan),
            PvFoot::Core(CoreFoot::PageTurn),
            PvFoot::Core(CoreFoot::Outline),
            PvFoot::Op(host(HostOp::ImageReturnToDiff)),
            PvFoot::Group(PvGroup::FileJump, CoreMsg::HintFileJump),
            PvFoot::Core(CoreFoot::Back),
            PvFoot::Core(CoreFoot::Help),
            PvFoot::Core(CoreFoot::Edit),
            PvFoot::Core(CoreFoot::Open),
            PvFoot::Group(PvGroup::Tabs, CoreMsg::HintTab),
            PvFoot::Core(CoreFoot::Options),
        ],
        CoreContext::Table => vec![
            PvFoot::Core(CoreFoot::Cell),
            PvFoot::Op(PreviewOp::ToggleTableCell),
            PvFoot::Dyn(PvDyn::SearchStatus),
            PvFoot::Core(CoreFoot::PageTurn),
            PvFoot::Core(CoreFoot::Outline),
            PvFoot::Op(host(HostOp::CycleRepresentation)),
            PvFoot::Op(host(HostOp::SearchStart)),
            PvFoot::Core(CoreFoot::TableCopy),
            PvFoot::Core(CoreFoot::Ends),
            PvFoot::Group(PvGroup::FileJump, CoreMsg::HintFileJump),
            PvFoot::Core(CoreFoot::Back),
            PvFoot::Core(CoreFoot::Help),
            PvFoot::Core(CoreFoot::Edit),
            PvFoot::Core(CoreFoot::Open),
            PvFoot::Group(PvGroup::Tabs, CoreMsg::HintTab),
            PvFoot::Core(CoreFoot::Options),
        ],
        CoreContext::Canvas => vec![
            PvFoot::Core(CoreFoot::Back),
            PvFoot::Core(CoreFoot::PageTurn),
            PvFoot::Core(CoreFoot::Outline),
            PvFoot::Op(host(HostOp::CycleRepresentation)),
            PvFoot::Group(PvGroup::FileJump, CoreMsg::HintFileJump),
            PvFoot::Core(CoreFoot::Help),
            PvFoot::Core(CoreFoot::Edit),
            PvFoot::Core(CoreFoot::Open),
            PvFoot::Group(PvGroup::Tabs, CoreMsg::HintTab),
            PvFoot::Core(CoreFoot::Options),
        ],
    }
}

fn text_plan(st: &PreviewState) -> Vec<PvFoot> {
    if st.text_missing {
        return vec![
            PvFoot::Op(core(CoreAction::ToggleFollow)),
            PvFoot::Group(PvGroup::FileJump, CoreMsg::HintFileJump),
            PvFoot::Core(CoreFoot::Back),
            PvFoot::Core(CoreFoot::Help),
            PvFoot::Core(CoreFoot::Edit),
            PvFoot::Core(CoreFoot::Open),
            PvFoot::Group(PvGroup::Tabs, CoreMsg::HintTab),
            PvFoot::Core(CoreFoot::Options),
        ];
    }
    if st.decorated {
        // `Tab` always cycles focus; everything else here depends on the focused item.
        let mut v = vec![
            PvFoot::Core(CoreFoot::Scroll),
            PvFoot::Op(host(HostOp::LinkFocusNext)),
            PvFoot::Core(CoreFoot::PageTurn),
        ];
        match st.focus {
            Some(Focus::Task) | Some(Focus::Details) => {
                v.push(PvFoot::Union(PvGroup::ToggleItem, CoreMsg::HintToggle));
            }
            Some(Focus::CodeBlock) => v.push(PvFoot::Dyn(PvDyn::CopyCode)),
            Some(Focus::MermaidFence) => {
                v.push(PvFoot::Op(PreviewOp::LinkOpen));
                v.push(PvFoot::Group(PvGroup::Zoom, CoreMsg::Zoom));
                if st.mermaid_pannable {
                    v.push(PvFoot::Group(PvGroup::Hjkl, CoreMsg::HintPan));
                    v.push(PvFoot::Group(PvGroup::Home, CoreMsg::HintFit));
                }
            }
            Some(_) => {
                v.push(PvFoot::Op(PreviewOp::LinkOpen));
                v.push(PvFoot::Op(host(HostOp::OpenLinkNewTab)));
            }
            None => {}
        }
        v.extend([
            PvFoot::Core(CoreFoot::Outline),
            PvFoot::Op(host(HostOp::CycleRepresentation)),
            PvFoot::Op(host(HostOp::SearchStart)),
            PvFoot::Dyn(PvDyn::ChangeStatus),
            PvFoot::Op(core(CoreAction::ToggleFollow)),
            PvFoot::Group(PvGroup::FileJump, CoreMsg::HintFileJump),
            PvFoot::Core(CoreFoot::Back),
            PvFoot::Core(CoreFoot::Help),
            PvFoot::Core(CoreFoot::Edit),
            PvFoot::Core(CoreFoot::Open),
            PvFoot::Core(CoreFoot::Ends),
            PvFoot::Group(PvGroup::Tabs, CoreMsg::HintTab),
            PvFoot::Core(CoreFoot::Options),
            PvFoot::Core(CoreFoot::PageKeys),
        ]);
        return v;
    }
    // Code / plain text. While searching, the match status is up front.
    let mut v = vec![
        PvFoot::Core(CoreFoot::Scroll),
        PvFoot::Dyn(PvDyn::SearchStatus),
        PvFoot::Op(host(HostOp::SearchStart)),
        PvFoot::Dyn(PvDyn::ChangeStatus),
    ];
    // Range-selection copy is windowed text only; the label of `Y` is the short one here.
    if st.windowed {
        v.push(PvFoot::Group(PvGroup::Select, CoreMsg::HintSelect));
        v.push(PvFoot::As(
            host(HostOp::PreviewCopySelectionRef),
            CoreMsg::WkAtRef,
        ));
    }
    v.push(PvFoot::Op(core(CoreAction::ToggleFollow)));
    v.push(PvFoot::Op(PreviewOp::ToggleFollowDiffScope));
    v.extend([
        PvFoot::Op(host(HostOp::CycleRepresentation)),
        PvFoot::Group(PvGroup::FileJump, CoreMsg::HintFileJump),
        PvFoot::Core(CoreFoot::Back),
        PvFoot::Core(CoreFoot::Help),
        PvFoot::Core(CoreFoot::Edit),
        PvFoot::Core(CoreFoot::Open),
        PvFoot::Core(CoreFoot::Ends),
        PvFoot::Core(CoreFoot::HScroll),
        PvFoot::Core(CoreFoot::LineEnds),
        PvFoot::Group(PvGroup::Tabs, CoreMsg::HintTab),
        PvFoot::Core(CoreFoot::Options),
        PvFoot::Core(CoreFoot::PageKeys),
    ]);
    v
}

/// The operations of a footer plan in plan order, each with the label the plan gives it (the
/// order of importance: the footer cuts from the end, and the `Space` menu lists in this order).
/// `op_label` says what the declaration calls an operation now (a plan's `Op` entry has no label
/// of its own), `word` turns a shared message into the host's own. A dynamic token names no
/// single operation and is left out; so is a menu key.
pub fn priority<L: Copy>(
    ctx: CoreContext,
    st: &PreviewState,
    facts: &HelpFacts,
    op_label: impl Fn(PreviewOp) -> Option<L>,
    word: impl Fn(CoreMsg) -> L,
) -> Vec<(PreviewOp, Option<L>)> {
    let mut v: Vec<(PreviewOp, Option<L>)> = Vec::new();
    for f in footer_plan(ctx, st) {
        match f {
            PvFoot::Op(o) => v.push((o, op_label(o))),
            PvFoot::As(o, m) => v.push((o, Some(word(m)))),
            PvFoot::Group(g, m) | PvFoot::Union(g, m) => {
                v.extend(g.ops().iter().map(|o| (*o, Some(word(m)))))
            }
            PvFoot::GroupOp(g) => {
                let label = g.ops().first().and_then(|o| op_label(*o));
                v.extend(g.ops().iter().map(|o| (*o, label)));
            }
            PvFoot::Dyn(_) => {}
            PvFoot::Core(c) => {
                for (op, label) in foot_priority(c, ctx, facts) {
                    let o = match op {
                        HelpOp::Act(a) => PreviewOp::Core(a),
                        HelpOp::Cancel => PreviewOp::Cancel,
                        HelpOp::Menu(_) => continue,
                    };
                    v.push((o, label.map(&word)));
                }
            }
        }
    }
    v
}

const SEARCH: &[PreviewOp] = &[host(HostOp::SearchStart)];
const SEARCH_MATCH: &[PreviewOp] = &[host(HostOp::SearchNext), host(HostOp::SearchPrev)];
const SELECT: &[PreviewOp] = G_SELECT;
const PAGE_TURN: &[PreviewOp] = &[core(CoreAction::PageNext), core(CoreAction::PagePrev)];
const PAGE_KEYS: &[PreviewOp] = &[nav(Motion::PageDown), nav(Motion::PageUp)];
const HALF_KEYS: &[PreviewOp] = &[nav(Motion::HalfDown), nav(Motion::HalfUp)];
const HSCROLL: &[PreviewOp] = &[nav(Motion::Left), nav(Motion::Right)];
const LINE_ENDS: &[PreviewOp] = &[nav(Motion::LineHome), nav(Motion::LineEnd)];
const BOOKMARKS: &[PreviewOp] = &[host(HostOp::MarkSet), host(HostOp::MarkJump)];
const FOCUS: &[PreviewOp] = &[host(HostOp::LinkFocusNext), host(HostOp::LinkFocusPrev)];
const ZOOM_ALL: &[PreviewOp] = &[
    core(CoreAction::ImageZoomIn),
    core(CoreAction::ImageZoomOut),
    core(CoreAction::ImageZoomReset),
];
const BACK: &[PreviewOp] = &[core(CoreAction::PreviewBack), PreviewOp::Cancel];

/// The operations the `?` help of a preview screen declares for the screen itself, in the order
/// it declares them (one entry per row's group; an operation a row repeats is repeated). A
/// host lists the operations that are not in the footer plan in this order.
pub fn help_order(ctx: CoreContext, st: &PreviewState) -> Vec<PreviewOp> {
    let mut v: Vec<PreviewOp> = Vec::new();
    let edit = core(CoreAction::RequestEdit);
    let open = core(CoreAction::OpenExternal);
    let launch = [
        edit,
        open,
        host(HostOp::OpenGitView),
        host(HostOp::GitLaunchTool),
    ];
    match ctx {
        CoreContext::Text | CoreContext::Document => {
            if st.text_missing {
                v.extend_from_slice(PvGroup::FileJump.ops());
                v.extend_from_slice(BOOKMARKS);
                v.push(edit);
                v.push(open);
                v.extend_from_slice(BACK);
            } else {
                v.extend_from_slice(PvGroup::Scroll.ops());
                v.extend_from_slice(PvGroup::Ends.ops());
                v.extend_from_slice(HSCROLL);
                v.extend_from_slice(LINE_ENDS);
                v.extend_from_slice(SEARCH);
                v.extend_from_slice(SEARCH_MATCH);
                v.extend_from_slice(SELECT);
                v.push(host(HostOp::PreviewCopySelectionRef));
                v.extend_from_slice(PAGE_TURN);
                v.push(host(HostOp::CycleRepresentation));
                v.push(core(CoreAction::ToggleOutline));
                if ctx == CoreContext::Document {
                    v.extend_from_slice(FOCUS);
                    v.push(PreviewOp::LinkOpen);
                    v.push(host(HostOp::OpenLinkNewTab));
                    if !st.office_document {
                        v.extend_from_slice(ZOOM_ALL);
                        v.extend_from_slice(PvGroup::ToggleItem.ops());
                        v.extend_from_slice(PvGroup::ToggleItem.ops());
                    }
                }
                v.extend_from_slice(PvGroup::FileJump.ops());
                v.extend_from_slice(BOOKMARKS);
                v.extend_from_slice(&launch);
                v.push(host(HostOp::Refresh));
                v.extend_from_slice(PAGE_KEYS);
                v.extend_from_slice(HALF_KEYS);
                v.push(core(CoreAction::CyclePathStyle));
                v.extend_from_slice(BACK);
            }
        }
        CoreContext::Image => {
            v.extend_from_slice(PvGroup::Zoom.ops());
            v.push(core(CoreAction::ImageZoomReset));
            v.extend_from_slice(PvGroup::Hjkl.ops());
            v.extend_from_slice(PAGE_TURN);
            v.push(core(CoreAction::ToggleOutline));
            v.push(host(HostOp::ImageReturnToDiff));
            v.extend_from_slice(PvGroup::FileJump.ops());
            v.extend_from_slice(BOOKMARKS);
            v.extend_from_slice(&launch);
            v.push(host(HostOp::Refresh));
            v.push(core(CoreAction::CyclePathStyle));
            v.extend_from_slice(BACK);
        }
        CoreContext::Canvas => {
            v.extend_from_slice(PAGE_TURN);
            v.push(core(CoreAction::ToggleOutline));
            v.push(host(HostOp::CycleRepresentation));
            v.extend_from_slice(PvGroup::FileJump.ops());
            v.extend_from_slice(BOOKMARKS);
            v.extend_from_slice(&launch);
            v.push(host(HostOp::Refresh));
            v.push(host(HostOp::PreviewCopySelectionRef));
            v.extend_from_slice(BACK);
        }
        CoreContext::Table => {
            v.extend_from_slice(PvGroup::Hjkl.ops());
            v.extend_from_slice(PvGroup::Ends.ops());
            v.extend_from_slice(LINE_ENDS);
            v.extend_from_slice(PAGE_TURN);
            v.push(core(CoreAction::ToggleOutline));
            v.push(host(HostOp::CycleRepresentation));
            v.extend_from_slice(SEARCH);
            v.extend_from_slice(SEARCH_MATCH);
            v.push(PreviewOp::ToggleTableCell);
            v.extend_from_slice(PAGE_KEYS);
            v.extend_from_slice(HALF_KEYS);
            v.extend_from_slice(PvGroup::FileJump.ops());
            v.extend_from_slice(BOOKMARKS);
            v.extend_from_slice(&launch);
            v.push(host(HostOp::Refresh));
            v.push(core(CoreAction::CyclePathStyle));
            v.extend_from_slice(BACK);
        }
    }
    v
}

/// The operations the help lists for every screen (layer 0), in the order it lists them.
pub fn help_everywhere() -> Vec<PreviewOp> {
    vec![
        PreviewOp::TabNew,
        core(CoreAction::TabNewAfter),
        core(CoreAction::TabPrev),
        core(CoreAction::TabNext),
        core(CoreAction::ToggleTabList),
        core(CoreAction::PasteJump),
        core(CoreAction::ToggleHelp),
        core(CoreAction::ToggleFollow),
        core(CoreAction::Quit),
    ]
}

/// Whether a menu can name `op` on a preview screen in state `st`: it has a label (the footer's,
/// the declaration's, the menu's own) or, for `R`, the label of the representation the open
/// plugin steps to. An operation without a name is not a candidate of the `Space` menu, so it
/// takes no letter. Movement, `Esc` and the payloads of the menus are never named.
pub fn is_named(op: PreviewOp, st: &PreviewState) -> bool {
    match op {
        PreviewOp::Core(a) => match a {
            CoreAction::Navigate(_)
            | CoreAction::TabGoto(_)
            | CoreAction::CopyPath(_)
            | CoreAction::TableCopy(_)
            | CoreAction::SortSet(_) => false,
            CoreAction::RequestEdit => st.edit_named,
            _ => true,
        },
        PreviewOp::Host(o) => match o {
            HostOp::CycleRepresentation => st.repr_step || st.repr_text,
            HostOp::ImageReturnToDiff => st.repr_step || st.returns_to_diff,
            // Named by the label the focused link gives it.
            HostOp::OpenLinkNewTab => st.focus == Some(Focus::LocalLink),
            _ => true,
        },
        PreviewOp::Cancel => false,
        PreviewOp::LinkOpen => matches!(
            st.focus,
            Some(
                Focus::LocalLink
                    | Focus::AnchorLink
                    | Focus::ExternalLink
                    | Focus::Task
                    | Focus::Details
                    | Focus::MermaidFence
            )
        ),
        // `TabNew` (the fixed key for a tab at the end) has no label anywhere: the menu offers
        // `TabNewAfter` instead.
        PreviewOp::TabNew => false,
        PreviewOp::ToggleTableCell | PreviewOp::ToggleFollowDiffScope => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTXS: [CoreContext; 5] = [
        CoreContext::Text,
        CoreContext::Document,
        CoreContext::Image,
        CoreContext::Table,
        CoreContext::Canvas,
    ];

    fn states() -> Vec<PreviewState> {
        let mut v = Vec::new();
        let focuses = [
            None,
            Some(Focus::Task),
            Some(Focus::Details),
            Some(Focus::CodeBlock),
            Some(Focus::MermaidFence),
            Some(Focus::LocalLink),
            Some(Focus::AnchorLink),
            Some(Focus::ExternalLink),
        ];
        for text_missing in [false, true] {
            for decorated in [false, true] {
                for focus in focuses {
                    for pan in [false, true] {
                        for windowed in [false, true] {
                            v.push(PreviewState {
                                text_missing,
                                decorated,
                                focus,
                                mermaid_pannable: pan,
                                windowed,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }
        v
    }

    /// Each state changes exactly what the field says it reads, and nothing else.
    #[test]
    fn the_footer_plan_reads_only_the_fields_it_names() {
        let base = PreviewState {
            decorated: true,
            ..Default::default()
        };
        let plan = |st: PreviewState| footer_plan(CoreContext::Document, &st);
        // The label-only fields change nothing.
        for st in [
            PreviewState {
                office_document: true,
                ..base
            },
            PreviewState {
                edit_named: true,
                ..base
            },
            PreviewState {
                repr_step: true,
                repr_text: true,
                returns_to_diff: true,
                ..base
            },
        ] {
            assert_eq!(plan(st), plan(base));
        }
        // A pannable diagram only matters while a diagram is focused.
        let pan = PreviewState {
            mermaid_pannable: true,
            ..base
        };
        assert_eq!(plan(pan), plan(base));
        let fence = PreviewState {
            focus: Some(Focus::MermaidFence),
            ..base
        };
        let fence_pan = PreviewState {
            mermaid_pannable: true,
            ..fence
        };
        assert_eq!(plan(fence_pan).len(), plan(fence).len() + 2);
        // `windowed` only matters for the text, not the decorated view.
        let w = PreviewState {
            windowed: true,
            ..base
        };
        assert_eq!(plan(w), plan(base));
        let code = PreviewState::default();
        let code_w = PreviewState {
            windowed: true,
            ..code
        };
        assert_eq!(
            footer_plan(CoreContext::Text, &code_w).len(),
            footer_plan(CoreContext::Text, &code).len() + 2
        );
        // The Word text that has not arrived has the shortest plan.
        let missing = PreviewState {
            text_missing: true,
            decorated: true,
            windowed: true,
            ..Default::default()
        };
        assert_eq!(plan(missing).len(), 8);
    }

    /// The plan's order is the order the footer shows: the movement first, the tabs and the view
    /// menu last. A swapped pair would change the `Space` menu's order.
    #[test]
    fn the_plans_lead_and_close_the_way_the_footer_does() {
        let code = footer_plan(CoreContext::Text, &PreviewState::default());
        assert_eq!(code[0], PvFoot::Core(CoreFoot::Scroll));
        assert_eq!(code.last(), Some(&PvFoot::Core(CoreFoot::PageKeys)));
        let table = footer_plan(CoreContext::Table, &PreviewState::default());
        assert_eq!(table[0], PvFoot::Core(CoreFoot::Cell));
        assert_eq!(table[1], PvFoot::Op(PreviewOp::ToggleTableCell));
        let canvas = footer_plan(CoreContext::Canvas, &PreviewState::default());
        assert_eq!(canvas[0], PvFoot::Core(CoreFoot::Back));
        let image = footer_plan(CoreContext::Image, &PreviewState::default());
        assert_eq!(image[0], PvFoot::Core(CoreFoot::Zoom));
    }

    /// No operation is planned twice, but the movement keys: a zoomed diagram's `hjkl` pan names
    /// `j` and `k` again after the scroll.
    #[test]
    fn a_plan_never_names_an_operation_twice() {
        let mut checked = 0;
        for ctx in CTXS {
            for st in states() {
                let facts = HelpFacts::default();
                let p = priority(ctx, &st, &facts, |_| Some(()), |_| ());
                let mut seen: Vec<PreviewOp> = Vec::new();
                for (o, _) in p {
                    assert!(
                        !seen.contains(&o) || matches!(o, PreviewOp::Core(CoreAction::Navigate(_))),
                        "{ctx:?} {st:?}: {o:?} twice"
                    );
                    seen.push(o);
                    checked += 1;
                }
            }
        }
        assert!(checked >= 5 * 200, "{checked}");
    }

    /// The help names the same operations whatever the state, but for the Word text that has not
    /// arrived and the Office text without diagrams.
    #[test]
    fn the_help_order_reads_only_the_fields_it_names() {
        let base = PreviewState::default();
        let order = |ctx, st| help_order(ctx, &st);
        for ctx in CTXS {
            for st in states() {
                let st = PreviewState {
                    text_missing: false,
                    ..st
                };
                assert_eq!(order(ctx, st), order(ctx, base), "{ctx:?} {st:?}");
            }
        }
        let office = PreviewState {
            office_document: true,
            ..base
        };
        assert!(
            order(CoreContext::Document, office).len() < order(CoreContext::Document, base).len()
        );
        assert_eq!(
            order(CoreContext::Text, office),
            order(CoreContext::Text, base)
        );
        let missing = PreviewState {
            text_missing: true,
            ..base
        };
        assert_eq!(order(CoreContext::Text, missing).len(), 2 + 2 + 1 + 1 + 2);
        assert_eq!(
            order(CoreContext::Image, missing),
            order(CoreContext::Image, base)
        );
    }

    /// An operation is named exactly where the host's declarations name it; the state-dependent
    /// ones follow their field.
    #[test]
    fn what_is_named_follows_the_state_fields() {
        let base = PreviewState::default();
        let e = PreviewOp::Core(CoreAction::RequestEdit);
        assert!(!is_named(e, &base));
        assert!(is_named(
            e,
            &PreviewState {
                edit_named: true,
                ..base
            }
        ));
        let r = PreviewOp::Host(HostOp::CycleRepresentation);
        assert!(!is_named(r, &base));
        for st in [
            PreviewState {
                repr_step: true,
                ..base
            },
            PreviewState {
                repr_text: true,
                ..base
            },
        ] {
            assert!(is_named(r, &st));
        }
        let d = PreviewOp::Host(HostOp::ImageReturnToDiff);
        assert!(!is_named(
            d,
            &PreviewState {
                repr_text: true,
                ..base
            }
        ));
        assert!(is_named(
            d,
            &PreviewState {
                returns_to_diff: true,
                ..base
            }
        ));
        assert!(is_named(
            d,
            &PreviewState {
                repr_step: true,
                ..base
            }
        ));
        let n = PreviewOp::Host(HostOp::OpenLinkNewTab);
        let l = PreviewOp::LinkOpen;
        for f in [
            None,
            Some(Focus::Task),
            Some(Focus::Details),
            Some(Focus::CodeBlock),
            Some(Focus::MermaidFence),
            Some(Focus::LocalLink),
            Some(Focus::AnchorLink),
            Some(Focus::ExternalLink),
        ] {
            let st = PreviewState { focus: f, ..base };
            assert_eq!(is_named(n, &st), f == Some(Focus::LocalLink), "{f:?}");
            assert_eq!(
                is_named(l, &st),
                !matches!(f, None | Some(Focus::CodeBlock)),
                "{f:?}"
            );
        }
        assert!(!is_named(PreviewOp::Cancel, &base));
        assert!(!is_named(nav(Motion::Down), &base));
        assert!(!is_named(PreviewOp::TabNew, &base));
    }

    /// The focus-dependent middle of the decorated plan, written out by hand.
    #[test]
    fn the_decorated_plan_follows_the_focus_literally() {
        let middle = |focus, pan| {
            let st = PreviewState {
                decorated: true,
                focus,
                mermaid_pannable: pan,
                ..Default::default()
            };
            let plan = footer_plan(CoreContext::Document, &st);
            let from = plan
                .iter()
                .position(|f| *f == PvFoot::Core(CoreFoot::PageTurn))
                .unwrap()
                + 1;
            let to = plan
                .iter()
                .position(|f| *f == PvFoot::Core(CoreFoot::Outline))
                .unwrap();
            plan[from..to].to_vec()
        };
        let toggle = vec![PvFoot::Union(PvGroup::ToggleItem, CoreMsg::HintToggle)];
        assert_eq!(middle(None, false), vec![]);
        assert_eq!(middle(Some(Focus::Task), false), toggle);
        assert_eq!(middle(Some(Focus::Details), false), toggle);
        assert_eq!(
            middle(Some(Focus::CodeBlock), false),
            vec![PvFoot::Dyn(PvDyn::CopyCode)]
        );
        let link = vec![
            PvFoot::Op(PreviewOp::LinkOpen),
            PvFoot::Op(host(HostOp::OpenLinkNewTab)),
        ];
        for f in [Focus::LocalLink, Focus::AnchorLink, Focus::ExternalLink] {
            assert_eq!(middle(Some(f), false), link, "{f:?}");
        }
        let fit = vec![
            PvFoot::Op(PreviewOp::LinkOpen),
            PvFoot::Group(PvGroup::Zoom, CoreMsg::Zoom),
        ];
        assert_eq!(middle(Some(Focus::MermaidFence), false), fit);
        let mut pan = fit;
        pan.push(PvFoot::Group(PvGroup::Hjkl, CoreMsg::HintPan));
        pan.push(PvFoot::Group(PvGroup::Home, CoreMsg::HintFit));
        assert_eq!(middle(Some(Focus::MermaidFence), true), pan);
    }
}
