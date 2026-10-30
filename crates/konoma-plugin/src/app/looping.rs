//! The loop: ask the session for its view, draw it, wait for a key.

use std::time::{Duration, Instant};

use konoma_host_core::help::{
    core_sections, ctrl_c_token, menu_view, render_foot, CoreFoot, CoreLookup, CoreSection,
    CoreToken, HelpFacts, HelpOp, KeyLookup, MenuView,
};
use konoma_host_core::help_msg::CoreMsg;
use konoma_host_core::keymap::{
    scheme_from_str, CoreAction, CoreContext, CoreKeys, CoreMenu, CoreResolution, KeyPress, Motion,
};
use konoma_host_core::keys::KeysConfig;
use konoma_host_core::lang::Lang;
use konoma_svg::image::ImageFailure;
use ratatui::backend::Backend;
use ratatui::Terminal;

use konoma_widgets::table::{popup_page_step, popup_scroll_by, popup_scroll_to, TableStep};

use super::draw::{self, FailedImage, Frame, ImageFrame, PopupState, TableFrame};
use super::pictures::Pictures;
use super::space;
use super::still::StillShape;
use super::window;
use super::AppError;
use crate::canvas_pictures::image_failure_for;
use crate::host::acts as hacts;
use crate::host::typing::{self, NextJob, Notice, QueuedAct};
use crate::key::{Key, KeyCode, KeyMods};
use crate::manifest::ViewKind;
use crate::pages;
use crate::ratatui_draw;
use crate::session::{ActCx, Outcome, Session, TermCaps, ViewCx};
use crate::view::{PageLayout, View, ViewReq};
use crate::Cancel;
use crate::{Line, Settings};

/// What the app tells the loop about its surroundings.
#[derive(Clone, Debug)]
pub struct AppCx {
    pub lang: String,
    pub term: TermCaps,
    /// Shown in the footer.
    pub title: String,
    /// What the plugin declared it shows when it shows nothing: it picks the words of a `view`
    /// that fails (the same ones konoma shows, see `canvas_pictures::unavailable_text`).
    pub unavailable: crate::manifest::Unavailable,
    /// The id of the plugin (the reason of a picture that failed can name its settings).
    pub plugin_id: String,
    /// The absolute path of the file (a failure's words end with it, as konoma's do).
    pub path: std::path::PathBuf,
    /// The note of the binding that picked the plugin ([`Opened::hint`]): a copied table row is
    /// joined by the separator the plugin read the file with.
    pub hint: Option<String>,
    /// The display types the plugin declared it shows ([`crate::Manifest::shows`]). A page that
    /// fails before anything was shown is told apart by this (a picture's or a table's); empty
    /// says nothing, and the kind of the last view on screen is believed instead.
    pub shows: Vec<crate::manifest::ViewKind>,
    /// Where a [`View::Reopen`] is opened again (who takes the content type it names). `None`:
    /// no plugin takes any, and a `Reopen` is "no plugin is available to show this document".
    pub reopen: Option<super::ReopenSource>,
    /// How the text and the documents look (`[ui] wrap`, `[ui.theme]`, ...).
    pub display: super::Display,
    /// The directory the program was opened in: the frame title is the path of the file shown
    /// from there, the way konoma's is from its root. `None`: the title is the file name.
    pub open_dir: Option<std::path::PathBuf>,
    /// The actions the plugin declared ([`crate::Manifest::actions`]), with the user's keys: the
    /// footer and the help name them, and their keys run them.
    pub acts: super::AppActs,
}

impl Default for AppCx {
    /// English, no terminal graphics, nothing declared, no file.
    fn default() -> AppCx {
        AppCx {
            lang: "en".into(),
            term: TermCaps::new(),
            title: String::new(),
            open_dir: None,
            unavailable: crate::manifest::Unavailable::default(),
            plugin_id: String::new(),
            path: std::path::PathBuf::new(),
            hint: None,
            shows: Vec::new(),
            reopen: None,
            display: super::Display::default(),
            acts: super::AppActs::default(),
        }
    }
}

/// The keys of the app: layers 0 to 3 of the key design, shared with the host
/// (`konoma_host_core::keymap`), with the user's `[keys]` overrides applied.
///
/// The app shows one text at a time, so it answers only what a text needs: moving and paging
/// (`j` `k` `g` `G`, `Ctrl-f` `Ctrl-b` `Ctrl-d` `Ctrl-u` or `f` `b` `d` `u` by `ui.keys`,
/// `PageUp` `PageDown`) and leaving (`q`, `Q`). The arrows, `Home` and `End` are fixed keys.
/// Every other command of the shared tables is ignored here until the app can do it.
///
/// A table is moved by the keys of the shared table context (`h j k l`, `g G`, `0 $`, paging,
/// `J K` between sheets) and the fixed arrows, `Home`, `End`; `Enter` opens the full-cell popup.
#[derive(Clone, Debug)]
pub struct AppKeys {
    keys: CoreKeys,
    /// The keys with no overrides: the `Space` menu's candidates include what a rebinding took away.
    defaults: CoreKeys,
    lang: Lang,
    /// `[ui] wrap`: while the text wraps, the sideways keys do nothing and are not named.
    wrap: bool,
    /// The text on screen has the caret (the file's text in the host's frame): the sideways keys
    /// move it whether the text wraps or not.
    caret: bool,
}

impl AppKeys {
    /// The keys for the `[keys]` table `keys` and the `ui.keys` paging style (`"vim"` | `"less"`).
    pub fn new(keys: &KeysConfig, ui_keys: &str) -> AppKeys {
        AppKeys {
            keys: CoreKeys::from_keys_config(scheme_from_str(ui_keys), keys),
            defaults: CoreKeys::defaults(scheme_from_str(ui_keys)),
            lang: Lang::En,
            wrap: true,
            caret: false,
        }
    }

    /// The same keys over a text that has the caret (`true`: the file's text in the host's
    /// frame) or none (a message, a page that failed to draw, a document).
    pub fn with_caret(mut self, caret: bool) -> AppKeys {
        self.caret = caret;
        self
    }

    /// The same keys for text that wraps (`true`, the default) or scrolls sideways (`[ui] wrap`).
    pub fn with_wrap(mut self, wrap: bool) -> AppKeys {
        self.wrap = wrap;
        self
    }

    /// The same keys with the help and the footer in `lang`.
    pub fn with_lang(mut self, lang: Lang) -> AppKeys {
        self.lang = lang;
        self
    }

    /// The language of the help and the footer.
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// What the help and the footer say about the keys of a text: every operation of the shared
    /// layers the app does not act on is off (its row and token are left out), and the two
    /// leaving keys are named as the app leaves (it has no tree to go back to).
    pub fn facts(&self) -> HelpFacts {
        self.facts_paged(false)
    }

    /// [`AppKeys::facts`] for a text whose document has pages to turn (`paged`): the page turn
    /// is named too, as the host names it over any text of a paged document. That is the text
    /// of a page that failed to draw.
    pub fn facts_paged(&self, paged: bool) -> HelpFacts {
        self.facts_scrolling(paged, true)
    }

    /// [`AppKeys::facts_paged`] for a text that `scrolls` or not: the picture's own frame that
    /// holds a failure has no text, so the motions do nothing there and are not named (as the
    /// host's gate leaves them out).
    pub fn facts_scrolling(&self, paged: bool, scrolls: bool) -> HelpFacts {
        self.facts_in(CoreContext::Text, paged, scrolls)
    }

    /// [`AppKeys::facts_scrolling`] over the text screen of key context `ctx` (see [`text_ctx`]).
    pub fn facts_in(&self, ctx: CoreContext, paged: bool, scrolls: bool) -> HelpFacts {
        let acted = |op: &HelpOp| match op {
            HelpOp::Act(CoreAction::PageNext | CoreAction::PagePrev) => paged,
            // The sideways motions only act on text that does not wrap.
            HelpOp::Act(CoreAction::Navigate(
                Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd,
            )) => scrolls && (self.caret || !self.wrap),
            HelpOp::Act(CoreAction::Navigate(_)) => scrolls,
            HelpOp::Act(CoreAction::Quit | CoreAction::PreviewBack | CoreAction::ToggleHelp) => {
                true
            }
            HelpOp::Cancel => true,
            _ => false,
        };
        let mut off: Vec<HelpOp> = Vec::new();
        for row in konoma_host_core::help::rows_of(ctx) {
            for op in row.ops() {
                if !acted(op) && !off.contains(op) {
                    off.push(*op);
                }
            }
        }
        HelpFacts {
            off,
            label: vec![(CoreAction::PreviewBack, Some(CoreMsg::HintQuit))],
            help: vec![
                (CoreAction::PreviewBack, Some(CoreMsg::QuitApp)),
                (CoreAction::Quit, Some(CoreMsg::QuitApp)),
            ],
        }
    }

    /// The footer's key hints: what leaves (`q` or else `Q` or else `Ctrl-C`, whichever really
    /// quits), what scrolls (only when the frame on screen scrolls: `scrolls`), what opens the
    /// help. A hint is there only when its key acts.
    pub fn footer_hints(&self, scrolls: bool) -> Vec<CoreToken> {
        self.footer_hints_paged(scrolls, false)
    }

    /// [`AppKeys::footer_hints`] over a text of a document with pages (`paged`).
    pub fn footer_hints_paged(&self, scrolls: bool, paged: bool) -> Vec<CoreToken> {
        self.footer_hints_in(CoreContext::Text, scrolls, paged)
    }

    /// [`AppKeys::footer_hints_paged`] over the text screen of key context `ctx`.
    pub fn footer_hints_in(&self, ctx: CoreContext, scrolls: bool, paged: bool) -> Vec<CoreToken> {
        let facts = self.facts_in(ctx, paged, true);
        let look = CoreLookup {
            keys: &self.keys,
            ctx,
        };
        let tok = |f: CoreFoot| render_foot(f, ctx, &facts, &look, self.lang);
        let leave = tok(CoreFoot::Back)
            .or_else(|| tok(CoreFoot::Quit))
            .unwrap_or_else(|| ctrl_c_token(self.lang));
        // The host's order for a text: the scroll, what leaves, the help, then the ends, the
        // sideways keys (text that does not wrap) and the page keys. `scrolls` is false for a
        // frame no motion moves: only what leaves and the help.
        let mut out = Vec::new();
        if scrolls {
            out.extend(tok(CoreFoot::Scroll));
        }
        out.push(leave);
        out.extend(tok(CoreFoot::Help));
        // The host names the `,` menu on any text screen, one that scrolls or not.
        for f in [
            CoreFoot::Ends,
            CoreFoot::HScroll,
            CoreFoot::LineEnds,
            CoreFoot::Options,
            CoreFoot::PageKeys,
        ] {
            if scrolls || f == CoreFoot::Options {
                out.extend(tok(f));
            }
        }
        out
    }

    /// The footer's key hints of a Markdown document: the same as a text (the host's footer of
    /// a document has no sideways keys: the lines wrap).
    pub fn document_footer_tokens(&self, scrolls: bool) -> Vec<String> {
        self.footer_hints_in(CoreContext::Document, scrolls, false)
            .into_iter()
            .filter(|t| {
                !t.ops.iter().any(|o| {
                    matches!(
                        o,
                        HelpOp::Act(CoreAction::Navigate(Motion::Left | Motion::LineHome))
                    )
                })
            })
            .map(|t| t.text)
            .collect()
    }

    /// [`AppKeys::footer_hints`] as the text the footer shows.
    pub fn footer_tokens(&self, scrolls: bool) -> Vec<String> {
        self.footer_tokens_paged(scrolls, false)
    }

    /// [`AppKeys::footer_hints_paged`] as the text the footer shows.
    pub fn footer_tokens_paged(&self, scrolls: bool, paged: bool) -> Vec<String> {
        self.footer_hints_paged(scrolls, paged)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// Whether `key` does something: in the `?` help (`in_help`) or on the screen of a text
    /// (`scrolls`: the frame scrolls) or of anything else (a message, a canvas: the motions do
    /// nothing there). What the hints and the help rows name must pass this.
    pub fn acts_on(&self, key: &Key, in_help: bool, scrolls: bool) -> bool {
        self.acts_on_in(CoreContext::Text, key, in_help, scrolls)
    }

    /// [`AppKeys::acts_on`] over the text screen of key context `ctx`.
    pub fn acts_on_in(&self, ctx: CoreContext, key: &Key, in_help: bool, scrolls: bool) -> bool {
        if in_help {
            return self.help_acts_on(key);
        }
        match self.act_in(ctx, key) {
            None => self.menu_key_acts(ctx, key),
            // Sideways, the text moves only when it does not wrap (the open help does not).
            Some(Act::Move(Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd)) => {
                scrolls && (self.caret || !self.wrap)
            }
            Some(Act::Move(_)) => scrolls,
            Some(_) => true,
        }
    }

    /// Whether `key` does something while the `?` help is open, over any screen: the answer of
    /// [`AppKeys::help_act`], the one the loop runs. The help scrolls on the vertical motions;
    /// the sideways ones leave it as it is.
    fn help_acts_on(&self, key: &Key) -> bool {
        match self.help_act(key) {
            None => false,
            Some(Act::Move(m)) => matches!(
                m,
                Motion::Down
                    | Motion::Up
                    | Motion::Top
                    | Motion::Bottom
                    | Motion::PageDown
                    | Motion::PageUp
                    | Motion::HalfDown
                    | Motion::HalfUp
            ),
            Some(_) => true,
        }
    }

    /// The sections of the `?` help.
    pub fn help_sections(&self) -> Vec<CoreSection> {
        self.help_sections_paged(false)
    }

    /// [`AppKeys::help_sections`] over a text of a document with pages (`paged`).
    pub fn help_sections_paged(&self, paged: bool) -> Vec<CoreSection> {
        self.help_sections_scrolling(paged, true)
    }

    /// [`AppKeys::help_sections_paged`] over a text that `scrolls` or not.
    pub fn help_sections_scrolling(&self, paged: bool, scrolls: bool) -> Vec<CoreSection> {
        self.help_sections_in(CoreContext::Text, paged, scrolls)
    }

    /// [`AppKeys::help_sections_scrolling`] over the text screen of key context `ctx`.
    pub fn help_sections_in(
        &self,
        ctx: CoreContext,
        paged: bool,
        scrolls: bool,
    ) -> Vec<CoreSection> {
        let look = CoreLookup {
            keys: &self.keys,
            ctx,
        };
        let secs = core_sections(ctx, &self.facts_in(ctx, paged, scrolls), &look, self.lang);
        self.with_copy_section(ctx, CoreMenu::Copy, secs)
    }

    /// The menu `key` opens in `ctx` when it is the copy menu (`y`) or the options menu (`,`):
    /// `None` for any other key.
    fn copy_menu_of(&self, ctx: CoreContext, key: &Key) -> Option<CoreMenu> {
        match self.keys.resolve(ctx, key_press(key)?) {
            CoreResolution::Menu(
                m @ (CoreMenu::Copy | CoreMenu::TableCopy | CoreMenu::Options),
            ) => Some(m),
            _ => None,
        }
    }

    /// Whether `key` opens a menu of `ctx` that has items (a key that acts on its own). The
    /// `Space` menu always has the way back and the help.
    fn menu_key_acts(&self, ctx: CoreContext, key: &Key) -> bool {
        self.opens_actions(ctx, key)
            || self
                .copy_menu_of(ctx, key)
                .is_some_and(|m| self.copy_menu_view(ctx, m).is_some())
    }

    /// Whether `key` is an item of an open copy menu (it acts only as the key after `y`; the help
    /// names it in a row such as `y n`).
    pub fn acts_in_copy_menu(&self, key: &Key) -> bool {
        let Some(kp) = key_press(key) else {
            return false;
        };
        [
            (CoreContext::Text, CoreMenu::Copy),
            (CoreContext::Document, CoreMenu::Copy),
            (CoreContext::Image, CoreMenu::Copy),
            (CoreContext::Table, CoreMenu::TableCopy),
            (CoreContext::Text, CoreMenu::Options),
            (CoreContext::Document, CoreMenu::Options),
            (CoreContext::Image, CoreMenu::Options),
            (CoreContext::Table, CoreMenu::Options),
            (CoreContext::Canvas, CoreMenu::Options),
        ]
        .into_iter()
        .any(|(c, m)| {
            self.copy_menu_of(c, key) == Some(m)
                || self
                    .copy_menu_view(c, m)
                    .is_some_and(|v| v.find(kp).is_some())
        })
    }

    /// The copy menu as the which-key line and the help list it (no code block is focused here,
    /// so `c` is not offered).
    fn copy_menu_view(&self, ctx: CoreContext, menu: CoreMenu) -> Option<MenuView> {
        menu_view(&self.keys, ctx, menu, self.lang, false).filter(|m| !m.items.is_empty())
    }

    /// The which-key line of an open menu: the spans the host draws (a bold heading, dim items).
    fn which_key_line(
        &self,
        ctx: CoreContext,
        menu: CoreMenu,
        width: u16,
    ) -> Option<Vec<ratatui::text::Span<'static>>> {
        let view = self.copy_menu_view(ctx, menu)?;
        let items = konoma_host_core::help::which_key_items(&view.pairs());
        Some(konoma_widgets::footer_line::which_key_spans(
            &view.title,
            &items,
            width,
        ))
    }

