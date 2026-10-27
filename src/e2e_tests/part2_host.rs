//! Stage 2.5 part 2: the host's behaviour around the job road, for plugins that are not canvases.
//!
//! - `R` and a declared action pressed while the file is being read again wait for the new
//!   session instead of being ignored (the standard document plugins are the ones that lost the
//!   key). The `Open` is held on a gate so that the order is the same on every machine.
//! - A plugin that declares `Manifest::sized` is asked for its view again when the area changes.
//! - What a plugin hands the interface as text is cut at the door.

use super::*;
use crate::plugins::test_plugins::{self, Latch};
use crate::plugins::test_views::DocFile;
use crate::plugins::PluginAct;
use konoma_plugin::prelude::*;
use konoma_plugin::view::{Layout, ReprDecl, TextSource, TextView};
use konoma_plugin::Line;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---- a document plugin whose `open` can be held ------------------------------------------------

struct Gate {
    hold_opens: AtomicBool,
    open_gate: Latch,
    opens: AtomicUsize,
    acts: Mutex<Vec<String>>,
}

struct GatedDoc {
    inner: DocFile,
    sh: Arc<Gate>,
}

impl Plugin for GatedDoc {
    fn manifest(&self) -> Manifest {
        self.inner.manifest().action(
            Act::new("ping")
                .key("z")
                .label(t!(en: "ping", ja: "ping"))
                .help(t!(en: "Say pong", ja: "pong と言う")),
        )
    }

    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        self.sh.opens.fetch_add(1, Ordering::SeqCst);
        if self.sh.hold_opens.load(Ordering::SeqCst) {
            self.sh.open_gate.wait();
        }
        Ok(Box::new(Pinger {
            inner: self.inner.open(input, cx)?,
            sh: self.sh.clone(),
        }))
    }
}

struct Pinger {
    inner: Box<dyn Session>,
    sh: Arc<Gate>,
}

impl Session for Pinger {
    fn layout(&self) -> Layout {
        self.inner.layout()
    }

    fn view(&mut self, req: &ViewReq, cx: &ViewCx) -> Result<View, Failure> {
        self.inner.view(req, cx)
    }

    fn act(&mut self, action: &str, _: &ActCx) -> Result<Outcome, Failure> {
        self.sh.acts.lock().unwrap().push(action.to_string());
        Ok(Outcome::Flash(Tr::new("pong")))
    }
}

struct Held {
    s: Sim,
    sh: Arc<Gate>,
    dir: crate::test_support::TmpDir,
}

fn open_gated(name: &str) -> Held {
    let sh = Arc::new(Gate {
        hold_opens: AtomicBool::new(false),
        open_gate: Latch::default(),
        opens: AtomicUsize::new(0),
        acts: Mutex::default(),
    });
    let dir = sandbox(name);
    std::fs::write(dir.join("a.gd"), "# Heading\n\nsome text\n").unwrap();
    let marker = test_plugins::register(
        name,
        Arc::new(GatedDoc {
            inner: DocFile {
                id: "x-gated",
                ext: "gd",
                takes_markdown: false,
            },
            sh: sh.clone(),
        }),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26).with_async_plugins();
    s.select("a.gd");
    s.enter();
    settle(&mut s);
    s.draw();
    Held { s, sh, dir }
}

#[track_caller]
fn settle(s: &mut Sim) {
    for _ in 0..20 {
        if !s.app.is_media_loading() {
            return;
        }
        s.drain_plugin();
    }
    panic!("a plugin job is still out after 20 answers");
}

/// Change the file so that the host reads it again, with the read held on the gate.
fn reopen_held(h: &mut Held) {
    h.sh.hold_opens.store(true, Ordering::SeqCst);
    std::fs::write(
        h.dir.join("a.gd"),
        "# Heading\n\nsome text, and more of it\n",
    )
    .unwrap();
    h.s.app.reload_preview();
    assert!(
        h.s.app.plugin_session_is_out_for_test(),
        "the old session is out of reach while the file is read again"
    );
    assert!(h.s.app.plugin_is_reopening());
}

