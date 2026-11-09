//! The diff screen's presentation, derived only from resolved values for one frame.
use crate::host::acts::{fixed_menu_items, FixedMenuLabel};
use konoma_host_core::{
    help::{footer_key, help_key, zip_columns, CoreSection},
    keymap::{KeyPress, Motion},
    menu::Item,
};
use konoma_widgets::media_diff::MediaDiffLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffUiOp {
    Navigate(Motion),
    Layout,
    PageNext,
    PagePrev,
    View,
    NextFile,
    PrevFile,
    NextBlock,
    PrevBlock,
    FollowScope,
    Edit,
    OpenExternal,
    Project,
    Tool,
    Refresh,
    Discard,
    Back,
    Cancel,
    Options,
    PathStyle,
    Tab(usize),
    Global(usize),
    Follow,
    /// A host action in a user menu, identified by its index in the resolved declaration.
    Host(usize),
}

impl DiffUiOp {
    pub fn core(self) -> Option<konoma_host_core::help::HelpOp> {
        use konoma_host_core::{
            help::HelpOp as H,
            keymap::{CoreAction as A, CoreMenu},
        };
        Some(match self {
            Self::Navigate(m) => H::Act(A::Navigate(m)),
            Self::Layout => H::Act(A::CycleDiffLayout),
            Self::PageNext => H::Act(A::PageNext),
            Self::PagePrev => H::Act(A::PagePrev),
            Self::Edit => H::Act(A::RequestEdit),
            Self::OpenExternal => H::Act(A::OpenExternal),
            Self::Back => H::Act(A::PreviewBack),
            Self::Cancel => H::Cancel,
            Self::Options => H::Menu(CoreMenu::Options),
            Self::PathStyle => H::Act(A::CyclePathStyle),
            Self::Global(0) => H::Act(A::PasteJump),
            Self::Global(1) => H::Act(A::ToggleHelp),
            Self::Global(2) => H::Act(A::Quit),
            Self::Follow => H::Act(A::ToggleFollow),
            Self::Tab(1) => H::Act(A::TabNewAfter),
            Self::Tab(2) => H::Act(A::TabPrev),
            Self::Tab(3) => H::Act(A::TabNext),
            Self::Tab(4) => H::Act(A::ToggleTabList),
            _ => return None,
        })
    }
    pub fn from_core(op: konoma_host_core::help::HelpOp) -> Option<Self> {
        use konoma_host_core::{
            help::HelpOp as H,
            keymap::{CoreAction as A, CoreMenu},
        };
        Some(match op {
            H::Act(A::Navigate(m)) => Self::Navigate(m),
            H::Act(A::CycleDiffLayout) => Self::Layout,
            H::Act(A::PageNext) => Self::PageNext,
            H::Act(A::PagePrev) => Self::PagePrev,
            H::Act(A::RequestEdit) => Self::Edit,
            H::Act(A::OpenExternal) => Self::OpenExternal,
            H::Act(A::PreviewBack) => Self::Back,
            H::Cancel => Self::Cancel,
            H::Menu(CoreMenu::Options) => Self::Options,
            H::Act(A::CyclePathStyle) => Self::PathStyle,
            H::Act(A::PasteJump) => Self::Global(0),
            H::Act(A::ToggleHelp) => Self::Global(1),
            H::Act(A::Quit) => Self::Global(2),
            H::Act(A::ToggleFollow) => Self::Follow,
            H::Act(A::TabNewAfter) => Self::Tab(1),
            H::Act(A::TabPrev) => Self::Tab(2),
            H::Act(A::TabNext) => Self::Tab(3),
            H::Act(A::ToggleTabList) => Self::Tab(4),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffSurface {
    #[default]
    Source,
    Rendered,
    Media,
    Summary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffRepresentation {
    Source,
    Rendered,
    Preview,
}

#[derive(Clone, Debug)]
pub struct DiffPresentationState {
    pub surface: DiffSurface,
    pub wrap: bool,
    pub media_pictures: bool,
    pub can_page: bool,
    pub next_representation: Option<DiffRepresentation>,
    pub representation_count: usize,
    pub next_media_layout: MediaDiffLayout,
    pub change_blocks: bool,
    pub change_files: bool,
    pub follow_scope: bool,
    pub jj: bool,
    pub read_only: bool,
}

impl Default for DiffPresentationState {
    fn default() -> Self {
        Self {
            surface: DiffSurface::Source,
            wrap: false,
            media_pictures: false,
            can_page: false,
            next_representation: None,
            representation_count: 1,
            next_media_layout: MediaDiffLayout::Auto,
            change_blocks: false,
            change_files: true,
            follow_scope: false,
            jj: false,
            read_only: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DiffCapabilities {
    pub global: bool,
    pub tabs: bool,
    pub follow: bool,
    pub discard: bool,
    pub tool: bool,
    pub project: bool,
    pub view: bool,
    pub blocks: bool,
}

impl Default for DiffCapabilities {
    fn default() -> Self {
        Self {
            global: false,
            tabs: false,
            follow: false,
            discard: true,
            tool: false,
            project: true,
            view: true,
            blocks: true,
        }
    }
}

impl DiffCapabilities {
    pub fn provides(self, op: DiffUiOp) -> bool {
        match op {
            DiffUiOp::Global(_) => self.global,
            DiffUiOp::Tab(_) => self.tabs,
            DiffUiOp::Follow | DiffUiOp::FollowScope => self.follow,
            DiffUiOp::Discard => self.discard,
            DiffUiOp::Tool => self.tool,
            DiffUiOp::Project => self.project,
            DiffUiOp::View => self.view,
            DiffUiOp::NextBlock | DiffUiOp::PrevBlock => self.blocks,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffTextId {
    MediaLayout(MediaDiffLayout),
    MediaLayoutHelp,
    Representation(DiffRepresentation),
    CyclePair,
    CycleAll,
    Files,
}

impl DiffPresentationState {
    pub fn enabled(&self, capabilities: DiffCapabilities, op: DiffUiOp) -> bool {
        if !capabilities.provides(op) {
            return false;
        }
        match op {
            DiffUiOp::Layout => self.media_pictures || self.surface == DiffSurface::Source,
            DiffUiOp::PageNext | DiffUiOp::PagePrev => self.can_page,
            DiffUiOp::View => self.next_representation.is_some(),
            DiffUiOp::NextBlock | DiffUiOp::PrevBlock => self.change_blocks,
            DiffUiOp::NextFile | DiffUiOp::PrevFile => self.change_files,
            DiffUiOp::FollowScope => self.follow_scope,
            DiffUiOp::Discard => !self.jj && !self.read_only,
            _ => true,
        }
    }
    /// Dynamic label selection, shared with the host's execution declarations.
    pub fn text(&self, op: DiffUiOp) -> (Option<DiffTextId>, Option<DiffTextId>) {
        match op {
            DiffUiOp::Layout if self.media_pictures => (
                Some(DiffTextId::MediaLayout(self.next_media_layout)),
                Some(DiffTextId::MediaLayoutHelp),
            ),
            DiffUiOp::View => (
                self.next_representation.map(DiffTextId::Representation),
                (self.representation_count > 1).then_some(if self.representation_count == 2 {
                    DiffTextId::CyclePair
                } else {
                    DiffTextId::CycleAll
                }),
            ),
            DiffUiOp::NextFile | DiffUiOp::PrevFile => (Some(DiffTextId::Files), None),
            _ => (None, None),
        }
    }
    fn reduced(&self) -> bool {
        matches!(self.surface, DiffSurface::Media | DiffSurface::Summary)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffHelpSection {
    View,
    Move,
    Launch,
    Tabs,
    Global,
}

#[derive(Clone, Debug)]
pub struct DiffBinding {
    pub op: DiffUiOp,
    pub keys: Vec<KeyPress>,
    pub enabled: bool,
    pub label: Option<String>,
    pub help: Option<String>,
    pub launch: bool,
    pub layers: Vec<(KeyPress, DiffHelpSection)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffWord {
    Scroll,
    Files,
    Change,
    Hscroll,
    Back,
    Options,
    HorizontalEnds,
    BackHelp,
    ToolHelp,
}

#[derive(Clone, Debug)]
pub struct DiffPresentationWords {
    pub view: String,
    pub movement: String,
    pub launch: String,
    pub tabs: String,
    pub global: String,
    pub words: Vec<(DiffWord, String)>,
    pub dynamic: Vec<(DiffTextId, String)>,
}

impl DiffPresentationWords {
    fn word(&self, word: DiffWord) -> &str {
        self.words
            .iter()
            .find(|(id, _)| *id == word)
            .map_or("", |(_, s)| s)
    }
    fn dynamic(&self, id: DiffTextId) -> Option<&str> {
        self.dynamic
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, s)| s.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHelpRow {
    pub keys_text: String,
    pub description: String,
    pub keys: Vec<KeyPress>,
    pub ops: Vec<DiffUiOp>,
    pub section: DiffHelpSection,
    pub ungated: bool,
}

#[derive(Clone, Debug)]
pub struct DiffRowDeclaration {
    pub groups: Vec<Vec<DiffUiOp>>,
    pub literal: Option<&'static str>,
    pub description: Option<DiffWord>,
}

#[derive(Clone, Debug)]
pub struct DiffHelpMenu {
    pub title: String,
    pub prefix: String,
    pub items: Vec<Item<DiffUiOp, String>>,
}

#[derive(Clone, Debug)]
pub struct DiffToken {
    /// Position before enabled/unbound entries are removed, for host plugin insertion.
    pub plan_index: usize,
    pub text: String,
    pub keys: Vec<KeyPress>,
    pub ops: Vec<DiffUiOp>,
}

#[derive(Clone, Copy, Debug)]
pub struct DiffPriority {
    pub plan_index: usize,
    pub op: DiffUiOp,
    pub word: Option<DiffWord>,
}

struct Foot {
    ops: Vec<DiffUiOp>,
    word: Option<DiffWord>,
    union: bool,
}

pub struct DiffPresentation {
    state: DiffPresentationState,
    capabilities: DiffCapabilities,
    bindings: Vec<DiffBinding>,
    words: DiffPresentationWords,
    declarations: Vec<DiffRowDeclaration>,
    rows: Vec<Option<DiffHelpRow>>,
    footer: Vec<DiffToken>,
    priority: Vec<DiffPriority>,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {static BUILDS:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[cfg(any(test, feature = "test-hooks"))]
pub fn diff_presentation_build_count() -> usize {
    BUILDS.with(std::cell::Cell::get)
}

/// The default declaration only; configured menus must be passed to `options_menu` unchanged.
pub fn default_options() -> [(KeyPress, DiffUiOp); 2] {
    [
        (KeyPress::ch('f'), DiffUiOp::FollowScope),
        (KeyPress::ch('p'), DiffUiOp::PathStyle),
    ]
}

impl DiffPresentation {
    /// Standalone adapter: resolve shared keys from the small host's actual keymap, keeping
    /// its translated declarations and the keys of operations supplied by the project host.
    pub fn build_with_lookup(
        state: &DiffPresentationState,
        capabilities: DiffCapabilities,
        bindings: &[DiffBinding],
        words: &DiffPresentationWords,
        lookup: &impl konoma_host_core::help::KeyLookup,
    ) -> Self {
        use konoma_host_core::help::KeyLayer;
        let bindings: Vec<_> = bindings
            .iter()
            .cloned()
            .map(|mut binding| {
                if let Some(op) = binding.op.core() {
                    binding.keys = lookup.keys_of(op);
                    binding.layers = binding
                        .keys
                        .iter()
                        .map(|key| {
                            (
                                *key,
                                match lookup.layer_of(*key) {
                                    Some(KeyLayer::Global) => DiffHelpSection::Global,
                                    Some(KeyLayer::Basic) => DiffHelpSection::Move,
                                    _ => DiffHelpSection::View,
                                },
                            )
                        })
                        .collect();
                }
                binding
            })
            .collect();
        Self::build(state, capabilities, &bindings, words)
    }
    pub fn build(
        state: &DiffPresentationState,
        capabilities: DiffCapabilities,
        bindings: &[DiffBinding],
        words: &DiffPresentationWords,
    ) -> Self {
        #[cfg(any(test, feature = "test-hooks"))]
        BUILDS.with(|n| n.set(n.get() + 1));
        let mut p = Self {
            state: state.clone(),
            capabilities,
            bindings: bindings.to_vec(),
            words: words.clone(),
            declarations: vec![],
            rows: vec![],
            footer: vec![],
            priority: vec![],
        };
        p.declarations = p.row_plan();
        p.rows = p.declarations.iter().map(|row| p.render_row(row)).collect();
        for (plan_index, foot) in p.footer_plan().into_iter().enumerate() {
            if foot.ops != [DiffUiOp::Options] {
                p.priority.extend(foot.ops.iter().map(|op| DiffPriority {
                    plan_index,
                    op: *op,
                    word: foot.word,
                }));
            }
            let on: Vec<_> = foot
                .ops
                .iter()
                .copied()
                .filter(|op| p.enabled(*op))
                .collect();
            let mut keys = if foot.union {
                on.iter().flat_map(|op| p.keys(*op)).collect()
            } else {
                zip_columns(&on, |op| p.keys(op))
                    .0
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
            };
            let mut seen = Vec::new();
            keys.retain(|key| {
                if seen.contains(key) {
                    false
                } else {
                    seen.push(*key);
                    true
                }
            });
            if keys.is_empty() {
                continue;
            }
            let label = foot
                .word
                .map(|w| p.words.word(w).to_string())
                .or_else(|| p.label(foot.ops[0], false));
            if let Some(label) = label {
                let text = format!(
                    "{}:{label}",
                    keys.iter()
                        .map(|k| footer_key(*k, false))
                        .collect::<Vec<_>>()
                        .join("/")
                );
                p.footer.push(DiffToken {
                    plan_index,
                    text,
                    keys,
                    ops: if foot.ops == [DiffUiOp::Options] {
                        vec![]
                    } else {
                        foot.ops
                    },
                });
            }
        }
        p
    }
    fn binding(&self, op: DiffUiOp) -> Option<&DiffBinding> {
        self.bindings.iter().find(|b| b.op == op)
    }
    fn keys(&self, op: DiffUiOp) -> Vec<KeyPress> {
        self.binding(op).map(|b| b.keys.clone()).unwrap_or_default()
    }
    pub fn enabled(&self, op: DiffUiOp) -> bool {
        self.state.enabled(self.capabilities, op) && self.binding(op).is_some_and(|b| b.enabled)
    }
    fn label(&self, op: DiffUiOp, help: bool) -> Option<String> {
        let binding = self.binding(op)?;
        let (label, description) = self.state.text(op);
        let dynamic = if help { description } else { label };
        dynamic
            .and_then(|id| self.words.dynamic(id).map(str::to_string))
            .or_else(|| {
                if help {
                    binding.help.clone()
                } else {
                    binding.label.clone()
                }
            })
    }
    pub fn row_declarations(&self) -> &[DiffRowDeclaration] {
        &self.declarations
    }
    pub fn resolved_rows(&self) -> &[Option<DiffHelpRow>] {
        &self.rows
    }
    pub fn footer_tokens(&self) -> &[DiffToken] {
        &self.footer
    }
    pub fn action_priority(&self) -> &[DiffPriority] {
        &self.priority
    }
    /// Filter the resolved user declaration without sorting or recreating its keys.
    pub fn options_menu(
        &self,
        declaration: &[Item<DiffUiOp, FixedMenuLabel>],
    ) -> Vec<Item<DiffUiOp, String>> {
        fixed_menu_items(declaration, |op| self.enabled(op), |words, _| words.clone())
    }
    pub fn help_rows_with(&self, extra: &[DiffHelpRow]) -> Vec<DiffHelpRow> {
        self.rows
            .iter()
            .flatten()
            .chain(extra)
            .filter(|row| {
                (row.section != DiffHelpSection::Tabs || self.capabilities.tabs)
                    && (row.section != DiffHelpSection::Global || self.capabilities.global)
                    && row.ops.iter().all(|op| self.capabilities.provides(*op))
            })
            .cloned()
            .collect()
    }
    /// General help remains the host's; the diff only places its supplied rows and menus.
    pub fn help_sections(&self, extra: &[DiffHelpRow], menus: &[DiffHelpMenu]) -> Vec<CoreSection> {
        let rows = self.help_rows_with(extra);
        let section = |id: DiffHelpSection, title: &str| {
            let rows: Vec<_> = rows
                .iter()
                .filter(|r| r.section == id)
                .map(|r| (r.keys_text.clone(), r.description.clone()))
                .collect();
            (!rows.is_empty()).then(|| CoreSection {
                title: title.into(),
                rows,
            })
        };
        let mut out = Vec::new();
        for (id, title) in [
            (DiffHelpSection::View, &self.words.view),
            (DiffHelpSection::Move, &self.words.movement),
            (DiffHelpSection::Launch, &self.words.launch),
        ] {
            out.extend(section(id, title));
        }
        for menu in menus {
            let pairs: Vec<_> = menu
                .items
                .iter()
                .filter(|i| self.capabilities.provides(i.action))
                .map(|i| (i.key, i.label.clone()))
                .collect();
            out.extend(konoma_host_core::help::menu_section(
                &menu.title,
                &menu.prefix,
                &pairs,
            ));
        }
        out.extend(section(DiffHelpSection::Tabs, &self.words.tabs));
        out.extend(section(DiffHelpSection::Global, &self.words.global));
        out
    }
    fn row_plan(&self) -> Vec<DiffRowDeclaration> {
        use DiffUiOp as O;
        use Motion as M;
        let r = |ops: Vec<O>| DiffRowDeclaration {
            groups: vec![ops],
            literal: None,
            description: None,
        };
        let mut rows = Vec::new();
        if !self.state.reduced() {
            let mut scroll = r(vec![O::Navigate(M::Down), O::Navigate(M::Up)]);
            scroll.literal = Some("↑ ↓");
            rows.push(scroll);
            rows.push(r(vec![O::Navigate(M::Top), O::Navigate(M::Bottom)]));
            rows.push(DiffRowDeclaration {
                groups: vec![
                    vec![O::Navigate(M::Left), O::Navigate(M::Right)],
                    vec![O::Navigate(M::LineHome), O::Navigate(M::LineEnd)],
                ],
                literal: None,
                description: Some(DiffWord::HorizontalEnds),
            });
        }
        rows.extend([
            r(vec![O::Layout]),
            r(vec![O::PageNext, O::PagePrev]),
            r(vec![O::View]),
            r(vec![O::NextFile, O::PrevFile]),
            r(vec![O::NextBlock, O::PrevBlock]),
            r(vec![O::FollowScope]),
            r(vec![O::Edit]),
            r(vec![O::OpenExternal]),
            r(vec![O::Project]),
        ]);
        let mut tool = r(vec![O::Tool]);
        tool.description = Some(DiffWord::ToolHelp);
        rows.push(tool);
        rows.push(r(vec![O::Refresh]));
        if !self.state.reduced() {
            rows.extend([
                r(vec![O::Navigate(M::PageDown), O::Navigate(M::PageUp)]),
                r(vec![O::Navigate(M::HalfDown), O::Navigate(M::HalfUp)]),
            ]);
        }
        rows.push(r(vec![O::Discard]));
        let mut back = r(vec![O::Back, O::Cancel]);
        back.description = Some(DiffWord::BackHelp);
        rows.push(back);
        for row in &mut rows {
            for group in &mut row.groups {
                group.retain(|op| self.capabilities.provides(*op));
            }
            row.groups.retain(|g| !g.is_empty());
        }
        rows.retain(|r| !r.groups.is_empty());
        rows
    }
    fn render_row(&self, row: &DiffRowDeclaration) -> Option<DiffHelpRow> {
        let mut parts = Vec::new();
        let mut ops = Vec::new();
        let mut keys = Vec::new();
        let mut lead = None;
        let mut first_key = None;
        for group in &row.groups {
            let on: Vec<_> = group
                .iter()
                .copied()
                .filter(|op| self.enabled(*op))
                .collect();
            if on.is_empty() {
                continue;
            }
            let (cols, zipped) = zip_columns(&on, |op| self.keys(op));
            if cols.is_empty() {
                continue;
            }
            if lead.is_none() {
                lead = on.first().copied();
                first_key = cols.first().and_then(|col| col.first()).copied();
            }
            ops.extend(on);
            keys.extend(cols.iter().flatten().copied());
            let mut seen = Vec::new();
            let columns: Vec<_> = cols
                .iter()
                .map(|col| {
                    col.iter()
                        .filter_map(|key| {
                            let text = help_key(*key);
                            if seen.contains(&text) {
                                None
                            } else {
                                seen.push(text.clone());
                                Some(text)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .filter(|s| !s.is_empty())
                .collect();
            parts.push(columns.join(if zipped { "  ·  " } else { " / " }));
        }
        let lead = lead?;
        let mut keys_text = parts.join("  ");
        if let Some(lit) = row.literal {
            keys_text.push_str(" / ");
            keys_text.push_str(lit);
        }
        let description = row
            .description
            .map(|id| self.words.word(id).to_string())
            .or_else(|| self.label(lead, true))?;
        let section = if self.binding(lead).is_some_and(|b| b.launch) {
            DiffHelpSection::Launch
        } else {
            first_key
                .and_then(|key| {
                    self.bindings
                        .iter()
                        .flat_map(|b| &b.layers)
                        .find(|(k, _)| *k == key)
                        .map(|(_, layer)| *layer)
                })
                .unwrap_or(DiffHelpSection::View)
        };
        Some(DiffHelpRow {
            keys_text,
            description,
            keys,
            ops,
            section,
            ungated: false,
        })
    }
    fn footer_plan(&self) -> Vec<Foot> {
        use DiffUiOp as O;
        use DiffWord as W;
        use Motion as M;
        let f = |ops: Vec<O>, word| Foot {
            ops,
            word,
            union: false,
        };
        let mut out = Vec::new();
        if !self.state.reduced() {
            out.push(f(
                vec![O::Navigate(M::Down), O::Navigate(M::Up)],
                Some(W::Scroll),
            ));
        }
        out.extend([
            f(vec![O::NextFile, O::PrevFile], Some(W::Files)),
            f(vec![O::NextBlock, O::PrevBlock], Some(W::Change)),
        ]);
        if self.state.reduced() {
            out.extend([
                f(vec![O::Layout], None),
                f(vec![O::PageNext, O::PagePrev], None),
                f(vec![O::View], None),
            ]);
        } else {
            if !(self.state.surface == DiffSurface::Rendered && self.state.wrap) {
                out.push(f(
                    vec![O::Navigate(M::Left), O::Navigate(M::Right)],
                    Some(W::Hscroll),
                ));
            }
            out.push(f(vec![O::Layout], None));
        }
        out.push(f(vec![O::Discard], None));
        out.push(Foot {
            ops: vec![O::Back, O::Cancel],
            word: Some(W::Back),
            union: true,
        });
        if !self.state.reduced() {
            out.push(f(vec![O::View], None));
        }
        out.extend([
            f(vec![O::Options], Some(W::Options)),
            f(vec![O::Edit], None),
            f(vec![O::OpenExternal], None),
        ]);
        for foot in &mut out {
            foot.ops.retain(|op| self.capabilities.provides(*op));
        }
        out.retain(|f| !f.ops.is_empty());
        out
    }
}

pub struct DiffPresentationValues {
    pub state: DiffPresentationState,
    pub capabilities: DiffCapabilities,
    pub bindings: Vec<DiffBinding>,
    pub words: DiffPresentationWords,
}
impl DiffPresentationValues {
    pub fn build(&self) -> DiffPresentation {
        DiffPresentation::build(&self.state, self.capabilities, &self.bindings, &self.words)
    }
}