    /// `sections` with the sections of the menus the screen opens (`, p  ...` and the copy
    /// menu's `y n  name`) between the launch verbs and what is true everywhere, in the order of
    /// their opening keys, where the host lists them.
    fn with_copy_section(
        &self,
        ctx: CoreContext,
        menu: CoreMenu,
        mut sections: Vec<CoreSection>,
    ) -> Vec<CoreSection> {
        let look = CoreLookup {
            keys: &self.keys,
            ctx,
        };
        let mut found: Vec<(KeyPress, CoreSection)> = Vec::new();
        for m in [CoreMenu::Options, menu] {
            let Some(open) = look.keys_of(HelpOp::Menu(m)).first().copied() else {
                continue;
            };
            let Some(view) = self.copy_menu_view(ctx, m) else {
                continue;
            };
            if let Some(sec) = konoma_host_core::help::menu_section(
                &view.title,
                &konoma_host_core::help::help_key(open),
                &view.pairs(),
            ) {
                found.push((open, sec));
            }
        }
        found.sort_by_key(|(k, _)| konoma_host_core::help::display_order(*k));
        let global = CoreMsg::HelpGlobal.tr(self.lang);
        let at = sections
            .iter()
            .position(|s| s.title == global)
            .unwrap_or(sections.len());
        for (i, (_, sec)) in found.into_iter().enumerate() {
            sections.insert(at + i, sec);
        }
        sections
    }

    /// The title of the `?` help, shown in the footer while it is open.
    ///
    /// Named from the actual bindings: the keys that toggle the help (the fixed `Esc` is added),
    /// and the keys that scroll it, left out when none does. The leaving key `q` closes it too
    /// and is not named, as in the host.
    pub fn help_title(&self) -> String {
        self.help_title_in(CoreContext::Text)
    }

    /// [`AppKeys::help_title`] over the text screen of key context `ctx`.
    pub fn help_title_in(&self, _ctx: CoreContext) -> String {
        self.overlay_title()
    }

    /// The title of the `?` help. The help is konoma's `Help` context whatever is under it, so
    /// its keys (and the title naming them) do not depend on the screen.
    fn overlay_title(&self) -> String {
        konoma_host_core::help::help_overlay_title(&self.keys, self.lang)
    }