#[test]
fn e2e_r_pressed_while_the_file_is_read_again_is_served_when_the_new_session_lands() {
    let mut h = open_gated("p2_r_reopen");
    h.s.see("Heading");
    h.s.dont_see("# Heading");
    assert!(!h.s.app.is_md_raw());
    reopen_held(&mut h);
    // `R` while the read is held: not ignored, not a flash, the choice is made and waits.
    h.s.key('R');
    assert!(h.s.app.is_md_raw(), "the choice stands");
    assert!(
        h.s.app.plugin_wants_view_for_test(),
        "and the view is wished"
    );
    assert!(h.s.app.flash.is_none());
    // The read finishes: the new session answers the request the tab then makes (the source).
    h.sh.open_gate.open();
    settle(&mut h.s);
    assert!(h.s.app.is_md_raw());
    assert!(!h.s.app.plugin_wants_view_for_test(), "served");
    assert_eq!(h.sh.opens.load(Ordering::SeqCst), 2);
    h.s.see("# Heading");
    h.s.see("some text, and more of it");
    // And `R` again goes back, as ever.
    h.s.key('R');
    settle(&mut h.s);
    assert!(!h.s.app.is_md_raw());
    h.s.dont_see("# Heading");
}

#[test]
fn e2e_a_declared_action_pressed_while_the_file_is_read_again_runs_on_the_new_session() {
    let mut h = open_gated("p2_act_reopen");
    let ping = PluginAct::new("x-gated", "ping");
    assert!(h.s.app.plugin_act_enabled(ping));
    reopen_held(&mut h);
    // The hint stays truthful: the key acts (it waits), so it is shown.
    assert!(
        h.s.app.plugin_act_enabled(ping),
        "the key acts while the read is held"
    );
    h.s.key('z');
    assert!(
        h.sh.acts.lock().unwrap().is_empty(),
        "not run on the old session"
    );
    h.s.key('z');
    h.sh.open_gate.open();
    settle(&mut h.s);
    assert_eq!(
        *h.sh.acts.lock().unwrap(),
        ["ping", "ping"],
        "both presses ran, in order, on the new session"
    );
    assert_eq!(h.s.app.flash.as_deref(), Some("pong"));
    assert!(h.s.app.plugin_act_enabled(ping));
}

#[test]
fn e2e_a_free_session_runs_the_action_at_once_and_the_reopening_is_over_when_it_lands() {
    // The waiting is bound to the read-again state: once the new session landed there is nothing
    // to wait for, and a press runs at once.
    let mut h = open_gated("p2_act_plain");
    assert!(!h.s.app.plugin_is_reopening());
    reopen_held(&mut h);
    h.sh.open_gate.open();
    settle(&mut h.s);
    assert!(!h.s.app.plugin_is_reopening(), "the Open answered");
    h.s.app.run_plugin_action(PluginAct::new("x-gated", "ping"));
    settle(&mut h.s);
    assert_eq!(
        *h.sh.acts.lock().unwrap(),
        ["ping"],
        "a free session runs it"
    );
}

// ---- a text plugin whose views depend on the size ----------------------------------------------

struct Rules {
    id: String,
    sized: bool,
    asked: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl Plugin for Rules {
    fn manifest(&self) -> Manifest {
        let m = Manifest::new(self.id.as_str(), "0.1.0")
            .bind(Bind::files(["*.rul"]))
            .shows(ViewKind::Text);
        if self.sized {
            m.sized()
        } else {
            m
        }
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(RulesSession {
            asked: self.asked.clone(),
        }))
    }
}

struct RulesSession {
    asked: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl Session for RulesSession {
    fn view(&mut self, req: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        self.asked.lock().unwrap().push((req.cols, req.rows));
        Ok(View::Text(TextView {
            source: TextSource::Lines(vec![Line::raw(format!("cols={}", req.cols))]),
            styler: None,
            file_lines: false,
            syntax_name: None,
        }))
    }
}

type RulesOpened = (
    Sim,
    Arc<Mutex<Vec<(u16, u16)>>>,
    crate::test_support::TmpDir,
);

fn open_rules(name: &str, sized: bool) -> RulesOpened {
    open_rules_as(name, sized, false)
}

fn open_rules_as(name: &str, sized: bool, asynchronous: bool) -> RulesOpened {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let dir = sandbox(name);
    std::fs::write(dir.join("a.rul"), "x").unwrap();
    let marker = test_plugins::register(
        name,
        Arc::new(Rules {
            id: format!("x-rules-{name}"),
            sized,
            asked: asked.clone(),
        }),
    );
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26);
    if asynchronous {
        s = s.with_async_plugins();
    }
    s.select("a.rul");
    s.enter();
    s.draw();
    (s, asked, dir)
}

