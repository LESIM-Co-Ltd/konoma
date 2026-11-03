//! Reuse exact input prefixes of read-only text motions. Every requested case is still compared.

use super::super::image_parity::WhenIdle;
use super::super::table_parity::presses;
use super::super::*;
use konoma_plugin::app::{key_from_crossterm, AppEvent, EventSource, Next};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub(crate) struct Frame {
    pub term: Terminal<TestBackend>,
    pub edit: Option<usize>,
    pub highlight_pending: bool,
}

impl Frame {
    pub fn new(buf: &ratatui::buffer::Buffer, edit: Option<usize>) -> Self {
        let mut term = Terminal::new(TestBackend::new(buf.area.width, buf.area.height)).unwrap();
        term.draw(|f| *f.buffer_mut() = buf.clone()).unwrap();
        Self {
            term,
            edit,
            highlight_pending: false,
        }
    }
}

struct HostState {
    tab: crate::app::PerTab,
    buf: ratatui::buffer::Buffer,
    edit: Option<usize>,
    highlight_pending: bool,
}

impl HostState {
    fn take(s: &mut Sim, repaint: bool) -> Self {
        let highlight_pending = s.app.is_highlight_pending();
        // Match the separate-run helpers' final repaint, including wide-character cleanup.
        if repaint {
            s.term.clear().unwrap();
            s.draw();
        }
        Self {
            tab: s.app.tab.clone(),
            buf: s.term.backend().buffer().clone(),
            edit: s.app.preview_edit_line_for_test(),
            highlight_pending,
        }
    }

    fn restore(&self, s: &mut Sim) {
        s.app.tab = self.tab.clone();
        s.term.draw(|f| *f.buffer_mut() = self.buf.clone()).unwrap();
    }
}

/// Only read-only text motions: the mutable cursor/window state belongs to `PerTab`. Sessions
/// and the file stay unchanged; render caches validate their own path/window keys on the next draw.
pub(crate) fn host_cases(mut s: Sim, specs: &[String]) -> BTreeMap<String, Frame> {
    let requested: std::collections::BTreeSet<String> = specs.iter().cloned().collect();
    let mut children: BTreeMap<String, std::collections::BTreeSet<&str>> = BTreeMap::new();
    for spec in specs {
        let mut prefix = String::new();
        for token in spec.split_whitespace() {
            children.entry(prefix.clone()).or_default().insert(token);
            if !prefix.is_empty() {
                prefix.push(' ');
            }
            prefix.push_str(token);
        }
    }
    let mut states = BTreeMap::from([(String::new(), HostState::take(&mut s, true))]);
    for spec in specs {
        if states.contains_key(spec) {
            continue;
        }
        let tokens: Vec<_> = spec.split_whitespace().collect();
        let mut kept = tokens.len();
        let mut prefix;
        loop {
            prefix = tokens[..kept].join(" ");
            if let Some(state) = states.get(&prefix) {
                state.restore(&mut s);
                break;
            }
            kept -= 1;
        }
        for token in &tokens[kept..] {
            assert!(
                [
                    "j", "k", "g", "G", "<C-f>", "<C-b>", "<C-d>", "<C-u>", "<PgDn>", "<PgUp>",
                    "h", "l", "0", "$"
                ]
                .contains(token),
                "read-only text motion: {token}"
            );
            if !prefix.is_empty() {
                prefix.push(' ');
            }
            prefix.push_str(token);
            let [(code, mods)] = presses(token)[..] else {
                panic!("one motion");
            };
            s.press(code, mods);
            assert!(!s.quit, "the host ended early: {prefix}");
            assert!(!s.app.show_help && s.app.pending_leader.is_none());
            assert!(s.app.flash.is_none(), "motions must not leave a notice");
            // A straight path needs neither a copy nor an extra redraw. Keep only requested
            // screens and the states from which another input sequence branches.
            if requested.contains(&prefix) || children.get(&prefix).is_some_and(|c| c.len() > 1) {
                let state = HostState::take(&mut s, requested.contains(&prefix));
                states.insert(prefix.clone(), state);
            }
        }
    }
    specs
        .iter()
        .map(|spec| {
            let state = &states[spec];
            let mut frame = Frame::new(&state.buf, state.edit);
            frame.highlight_pending = state.highlight_pending;
            (spec.clone(), frame)
        })
        .collect()
}