    /// What the app does for `key` while the `?` help is open: the help's own keys, the basic
    /// table and `global`, as konoma's `Help` context resolves them.
    fn help_act(&self, key: &Key) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        if key.mods.is_empty() {
            match key.code {
                KeyCode::Down => return Some(Act::Move(Motion::Down)),
                KeyCode::Up => return Some(Act::Move(Motion::Up)),
                KeyCode::Home => return Some(Act::Move(Motion::Top)),
                KeyCode::End => return Some(Act::Move(Motion::Bottom)),
                KeyCode::Esc => return Some(Act::Back),
                _ => {}
            }
        }
        match self.keys.help_resolve(key_press(key)?) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::Navigate(m)) => Some(Act::Move(m)),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            _ => None,
        }
    }

    /// The footer's key hints while the `?` help is open: scrolling, the ends, closing (the
    /// host's footer of its help).
    pub fn help_footer_tokens(&self) -> Vec<String> {
        konoma_host_core::help::help_footer_tokens(&self.keys, self.lang)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    // ---- Tables ----

    /// What the help and the footer say about a table: what the app does not act on is off.
    fn table_facts(&self, mode: TableMode) -> HelpFacts {
        let acted = |op: &HelpOp| match op {
            HelpOp::Act(CoreAction::Navigate(_)) => true,
            HelpOp::Act(CoreAction::PageNext | CoreAction::PagePrev) => mode.paged,
            HelpOp::Act(CoreAction::Quit | CoreAction::PreviewBack | CoreAction::ToggleHelp) => {
                true
            }
            HelpOp::Cancel => true,
            _ => false,
        };
        let mut off: Vec<HelpOp> = Vec::new();
        for row in konoma_host_core::help::rows_of(CoreContext::Table) {
            for op in row.ops() {
                if !acted(op) && !off.contains(op) {
                    off.push(*op);
                }
            }
        }
        HelpFacts {
            off,
            label: {
                let mut v = vec![(CoreAction::PreviewBack, Some(CoreMsg::HintQuit))];
                if mode.sheets {
                    v.push((CoreAction::PageNext, Some(CoreMsg::HintSheet)));
                    v.push((CoreAction::PagePrev, Some(CoreMsg::HintSheet)));
                }
                v
            },
            help: {
                let mut v = vec![
                    (CoreAction::PreviewBack, Some(CoreMsg::QuitApp)),
                    (CoreAction::Quit, Some(CoreMsg::QuitApp)),
                ];
                if mode.sheets {
                    v.push((CoreAction::PageNext, Some(CoreMsg::SheetSwitchHelp)));
                    v.push((CoreAction::PagePrev, Some(CoreMsg::SheetSwitchHelp)));
                }
                v
            },
        }
    }

    fn table_lookup(&self) -> CoreLookup<'_> {
        CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Table,
        }
    }

    /// The footer's key hints of a table: the cell cursor, the cell popup (`Enter`), the sheets,
    /// the ends, what leaves and what opens the help. A hint is there only when its key acts
    /// (`J` `K` only with several sheets, `Enter` only when there is a cell).
    pub fn table_footer_hints(&self, mode: TableMode) -> Vec<CoreToken> {
        let facts = self.table_facts(mode);
        let look = self.table_lookup();
        let tok = |f: CoreFoot| render_foot(f, CoreContext::Table, &facts, &look, self.lang);
        let mut out: Vec<CoreToken> = Vec::new();
        if mode.popup {
            out.extend(self.popup_tokens());
            return out;
        }
        out.extend(tok(CoreFoot::Cell));
        if mode.cells {
            out.push(self.enter_token());
        }
        out.extend(tok(CoreFoot::PageTurn));
        out.extend(tok(CoreFoot::TableCopy));
        out.extend(tok(CoreFoot::Ends));
        out.push(
            tok(CoreFoot::Back)
                .or_else(|| tok(CoreFoot::Quit))
                .unwrap_or_else(|| ctrl_c_token(self.lang)),
        );
        out.extend(tok(CoreFoot::Help));
        out.extend(tok(CoreFoot::Options));
        out
    }

    /// [`AppKeys::table_footer_hints`] as the text the footer shows.
    pub fn table_footer_tokens(&self, mode: TableMode) -> Vec<String> {
        self.table_footer_hints(mode)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// `Enter:view cell`: the fixed key that opens the popup.
    fn enter_token(&self) -> CoreToken {
        let key = KeyPress::key(crossterm::event::KeyCode::Enter);
        CoreToken {
            text: format!(
                "{}:{}",
                konoma_host_core::help::write_keys(&[key], true),
                CoreMsg::HintViewCell.tr(self.lang)
            ),
            keys: vec![key],
            ops: Vec::new(),
        }
    }

    /// The hints of the full-cell popup: scrolling, the ends, closing (the leave keys, `Enter` and `Esc`).
    pub fn popup_tokens(&self) -> Vec<CoreToken> {
        use konoma_host_core::help::write_keys;
        let facts = self.table_facts(TableMode {
            popup: true,
            ..TableMode::default()
        });
        let look = self.table_lookup();
        let mut out: Vec<CoreToken> = Vec::new();
        out.extend(render_foot(
            CoreFoot::Scroll,
            CoreContext::Table,
            &facts,
            &look,
            self.lang,
        ));
        if let Some(t) = render_foot(CoreFoot::Ends, CoreContext::Table, &facts, &look, self.lang) {
            out.push(CoreToken {
                text: format!(
                    "{}:{}",
                    write_keys(&t.keys, true),
                    CoreMsg::FtTopBottom.tr(self.lang)
                ),
                keys: t.keys,
                ops: t.ops,
            });
        }
        // What closes the popup: the keys that leave (`q`), and the fixed `Enter` and `Esc`.
        let mut close = look.keys_of(HelpOp::Act(CoreAction::PreviewBack));
        for k in [
            KeyPress::key(crossterm::event::KeyCode::Enter),
            KeyPress::key(crossterm::event::KeyCode::Esc),
        ] {
            if !close.contains(&k) {
                close.push(k);
            }
        }
        out.push(CoreToken {
            text: format!(
                "{}:{}",
                write_keys(&close, false),
                CoreMsg::FtClose.tr(self.lang)
            ),
            keys: close,
            ops: Vec::new(),
        });
        out
    }

    /// The sections of the `?` help of a table, with the fixed `Enter` row.
    ///
    /// The help describes the table under it: with the popup open it still lists the keys of the
    /// table (the host's does), so the rows are the ones of the table with no popup.
    pub fn table_help_sections(&self, mode: TableMode) -> Vec<CoreSection> {
        let mode = TableMode {
            popup: false,
            ..mode
        };
        let look = self.table_lookup();
        let secs = core_sections(
            CoreContext::Table,
            &self.table_facts(mode),
            &look,
            self.lang,
        );
        let mut secs = self.with_copy_section(CoreContext::Table, CoreMenu::TableCopy, secs);
        if mode.cells {
            let row = (
                konoma_host_core::help::help_key(KeyPress::key(crossterm::event::KeyCode::Enter)),
                CoreMsg::TableCellViewHelp.tr(self.lang).to_string(),
            );
            let view_title = konoma_host_core::help::view_title(CoreContext::Table).tr(self.lang);
            match secs.iter_mut().find(|s| s.title == view_title) {
                Some(s) => s.rows.push(row),
                None => secs.insert(
                    0,
                    CoreSection {
                        title: view_title.to_string(),
                        rows: vec![row],
                    },
                ),
            }
        }
        secs
    }

    /// The title of the `?` help over a table (see [`AppKeys::help_title`]).
    pub fn table_help_title(&self, _mode: TableMode) -> String {
        self.overlay_title()
    }

    /// What the app does for `key` over a table (`None` = nothing). `popup`: the full-cell popup
    /// is open, where the keys are the popup's own: the vertical motions scroll it, `PageUp`
    /// and `PageDown` page it, `Enter` / `Esc` / `q` close it; the others do nothing.
    fn table_act(&self, key: &Key, popup: bool) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        let allowed = |m: Motion, literal_page: bool| match m {
            Motion::Down | Motion::Up | Motion::Top | Motion::Bottom => true,
            Motion::PageDown | Motion::PageUp => !popup || literal_page,
            _ => !popup,
        };
        if key.mods.is_empty() {
            let fixed = match key.code {
                KeyCode::Down => Some(Motion::Down),
                KeyCode::Up => Some(Motion::Up),
                KeyCode::Left => Some(Motion::Left),
                KeyCode::Right => Some(Motion::Right),
                KeyCode::Home => Some(Motion::Top),
                KeyCode::End => Some(Motion::Bottom),
                _ => None,
            };
            if let Some(m) = fixed {
                return allowed(m, false).then_some(Act::Move(m));
            }
            match key.code {
                KeyCode::Esc => return Some(Act::Back),
                KeyCode::Enter => return Some(Act::Cell),
                _ => {}
            }
        }
        let kp = key_press(key)?;
        let literal_page =
            key.mods.is_empty() && matches!(key.code, KeyCode::PageDown | KeyCode::PageUp);
        match self.keys.resolve(CoreContext::Table, kp) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            // A key written for the cell popup opens it; the popup itself answers to its own keys.
            CoreResolution::Action(CoreAction::ToggleTableCell) if !popup => Some(Act::Cell),
            CoreResolution::Action(CoreAction::Navigate(m)) => {
                allowed(m, literal_page).then_some(Act::Move(m))
            }
            CoreResolution::Action(CoreAction::PageNext) if !popup => Some(Act::Page(1)),
            CoreResolution::Action(CoreAction::PagePrev) if !popup => Some(Act::Page(-1)),
            _ => None,
        }
    }

    /// Whether `key` does something over a table (in the `?` help when `in_help`). What the hints
    /// and the help rows name must pass this.
    pub fn table_acts_on(&self, key: &Key, in_help: bool, mode: TableMode) -> bool {
        if in_help {
            return self.help_acts_on(key);
        }
        match self.table_act(key, mode.popup && !in_help) {
            None => !in_help && !mode.popup && self.menu_key_acts(CoreContext::Table, key),
            // The help scrolls on the vertical motions; the others do nothing while it is open.
            Some(Act::Move(m)) => {
                !in_help
                    || matches!(
                        m,
                        Motion::Down
                            | Motion::Up
                            | Motion::Top
                            | Motion::Bottom
                            | Motion::PageDown
                            | Motion::PageUp
                            | Motion::HalfDown
                            | Motion::HalfUp
                    )
            }
            Some(Act::Page(_)) => !in_help && mode.paged,
            Some(Act::Cell) => !in_help && mode.cells,
            Some(_) => true,
        }
    }

    // ---- Pictures ----

    /// What the help and the footer say about a still picture. The host names zoom, the reset and
    /// the pan whatever the picture is doing, so the app does too; the page turn is named only
    /// when the picture has pages to turn (`paged`, as the host does); the outline, the editor and
    /// the opener are not done by the app, so they are off.
    fn image_facts(&self, paged: bool) -> HelpFacts {
        let acted = |op: &HelpOp| match op {
            HelpOp::Act(CoreAction::PageNext | CoreAction::PagePrev) => paged,
            HelpOp::Act(
                CoreAction::ImageZoomIn | CoreAction::ImageZoomOut | CoreAction::ImageZoomReset,
            ) => true,
            HelpOp::Act(CoreAction::Navigate(m)) => {
                matches!(m, Motion::Left | Motion::Right | Motion::Up | Motion::Down)
            }
            HelpOp::Act(CoreAction::Quit | CoreAction::PreviewBack | CoreAction::ToggleHelp) => {
                true
            }
            HelpOp::Cancel => true,
            _ => false,
        };
        let mut off: Vec<HelpOp> = Vec::new();
        for row in konoma_host_core::help::rows_of(CoreContext::Image) {
            for op in row.ops() {
                if !acted(op) && !off.contains(op) {
                    off.push(*op);
                }
            }
        }
        HelpFacts {
            off,
            label: vec![(CoreAction::PreviewBack, Some(CoreMsg::HintQuit))],
            help: vec![
                (CoreAction::PreviewBack, Some(CoreMsg::QuitApp)),
                (CoreAction::Quit, Some(CoreMsg::QuitApp)),
            ],
        }
    }

    fn image_lookup(&self) -> CoreLookup<'_> {
        CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Image,
        }
    }

    /// The footer's key hints of a still picture: zoom, the reset and the pan, what leaves, what
    /// opens the help (the host's list, less what the app does not do).
    pub fn image_footer_hints(&self, paged: bool) -> Vec<CoreToken> {
        let facts = self.image_facts(paged);
        let look = self.image_lookup();
        let tok = |f: CoreFoot| render_foot(f, CoreContext::Image, &facts, &look, self.lang);
        let mut out: Vec<CoreToken> = Vec::new();
        out.extend(tok(CoreFoot::Zoom));
        out.extend(tok(CoreFoot::ZoomReset));
        out.extend(tok(CoreFoot::Pan));
        out.extend(tok(CoreFoot::PageTurn));
        out.push(
            tok(CoreFoot::Back)
                .or_else(|| tok(CoreFoot::Quit))
                .unwrap_or_else(|| ctrl_c_token(self.lang)),
        );
        out.extend(tok(CoreFoot::Help));
        out.extend(tok(CoreFoot::Options));
        out
    }

    /// [`AppKeys::image_footer_hints`] as the text the footer shows.
    pub fn image_footer_tokens(&self, paged: bool) -> Vec<String> {
        self.image_footer_hints(paged)
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// The sections of the `?` help of a still picture.
    pub fn image_help_sections(&self, paged: bool) -> Vec<CoreSection> {
        let secs = core_sections(
            CoreContext::Image,
            &self.image_facts(paged),
            &self.image_lookup(),
            self.lang,
        );
        self.with_copy_section(CoreContext::Image, CoreMenu::Copy, secs)
    }

    /// The title of the `?` help over a picture (see [`AppKeys::help_title`]). The help scrolls
    /// on the vertical motions whatever the picture is doing, so they are named from the text's
    /// facts.
    pub fn image_help_title(&self) -> String {
        self.overlay_title()
    }

    /// What the app does for `key` over a still picture (`None` = nothing). Over the picture the
    /// four directions pan (the `?` help has its own keys: [`AppKeys::help_act`]).
    fn image_act(&self, key: &Key) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        let allowed =
            |m: Motion| matches!(m, Motion::Left | Motion::Right | Motion::Up | Motion::Down);
        if key.mods.is_empty() {
            let fixed = match key.code {
                KeyCode::Down => Some(Motion::Down),
                KeyCode::Up => Some(Motion::Up),
                KeyCode::Left => Some(Motion::Left),
                KeyCode::Right => Some(Motion::Right),
                KeyCode::Home => Some(Motion::Top),
                KeyCode::End => Some(Motion::Bottom),
                _ => None,
            };
            if let Some(m) = fixed {
                return allowed(m).then_some(Act::Move(m));
            }
            if matches!(key.code, KeyCode::Esc) {
                return Some(Act::Back);
            }
        }
        let kp = key_press(key)?;
        match self.keys.resolve(CoreContext::Image, kp) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            CoreResolution::Action(CoreAction::Navigate(m)) => allowed(m).then_some(Act::Move(m)),
            CoreResolution::Action(CoreAction::ImageZoomIn) => Some(Act::Zoom(1)),
            CoreResolution::Action(CoreAction::ImageZoomOut) => Some(Act::Zoom(-1)),
            CoreResolution::Action(CoreAction::ImageZoomReset) => Some(Act::ZoomReset),
            CoreResolution::Action(CoreAction::PageNext) => Some(Act::Page(1)),
            CoreResolution::Action(CoreAction::PagePrev) => Some(Act::Page(-1)),
            _ => None,
        }
    }

    /// Whether `key` does something over a still picture (in the `?` help when `in_help`).
    /// The hints and rows follow the host and name the reset and the pan even at fit, where this
    /// says no.
    pub fn image_acts_on(&self, key: &Key, in_help: bool, mode: ImageMode) -> bool {
        if in_help {
            return self.help_acts_on(key);
        }
        let ready = mode.shape == StillShape::Ready;
        match self.image_act(key) {
            None => !in_help && self.menu_key_acts(CoreContext::Image, key),
            Some(Act::Zoom(_)) => !in_help && ready,
            Some(Act::ZoomReset) => !in_help && ready && mode.zoomed,
            // As the host's gate: any picture with several pages, whatever it is doing.
            Some(Act::Page(_)) => !in_help && mode.paged,
            // The help scrolls on the vertical motions; over the picture the directions pan.
            Some(Act::Move(m)) if in_help => matches!(
                m,
                Motion::Down
                    | Motion::Up
                    | Motion::Top
                    | Motion::Bottom
                    | Motion::PageDown
                    | Motion::PageUp
                    | Motion::HalfDown
                    | Motion::HalfUp
            ),
            Some(Act::Move(_)) => ready && mode.zoomed,
            Some(_) => true,
        }
    }

    // ---- Canvases ----

    /// What the help and the footer say about a plugin's canvas: the host's list for
    /// `CoreContext::Canvas`, less what the app does not do. The canvas holds no text, so no
    /// motion scrolls it; the page turn, the outline, the editor, the opener and the menus of
    /// the view are not done by the app, so they are off.
    fn canvas_facts(&self) -> HelpFacts {
        let acted = |op: &HelpOp| {
            matches!(
                op,
                HelpOp::Act(CoreAction::Quit | CoreAction::PreviewBack | CoreAction::ToggleHelp)
                    | HelpOp::Cancel
            )
        };
        let mut off: Vec<HelpOp> = Vec::new();
        for row in konoma_host_core::help::rows_of(CoreContext::Canvas) {
            for op in row.ops() {
                if !acted(op) && !off.contains(op) {
                    off.push(*op);
                }
            }
        }
        HelpFacts {
            off,
            label: vec![(CoreAction::PreviewBack, Some(CoreMsg::HintQuit))],
            help: vec![
                (CoreAction::PreviewBack, Some(CoreMsg::QuitApp)),
                (CoreAction::Quit, Some(CoreMsg::QuitApp)),
            ],
        }
    }

    fn canvas_lookup(&self) -> CoreLookup<'_> {
        CoreLookup {
            keys: &self.keys,
            ctx: CoreContext::Canvas,
        }
    }

    /// The footer's key hints of a canvas: what leaves, what opens the help.
    pub fn canvas_footer_hints(&self) -> Vec<CoreToken> {
        let facts = self.canvas_facts();
        let look = self.canvas_lookup();
        let tok = |f: CoreFoot| render_foot(f, CoreContext::Canvas, &facts, &look, self.lang);
        let mut out: Vec<CoreToken> = Vec::new();
        out.push(
            tok(CoreFoot::Back)
                .or_else(|| tok(CoreFoot::Quit))
                .unwrap_or_else(|| ctrl_c_token(self.lang)),
        );
        out.extend(tok(CoreFoot::Help));
        out.extend(tok(CoreFoot::Options));
        out
    }

    /// [`AppKeys::canvas_footer_hints`] as the text the footer shows.
    pub fn canvas_footer_tokens(&self) -> Vec<String> {
        self.canvas_footer_hints()
            .into_iter()
            .map(|t| t.text)
            .collect()
    }

    /// The sections of the `?` help of a canvas.
    pub fn canvas_help_sections(&self) -> Vec<CoreSection> {
        let secs = core_sections(
            CoreContext::Canvas,
            &self.canvas_facts(),
            &self.canvas_lookup(),
            self.lang,
        );
        self.with_copy_section(CoreContext::Canvas, CoreMenu::Copy, secs)
    }

    /// Whether `key` opens the `Space` menu of `ctx`.
    fn opens_actions(&self, ctx: CoreContext, key: &Key) -> bool {
        key_press(key)
            .is_some_and(|kp| self.keys.resolve(ctx, kp) == CoreResolution::Menu(CoreMenu::Actions))
    }

    /// The items of the `Space` menu of the screen (see [`super::space`]).
    fn space_items(
        &self,
        ctx: CoreContext,
        lang: &str,
        acts: &super::AppActs,
        act_ctx: &str,
        windowed: bool,
        enabled: impl Fn(&str) -> bool,
    ) -> Vec<konoma_host_core::menu::Item<super::space::SpaceOp, String>> {
        super::space::items(
            &self.keys,
            &self.defaults,
            ctx,
            self.lang,
            lang,
            acts,
            act_ctx,
            windowed,
            enabled,
        )
    }

    /// The which-key line of the open `Space` menu.
    fn space_line(
        &self,
        items: &[konoma_host_core::menu::Item<super::space::SpaceOp, String>],
        width: u16,
    ) -> Vec<ratatui::text::Span<'static>> {
        let pairs: Vec<(KeyPress, String)> =
            items.iter().map(|i| (i.key, i.label.clone())).collect();
        konoma_widgets::footer_line::which_key_spans(
            CoreMsg::WkActionsTitle.tr(self.lang),
            &konoma_host_core::help::which_key_items(&pairs),
            width,
        )
    }

    /// Puts the `Space` menu's section into the help `secs` where the host lists the menus: before
    /// the sections of the other menus (`,` and `y`), else before what is true everywhere.
    fn insert_actions_section(
        &self,
        ctx: CoreContext,
        items: &[konoma_host_core::menu::Item<super::space::SpaceOp, String>],
        secs: &mut Vec<CoreSection>,
    ) {
        let look = CoreLookup {
            keys: &self.keys,
            ctx,
        };
        let Some(open) = look
            .keys_of(HelpOp::Menu(CoreMenu::Actions))
            .first()
            .copied()
        else {
            return;
        };
        let pairs: Vec<(KeyPress, String)> =
            items.iter().map(|i| (i.key, i.label.clone())).collect();
        let Some(sec) = konoma_host_core::help::menu_section(
            CoreMsg::WkActionsTitle.tr(self.lang),
            &konoma_host_core::help::help_key(open),
            &pairs,
        ) else {
            return;
        };
        let menus = [
            CoreMsg::WkOptionsTitle,
            CoreMsg::WkCopyPathTitle,
            CoreMsg::WkTableCopyTitle,
            CoreMsg::HelpGlobal,
        ]
        .map(|m| m.tr(self.lang));
        let at = secs
            .iter()
            .position(|s| menus.contains(&s.title.as_str()))
            .unwrap_or(secs.len());
        secs.insert(at, sec);
    }

    /// Where the plugin's section goes in the help of the key context `ctx`: right after the
    /// section of the screen's own operations, first when the screen has none.
    fn plugin_section_at(&self, ctx: &str, secs: &[CoreSection]) -> usize {
        let Some(c) = CoreContext::from_name(ctx) else {
            return 0;
        };
        let title = konoma_host_core::help::view_title(c).tr(self.lang);
        secs.iter()
            .position(|s| s.title == title)
            .map_or(0, |i| i + 1)
    }

    /// The title of the `?` help over a canvas (see [`AppKeys::help_title`]). The help scrolls
    /// like any other `?` help.
    pub fn canvas_help_title(&self) -> String {
        self.overlay_title()
    }

    /// What the app does for `key` over a canvas (`None` = nothing). Over the canvas only the
    /// keys that leave or open the help act (the host hands the others to the plugin); the `?`
    /// help has its own keys ([`AppKeys::help_act`]).
    fn canvas_act(&self, key: &Key) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        if key.mods.is_empty() && matches!(key.code, KeyCode::Esc) {
            return Some(Act::Back);
        }
        let kp = key_press(key)?;
        match self.keys.resolve(CoreContext::Canvas, kp) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            _ => None,
        }
    }

    /// Whether `key` does something over a canvas (in the `?` help when `in_help`). What the
    /// hints and the help rows name must pass this.
    pub fn canvas_acts_on(&self, key: &Key, in_help: bool) -> bool {
        if in_help {
            return self.help_acts_on(key);
        }
        match self.canvas_act(key) {
            None => !in_help && self.menu_key_acts(CoreContext::Canvas, key),
            Some(Act::Move(m)) if in_help => matches!(
                m,
                Motion::Down
                    | Motion::Up
                    | Motion::Top
                    | Motion::Bottom
                    | Motion::PageDown
                    | Motion::PageUp
                    | Motion::HalfDown
                    | Motion::HalfUp
            ),
            Some(Act::Move(_)) => false,
            Some(_) => true,
        }
    }

    /// The built-in keys (no overrides, vim paging).
    pub fn defaults() -> AppKeys {
        AppKeys::new(&KeysConfig::default(), "vim")
    }

    /// What the app does for `key` over the text screen of key context `ctx` (`None` = nothing).
    fn act_in(&self, ctx: CoreContext, key: &Key) -> Option<Act> {
        if key.mods == KeyMods::CTRL && matches!(key.code, KeyCode::Char('c')) {
            return Some(Act::Quit);
        }
        // The fixed keys: not in the keymap, so not rebindable.
        if key.mods.is_empty() {
            match key.code {
                KeyCode::Down => return Some(Act::Move(Motion::Down)),
                KeyCode::Up => return Some(Act::Move(Motion::Up)),
                KeyCode::Left => return Some(Act::Move(Motion::Left)),
                KeyCode::Right => return Some(Act::Move(Motion::Right)),
                KeyCode::Home => return Some(Act::Move(Motion::Top)),
                KeyCode::End => return Some(Act::Move(Motion::Bottom)),
                // `Esc` is fixed (not in the keymap) and the help says `q / Esc` leaves, so it
                // does: it closes the help, and out of the help it ends the app.
                KeyCode::Esc => return Some(Act::Back),
                _ => {}
            }
        }
        let kp = key_press(key)?;
        match self.keys.resolve(ctx, kp) {
            CoreResolution::Action(CoreAction::Quit) => Some(Act::Quit),
            CoreResolution::Action(CoreAction::PreviewBack) => Some(Act::Back),
            CoreResolution::Action(CoreAction::Navigate(m)) => Some(Act::Move(m)),
            CoreResolution::Action(CoreAction::ToggleHelp) => Some(Act::Help),
            _ => None,
        }
    }
}