#[test]
fn e2e_a_sized_plugin_is_asked_again_when_the_area_changes() {
    let (mut s, asked, _dir) = open_rules("p2_sized", true);
    // The first request had no measure yet; the first frame measured the area and asked again.
    let first = *asked.lock().unwrap().last().unwrap();
    assert!(first.0 > 80, "asked again at the real width: {first:?}");
    s.see(&format!("cols={}", first.0));
    let n = asked.lock().unwrap().len();
    // The terminal shrinks: once more, for the newest size.
    s.resize(50, 26);
    // The frame that measured the new size asked; its answer is drawn by the next frame.
    s.draw();
    let after = *asked.lock().unwrap().last().unwrap();
    assert!(after.0 < first.0, "{after:?}");
    assert_eq!(asked.lock().unwrap().len(), n + 1, "once per change");
    s.see(&format!("cols={}", after.0));
    s.dont_see(&format!("cols={}", first.0));
    // Nothing changed: nothing is asked.
    s.draw();
    s.draw();
    assert_eq!(asked.lock().unwrap().len(), n + 1);
}

#[test]
fn e2e_a_sized_plugin_refit_arrives_through_the_channel_and_is_drawn_without_another_key() {
    // The refit is asked from inside the draw of a frame: taken on the spot it would land after
    // the body was drawn and the old width would stay until the next key.
    let (mut s, asked, _dir) = open_rules_as("p2_sized_chan", true, true);
    for _ in 0..20 {
        s.draw();
        if !s.app.is_media_loading() {
            break;
        }
        s.drain_plugin();
    }
    let first = *asked.lock().unwrap().last().unwrap();
    s.see(&format!("cols={}", first.0));
    let _inline = crate::app::InlineAnswers::force();
    s.resize(50, 26);
    assert!(
        s.app.is_media_loading(),
        "the answer is in the channel, not installed inside the draw"
    );
    assert!(s.drain_plugin());
    let after = *asked.lock().unwrap().last().unwrap();
    assert!(after.0 < first.0);
    s.see(&format!("cols={}", after.0));
}

#[test]
fn e2e_a_plugin_that_does_not_declare_sized_is_not_asked_again() {
    let (mut s, asked, _dir) = open_rules("p2_not_sized", false);
    let n = asked.lock().unwrap().len();
    s.resize(50, 26);
    assert_eq!(
        asked.lock().unwrap().len(),
        n,
        "its view does not depend on the size: {:?}",
        asked.lock().unwrap()
    );
}

#[test]
fn the_profile_of_a_plugin_knows_whether_it_is_sized() {
    use crate::plugins::profile_of;
    assert!(
        profile_of(konoma_mermaid::MERMAID_ID).sized,
        "mermaid declares it"
    );
    assert!(!profile_of(konoma_markdown::MARKDOWN_ID).sized);
    // A canvas is sized without saying so.
    let m = Manifest::new("p2-canvas-profile", "0.1.0").shows(ViewKind::Canvas);
    crate::plugins::record_for_test(&m);
    assert!(profile_of("p2-canvas-profile").sized);
}

// ---- text handed over by a plugin is cut at the door -------------------------------------------

struct Chatty {
    flash: usize,
}

impl Plugin for Chatty {
    fn manifest(&self) -> Manifest {
        Manifest::new("x-chatty", "0.1.0")
            .bind(Bind::files(["*.cht"]))
            .shows(ViewKind::Text)
            .action(
                Act::new("shout")
                    .key("z")
                    .label(Tr::new("y".repeat(self.flash)))
                    .help(Tr::new("h".repeat(self.flash))),
            )
    }

    fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(ChattySession { flash: self.flash }))
    }
}

struct ChattySession {
    flash: usize,
}