/// Longest sequences first: a shorter requested sequence can use its exact idle prefix from
/// a longer run. Duplicates retain their comparisons at the call site.
pub(crate) fn longest_first(specs: &[String]) -> Vec<&String> {
    let mut out: Vec<_> = specs.iter().collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.split_whitespace().count()));
    out
}

pub(crate) fn app_cases(
    session: &mut dyn konoma_plugin::Session,
    cx: &konoma_plugin::app::AppCx,
    keys: &konoma_plugin::app::AppKeys,
    (w, h): (u16, u16),
    specs: &[String],
    picture_id: Option<&str>,
) -> BTreeMap<String, Frame> {
    let mut frames = BTreeMap::new();
    let requested: std::collections::BTreeSet<String> = specs.iter().cloned().collect();
    for spec in longest_first(specs) {
        if frames.contains_key(spec) {
            continue;
        }
        let screen = Arc::new(Mutex::new(konoma_test_support::TermScreen::new(w, h)));
        let mut term = Terminal::new(super::SharedScreen(screen.clone())).unwrap();
        let mut events = Capturing::new(spec, screen);
        events.wanted.retain(|n| {
            let prefix = events.tokens[..*n].join(" ");
            requested.contains(&prefix) && !frames.contains_key(&prefix)
        });
        let pics = picture_id.map(|id| {
            konoma_plugin::app::Pictures::new(
                ratatui_image::picker::Picker::halfblocks(),
                id,
                &cx.path,
                800,
            )
        });
        konoma_plugin::app::run_with_pictures(&mut term, session, &mut events, cx, keys, pics)
            .unwrap();
        assert!(
            events.frames.contains_key(spec),
            "the app ended early: {spec}"
        );
        for (prefix, frame) in events.frames {
            frames.entry(prefix).or_insert(frame);
        }
    }
    frames
}

pub(crate) struct Capturing {
    idle: WhenIdle,
    total: usize,
    screen: Arc<Mutex<konoma_test_support::TermScreen>>,
    tokens: Vec<String>,
    wanted: std::collections::BTreeSet<usize>,
    pub frames: BTreeMap<String, Frame>,
}

impl Capturing {
    pub fn new(spec: &str, screen: Arc<Mutex<konoma_test_support::TermScreen>>) -> Self {
        let keys: std::collections::VecDeque<_> = presses(spec)
            .into_iter()
            .map(|(c, m)| AppEvent::Key(key_from_crossterm(&KeyEvent::new(c, m)).unwrap()))
            .collect();
        Self {
            total: keys.len(),
            wanted: (0..=keys.len()).collect(),
            idle: WhenIdle { keys, turns: 0 },
            screen,
            tokens: spec.split_whitespace().map(str::to_owned).collect(),
            frames: BTreeMap::new(),
        }
    }
}

impl EventSource for Capturing {
    fn next(&mut self, wait: Option<std::time::Duration>) -> Next {
        // `None` is the loop's synchronization boundary: all work has landed and the frame was
        // drawn. Timed polls never capture a loading frame or advance the input.
        if wait.is_none() {
            let n = self.total - self.idle.keys.len();
            if self.wanted.contains(&n) {
                self.frames
                    .entry(self.tokens[..n].join(" "))
                    .or_insert_with(|| {
                        Frame::new(
                            &self.screen.lock().unwrap().1,
                            konoma_plugin::app::edit_line(),
                        )
                    });
            }
        }
        self.idle.next(wait)
    }
}

#[test]
fn captures_only_idle_prefixes_and_keeps_them_unchanged() {
    let screen = Arc::new(Mutex::new(konoma_test_support::TermScreen::new(8, 3)));
    screen.lock().unwrap().1[(0, 0)].set_symbol("a");
    let mut events = Capturing::new("j k", screen.clone());
    assert!(matches!(
        events.next(Some(std::time::Duration::ZERO)),
        Next::Timeout
    ));
    assert!(events.frames.is_empty());
    assert!(matches!(events.next(None), Next::Event(_)));
    screen.lock().unwrap().1[(0, 0)].set_symbol("b");
    assert!(matches!(events.next(None), Next::Event(_)));
    screen.lock().unwrap().1[(0, 0)].set_symbol("c");
    assert!(matches!(events.next(None), Next::End));
    assert_eq!(
        events.frames[""].term.backend().buffer()[(0, 0)].symbol(),
        "a"
    );
    assert_eq!(
        events.frames["j"].term.backend().buffer()[(0, 0)].symbol(),
        "b"
    );
    assert_eq!(
        events.frames["j k"].term.backend().buffer()[(0, 0)].symbol(),
        "c"
    );
}