/// The shared keys' context of a text screen: the decorated document (`Frame::Document`) is
/// `Document`, source and plain text (and a page that failed to draw) are `Text`. The one place
/// that decides it, as konoma's `key_context` is for the host.
fn text_ctx(base: &Frame) -> CoreContext {
    if matches!(base, Frame::Document(_)) {
        CoreContext::Document
    } else {
        CoreContext::Text
    }
}

/// Whether the screen is the file's text in the host's frame, which the host windows
/// (`is_windowed`) and so plans the range-selection keys for. A message, a page that failed to
/// draw and the bare text of one are not.
fn windowed_text(base: &Frame) -> bool {
    matches!(base, Frame::Text(t) if t.border.as_ref().is_some_and(|b| b.extent.is_some()))
}

/// The text of a page that failed to draw, in the host's frame (the path alone on the border).
fn failed_text(base: &Frame) -> bool {
    matches!(base, Frame::Text(t) if t.border.as_ref().is_some_and(|b| b.extent.is_none()))
}

/// A help as lines: a heading and the aligned rows of each section (the host's drawing, shared).
fn lines_of(secs: &[CoreSection]) -> Vec<ratatui::text::Line<'static>> {
    let parts: Vec<(&str, &[(String, String)])> = secs
        .iter()
        .map(|s| (s.title.as_str(), s.rows.as_slice()))
        .collect();
    konoma_widgets::help_panel::help_lines(&parts)
}

/// What a still picture on screen lets the keys do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageMode {
    /// Loading, shown or failed.
    pub shape: StillShape,
    /// It is one of several pages: `J` `K` turn them.
    pub paged: bool,
    /// It is zoomed in (reset and pan act).
    pub zoomed: bool,
}

/// What a table on screen lets the keys do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableMode {
    /// The full-cell popup is open.
    pub popup: bool,
    /// The file has several sheets (pages): `J` `K` turn them.
    pub paged: bool,
    /// The table has a cell to show: `Enter` opens the popup.
    pub cells: bool,
    /// The cells have spreadsheet addresses: the pages are sheets (the footer and the help say
    /// `sheet`, as the host does for a workbook).
    pub sheets: bool,
}

/// What a key does in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// `Enter` over a table: opens the full-cell popup, or closes it.
    Cell,
    /// A page turn over a table or a picture: 1 next, -1 previous.
    Page(i32),
    /// Ends the app, even from the help (`Q`, `Ctrl-C`): the host's `Q` quits with its help open
    /// too.
    Quit,
    /// Leaves: closes the help when it is open, else ends the app (`q`, `Esc`); in the host `q`
    /// and `Esc` close the help.
    Back,
    Move(Motion),
    Help,
    /// Zoom a still picture: 1 in, -1 out.
    Zoom(i32),
    /// A still picture back to fit.
    ZoomReset,
}

/// The key as the shared tables name it. Alt and the keys that are not part of the shared
/// tables (function keys, a paste) have none; Shift is already in the character.
fn key_press(key: &Key) -> Option<KeyPress> {
    use konoma_host_core::keymap::KeyPress as P;
    if key.mods.contains(KeyMods::ALT) {
        return None;
    }
    let ctrl = key.mods.contains(KeyMods::CTRL);
    let code = match &key.code {
        KeyCode::Char(c) => crossterm::event::KeyCode::Char(*c),
        KeyCode::Enter => crossterm::event::KeyCode::Enter,
        KeyCode::Backspace => crossterm::event::KeyCode::Backspace,
        KeyCode::Delete => crossterm::event::KeyCode::Delete,
        KeyCode::Tab => crossterm::event::KeyCode::Tab,
        KeyCode::BackTab => crossterm::event::KeyCode::BackTab,
        KeyCode::Left => crossterm::event::KeyCode::Left,
        KeyCode::Right => crossterm::event::KeyCode::Right,
        KeyCode::Up => crossterm::event::KeyCode::Up,
        KeyCode::Down => crossterm::event::KeyCode::Down,
        KeyCode::Home => crossterm::event::KeyCode::Home,
        KeyCode::End => crossterm::event::KeyCode::End,
        KeyCode::PageUp => crossterm::event::KeyCode::PageUp,
        KeyCode::PageDown => crossterm::event::KeyCode::PageDown,
        KeyCode::Esc => crossterm::event::KeyCode::Esc,
        _ => return None,
    };
    Some(P { code, ctrl })
}

/// An event the loop reacts to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    Key(Key),
    /// The terminal was resized: draw again at the new size.
    Resize,
    /// Text pasted as one piece (bracketed paste). Only a canvas that takes the typing gets it
    /// (as one `KeyCode::Paste`); anywhere else it does nothing, as in konoma.
    Paste(String),
}

/// What an [`EventSource`] answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Event(AppEvent),
    /// Nothing came within the wait.
    Timeout,
    /// No more events: the loop ends (the input is closed, or a test's keys are used up).
    End,
}

/// Where the loop gets its events. Any iterator of [`AppEvent`] is one (it never times out), so
/// a test passes a list of keys.
pub trait EventSource {
    /// The next event; `wait` is how long to wait for it (`None` = as long as it takes).
    fn next(&mut self, wait: Option<Duration>) -> Next;
}

impl<I: Iterator<Item = AppEvent>> EventSource for I {
    fn next(&mut self, _wait: Option<Duration>) -> Next {
        match Iterator::next(self) {
            Some(e) => Next::Event(e),
            None => Next::End,
        }
    }
}

use crate::host::typing::{MAX_REDRAW_AFTER as MAX_REDRAW, MIN_REDRAW_AFTER as MIN_REDRAW};

/// The one content type the app shows a `Reopen` as (konoma's `markdown`, which the document
/// widget draws).
const REOPEN_ACCEPTS: &str = "markdown";

/// The declarations of the plugin a `Reopen` was opened in (its manifest).
struct ReopenedAs {
    shows: Vec<ViewKind>,
    unavailable: crate::manifest::Unavailable,
    plugin_id: String,
    /// The actions the destination declared, with the user's `[keys.<id>]` of it: from a
    /// `Reopen` on, the keys, the footer and the help are its, as the session is.
    acts: super::AppActs,
}

impl State {
    /// The display types of the plugin showing the file now.
    fn shows<'a>(&'a self, cx: &'a AppCx) -> &'a [ViewKind] {
        self.reopened_as.as_ref().map_or(&cx.shows, |r| &r.shows)
    }

    /// What the plugin showing the file now says when it shows nothing.
    fn unavailable<'a>(&'a self, cx: &'a AppCx) -> &'a crate::manifest::Unavailable {
        self.reopened_as
            .as_ref()
            .map_or(&cx.unavailable, |r| &r.unavailable)
    }

    /// The declared actions of the plugin showing the file now (the destination's after a
    /// `Reopen`: the session that answers an action is the destination's).
    fn acts<'a>(&'a self, cx: &'a AppCx) -> &'a super::AppActs {
        self.reopened_as.as_ref().map_or(&cx.acts, |r| &r.acts)
    }

    /// The id of the plugin showing the file now.
    fn plugin_id<'a>(&'a self, cx: &'a AppCx) -> &'a str {
        self.reopened_as
            .as_ref()
            .map_or(&cx.plugin_id, |r| &r.plugin_id)
    }
}

/// The state after a `Reopen`: everything of the old document is dropped (the scroll, the laid-out
/// page) and the plugin showing the file is `to`. What waited for the session is kept by the rule
/// the host follows ([`typing::retain_declared`]): the typed keys stay, and an action stays only if
/// `to` declared it (`None`: the show's own plugin, whose declarations are `own`).
fn state_after_reopen(old: State, to: Option<ReopenedAs>, own: &super::AppActs) -> State {
    let mut act_queue = old.act_queue;
    let declared = to.as_ref().map_or(own, |r| &r.acts);
    typing::retain_declared(&mut act_queue, |action| {
        declared.bindings.iter().any(|b| b.act == action)
    });
    State {
        display: old.display,
        reopened_as: to,
        key_queue: old.key_queue,
        act_queue,
        ..State::default()
    }
}

/// What the loop keeps between two frames.
#[derive(Default)]
struct State {
    /// An action or a key answered `Outcome::Reload`: the file is opened again with its plugin
    /// before the next view (konoma's `reload_plugin`).
    reload: bool,
    /// What the plugin a `Reopen` was opened in is: from then on it, not the plugin that
    /// converted the file, says what a failed page was (`AppCx::shows` and the like).
    reopened_as: Option<ReopenedAs>,
    /// The first line shown of a text.
    scroll: usize,
    /// The text on screen has the caret (the file's text in the host's frame).
    caret_on: bool,
    /// The line and the column (a char index) of the caret.
    caret_line: usize,
    caret_col: usize,
    /// The text on screen is the file's own lines (not generated text): `e` has a line in it.
    file_text: bool,
    /// The Markdown document on screen was read from this file, which has this many lines
    /// (`None`: it was converted, or there is none).
    doc_edit: Option<(usize, std::path::PathBuf)>,
    /// The number of lines and the rows shown of the text on screen (what the caret moves in).
    lines: usize,
    text_rows: usize,
    /// Cells scrolled off the left edge of a text that does not wrap.
    hscroll: u16,
    /// How the text and the documents look.
    display: super::Display,
    /// The number of lines of a text file, counted once.
    counted: Option<(std::path::PathBuf, usize)>,
    /// The reader of a text file that measures where the window is, in bytes.
    win: Option<window::ByteWindow>,
    /// The height of the body at the last draw (the page of `PageUp` / `PageDown`).
    page: usize,
    /// The last number of lines a text had (the bound of `End`).
    total: usize,
    /// The `?` help while it is open: the first line shown.
    help: Option<usize>,
    /// Whether the frame on screen scrolls (a text does; a message and a canvas do not).
    scrolls: bool,
    /// The page (sheet) of a table or a picture the session is asked for, from 0.
    at_page: usize,
    /// How many pages the session declares (`None` = it does not say, or has none).
    pages: Option<usize>,
    /// The last view on screen was a picture (so are the pages `pages` counts).
    last_image: bool,
    /// The last `view` of a paged picture failed: the message is on screen, and the page keys
    /// still turn the pages (the host keeps them too: the page number is not rolled back).
    image_failed: bool,
    /// The next view is the page a page key asked for: the picture on screen stays until it lands.
    turned: bool,
    /// The last `view` of a paged table failed: the message is on screen, and the page keys
    /// still turn the pages (the host keeps them too: the page number is not rolled back).
    table_failed: bool,
    /// Whether the last table on screen had spreadsheet addresses (its pages are sheets): the
    /// footer and the help of a failed page keep saying `sheet`.
    table_sheets: bool,
    /// The text of the page that failed to draw as the file's text (an SVG that cannot be drawn),
    /// read once: a key scrolls it without asking the session again or reading the file again.
    failed_body: Option<FailedBody>,
    /// The width of the terminal at the last view (a document is laid out at it).
    width: u16,
    /// A half page, when the view sets its own (a document: half the frame, as the host).
    half: Option<usize>,
    /// The document laid out last, kept while its text and width are the same.
    doc: Option<super::document::DocCache>,
    /// The menu waiting for its key (`y` was pressed), and the keys it was opened in.
    menu: Option<(CoreContext, CoreMenu)>,
    /// What the last key did, shown in the footer until the next key.
    flash: Option<String>,
    /// `Esc` left the plugin's text input: the typing is not taken again until the user does
    /// something the plugin answers with a canvas that asks for it (`typing::retake`).
    input_left: bool,
    /// A declared action the user pressed is out: the answer to it (and the view it asks for) is
    /// the user's (`typing::settle_user_act`).
    user_act: bool,
    /// The keys and the actions waiting for the session. The loop calls the session itself, so
    /// they are sent at once; they go through the same line as konoma's so the order and the
    /// bounds are the same rules.
    key_queue: Vec<Key>,
    act_queue: Vec<QueuedAct>,
}

/// The title of the frame: the path in the `style` the `,` menu chose, or the title the host gave
/// when it launched no path.
fn frame_name_of(cx: &AppCx, style: konoma_host_core::view_options::PathStyle) -> String {
    use konoma_host_core::view_options::PathStyle;
    if cx.path.as_os_str().is_empty() || (cx.open_dir.is_none() && style == PathStyle::Relative) {
        return cx.title.clone();
    }
    style.format(
        cx.open_dir.as_deref().unwrap_or(std::path::Path::new("")),
        &cx.path,
    )
}

/// Writes `name` into the title of the frame on screen (a frame without a path in its title keeps
/// none).
fn retitle(frame: &mut Frame, name: &str) {
    match frame {
        Frame::Text(t) => {
            if let Some(b) = t.border.as_mut() {
                b.name = name.to_string();
            }
        }
        Frame::Table(t) => t.name = name.to_string(),
        Frame::Image(i) => i.name = name.to_string(),
        Frame::Document(d) => d.name = name.to_string(),
        Frame::Lines(_) | Frame::Canvas(_) | Frame::ImageFailed(_) => {}
    }
}

/// The lines (the reason, then the file) a failed page shows, and the page they belong to.
struct FailedBody {
    page: usize,
    lines: Vec<String>,
}

/// Run the app on `terminal` until the key of leaving (`q` by default, or `Ctrl-C`, or the end of
/// `events`). `keys` says what each key does. A canvas shows its cells only: see
/// [`run_with_pictures`] for the pictures a canvas places.
pub fn run_with<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    session: &mut dyn Session,
    events: &mut E,
    cx: &AppCx,
    keys: &AppKeys,
) -> Result<(), AppError> {
    run_with_pictures(terminal, session, events, cx, keys, None)
}