impl Session for ChattySession {
    fn layout(&self) -> Layout {
        Layout {
            reprs: vec![
                ReprDecl::new("one", Tr::new("o".repeat(self.flash))),
                ReprDecl::new("two", Tr::new("t".repeat(self.flash))),
            ],
            default_repr: Some("one".into()),
            default_repr_without_pixels: None,
            pages: None,
        }
    }

    fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
        Ok(View::text(["text"]))
    }

    fn act(&mut self, _: &str, _: &ActCx) -> Result<Outcome, Failure> {
        Ok(Outcome::Flash(
            Tr::new("f".repeat(self.flash)).ja("g".repeat(self.flash)),
        ))
    }
}

#[test]
fn e2e_what_a_plugin_says_to_the_interface_is_cut_to_the_limit() {
    use konoma_plugin::text::MAX_TEXT_CHARS;
    let dir = sandbox("p2_chatty");
    std::fs::write(dir.join("a.cht"), "x").unwrap();
    let marker = test_plugins::register("p2_chatty", Arc::new(Chatty { flash: 200_000 }));
    let mut cfg = test_plugins::marker_config(&marker);
    cfg.ui.lang = "en".into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 90, 26);
    s.select("a.cht");
    s.enter();
    // The flash of an action.
    s.key('z');
    let flash = s.app.flash.clone().expect("the plugin spoke");
    assert_eq!(flash.chars().count(), MAX_TEXT_CHARS, "cut at the door");
    assert!(flash.ends_with('…'));
    // The action's label and help in the registry's manifest.
    let registry = crate::plugins::shared_registry(&s.app.cfg.plugin_disabled());
    let act = &registry.manifest("x-chatty").unwrap().actions[0];
    assert_eq!(act.label.en().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(act.help.en().chars().count(), MAX_TEXT_CHARS);
    // The representation labels the layout carries.
    let layout = s.app.plugin_layout_for_test().expect("a layout").clone();
    assert!(layout
        .reprs
        .iter()
        .all(|r| r.label.en().chars().count() == MAX_TEXT_CHARS));
}

#[test]
fn e2e_a_failure_message_a_plugin_returns_is_cut_to_the_limit() {
    use konoma_plugin::text::MAX_TEXT_CHARS;
    struct Failing;
    impl Plugin for Failing {
        fn manifest(&self) -> Manifest {
            Manifest::new("x-failing", "0.1.0")
                .bind(Bind::files(["*.fal"]))
                .shows(ViewKind::Text)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Err(Failure::corrupt(Tr::new("e".repeat(300_000))).with_detail("d".repeat(300_000)))
        }
    }
    let dir = sandbox("p2_failing");
    std::fs::write(dir.join("a.fal"), "x").unwrap();
    let marker = test_plugins::register("p2_failing", Arc::new(Failing));
    let mut s = Sim::with_config_sized(&canon(&dir), test_plugins::marker_config(&marker), 90, 26);
    s.select("a.fal");
    s.enter();
    let f = s.app.plugin_failure().expect("the open failed");
    assert_eq!(f.message.en().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(f.detail.as_deref().unwrap().chars().count(), MAX_TEXT_CHARS);
}

#[test]
fn e2e_a_plugin_that_declares_billions_of_pages_is_read_as_declaring_a_bounded_number() {
    use konoma_plugin::view::{PageLayout, MAX_LAYOUT_PAGES};
    struct Paged;
    impl Plugin for Paged {
        fn manifest(&self) -> Manifest {
            Manifest::new("x-paged", "0.1.0")
                .bind(Bind::files(["*.pgd"]))
                .shows(ViewKind::Text)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(PagedSession))
        }
    }
    struct PagedSession;
    impl Session for PagedSession {
        fn layout(&self) -> Layout {
            Layout {
                pages: Some(PageLayout::Separate {
                    count: Some(4_000_000_000),
                    titles: vec![],
                }),
                ..Layout::default()
            }
        }
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::text(["page"]))
        }
    }
    let dir = sandbox("p2_paged");
    std::fs::write(dir.join("a.pgd"), "x").unwrap();
    let marker = test_plugins::register("p2_paged", Arc::new(Paged));
    let mut s = Sim::with_config_sized(&canon(&dir), test_plugins::marker_config(&marker), 90, 26);
    s.select("a.pgd");
    s.enter();
    assert_eq!(
        s.app.page_count_for_test(),
        Some(MAX_LAYOUT_PAGES as u32),
        "the declared count is bounded at the door"
    );
    assert!(s.app.page_list_usable());
    // The question the footer asks every frame is answered by the count alone: with a count no
    // list could be built for (4 billion strings), it still answers at once.
    s.app.set_page_count_for_test(Some(u32::MAX));
    assert!(s.app.page_list_usable());
    assert!(s.app.is_page_list_preview());
    s.app.set_page_count_for_test(Some(1));
    assert!(!s.app.page_list_usable(), "one page: nothing to choose");
    assert!(s.app.is_page_list_preview());
    s.app.set_page_count_for_test(Some(0));
    assert!(!s.app.page_list_usable());
}