/// [`run_with`] with the pictures a canvas places (`pictures`: `None` is a terminal that cannot
/// show any, where the canvas shows its cells and nothing is decoded).
///
/// Every wait is bounded: while a decode or an encode is out, or a GIF plays, the loop wakes
/// every few milliseconds to draw what the workers finished. Such a wake does not ask the
/// session for a view again (it would not have changed): only a key, a resize and the view's
/// own redraw timer do. Leaving drops `pictures`, which cancels the decode in flight and
/// deletes every kitty image placed.
pub fn run_with_pictures<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    session: &mut dyn Session,
    events: &mut E,
    cx: &AppCx,
    keys_in: &AppKeys,
    mut pictures: Option<Pictures>,
) -> Result<(), AppError> {
    // The keys follow the options the `,` menu changes (whether the text wraps decides which
    // sideways keys act), so the loop keeps its own copy.
    let mut own = keys_in.clone();
    super::edit_line::clear();
    let mut st = State {
        display: cx.display.clone(),
        // The mistakes in the user's `[keys.<id>]` are told at start, as konoma's footer does.
        flash: crate::host::bind::key_notes_line(&cx.acts.notes, own.lang()),
        ..State::default()
    };
    // The session a `Reopen` opened in place of `session` (the plugin that converted the file):
    // from then on it is the one asked.
    let mut swapped: Option<Box<dyn Session>> = None;
    // A `Reopen` that failed: the converting session is still the one asked, and it would answer
    // `Reopen` again at every key and resize; the failure is the answer from then on, so the
    // destination is opened (and fails) once, as konoma's one load does.
    let mut reopen_failed: Option<crate::session::Failure> = None;
    macro_rules! sess {
        () => {
            match swapped.as_deref_mut() {
                Some(s) => s,
                None => &mut *session,
            }
        };
    }
    let terr = |e: &dyn std::fmt::Display| AppError::Terminal(e.to_string());
    // The frame of the last view, and when that view is due again (a wake for the pictures
    // draws this frame again).
    let mut base = Frame::Lines(Vec::new());
    let mut view_due: Option<Instant> = None;
    let mut need_view = true;
    loop {
        let size = terminal.size().map_err(|e| terr(&e))?;
        let body_rows = size.height.saturating_sub(1);
        st.page = usize::from(body_rows).max(1);
        st.width = size.width;
        st.half = None;
        // The kept text of a failed page is scrolled by the keys without asking the session
        // again: its window is the rows inside the host's border, paged as the host pages it.
        if st
            .failed_body
            .as_ref()
            .is_some_and(|b| b.page == st.at_page)
        {
            set_failed_paging(&mut st, usize::from(body_rows));
        }
        if view_due.is_some_and(|d| Instant::now() >= d) {
            need_view = true;
        }
        let mut view_wait = None;
        // The file is opened again with its plugin (an `Outcome::Reload`): the new session is the
        // one asked from here on, and a `Reopen` the file leads to is decided again. The frame
        // on screen and the scroll stay until the new view lands. A source that does not know the
        // file (`ReopenSource::with_file`) leaves a plain redraw.
        while std::mem::take(&mut st.reload) {
            if let Some(opened) = cx.reopen.as_ref().and_then(|r| r.reload(&cx.lang, cx.term)) {
                reopen_failed = None;
                st.reopened_as = None;
                match opened {
                    Ok(fresh) => swapped = Some(fresh),
                    // The reason is what the view shows, as a file that fails to open does.
                    Err(f) => reopen_failed = Some(f),
                }
                need_view = true;
                // The keys and actions that waited for the new session go to it, in order.
                if flush_jobs(sess!(), &mut st, cx) {
                    need_view = true;
                }
            }
        }
        if need_view {
            let mut req = ViewReq::new(size.width, body_rows);
            if st.at_page > 0 {
                req = req.page(st.at_page);
            }
            let vcx = ViewCx::new(Settings::empty(), &cx.lang).with_term(cx.term);
            let mut viewed = match &reopen_failed {
                Some(f) => Err(f.clone()),
                None => sess!().view(&req, &vcx),
            };
            // "Open this again as Markdown": the plugin that takes the content type is opened
            // on the text (the rule konoma follows, `crate::reopen`), and its session replaces
            // this one. Everything of the old document is dropped with it: the scroll, the
            // laid-out page and the pictures (a result the old document asked for lands on
            // nothing).
            if let Ok(View::Reopen(r)) = viewed {
                let target = cx.reopen.as_ref().and_then(|s| s.target(&r.content_type));
                viewed = match crate::reopen::reopen_document(
                    target.as_ref(),
                    REOPEN_ACCEPTS,
                    r,
                    &cx.lang,
                    &Cancel::never(),
                    cx.term,
                ) {
                    Ok(opened) => {
                        swapped = Some(opened.session);
                        st = state_after_reopen(
                            st,
                            target.as_ref().map(|t| {
                                let m = t.plugin.manifest();
                                ReopenedAs {
                                    acts: cx
                                        .reopen
                                        .as_ref()
                                        .map(|r| r.acts_for(&m))
                                        .unwrap_or_default(),
                                    shows: m.shows,
                                    unavailable: m.unavailable,
                                    plugin_id: m.id,
                                }
                            }),
                            &cx.acts,
                        );
                        st.page = usize::from(body_rows).max(1);
                        st.width = size.width;
                        if let Some(p) = pictures.as_mut() {
                            p.forget();
                            p.forget_still();
                            p.reset_document();
                        }
                        Ok(View::Document(opened.document))
                    }
                    Err(f) => {
                        reopen_failed = Some(f.clone());
                        Err(f)
                    }
                };
            }
            let frame_name = frame_name_of(cx, st.display.path_style);
            let shown = match viewed {
                Ok(v) => {
                    st.table_failed = false;
                    st.image_failed = false;
                    st.failed_body = None;
                    st.last_image = matches!(v, View::Image(_));
                    Ok(v)
                }
                // A dropped stale answer says nothing and changes nothing (konoma does the same).
                Err(f) if f.kind == crate::session::FailureKind::Cancelled => Err(None),
                Err(f) => {
                    // What kind of paged page failed: what the plugin declares it shows, else
                    // what was on screen before (a first page that fails has nothing before it).
                    let picture = failed_page_is_picture(cx, &st);
                    note_failed_page(sess!(), cx, &mut st, picture);
                    Err(Some(failure_frame(
                        image_failure_for(st.unavailable(cx), &f),
                        f.message.get(&cx.lang),
                        cx,
                        own.lang(),
                        usize::from(body_rows),
                        &mut st,
                    )))
                }
            };
            let (mut frame, redraw_after) = match shown {
                Ok(view) => frame_of(
                    view,
                    &mut st,
                    usize::from(body_rows),
                    (&cx.lang, own.lang()),
                    &frame_name,
                    pictures.as_mut(),
                ),
                Err(Some(frame)) => (frame, None),
                // The frame on screen stays.
                Err(None) => (std::mem::replace(&mut base, Frame::Lines(Vec::new())), None),
            };
            // The typing: a canvas that asks for it after something the user did takes it back
            // after `Esc`; the show going (no canvas on screen) forgets the keys and the latch.
            if let Frame::Canvas(c) = &frame {
                typing::retake(&mut st.input_left, c.text_input, st.user_act);
                st.user_act = false;
            } else {
                typing::forget(&mut st.key_queue, &mut st.input_left, &mut st.user_act);
                st.act_queue.clear();
            }
            if let Frame::Table(new) = &mut frame {
                // A table read again (a resize, a page turn) keeps the cursor and the popup; a
                // page turn has reset them before asking.
                if let Frame::Table(old) = &mut base {
                    new.state = old.state;
                    new.popup = old.popup.take();
                }
                new.clamp_cursor();
                st.table_sheets = new.addressed;
                let layout = sess!().layout().clipped();
                st.pages = match layout.pages {
                    Some(PageLayout::Separate { count, titles }) => {
                        count.or((!titles.is_empty()).then_some(titles.len()))
                    }
                    _ => None,
                };
            }
            if matches!(frame, Frame::Image(_)) {
                // A picture's pages are what the host counts: the count the layout reports.
                let layout = sess!().layout().clipped();
                st.pages = pages::count_of(&layout)
                    .flatten()
                    .map(|n| n as usize)
                    .filter(|n| *n > 0);
                if let Some(n) = st.pages {
                    st.at_page = st.at_page.min(n - 1);
                }
            }
            let turned = std::mem::take(&mut st.turned);
            if let Some(p) = pictures.as_mut() {
                match &frame {
                    Frame::Canvas(c) => {
                        p.submit(c);
                        p.forget_still();
                    }
                    Frame::Image(i) => {
                        p.forget();
                        p.submit_image(&i.view, turned);
                    }
                    _ => {
                        p.forget();
                        p.forget_still();
                    }
                }
            }
            view_wait = redraw_after.map(|d| d.clamp(MIN_REDRAW, MAX_REDRAW));
            view_due = view_wait.map(|d| Instant::now() + d);
            base = frame;
        }
        if let Some(p) = pictures.as_mut() {
            p.poll();
            // A picture's result changed what the page reserves: lay it out again before drawing.
            if matches!(base, Frame::Document(_)) && p.take_doc_relayout() {
                need_view = true;
                continue;
            }
        }
        // A picture whose decode failed is a page that failed to draw, as one whose `view`
        // failed: the same screen, footer and help (konoma's one `unavailable_body` serves both).
        if let (Frame::Image(_), Some(why)) =
            (&base, pictures.as_ref().and_then(Pictures::still_failure))
        {
            note_failed_page(sess!(), cx, &mut st, true);
            base = failure_frame(
                why,
                "[can not preview]",
                cx,
                own.lang(),
                usize::from(body_rows),
                &mut st,
            );
        }
        // The sideways keys are named and act on the file's text (they move the caret) and on the
        // text of a failed page (the host names them there whether it wraps or not).
        own.caret = windowed_text(&base) || failed_text(&base);
        match &base {
            Frame::Text(_) if windowed_text(&base) && st.file_text => {
                super::edit_line::publish_text(st.caret_line)
            }
            Frame::Document(d) => {
                super::edit_line::publish_document(&d.laid, d.scroll, d.wrap, st.doc_edit.clone())
            }
            _ => super::edit_line::clear(),
        }
        let keys = &own;
        let mode = shown_mode(&base, &st);
        let image_mode = image_mode(&base, pictures.as_ref(), &st);
        let paged = image_mode.is_some_and(|m| m.paged);
        // After a page that failed and shows the file as text, or in the picture's own frame
        // (the host draws that as a text too), the screen is a text (the host's): its footer and
        // help, with the page turn named.
        let failed_text_screen = matches!(base, Frame::ImageFailed(_))
            || (image_mode.is_some() && st.image_failed && st.scrolls);
        // The picture's own frame holds no text: no motion scrolls it, and none is named.
        let text_scrolls = !matches!(base, Frame::ImageFailed(_));
        // A plugin's canvas has its own keys (the host's `CoreContext::Canvas`).
        let canvas_keys = matches!(base, Frame::Canvas(_));
        // The key context of the plugin's layer for the screen (konoma's `plugin_acts_in(ctx)`):
        // one answer for the key, the footer and the help. After a `Reopen` the plugin is the
        // destination, and its actions are the ones that act.
        let act_ctx = if mode.is_some() {
            "table"
        } else if image_mode.is_some() && !failed_text_screen {
            "image"
        } else if canvas_keys {
            "canvas"
        } else if matches!(base, Frame::Document(_)) {
            "document"
        } else {
            "text"
        };
        // The shared keys' context of the same screen (the `Space` menu's).
        let core_ctx = if mode.is_some() {
            CoreContext::Table
        } else if image_mode.is_some() && !failed_text_screen {
            CoreContext::Image
        } else if canvas_keys {
            CoreContext::Canvas
        } else {
            text_ctx(&base)
        };
        // Whether the text is the file's, in the host's frame (the footer plan's selection keys).
        let windowed = windowed_text(&base);
        // The actions of the plugin whose session this is (the destination's after a `Reopen`).
        let acts = st.acts(cx).clone();
        let en: Vec<String> = enabled_ids(&*sess!(), &acts);
        let act_on = |a: &str| en.iter().any(|e| e == a);
        let capturing = typing::is_capturing(
            st.input_left,
            st.help.is_none(),
            matches!(&base, Frame::Canvas(c) if c.text_input),
        );
        let footer;
        let mut panel: Option<HelpPanel> = None;
        let width = terminal.size().map(|s| s.width).unwrap_or(0);
        let frame: &mut Frame = match st.help {
            Some(top) => {
                let (mut secs, title) = match (mode, image_mode) {
                    (Some(m), _) => (keys.table_help_sections(m), keys.table_help_title(m)),
                    (None, Some(_)) if !failed_text_screen => {
                        (keys.image_help_sections(paged), keys.image_help_title())
                    }
                    (None, None) if canvas_keys => {
                        (keys.canvas_help_sections(), keys.canvas_help_title())
                    }
                    (None, _) => (
                        keys.help_sections_in(text_ctx(&base), paged, text_scrolls),
                        keys.help_title_in(text_ctx(&base)),
                    ),
                };
                // The plugin's own keys: one section under its name, right after the screen's
                // own (the host's order).
                if let Some(own) = acts.help_section(act_ctx, &cx.lang, act_on) {
                    secs.insert(keys.plugin_section_at(act_ctx, &secs), own);
                }
                let items = keys.space_items(
                    core_ctx,
                    &cx.lang,
                    &acts,
                    act_ctx,
                    windowed_text(&base),
                    act_on,
                );
                keys.insert_actions_section(core_ctx, &items, &mut secs);
                let lines = lines_of(&secs);
                // The panel is laid out in the whole screen, as the host's is; the scroll is
                // settled at draw time (the end of the help is as far as it goes).
                let screen = terminal
                    .size()
                    .map(|s| ratatui::layout::Rect::new(0, 0, s.width, s.height))
                    .unwrap_or_default();
                let top = top.min(usize::from(konoma_widgets::help_panel::max_scroll(
                    lines.len(),
                    screen,
                )));
                st.help = Some(top);
                footer = konoma_widgets::help_panel::fit_tokens(
                    &keys.help_footer_tokens(),
                    screen.width,
                );
                panel = Some(HelpPanel {
                    lines,
                    scroll: top as u16,
                    title,
                });
                &mut base
            }
            None => {
                footer = match mode {
                    Some(m) => {
                        if let Frame::Table(t) = &mut base {
                            if let Some(p) = t.popup.as_mut() {
                                p.footer = keys
                                    .popup_tokens()
                                    .into_iter()
                                    .map(|t| t.text)
                                    .collect::<Vec<_>>()
                                    .join("  ");
                            }
                        }
                        // The cell popup has its own keys: the plugin's are not there.
                        let tokens = if m.popup {
                            keys.table_footer_tokens(m)
                        } else {
                            acts.with_footer(act_ctx, &cx.lang, act_on, keys.table_footer_tokens(m))
                        };
                        draw::footer_row(&tokens, "", "", width)
                    }
                    None if image_mode.is_some() && !failed_text_screen => {
                        let m = image_mode.unwrap_or(ImageMode {
                            shape: StillShape::Failed,
                            paged: false,
                            zoomed: false,
                        });
                        let zoom = match (m.shape, pictures.as_ref()) {
                            (StillShape::Ready, Some(p)) => {
                                format!("  x{:.2}", p.still_zoom())
                            }
                            _ => String::new(),
                        };
                        // The host's status row says the page after the zoom (`2/5`), and a
                        // spinner while the picture of a page is on its way.
                        let page = if pictures.is_some() {
                            pages::indicator(st.at_page as u32 + 1, st.pages.map(|n| n as u32))
                                .map(|(cur, total)| {
                                    format!("  {}", pages::indicator_text(cur, total))
                                })
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let busy = pictures
                            .as_ref()
                            .and_then(Pictures::still_busy)
                            .map(|g| format!("{g} "))
                            .unwrap_or_default();
                        draw::footer_row(
                            &acts.with_footer(
                                act_ctx,
                                &cx.lang,
                                act_on,
                                keys.image_footer_tokens(m.paged),
                            ),
                            format!("{zoom}{page}  {busy}").trim_end(),
                            &cx.title,
                            width,
                        )
                    }
                    None => draw::footer(
                        &acts.with_footer(
                            act_ctx,
                            &cx.lang,
                            act_on,
                            if canvas_keys {
                                keys.canvas_footer_tokens()
                            } else if matches!(base, draw::Frame::Document(_)) {
                                keys.document_footer_tokens(st.scrolls)
                            } else {
                                keys.footer_hints_in(
                                    text_ctx(&base),
                                    st.scrolls || (failed_text_screen && text_scrolls),
                                    paged,
                                )
                                .into_iter()
                                .map(|t| t.text)
                                .collect()
                            },
                        ),
                        &cx.title,
                        &base,
                        st.scroll,
                        st.scrolls,
                        width,
                    ),
                };
                &mut base
            }
        };
        // What the last key did, or the menu waiting for its key, takes the footer over.
        // The hints are dim, the notice bold and the menu's line a bold heading with dim items:
        // the host's footer, spans and all.
        let footer = match (&st.flash, st.menu) {
            (Some(note), _) => konoma_widgets::footer_line::flash_spans(note),
            // The plugin takes the typing: the footer says how to leave (konoma's mode hint).
            (None, _) if capturing => {
                vec![ratatui::style::Stylize::bold(ratatui::text::Span::from(
                    CoreMsg::StPluginInputHint.tr(keys.lang()),
                ))]
            }
            (None, Some((c, CoreMenu::Actions))) => keys.space_line(
                &keys.space_items(c, &cx.lang, &acts, act_ctx, windowed, act_on),
                width,
            ),
            (None, Some((c, m))) => keys
                .which_key_line(c, m, width)
                .unwrap_or_else(|| hint_spans(footer)),
            (None, None) => hint_spans(footer),
        };
        draw_frame(
            terminal,
            frame,
            &footer,
            st.display.bg(),
            pictures.as_mut(),
            panel,
        )?;

        // How long to wait: the view's own timer, or sooner for the pictures. A timeout that
        // came from the pictures' shorter wake leaves the view as it is.
        let view_left = match view_wait {
            Some(w) => Some(w),
            None => view_due.map(|d| d.saturating_duration_since(Instant::now())),
        };
        // The canvas under the open help is still drawn (the host draws it too) but not animated:
        // nothing wakes the loop for it.
        let pic_wait = pictures
            .as_ref()
            .and_then(Pictures::wake_in)
            .filter(|_| st.help.is_none());
        let picture_first = match (view_left, pic_wait) {
            (Some(v), Some(p)) => p < v,
            (None, Some(_)) => true,
            _ => false,
        };
        let wait = if picture_first { pic_wait } else { view_left };
        // A key and a resize ask for a new view; a timeout does unless the pictures woke the loop.
        need_view = true;
        match events.next(wait) {
            Next::End => return Ok(()),
            Next::Timeout => need_view = !picture_first,
            Next::Event(AppEvent::Resize) => need_view = true,
            Next::Event(AppEvent::Paste(text)) => {
                // Only a canvas that takes the typing gets a paste; elsewhere it does nothing.
                need_view = false;
                if capturing {
                    st.flash = None;
                    let admitted = typing::admit_paste(&mut st.key_queue, text);
                    if let Some(n) = admitted.notice {
                        st.flash = Some(notice_text(n, keys.lang()));
                    }
                    if admitted.queued && flush_jobs(sess!(), &mut st, cx) {
                        need_view = true;
                    }
                }
            }
            Next::Event(AppEvent::Key(key)) if capturing => {
                // Every key is the plugin's; `Esc` leaves (and is told to the plugin).
                st.flash = None;
                st.menu = None;
                let admitted = typing::admit_key(&mut st.key_queue, &mut st.input_left, key);
                if let Some(n) = admitted.notice {
                    st.flash = Some(notice_text(n, keys.lang()));
                }
                need_view = false;
                if admitted.queued && flush_jobs(sess!(), &mut st, cx) {
                    need_view = true;
                }
                // The footer changes when the typing is left.
                if st.input_left {
                    need_view = true;
                }
            }
            Next::Event(AppEvent::Key(key)) => {
                // The previous notice goes with the next key.
                st.flash = None;
                // A key waiting in the copy menu runs its item; any other closes the menu and
                // does nothing else (`Esc` and `q` included), as the host's.
                let mut picked: Option<space::SpaceOp> = None;
                let mut in_menu = false;
                let mut item: Option<CoreAction> = None;
                match st.menu.take() {
                    Some((ctx, CoreMenu::Actions)) => {
                        // The `Space` menu: a key of an item picks it; the display options run
                        // as the `,` menu's do, the others as the key of the operation would
                        // (below); any other key only closes the menu.
                        in_menu = true;
                        need_view = false;
                        let items = keys.space_items(
                            ctx,
                            &cx.lang,
                            &acts,
                            act_ctx,
                            windowed_text(&base),
                            act_on,
                        );
                        match key_press(&key)
                            .and_then(|kp| items.iter().find(|i| i.key == kp))
                            .map(|i| i.action)
                        {
                            Some(space::SpaceOp::Core(a)) => item = Some(a),
                            Some(op) => {
                                picked = Some(op);
                                in_menu = false;
                            }
                            None => {}
                        }
                    }
                    Some((ctx, menu)) => {
                        in_menu = true;
                        need_view = false;
                        item =
                            key_press(&key).and_then(|kp| keys.copy_menu_view(ctx, menu)?.find(kp));
                    }
                    None => {}
                }
                if in_menu {
                    if let Some(action @ (CoreAction::ToggleWrap | CoreAction::CyclePathStyle)) =
                        item
                    {
                        // The options menu: the core both hosts share says what to redo.
                        let mut o = konoma_host_core::view_options::ViewOptions {
                            wrap: st.display.wrap,
                            path_style: st.display.path_style,
                        };
                        let effect = o.apply(action);
                        st.display.wrap = o.wrap;
                        st.display.path_style = o.path_style;
                        if effect.hscroll_home {
                            st.hscroll = 0;
                        }
                        if effect.relayout {
                            // The laid-out document and the kept text of a failed page are
                            // for the old wrap; the view is asked for again.
                            st.doc = None;
                            st.failed_body = None;
                            own.wrap = o.wrap;
                            need_view = true;
                        }
                        if effect.retitle {
                            retitle(&mut base, &frame_name_of(cx, o.path_style));
                        }
                    } else if let Some(action) = item {
                        let table = match &base {
                            Frame::Table(t) => Some(&**t),
                            _ => None,
                        };
                        st.flash = super::copy::run(
                            action,
                            keys.lang,
                            &cx.path,
                            cx.hint.as_deref(),
                            cx.open_dir.as_deref(),
                            table,
                        );
                    }
                    continue;
                }
                let table_mode = table_mode(&base, st.pages);
                let popup_open = table_mode.is_some_and(|m| m.popup);
                // After a failed page the message is on screen, but the table keys stay (the page
                // keys turn on, as the host's do).
                let table_keys = table_mode.is_some() || st.table_failed;
                // A failed page that shows the file as text scrolls like any text; the page keys
                // stay (as the host's do).
                let failed_text = failed_text_screen && st.help.is_none();
                let image_keys = image_mode.is_some() && !failed_text;
                // The page keys act over a picture (a failed page too) whatever scrolls, where the
                // footer and the help name them (`ImageMode::paged`): with no pictures to draw
                // them they do nothing, and are not named.
                let on_image = image_mode.is_some_and(|m| m.paged);
                // The plugin's layer (konoma asks it before the shared keys; on a canvas the fixed
                // keys too, elsewhere the fixed keys are the host's): a key it declares for the
                // screen on view. Not in the help, not in the cell popup.
                let declared = if let Some(op) = picked {
                    space::declared(&acts, act_ctx, op).cloned()
                } else if st.help.is_none() && !popup_open && (canvas_keys || !is_fixed_key(&key)) {
                    key_press(&key)
                        .and_then(|kp| acts.for_key(act_ctx, kp))
                        .cloned()
                } else {
                    None
                };
                let act = match (table_keys, image_mode) {
                    _ if picked.is_some() => match picked {
                        Some(space::SpaceOp::Back) => Some(Act::Back),
                        Some(space::SpaceOp::Help) => Some(Act::Help),
                        Some(space::SpaceOp::Cell) => Some(Act::Cell),
                        Some(space::SpaceOp::Zoom(d)) => Some(Act::Zoom(d.into())),
                        Some(space::SpaceOp::ZoomReset) => Some(Act::ZoomReset),
                        _ => None,
                    },
                    _ if declared.is_some() => None,
                    // The help is its own context: the same keys over any screen.
                    _ if st.help.is_some() => keys.help_act(&key),
                    (true, _) => keys.table_act(&key, popup_open && st.help.is_none()),
                    (false, Some(_)) if failed_text => keys
                        .act_in(text_ctx(&base), &key)
                        .or_else(|| keys.image_act(&key).filter(|a| matches!(a, Act::Page(_)))),
                    (false, Some(_)) => keys.image_act(&key),
                    (false, None) if canvas_keys => keys.canvas_act(&key),
                    (false, None) => keys.act_in(text_ctx(&base), &key),
                };
                // A key over a table never asks the session again, but a page turn: a
                // spreadsheet is read again on every `view`. Nor does a key over a picture: the
                // picture is held, and the pose is the app's.
                // Nor does a key over the text of a page that failed to draw: it is kept, and
                // scrolled from there (a page key asks again below).
                let kept_failure = st
                    .failed_body
                    .as_ref()
                    .is_some_and(|b| b.page == st.at_page)
                    && matches!(base, Frame::Lines(_) | Frame::Text(_));
                if table_keys || image_keys || kept_failure {
                    need_view = false;
                }
                // The copy menu opens on the key bound to it, where a key acts (not in the help
                // or the cell popup).
                if picked.is_none()
                    && act.is_none()
                    && declared.is_none()
                    && st.help.is_none()
                    && !popup_open
                    && keys.opens_actions(core_ctx, &key)
                {
                    st.menu = Some((core_ctx, CoreMenu::Actions));
                    need_view = false;
                } else if act.is_none() && declared.is_none() && st.help.is_none() && !popup_open {
                    let ctx = if table_keys {
                        CoreContext::Table
                    } else if image_keys {
                        CoreContext::Image
                    } else if canvas_keys {
                        CoreContext::Canvas
                    } else {
                        text_ctx(&base)
                    };
                    if let Some(menu) = keys.copy_menu_of(ctx, &key) {
                        if keys.copy_menu_view(ctx, menu).is_some() {
                            st.menu = Some((ctx, menu));
                            need_view = false;
                        }
                    }
                }
                // The declared action runs when the session says it works now; a key of an action
                // that does not work does nothing (and is not offered).
                if let Some(b) = declared {
                    let enabled = sess!().enabled(&b.act);
                    match hacts::gate(Some(&b), enabled, acts.read_only) {
                        hacts::ActGate::Off => {}
                        hacts::ActGate::ReadOnly => {
                            st.flash = Some(CoreMsg::PluginReadOnly.tr(keys.lang()).to_string());
                            need_view = false;
                        }
                        hacts::ActGate::On => {
                            st.user_act = true;
                            let admitted =
                                typing::admit_act(&mut st.act_queue, st.key_queue.len(), &b.act);
                            if let Some(n) = admitted.notice {
                                st.flash = Some(notice_text(n, keys.lang()));
                            }
                            // A view is asked for again only when the answer says so.
                            need_view = admitted.queued && flush_jobs(sess!(), &mut st, cx);
                        }
                    }
                }
                match (act, st.help) {
                    // `Q` and Ctrl-C end the app even from the help (the host's `Q` does); `q`
                    // and `Esc` close it.
                    (Some(Act::Back), Some(_)) => st.help = None,
                    // The popup is closed first; the app is left from the table.
                    (Some(Act::Back), None) if popup_open => {
                        if let Frame::Table(t) = &mut base {
                            t.popup = None;
                        }
                    }
                    (Some(Act::Quit | Act::Back), _) => return Ok(()),
                    (Some(Act::Help), Some(_)) => st.help = None,
                    (Some(Act::Help), None) => st.help = Some(0),
                    (Some(Act::Move(m)), Some(top)) => {
                        st.help = Some(match m {
                            Motion::Down => top.saturating_add(1),
                            Motion::Up => top.saturating_sub(1),
                            Motion::Top => 0,
                            Motion::Bottom => usize::MAX / 2,
                            Motion::PageDown => top.saturating_add(st.page),
                            Motion::PageUp => top.saturating_sub(st.page),
                            Motion::HalfDown => top.saturating_add((st.page / 2).max(1)),
                            Motion::HalfUp => top.saturating_sub((st.page / 2).max(1)),
                            _ => top,
                        })
                    }
                    (Some(Act::Move(m)), None) => match &mut base {
                        Frame::Table(t) => table_move(t, m),
                        Frame::Image(_) => {
                            if let Some(p) = pictures.as_mut() {
                                let (dx, dy) = match m {
                                    Motion::Left => (-1.0, 0.0),
                                    Motion::Right => (1.0, 0.0),
                                    Motion::Up => (0.0, -1.0),
                                    Motion::Down => (0.0, 1.0),
                                    _ => (0.0, 0.0),
                                };
                                p.still_pan(dx, dy);
                            }
                        }
                        _ if st.scrolls => scroll_by(m, &mut st),
                        _ => {}
                    },
                    (Some(Act::Cell), None) => {
                        if let Frame::Table(t) = &mut base {
                            t.popup = match t.popup {
                                Some(_) => None,
                                None if t.grid.ncols() > 0 => Some(PopupState::default()),
                                None => None,
                            };
                        }
                    }
                    (Some(Act::Page(d)), None) => {
                        let count = st.pages.map(|n| n as u32);
                        if let (true, Some(to)) = (
                            table_keys || on_image,
                            pages::turn(st.at_page as u32 + 1, count, d),
                        ) {
                            st.at_page = to as usize - 1;
                            // The new sheet starts at its top left with no popup.
                            if let Frame::Table(t) = &mut base {
                                t.state = Default::default();
                                // Never reached while a popup is open (no `Page` comes then);
                                // kept for when J/K are made to work inside the popup too.
                                t.popup = None;
                            }
                            // A picture stays on screen until the new page's picture lands.
                            st.turned = on_image;
                            if let (true, Some(p)) = (on_image, pictures.as_mut()) {
                                p.still_turn_page();
                            }
                            need_view = true;
                        }
                    }
                    (Some(Act::Zoom(d)), None) => {
                        if let Some(p) = pictures.as_mut() {
                            p.still_zoom_steps(d);
                        }
                    }
                    (Some(Act::ZoomReset), None) => {
                        if let Some(p) = pictures.as_mut() {
                            p.still_zoom_reset();
                        }
                    }
                    (Some(Act::Cell | Act::Page(_) | Act::Zoom(_) | Act::ZoomReset), Some(_)) => {}
                    (None, _) => {}
                }
                if kept_failure && !need_view {
                    let name = frame_name_of(cx, st.display.path_style);
                    base = failed_window(&mut st, usize::from(body_rows), &name, own.lang());
                }
            }
        }
    }
}

/// Whether `key` is one of the host's fixed keys (arrows, `Home`, `End`, `Esc`, `Enter`, `Tab`,
/// `Shift-Tab`): outside a canvas they never reach the plugin's layer (konoma's
/// `ops::keys::is_fixed_code`).
fn is_fixed_key(key: &Key) -> bool {
    matches!(
        key.code,
        KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Esc
            | KeyCode::Enter
            | KeyCode::Tab
            | KeyCode::BackTab
    )
}

/// The declared actions the session says work now (the one answer the key, the footer and the
/// help all use, as konoma's `Session::enabled` cache).
fn enabled_ids(session: &dyn Session, acts: &super::AppActs) -> Vec<String> {
    acts.bindings
        .iter()
        .filter(|b| session.enabled(&b.act))
        .map(|b| b.act.clone())
        .collect()
}

/// Hand what waits to the session, in the order it was pressed (the same line as konoma's
/// `flush_plugin_keys`), and merge the answers the way konoma does: a reload beats a redraw, the
/// last message wins, a failure is a message. Returns whether the view is to be asked again.
/// `by_user` says the job began with a declared action the user pressed.
fn flush_jobs(session: &mut dyn Session, st: &mut State, cx: &AppCx) -> bool {
    let mut redraw = false;
    while let Some(job) = typing::next_job(&mut st.key_queue, &mut st.act_queue) {
        let acx = ActCx::new(Settings::empty(), &cx.lang).with_term(cx.term);
        let mut results = Vec::new();
        let is_act = matches!(job, NextJob::Act(_));
        match job {
            NextJob::Act(action) => results.push(session.act(&action, &acx)),
            NextJob::Keys(keys) => {
                for k in &keys {
                    results.push(session.key(k, &acx));
                }
            }
        }
        let (mut again, mut flash, mut reload) = (false, None, false);
        for r in results {
            match r {
                Ok(Outcome::Reload) => {
                    again = true;
                    reload = true;
                }
                Ok(Outcome::Redraw) => again = true,
                Ok(Outcome::Flash(msg)) => flash = Some(msg.get(&cx.lang).to_string()),
                Ok(Outcome::Unhandled) => {}
                Err(f) => flash = Some(f.message.get(&cx.lang).to_string()),
            }
        }
        // A reload beats everything (konoma's merge of the keys' answers): the message is dropped.
        if reload {
            st.reload = true;
        } else if flash.is_some() {
            st.flash = flash;
        }
        if is_act {
            // An answer that asks for no redraw is not followed by a view: the mark ends here.
            typing::settle_user_act(&mut st.user_act, false, again);
        }
        redraw |= again;
        // What is still queued waits for the new session (the loop flushes it after the reload).
        if reload {
            break;
        }
    }
    redraw
}

/// The words of a notice of the shared input queue.
fn notice_text(n: Notice, lang: Lang) -> String {
    match n {
        Notice::InputFull => CoreMsg::PluginInputFull,
        Notice::PasteCut => CoreMsg::PluginPasteCut,
    }
    .tr(lang)
    .to_string()
}

/// The mode of the table on screen (`None` = the frame is not a table).
fn table_mode(base: &Frame, pages: Option<usize>) -> Option<TableMode> {
    match base {
        Frame::Table(t) => Some(TableMode {
            popup: t.popup.is_some(),
            paged: pages.is_some_and(|n| n > 1),
            cells: t.grid.ncols() > 0,
            sheets: t.addressed,
        }),
        _ => None,
    }
}

/// The mode of the picture on screen (`None` = the frame is not a picture). With no pictures
/// (no terminal graphics) a picture cannot be shown: only leaving and the help act.
fn image_mode(base: &Frame, pictures: Option<&Pictures>, st: &State) -> Option<ImageMode> {
    // Pages are turned only where a picture can be shown (the host does not even open the file
    // with no graphics backend).
    let paged = pictures.is_some() && pages::can_turn(st.pages.map(|n| n as u32));
    let Frame::Image(_) = base else {
        // After a page that failed the message is on screen, but the page keys stay (the page
        // number is not rolled back, as the host's).
        return st.image_failed.then_some(ImageMode {
            shape: StillShape::Failed,
            paged,
            zoomed: false,
        });
    };
    let shape = pictures
        .and_then(Pictures::still_shape)
        .unwrap_or(StillShape::Failed);
    Some(ImageMode {
        shape,
        paged,
        zoomed: pictures.is_some_and(|p| p.still_zoom() > 1.0),
    })
}

/// What the footer and the help say about the screen: a table's mode, or, after a page that
/// failed, the mode of a paged table with no cell (the page keys act, as the host's do, and
/// `Enter` has nothing to open).
fn shown_mode(base: &Frame, st: &State) -> Option<TableMode> {
    table_mode(base, st.pages).or_else(|| {
        st.table_failed.then(|| TableMode {
            popup: false,
            paged: st.pages.is_some_and(|n| n > 1),
            cells: false,
            sheets: st.table_sheets,
        })
    })
}

/// Move the cell cursor, or scroll the popup while it is open.
fn table_move(t: &mut TableFrame, m: Motion) {
    if let Some(p) = t.popup.as_mut() {
        match m {
            Motion::Down => p.scroll = popup_scroll_by(p.scroll, 1),
            Motion::Up => p.scroll = popup_scroll_by(p.scroll, -1),
            Motion::Top => p.scroll = popup_scroll_to(false),
            Motion::Bottom => p.scroll = popup_scroll_to(true),
            Motion::PageDown => p.scroll = popup_scroll_by(p.scroll, popup_page_step(p.viewport)),
            Motion::PageUp => p.scroll = popup_scroll_by(p.scroll, -popup_page_step(p.viewport)),
            _ => {}
        }
        return;
    }
    let step = match m {
        Motion::Up => TableStep::Up,
        Motion::Down => TableStep::Down,
        Motion::Left => TableStep::Left,
        Motion::Right => TableStep::Right,
        Motion::Top => TableStep::FirstRow,
        Motion::Bottom => TableStep::LastRow,
        Motion::LineHome => TableStep::FirstCol,
        Motion::LineEnd => TableStep::LastCol,
        Motion::PageDown => TableStep::PageDown,
        Motion::PageUp => TableStep::PageUp,
        Motion::HalfDown => TableStep::HalfDown,
        Motion::HalfUp => TableStep::HalfUp,
    };
    t.state.step(step, t.grid.nrows(), t.grid.ncols());
}

/// The `?` help to draw over the screen: its lines, the first line shown and the title.
struct HelpPanel {
    lines: Vec<ratatui::text::Line<'static>>,
    scroll: u16,
    title: String,
}

/// The key hints as the host draws them: one dim span.
fn hint_spans(row: String) -> Vec<ratatui::text::Span<'static>> {
    use ratatui::style::Stylize;
    vec![ratatui::text::Span::from(row).dim()]
}

/// Draw `frame` and `footer`, then the pictures over it. A frame on which a picture moved or went
/// is cleared and drawn again (the cells of its old place would stay on screen otherwise), and the
/// deletes of the kitty images that went are written after it. The right to write to the terminal
/// is held for all of it, so an exit's cleanup never lands between the chunks of a transmit.
fn draw_frame<B: Backend>(
    terminal: &mut Terminal<B>,
    frame: &mut Frame,
    footer: &[ratatui::text::Span<'static>],
    bg: Option<ratatui::style::Color>,
    mut pictures: Option<&mut Pictures>,
    panel: Option<HelpPanel>,
) -> Result<(), AppError> {
    let terr = |e: &dyn std::fmt::Display| AppError::Terminal(e.to_string());
    let on_canvas = matches!(frame, Frame::Canvas(_));
    let image = match frame {
        Frame::Image(i) => Some(i.lang),
        _ => None,
    };
    let _writing = konoma_host_core::term::terminal_write();
    for pass in 0..2 {
        terminal
            .draw(|f| {
                draw::draw(f, frame, footer, bg);
                let body = draw::body_of(f.area());
                if let Some(p) = pictures.as_deref_mut() {
                    p.draw(f.buffer_mut(), body, on_canvas);
                    let doc = match &*frame {
                        Frame::Document(d) => Some((&*d.laid, d.scroll)),
                        _ => None,
                    };
                    p.draw_document(f.buffer_mut(), super::document::DocFrame::inner(body), doc);
                }
                if let Some(lang) = image {
                    let inner = draw::image_inner(body);
                    match pictures.as_deref_mut() {
                        Some(p) => p.draw_still(f.buffer_mut(), inner, lang),
                        None => ratatui::widgets::Widget::render(
                            ratatui::widgets::Paragraph::new(super::still::failure_line(
                                None, lang, "",
                            )),
                            inner,
                            f.buffer_mut(),
                        ),
                    }
                }
                // The `?` help sits above everything else, as the host draws it.
                if let Some(h) = &panel {
                    let area = f.area();
                    konoma_widgets::help_panel::render_panel(
                        f.buffer_mut(),
                        area,
                        h.lines.clone(),
                        h.scroll,
                        h.title.clone(),
                    );
                }
            })
            .map_err(|e| terr(&e))?;
        let moved = pictures.as_deref_mut().is_some_and(Pictures::take_moved);
        if pass == 0 && moved {
            terminal.clear().map_err(|e| terr(&e))?;
        } else {
            break;
        }
    }
    if let Some(p) = pictures {
        p.flush_deletes();
    }
    Ok(())
}

/// The caret's moves over the text on screen: the host's, shared (`konoma_widgets::caret`).
fn caret_by(m: Motion, st: &mut State) {
    let rows = st.text_rows;
    let page = i32::try_from(rows.saturating_sub(1).max(1)).unwrap_or(i32::MAX);
    let half = i32::try_from((rows / 2).max(1)).unwrap_or(i32::MAX);
    let mut c = konoma_widgets::caret::Caret {
        top: st.scroll,
        line: st.caret_line,
        col: st.caret_col,
    };
    let total = st.lines;
    match m {
        Motion::Down => c.step(1, total, rows),
        Motion::Up => c.step(-1, total, rows),
        Motion::PageDown => c.page(page, total, rows),
        Motion::PageUp => c.page(-page, total, rows),
        Motion::HalfDown => c.page(half, total, rows),
        Motion::HalfUp => c.page(-half, total, rows),
        Motion::Top => c.to_top(),
        Motion::Bottom => c.to_bottom(total, rows),
        Motion::Left => c.col_move(-1),
        Motion::Right => c.col_move(1),
        Motion::LineHome => c.col_home(),
        Motion::LineEnd => c.col_end(),
    }
    st.scroll = c.top;
    st.caret_line = c.line;
    st.caret_col = c.col;
}

fn scroll_by(m: Motion, st: &mut State) {
    if st.caret_on {
        return caret_by(m, st);
    }
    let page = st.page;
    let half = st.half.unwrap_or((page / 2).max(1));
    // The host's text window (the caret's) steps 4 cells; its failed page, with no caret, 2.
    let step = if st
        .failed_body
        .as_ref()
        .is_some_and(|b| b.page == st.at_page)
    {
        2
    } else {
        4
    };
    match m {
        Motion::Down => st.scroll = st.scroll.saturating_add(1),
        Motion::Up => st.scroll = st.scroll.saturating_sub(1),
        Motion::PageDown => st.scroll = st.scroll.saturating_add(page),
        Motion::PageUp => st.scroll = st.scroll.saturating_sub(page),
        Motion::HalfDown => st.scroll = st.scroll.saturating_add(half),
        Motion::HalfUp => st.scroll = st.scroll.saturating_sub(half),
        Motion::Top => st.scroll = 0,
        Motion::Bottom => st.scroll = st.total,
        // Sideways, the host's steps (see `step`, and the ends settled at draw time). Wrapping,
        // there is nothing to scroll.
        Motion::Left | Motion::Right | Motion::LineHome | Motion::LineEnd if st.display.wrap => {
            st.hscroll = 0
        }
        Motion::Left => st.hscroll = st.hscroll.saturating_sub(step),
        Motion::Right => st.hscroll = st.hscroll.saturating_add(step),
        Motion::LineHome => st.hscroll = 0,
        Motion::LineEnd => st.hscroll = u16::MAX,
    }
}

/// Turn a view into what is drawn, clamping the scroll to the text's length.
fn frame_of(
    view: View,
    st: &mut State,
    rows: usize,
    (lang_code, lang): (&str, Lang),
    name: &str,
    pictures: Option<&mut Pictures>,
) -> (Frame, Option<Duration>) {
    use crate::view::TextSource;
    st.scrolls = false;
    st.caret_on = false;
    st.file_text = false;
    match view {
        View::Table(t) => (
            Frame::Table(Box::new(TableFrame {
                grid: t.grid,
                name: name.to_string(),
                note: t.title,
                rainbow: t.rainbow,
                addressed: t.addressed,
                lang,
                lang_code: lang_code.to_string(),
                state: Default::default(),
                popup: None,
            })),
            None,
        ),
        View::Message(m) => (Frame::Lines(m.lines), None),
        View::Image(view) => (
            Frame::Image(ImageFrame {
                view,
                name: name.to_string(),
                lang,
            }),
            None,
        ),
        View::Canvas(c) => {
            let after = c.redraw_after;
            (Frame::Canvas(c), after)
        }
        View::Text(t) => {
            // The text sits in the host's frame: the window is the rows inside its border.
            let rows = rows.saturating_sub(2);
            st.text_rows = rows;
            st.page = rows.saturating_sub(1).max(1);
            st.half = Some((rows / 2).max(1));
            let framed = |st: &mut State, lines: Vec<Line>, extent| {
                text_frame(st, lines, Some((name, lang, Some(extent))), true)
            };
            // Text a plugin made has a line in the file only if the plugin says so (the host's
            // `text_plugin_lines_are_file_lines`): that decides whether `e` opens at the caret.
            st.file_text = t.file_lines;
            let frame = match t.source {
                TextSource::Owned(s) => {
                    let lens: Vec<u64> = s.split_inclusive('\n').map(|l| l.len() as u64).collect();
                    clamp(st, lens.len(), rows);
                    let extent = window::extent_of_lines(&lens, st.scroll, rows);
                    let lines: Vec<&str> = s.lines().collect();
                    framed(
                        st,
                        lines
                            .iter()
                            .skip(st.scroll)
                            .take(rows)
                            .map(|l| Line::raw(window::clean_line(l.as_bytes())))
                            .collect(),
                        extent,
                    )
                }
                TextSource::Lines(all) => {
                    // The host writes the lines joined by a newline and windows that file.
                    let last = all.len().saturating_sub(1);
                    let lens: Vec<u64> = all
                        .iter()
                        .enumerate()
                        .map(|(i, l)| {
                            l.spans.iter().map(|s| s.text.len() as u64).sum::<u64>()
                                + u64::from(i < last)
                        })
                        .collect();
                    clamp(st, all.len(), rows);
                    let extent = window::extent_of_lines(&lens, st.scroll, rows);
                    let shown = all.into_iter().skip(st.scroll).take(rows).collect();
                    framed(st, shown, extent)
                }
                TextSource::File(path) => {
                    st.file_text = true;
                    let total = match &st.counted {
                        Some((p, n)) if *p == path => *n,
                        _ => match window::count_lines(&path) {
                            Ok(n) => {
                                st.counted = Some((path.clone(), n));
                                n
                            }
                            Err(e) => {
                                return (
                                    Frame::Lines(vec![Line::raw(format!(
                                        "[can not read {}: {e}]",
                                        path.display()
                                    ))]),
                                    None,
                                )
                            }
                        },
                    };
                    clamp(st, total, rows);
                    let extent = window::file_extent(&mut st.win, &path, st.scroll, rows);
                    match window::read_window(&path, st.scroll, rows) {
                        Ok(w) => framed(st, w.into_iter().map(Line::raw).collect(), extent),
                        Err(e) => Frame::Lines(vec![Line::raw(format!(
                            "[can not read {}: {e}]",
                            path.display()
                        ))]),
                    }
                }
            };
            st.scrolls = true;
            st.caret_on = windowed_text(&frame);
            (frame, None)
        }
        View::Document(dv) => {
            // The frame has a border: the page is laid out inside it.
            let inner_w = st.width.saturating_sub(2);
            let inner_h = rows.saturating_sub(2);
            let mut doc_pics = pictures.map(Pictures::doc_pics);
            let laid = super::document::DocCache::get(
                &mut st.doc,
                &dv,
                inner_w,
                u16::try_from(rows).unwrap_or(u16::MAX),
                lang,
                doc_pics.as_deref_mut(),
                &st.display,
            );
            // A converted document cut at a budget, or a slide that had to leave something out,
            // says so in the title; a Markdown file cut at the display limit says so in its
            // text (see `document::lay_out`).
            st.doc_edit = match &dv.origin {
                crate::input::Origin::File(p) => Some((dv.markdown.lines().count(), p.clone())),
                crate::input::Origin::Converted { .. } => None,
            };
            let cut = matches!(dv.origin, crate::input::Origin::Converted { .. })
                && (dv.truncated || doc_pics.as_deref().is_some_and(|d| d.render_truncated()));
            clamp(st, laid.rows(), inner_h);
            // The host's vertical scroll is a `u16`: a document taller than that scrolls no further.
            st.scroll = st.scroll.min(super::document::MAX_DOC_SCROLL);
            let hscroll = settle_hscroll(st, laid.max_line_cols, inner_w);
            // The host pages by one row less than the frame and halves the frame itself.
            st.page = inner_h.saturating_sub(1).max(1);
            st.half = Some((inner_h / 2).max(1));
            st.scrolls = true;
            (
                Frame::Document(super::document::DocFrame {
                    name: name.to_string(),
                    laid,
                    scroll: st.scroll,
                    wrap: st.display.wrap,
                    hscroll,
                    lang,
                    truncated: cut,
                }),
                None,
            )
        }
        other => (
            Frame::Lines(vec![Line::raw(format!(
                "[not displayable yet: {}]",
                kind_name(&other)
            ))]),
            None,
        ),
    }
}

/// Whether the page that failed is a picture: what the plugin declares it shows, else what was
/// on screen before (a first page that fails has nothing before it).
fn failed_page_is_picture(cx: &AppCx, st: &State) -> bool {
    let declared = match (
        st.shows(cx).contains(&ViewKind::Image),
        st.shows(cx).contains(&ViewKind::Table),
    ) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    };
    declared.unwrap_or(st.last_image)
}