#[test]
fn e2e_the_page_list_of_a_huge_document_is_built_once_when_it_opens_not_per_frame_or_key() {
    use crate::app::outline::PAGE_LIST_BUILDS;
    use konoma_plugin::view::PageLayout;
    struct Paged;
    impl Plugin for Paged {
        fn manifest(&self) -> Manifest {
            Manifest::new("x-paged2", "0.1.0")
                .bind(Bind::files(["*.pgd"]))
                .shows(ViewKind::Text)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(PagedSession))
        }
    }
    struct PagedSession;
    impl Session for PagedSession {
        fn layout(&self) -> Layout {
            Layout {
                pages: Some(PageLayout::Separate {
                    count: Some(100_000),
                    titles: (0..100_000).map(|i| format!("title {i}")).collect(),
                }),
                ..Layout::default()
            }
        }
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::text(["page"]))
        }
    }
    let dir = sandbox("p2_paged_once");
    std::fs::write(dir.join("a.pgd"), "x").unwrap();
    let marker = test_plugins::register("p2_paged_once", Arc::new(Paged));
    let mut s = Sim::with_config_sized(&canon(&dir), test_plugins::marker_config(&marker), 90, 26);
    s.select("a.pgd");
    s.enter();
    // Asking whether `O` is usable builds nothing, however often.
    let before = PAGE_LIST_BUILDS.with(std::cell::Cell::get);
    for _ in 0..20 {
        assert!(s.app.page_list_usable());
        assert!(s.app.is_page_list_preview());
    }
    assert_eq!(
        PAGE_LIST_BUILDS.with(std::cell::Cell::get),
        before,
        "a count, not a list"
    );
    s.key('O');
    assert!(s.app.is_outline(), "the page list is open");
    // Opening builds it; frames and keys while it is open reuse it.
    let opened = PAGE_LIST_BUILDS.with(std::cell::Cell::get);
    assert!(opened > before, "built when it opened");
    for _ in 0..10 {
        s.draw();
        s.key('j');
    }
    s.draw();
    assert!(s.screen().contains("title 10"), "and it is the list");
    assert_eq!(
        PAGE_LIST_BUILDS.with(std::cell::Cell::get),
        opened,
        "ten frames and ten keys built nothing more"
    );
    // Closing drops it; the next opening builds it again.
    s.key('O');
    assert!(!s.app.is_outline());
    s.key('O');
    assert!(PAGE_LIST_BUILDS.with(std::cell::Cell::get) > opened);
}

#[test]
fn e2e_the_page_list_of_a_hundred_thousand_pages_draws_the_end_and_enter_goes_there() {
    use konoma_plugin::view::PageLayout;
    struct Paged;
    impl Plugin for Paged {
        fn manifest(&self) -> Manifest {
            Manifest::new("x-paged3", "0.1.0")
                .bind(Bind::files(["*.pgd"]))
                .shows(ViewKind::Text)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(PagedSession))
        }
    }
    struct PagedSession;
    impl Session for PagedSession {
        fn layout(&self) -> Layout {
            Layout {
                pages: Some(PageLayout::Separate {
                    count: Some(100_000),
                    titles: (0..100_000).map(|i| format!("title {i}")).collect(),
                }),
                ..Layout::default()
            }
        }
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::text(["page"]))
        }
    }
    let dir = sandbox("p2_paged_100k");
    std::fs::write(dir.join("a.pgd"), "x").unwrap();
    let marker = test_plugins::register("p2_paged_100k", Arc::new(Paged));
    let mut s = Sim::with_config_sized(&canon(&dir), test_plugins::marker_config(&marker), 90, 26);
    s.select("a.pgd");
    s.enter();
    s.key('O');
    assert!(s.app.is_outline());
    s.see("title 0");
    s.dont_see("title 99999");
    s.key('G');
    assert_eq!(s.app.outline_sel(), 99_999);
    s.draw();
    s.see("title 99999");
    // One up and back: the window follows the cursor across the u16 boundary.
    for _ in 0..40_000 {
        s.app.outline_move(-1);
    }
    s.draw();
    assert_eq!(s.app.outline_sel(), 59_999);
    s.see("title 59999");
    s.key('G');
    s.enter();
    assert!(!s.app.is_outline());
    assert_eq!(s.app.page_indicator().map(|p| p.0), Some(100_000));
}