/// What a page that failed to draw leaves in the state, whichever step failed (the `view`, or the
/// picture's decode): the kind of paged page it was, the pages the session declares (known before
/// any page was read), and that the page keys still turn them.
fn note_failed_page(session: &dyn Session, cx: &AppCx, st: &mut State, picture: bool) {
    let declared = matches!(
        (
            st.shows(cx).contains(&ViewKind::Image),
            st.shows(cx).contains(&ViewKind::Table),
        ),
        (true, false) | (false, true)
    );
    if st.pages.is_none() && declared {
        st.pages = declared_pages(session, picture);
        // Pages that carry names are a workbook's sheets.
        st.table_sheets = !picture
            && matches!(
                session.layout().pages,
                Some(PageLayout::Separate { ref titles, .. }) if !titles.is_empty()
            );
    }
    st.table_failed = st.pages.is_some() && !picture;
    st.image_failed = st.pages.is_some() && picture;
    st.scrolls = false;
}

/// What a page that failed to draw shows, in the words konoma shows for the same failure: a
/// picture's frame with the reason inside (a plugin that names the picture frame), the file as
/// text with the reason above it (an SVG), the plugin's sentence, or the failure's message; the
/// last three end with the path. The same function serves a `view` that failed and a picture
/// whose decode failed (`why` is the reason, `message` the failure's own words), as konoma's one
/// `unavailable_body` serves both. The mapping and the words are shared with konoma
/// (`canvas_pictures::image_failure_for`, `unavailable_text`).
fn failure_frame(
    why: Option<ImageFailure>,
    message: &str,
    cx: &AppCx,
    lang: Lang,
    rows: usize,
    st: &mut State,
) -> Frame {
    use crate::canvas_pictures::{raw_file_body, unavailable_text};
    st.failed_body = None;
    let text = unavailable_text(
        st.unavailable(cx),
        message,
        why,
        (&cx.lang, lang),
        st.plugin_id(cx),
        &cx.path,
        &|| raw_file_body(&cx.path, lang),
    );
    match text {
        // The file as text (an SVG that cannot be drawn) scrolls like any text, as in konoma. It
        // is kept: a key scrolls it, the file is not read again.
        Some(t)
            if matches!(
                st.unavailable(cx),
                crate::manifest::Unavailable::RawText { .. }
            ) =>
        {
            st.failed_body = Some(FailedBody {
                page: st.at_page,
                // Not cleaned: konoma hands the text to ratatui as it is, which drops the control
                // characters (a tab too) when it draws.
                lines: t.lines().map(str::to_string).collect(),
            });
            failed_window(st, rows, &frame_name_of(cx, st.display.path_style), lang)
        }
        Some(t) => Frame::Lines(t.lines().map(Line::raw).collect()),
        None => Frame::ImageFailed(FailedImage {
            name: cx.title.clone(),
            lang,
            why,
            plugin_id: st.plugin_id(cx).to_string(),
        }),
    }
}