#[test]
fn e2e_the_page_list_popup_of_a_long_list_keeps_its_width_while_scrolling_and_resets_on_reopen() {
    use konoma_plugin::view::PageLayout;
    struct Paged;
    impl Plugin for Paged {
        fn manifest(&self) -> Manifest {
            Manifest::new("x-paged4", "0.1.0")
                .bind(Bind::files(["*.pgd"]))
                .shows(ViewKind::Text)
        }
        fn open(&self, _: Input, _: &OpenCx) -> Result<Box<dyn Session>, Failure> {
            Ok(Box::new(PagedSession))
        }
    }
    struct PagedSession;
    impl Session for PagedSession {
        fn layout(&self) -> Layout {
            Layout {
                pages: Some(PageLayout::Separate {
                    count: Some(5_000),
                    titles: (0..5_000)
                        .map(|i| {
                            if i == 60 {
                                "a really rather long title of page sixty".to_string()
                            } else {
                                format!("t{i}")
                            }
                        })
                        .collect(),
                }),
                ..Layout::default()
            }
        }
        fn view(&mut self, _: &ViewReq, _: &ViewCx) -> Result<View, Failure> {
            Ok(View::text(["page"]))
        }
    }
    let dir = sandbox("p2_paged_width");
    std::fs::write(dir.join("a.pgd"), "x").unwrap();
    let marker = test_plugins::register("p2_paged_width", Arc::new(Paged));
    let mut s = Sim::with_config_sized(&canon(&dir), test_plugins::marker_config(&marker), 90, 26);
    s.select("a.pgd");
    s.enter();
    s.key('O');
    s.draw();
    let narrow = s.app.outline_width_floor("");
    for _ in 0..60 {
        s.app.outline_move(1);
    }
    s.draw();
    s.see("long title of page sixty");
    let wide = s.app.outline_width_floor("");
    assert!(wide > narrow, "the window with the long title widened it");
    for _ in 0..3000 {
        s.app.outline_move(1);
    }
    s.draw();
    assert_eq!(
        s.app.outline_width_floor(""),
        wide,
        "scrolling away keeps it"
    );
    // A filter query is a different list: it sizes itself afresh.
    assert_eq!(s.app.outline_width_floor("zz"), 0);
    // Closing and opening again starts over.
    s.key('O');
    s.key('O');
    assert!(
        s.app.outline_width_floor("") < wide,
        "the reopened list forgot the old width"
    );
}

#[cfg(feature = "demo-canvas")]
#[test]
fn the_canvas_trace_line_tells_the_job_road_apart() {
    // The demo-only trace: enough to tell "a job is out" from "the session was lost" from "the
    // file is read again and again".
    let (mut s, _asked, _dir) = open_rules("p2_trace", true);
    let line = s.app.canvas_trace_line(std::time::Instant::now());
    for field in [
        "phase=ready",
        "media_gen=",
        "open_gen=Some(",
        "show_req=Some((",
        "workers=",
        "reopens=",
        "last_discard=",
        "session_out=false",
    ] {
        assert!(line.contains(field), "{field} missing: {line}");
    }
    let reopens = |l: &str| -> u64 {
        l.split("reopens=")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let before = reopens(&line);
    std::fs::write(_dir.join("a.rul"), "changed and longer").unwrap();
    s.app.reload_preview();
    let after = s.app.canvas_trace_line(std::time::Instant::now());
    assert!(reopens(&after) > before, "{line} -> {after}");
}