/// The window of a failed page's text for a body of `rows` rows: the rows inside the host's
/// border, with the page and half-page steps the host takes. Returns the window's rows.
fn set_failed_paging(st: &mut State, rows: usize) -> usize {
    let rows = rows.saturating_sub(2);
    st.text_rows = rows;
    st.page = rows.saturating_sub(1).max(1);
    st.half = Some((rows / 2).max(1));
    rows
}

/// The kept text of a failed page, scrolled as the host scrolls it: the whole text is drawn
/// and moved up by the scroll, which counts drawn rows (the wrapped rows while wrapping, so a
/// long line takes several), clamped against the same count the host uses.
fn failed_window(st: &mut State, rows: usize, name: &str, lang: Lang) -> Frame {
    let rows = set_failed_paging(st, rows);
    let body = st.failed_body.take();
    let all: Vec<Line> = body
        .as_ref()
        .map_or(&[][..], |b| &b.lines[..])
        .iter()
        .map(Line::raw)
        .collect();
    st.scrolls = true;
    st.caret_on = false;
    // In the host's frame (the path alone on its border), as the host draws it: no caret, no
    // position label. The window is the rows inside the border.
    let mut frame = text_frame(st, all, Some((name, lang, None)), false);
    if let Frame::Text(t) = &mut frame {
        let total = if t.wrap {
            konoma_widgets::text::wrapped_row_count(t.lines.clone(), st.width.saturating_sub(2))
        } else {
            t.lines.len()
        };
        clamp(st, total, rows);
        t.vscroll = u16::try_from(st.scroll).unwrap_or(u16::MAX);
    }
    st.failed_body = body;
    frame
}

/// The page count the session declares for a picture's pages or a table's sheets (what a page
/// that read fine reports, asked of the layout when none did).
fn declared_pages(session: &dyn Session, picture: bool) -> Option<usize> {
    let layout = session.layout().clipped();
    if picture {
        return pages::count_of(&layout)
            .flatten()
            .map(|n| n as usize)
            .filter(|n| *n > 0);
    }
    match layout.pages {
        Some(PageLayout::Separate { count, titles }) => {
            count.or((!titles.is_empty()).then_some(titles.len()))
        }
        _ => None,
    }
}

/// The sideways scroll, settled against the widest line (`max_cols`) and the `width` shown: as
/// the host, none while wrapping, and at most the overhang of the widest line.
fn settle_hscroll(st: &mut State, max_cols: usize, width: u16) -> u16 {
    if st.display.wrap {
        st.hscroll = 0;
        return 0;
    }
    let max_h = u16::try_from(max_cols.saturating_sub(usize::from(width))).unwrap_or(u16::MAX);
    st.hscroll = st.hscroll.min(max_h);
    st.hscroll
}

/// The lines of a file's text as drawn: wrapped, or cut `hscroll` cells from the left. `framed`
/// is the host's frame around it (the path, the language, where the window is in the file); the
/// lines are then drawn inside its border, and `None` draws them bare.
fn text_frame(
    st: &mut State,
    lines: Vec<Line>,
    framed: Option<(&str, Lang, Option<konoma_widgets::scroll::ScrollExtent>)>,
    caret: bool,
) -> Frame {
    // The tabs become the host's mark and tab stops (the one function it uses, with the same
    // `[ui] tab_width`), before the width of the widest line is measured.
    let lines = konoma_widgets::text::expand_tabs(
        lines.into_iter().map(ratatui_draw::into_line).collect(),
        st.display.tab_width,
    );
    // The file's text in the host's frame has the caret: its line is kept inside the text and
    // the column on a real character, and the line and the character are drawn as the host's.
    let lines = if framed.is_some() && caret {
        st.caret_line = st.caret_line.min(st.lines.saturating_sub(1));
        st.caret_col = konoma_widgets::caret::settled_col(
            &lines,
            st.scroll,
            st.text_rows,
            st.caret_line,
            st.caret_col,
        );
        konoma_widgets::caret::apply_caret(
            lines,
            st.scroll,
            st.caret_line,
            st.caret_col,
            konoma_widgets::caret::Selection::None,
        )
    } else {
        lines
    };
    let max_cols = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    let width = if framed.is_some() {
        st.width.saturating_sub(2)
    } else {
        st.width
    };
    if framed.is_some() && caret && !st.display.wrap {
        st.hscroll = konoma_widgets::caret::follow_hscroll(&lines, usize::from(width), st.hscroll);
    }
    let hscroll = settle_hscroll(st, max_cols, width);
    Frame::Text(super::draw::TextFrame {
        lines,
        wrap: st.display.wrap,
        hscroll,
        vscroll: 0,
        border: framed.map(|(name, lang, extent)| super::draw::TextBorder {
            name: name.to_string(),
            lang,
            extent,
        }),
    })
}

/// Keep the last page full: the scroll is at most `total - rows`.
fn clamp(st: &mut State, total: usize, rows: usize) {
    st.lines = total;
    st.total = total.saturating_sub(rows);
    st.scroll = st.scroll.min(st.total);
}

fn kind_name(v: &View) -> &'static str {
    match v {
        View::Text(_) => "text",
        View::Document(_) => "document",
        View::Table(_) => "table",
        View::Image(_) => "image",
        View::List(_) => "list",
        View::Diff(_) => "diff",
        View::Canvas(_) => "canvas",
        View::Reopen(_) => "document",
        View::Message(_) => "message",
    }
}

#[cfg(test)]
mod reopen_state_tests {
    use super::*;
    use crate::host::acts::ActBinding;
    use crate::host::typing::QueuedAct;

    fn acts_of(ids: &[&str]) -> super::super::AppActs {
        super::super::AppActs {
            bindings: ids
                .iter()
                .map(|id| ActBinding {
                    act: id.to_string(),
                    key: None,
                    context: None,
                    label: crate::text::Tr::new(id.to_string()),
                    help: crate::text::Tr::new(id.to_string()),
                    footer: None,
                    writes: false,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn dest(ids: &[&str]) -> ReopenedAs {
        ReopenedAs {
            acts: acts_of(ids),
            shows: Vec::new(),
            unavailable: Default::default(),
            plugin_id: "dest".into(),
        }
    }

    fn waiting() -> State {
        State {
            scroll: 9,
            key_queue: vec![Key::char('a'), Key::char('b')],
            act_queue: vec![
                QueuedAct::now("bump"),
                QueuedAct {
                    action: "tbump".into(),
                    after_keys: 2,
                },
                QueuedAct::now("nope"),
            ],
            ..State::default()
        }
    }

    fn kept(st: &State) -> Vec<(&str, usize)> {
        st.act_queue
            .iter()
            .map(|q| (q.action.as_str(), q.after_keys))
            .collect()
    }

    #[test]
    fn a_reopen_keeps_the_typing_and_only_the_actions_the_destination_declared() {
        let own = acts_of(&["bump", "save"]);
        let st = state_after_reopen(waiting(), Some(dest(&["tbump", "tsave"])), &own);
        assert_eq!(kept(&st), [("tbump", 2)]);
        assert_eq!(st.key_queue.len(), 2, "the typed keys stay");
        assert_eq!(st.scroll, 0, "the old document's scroll is dropped");
        assert_eq!(st.plugin_id(&dummy_cx()), "dest");
    }

    #[test]
    fn without_a_destination_the_shows_own_declarations_decide() {
        let own = acts_of(&["bump", "save"]);
        let st = state_after_reopen(waiting(), None, &own);
        assert_eq!(kept(&st), [("bump", 0)]);
    }

    fn dummy_cx() -> AppCx {
        AppCx {
            lang: "en".into(),
            term: crate::TermCaps::new(),
            title: String::new(),
            unavailable: Default::default(),
            plugin_id: "own".into(),
            path: Default::default(),
            hint: None,
            shows: Vec::new(),
            reopen: None,
            open_dir: None,
            display: Default::default(),
            acts: Default::default(),
        }
    }
}

#[cfg(test)]
mod windowed_text_tests {
    use super::*;
    use crate::app::draw::{TextBorder, TextFrame};

    fn text(border: bool) -> Frame {
        Frame::Text(TextFrame {
            lines: Vec::new(),
            wrap: false,
            hscroll: 0,
            vscroll: 0,
            border: border.then(|| TextBorder {
                name: "a.txt".into(),
                lang: Lang::En,
                extent: Some(konoma_widgets::scroll::ScrollExtent {
                    pos: 0,
                    max: 0,
                    viewport: 1,
                }),
            }),
        })
    }

    #[test]
    fn only_the_files_text_in_the_hosts_frame_is_windowed() {
        assert!(windowed_text(&text(true)));
        // The bare text of a page that failed, a message and an empty body are not.
        assert!(!windowed_text(&text(false)));
        assert!(!windowed_text(&Frame::Lines(vec![Line::raw("message")])));
        assert!(!windowed_text(&Frame::Lines(Vec::new())));
    }
}
