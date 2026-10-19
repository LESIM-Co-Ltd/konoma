//! The canvas view: width-aware writing, validation, keys, and the testing helpers.

use std::sync::Arc;
use std::time::Duration;

use konoma_plugin::key::{Key, KeyCode, KeyMods};
use konoma_plugin::manifest::Manifest;
use konoma_plugin::prelude::*;
use konoma_plugin::session::{ColorDepth, TermCaps};
use konoma_plugin::testing::{check_manifest, open_bytes, send_keys, view_canvas, CanvasText};
use konoma_plugin::view::{CanvasCell, CanvasImage, CanvasIssue, CanvasView, CellRect, ImageData};
use konoma_plugin::{keys, Input};
use konoma_plugin::{Cancel, LazyResource, ResourceReq};

fn sym(c: &CanvasView, x: u16, y: u16) -> String {
    c.cell(x, y).unwrap().symbol.clone()
}

fn row(c: &CanvasView, y: u16) -> String {
    CanvasText::of(c).rows[y as usize].clone()
}

fn red() -> Style {
    Style::new().fg(Color::Red)
}

fn rgba() -> ImageData {
    ImageData::Rgba {
        width: 1,
        height: 1,
        pixels: Arc::new(vec![0, 0, 0, 255]),
    }
}

fn img(id: u64, x: u16, y: u16, cols: u16, rows: u16, image: ImageData) -> CanvasImage {
    CanvasImage::new(id, CellRect::new(x, y, cols, rows), image)
}

// ---- set_string ----

#[test]
fn new_canvas_is_blank_spaces() {
    let c = CanvasView::new(3, 2);
    assert_eq!(c.cells.len(), 6);
    assert!(c.cells.iter().all(|x| *x == CanvasCell::default()));
    assert_eq!(CanvasCell::default().symbol, " ");
    assert!(c.cell(3, 0).is_none() && c.cell(0, 2).is_none());
    assert!(c.check().is_ok());
}

#[test]
fn ascii_counts_columns_and_styles() {
    let mut c = CanvasView::new(6, 2);
    assert_eq!(c.set_string(1, 1, "abc", red()), 3);
    assert_eq!(row(&c, 1), " abc");
    assert_eq!(c.cell(2, 1).unwrap().style, red());
    assert_eq!(c.cell(0, 1).unwrap().style, Style::default());
    assert_eq!(c.cell(4, 1).unwrap().style, Style::default());
}

#[test]
fn cjk_takes_two_cells_with_a_continuation() {
    let mut c = CanvasView::new(6, 1);
    assert_eq!(c.set_string(0, 0, "日本a", red()), 5);
    assert_eq!(sym(&c, 0, 0), "日");
    assert_eq!(sym(&c, 1, 0), "");
    assert!(c.cell(1, 0).unwrap().is_continuation());
    assert_eq!(c.cell(1, 0).unwrap().style, red());
    assert_eq!(sym(&c, 2, 0), "本");
    assert_eq!(sym(&c, 4, 0), "a");
    assert_eq!(row(&c, 0), "日本a");
}

#[test]
fn emoji_is_two_wide() {
    let mut c = CanvasView::new(5, 1);
    assert_eq!(c.set_string(0, 0, "😀b", Style::new()), 3);
    assert_eq!(sym(&c, 0, 0), "😀");
    assert_eq!(sym(&c, 1, 0), "");
    assert_eq!(sym(&c, 2, 0), "b");
}

#[test]
fn zwj_sequence_stays_in_one_cell() {
    let mut c = CanvasView::new(8, 1);
    let n = c.set_string(0, 0, "👨\u{200D}👩x", Style::new());
    assert_eq!(n, 3, "the sequence is as wide as its first emoji");
    assert_eq!(sym(&c, 0, 0), "👨\u{200D}👩");
    assert_eq!(sym(&c, 1, 0), "");
    assert_eq!(sym(&c, 2, 0), "x");
}

#[test]
fn combining_mark_joins_the_previous_cell() {
    let mut c = CanvasView::new(5, 1);
    assert_eq!(c.set_string(0, 0, "e\u{301}x", Style::new()), 2);
    assert_eq!(sym(&c, 0, 0), "e\u{301}");
    assert_eq!(sym(&c, 1, 0), "x");
    // After a wide character the mark joins the wide cell, not its continuation.
    let mut c = CanvasView::new(5, 1);
    assert_eq!(c.set_string(0, 0, "日\u{301}", Style::new()), 2);
    assert_eq!(sym(&c, 0, 0), "日\u{301}");
    assert_eq!(sym(&c, 1, 0), "");
}

#[test]
fn leading_combining_mark_is_dropped() {
    let mut c = CanvasView::new(3, 1);
    assert_eq!(c.set_string(0, 0, "\u{301}a", Style::new()), 1);
    assert_eq!(sym(&c, 0, 0), "a");
}

#[test]
fn control_characters_are_skipped() {
    let mut c = CanvasView::new(5, 1);
    assert_eq!(c.set_string(0, 0, "a\nb\tc", Style::new()), 3);
    assert_eq!(row(&c, 0), "abc");
}

#[test]
fn clips_at_the_right_edge() {
    let mut c = CanvasView::new(4, 1);
    assert_eq!(c.set_string(2, 0, "abcdef", Style::new()), 2);
    assert_eq!(row(&c, 0), "  ab");
    assert_eq!(c.set_string(4, 0, "z", Style::new()), 0);
    assert_eq!(c.set_string(9, 0, "z", Style::new()), 0);
    assert_eq!(c.set_string(0, 5, "z", Style::new()), 0);
    assert_eq!(c.cells.len(), 4);
}

#[test]
fn wide_char_is_never_split_at_the_edge() {
    let mut c = CanvasView::new(3, 1);
    assert_eq!(c.set_string(2, 0, "日", Style::new()), 0);
    assert_eq!(sym(&c, 2, 0), " ", "nothing of the wide char is written");
    // The text before it still goes in, and what follows it is not written either.
    let mut c = CanvasView::new(3, 1);
    assert_eq!(c.set_string(1, 0, "a日b", Style::new()), 1);
    assert_eq!(row(&c, 0), " a");
}

#[test]
fn wide_char_exactly_fitting_the_last_two_columns() {
    let mut c = CanvasView::new(3, 1);
    assert_eq!(c.set_string(1, 0, "日", Style::new()), 2);
    assert_eq!(sym(&c, 1, 0), "日");
    assert_eq!(sym(&c, 2, 0), "");
}

#[test]
fn overwriting_the_left_half_blanks_the_continuation() {
    let mut c = CanvasView::new(4, 1);
    c.set_string(0, 0, "日本", red());
    c.set_string(0, 0, "x", Style::new());
    assert_eq!(sym(&c, 0, 0), "x");
    assert_eq!(
        sym(&c, 1, 0),
        " ",
        "the orphaned continuation becomes a space"
    );
    assert!(!c.cell(1, 0).unwrap().is_continuation());
    assert_eq!(sym(&c, 2, 0), "本", "the neighbour is untouched");
    assert_eq!(sym(&c, 3, 0), "");
}

#[test]
fn overwriting_the_right_half_blanks_the_left() {
    let mut c = CanvasView::new(4, 1);
    c.set_string(0, 0, "日本", red());
    c.set_string(1, 0, "x", Style::new());
    assert_eq!(sym(&c, 0, 0), " ", "the orphaned left half becomes a space");
    assert_eq!(sym(&c, 1, 0), "x");
    assert_eq!(sym(&c, 2, 0), "本");
    assert_eq!(row(&c, 0), " x本");
}

#[test]
fn wide_over_the_halves_of_two_wide_chars() {
    let mut c = CanvasView::new(6, 1);
    c.set_string(0, 0, "日本語", Style::new());
    // A wide char at columns 1..3 cuts both neighbours.
    c.set_string(1, 0, "ab", Style::new());
    assert_eq!(sym(&c, 0, 0), " ");
    assert_eq!(sym(&c, 1, 0), "a");
    assert_eq!(sym(&c, 2, 0), "b");
    assert_eq!(sym(&c, 3, 0), " ");
    assert_eq!(sym(&c, 4, 0), "語");
    let mut c = CanvasView::new(6, 1);
    c.set_string(0, 0, "日本語", Style::new());
    assert_eq!(c.set_string(1, 0, "漢", Style::new()), 2);
    assert_eq!(sym(&c, 0, 0), " ");
    assert_eq!(sym(&c, 1, 0), "漢");
    assert_eq!(sym(&c, 2, 0), "");
    assert_eq!(sym(&c, 3, 0), " ");
    assert_eq!(sym(&c, 4, 0), "語");
}

#[test]
fn no_orphan_continuation_survives_any_overwrite() {
    // Every continuation must follow a non-empty cell whose width is 2.
    fn sound(c: &CanvasView) -> bool {
        (0..c.cols).all(|x| {
            let cell = c.cell(x, 0).unwrap();
            if cell.is_continuation() {
                x > 0 && {
                    let l = c.cell(x - 1, 0).unwrap();
                    !l.is_continuation() && unicode_w(&l.symbol) == 2
                }
            } else if unicode_w(&cell.symbol) == 2 {
                c.cell(x + 1, 0).is_some_and(CanvasCell::is_continuation)
            } else {
                true
            }
        })
    }
    fn unicode_w(s: &str) -> usize {
        // The canvas test text only uses ASCII and CJK.
        s.chars()
            .map(|ch| if ch.is_ascii() { 1 } else { 2 })
            .max()
            .unwrap_or(0)
    }
    for start in 0..8u16 {
        for text in ["a", "日", "ab", "日本", "a日", "日a"] {
            let mut c = CanvasView::new(8, 1);
            c.set_string(0, 0, "日本語漢", Style::new());
            c.set_string(start, 0, text, Style::new());
            assert!(sound(&c), "start {start} text {text:?}: {:?}", c.cells);
        }
    }
}

// ---- fill ----

#[test]
fn fill_sets_style_and_blanks_symbols_inside_only() {
    let mut c = CanvasView::new(5, 3);
    c.set_string(0, 1, "abcde", Style::new());
    let bg = Style::new().bg(Color::Blue);
    c.fill(CellRect::new(1, 1, 3, 1), bg);
    assert_eq!(row(&c, 1), "a   e");
    for x in 1..4 {
        assert_eq!(c.cell(x, 1).unwrap().style, bg);
    }
    assert_eq!(c.cell(0, 1).unwrap().style, Style::default());
    assert_eq!(c.cell(4, 1).unwrap().style, Style::default());
    assert_eq!(c.cell(2, 0).unwrap().style, Style::default());
    assert_eq!(c.cell(2, 2).unwrap().style, Style::default());
}

#[test]
fn fill_clips_to_the_canvas() {
    let mut c = CanvasView::new(3, 2);
    let bg = Style::new().bg(Color::Green);
    c.fill(CellRect::new(2, 1, 50, 50), bg);
    assert_eq!(c.cell(2, 1).unwrap().style, bg);
    assert_eq!(c.cell(1, 1).unwrap().style, Style::default());
    c.fill(CellRect::new(9, 9, 2, 2), bg);
    assert_eq!(c.cells.len(), 6);
    c.fill(CellRect::new(0, 0, 0, 5), bg);
    assert_eq!(c.cell(0, 0).unwrap().style, Style::default());
}

#[test]
fn fill_blanks_the_other_half_of_a_cut_wide_char() {
    let mut c = CanvasView::new(6, 1);
    c.set_string(0, 0, "日本語", Style::new());
    c.fill(CellRect::new(1, 0, 4, 1), Style::new().bg(Color::Red));
    // 日's right half and 語's left half are inside; 日 and 語 lose their partner.
    assert_eq!(sym(&c, 0, 0), " ");
    for x in 1..5 {
        assert_eq!(sym(&c, x, 0), " ");
    }
    assert_eq!(sym(&c, 5, 0), " ", "the continuation of 語 is blanked too");
    assert!(c.cells.iter().all(|x| !x.is_continuation()));
}

// ---- check ----

#[test]
fn check_rejects_a_wrong_cell_count() {
    let mut c = CanvasView::new(3, 2);
    c.cells.pop();
    assert_eq!(
        c.check(),
        Err(CanvasIssue::CellCount {
            expected: 6,
            actual: 5
        })
    );
    c.cells
        .extend([CanvasCell::default(), CanvasCell::default()]);
    assert!(matches!(
        c.check(),
        Err(CanvasIssue::CellCount { actual: 7, .. })
    ));
}

#[test]
fn check_images_bounds() {
    let ok = CanvasView::new(4, 3).image(img(1, 2, 1, 2, 2, rgba()));
    assert_eq!(ok.check(), Ok(()));
    let right = CanvasView::new(4, 3).image(img(1, 3, 0, 2, 1, rgba()));
    assert_eq!(
        right.check(),
        Err(CanvasIssue::ImageOutOfBounds { index: 0 })
    );
    let down = CanvasView::new(4, 3)
        .image(img(1, 0, 0, 1, 1, rgba()))
        .image(img(2, 0, 2, 1, 2, rgba()));
    assert_eq!(
        down.check(),
        Err(CanvasIssue::ImageOutOfBounds { index: 1 })
    );
    // Coordinates that would overflow a u16 sum do not wrap into range.
    let wrap = CanvasView::new(4, 3).image(img(1, u16::MAX, 0, 2, 1, rgba()));
    assert_eq!(
        wrap.check(),
        Err(CanvasIssue::ImageOutOfBounds { index: 0 })
    );
}

#[test]
fn check_rejects_zero_size_images() {
    let a = CanvasView::new(4, 3).image(img(1, 0, 0, 0, 1, rgba()));
    assert_eq!(a.check(), Err(CanvasIssue::ImageZeroSize { index: 0 }));
    let b = CanvasView::new(4, 3).image(img(1, 0, 0, 1, 0, rgba()));
    assert_eq!(b.check(), Err(CanvasIssue::ImageZeroSize { index: 0 }));
}

#[derive(Debug)]
struct NoLazy;
impl LazyResource for NoLazy {
    fn natural_size(&self) -> Option<(u32, u32)> {
        None
    }
    fn render(&self, _req: &ResourceReq, _cancel: &Cancel) -> Result<ImageData, Failure> {
        Err(Failure::unsupported("never"))
    }
}

#[test]
fn check_rejects_lazy_images_also_inside_frames() {
    let lazy = ImageData::Lazy(Arc::new(NoLazy));
    let a = CanvasView::new(4, 3).image(img(1, 0, 0, 1, 1, lazy.clone()));
    assert_eq!(a.check(), Err(CanvasIssue::ImageLazy { index: 0 }));
    let frames = ImageData::Frames(vec![
        (rgba(), Duration::from_millis(40)),
        (lazy, Duration::from_millis(40)),
    ]);
    let b = CanvasView::new(4, 3).image(img(1, 0, 0, 1, 1, frames));
    assert_eq!(b.check(), Err(CanvasIssue::ImageLazy { index: 0 }));
    let fine = ImageData::Frames(vec![(rgba(), Duration::from_millis(40))]);
    assert!(CanvasView::new(4, 3)
        .image(img(1, 0, 0, 1, 1, fine))
        .check()
        .is_ok());
}

#[test]
fn issues_have_messages() {
    for i in [
        CanvasIssue::CellCount {
            expected: 1,
            actual: 2,
        },
        CanvasIssue::ImageOutOfBounds { index: 3 },
        CanvasIssue::ImageZeroSize { index: 4 },
        CanvasIssue::ImageLazy { index: 5 },
    ] {
        assert!(!i.to_string().is_empty());
    }
    assert!(CanvasIssue::ImageZeroSize { index: 4 }
        .to_string()
        .contains('4'));
}

#[test]
fn builders() {
    let c = CanvasView::new(2, 2)
        .redraw_after(Duration::from_millis(50))
        .text_input(true);
    assert_eq!(c.redraw_after, Some(Duration::from_millis(50)));
    assert!(c.text_input);
    let d = CanvasView::new(2, 2);
    assert_eq!(d.redraw_after, None);
    assert!(!d.text_input);
}

// ---- ViewKind, View, contexts ----

#[test]
fn view_kind_canvas_and_context() {
    assert_eq!(ViewKind::Canvas.context(), "canvas");
    let v = View::Canvas(CanvasView::new(1, 1));
    assert_eq!(v.kind(), Some(ViewKind::Canvas));
    assert!(v.fits(&[ViewKind::Canvas]));
    assert!(!v.fits(&[ViewKind::Text, ViewKind::Image, ViewKind::Document]));
    assert!(!v.fits(&[]));
    // Other views do not fit a canvas-only plugin (except a message and plain text? no: text needs a text-like kind).
    assert!(!View::text(["x"]).fits(&[ViewKind::Canvas]));
    assert!(View::Message(Default::default()).fits(&[ViewKind::Canvas]));
}

#[test]
fn canvas_context_reserves_little() {
    let r = |c: &str| -> Vec<String> {
        keys::reserved(c)
            .into_iter()
            .map(|(k, _)| format!("{:?}{}", k.code, if k.ctrl { "^" } else { "" }))
            .collect()
    };
    let canvas = keys::reserved("canvas");
    let has = |k: keys::KeySpec| canvas.iter().any(|(x, _)| *x == k);
    // Kept by the host: layer 0, the base keys that are not motion (copy, bookmarks, reload), the
    // verbs, the menus and the representation / page keys.
    for c in "?QFPtT[]1q, eop!RJKOyYrm'".chars() {
        assert!(has(keys::KeySpec::ch(c)), "{c:?} must stay reserved");
    }
    for c in "tnp".chars() {
        assert!(has(keys::KeySpec::ctrl(c)), "Ctrl-{c} must stay reserved");
    }
    assert!(has(keys::KeySpec::key(keys::KeyName::Esc)));
    // Left to the plugin: the type layer (motion, search, selection) and the navigation fixed keys.
    for c in "jkhlgGnNvV/0$".chars() {
        assert!(!has(keys::KeySpec::ch(c)), "{c:?} must be free in a canvas");
    }
    for c in "dufb".chars() {
        assert!(!has(keys::KeySpec::ctrl(c)), "Ctrl-{c} must be free");
    }
    for k in [
        keys::KeyName::Tab,
        keys::KeyName::BackTab,
        keys::KeyName::Up,
        keys::KeyName::Down,
        keys::KeyName::Left,
        keys::KeyName::Right,
        keys::KeyName::Home,
        keys::KeyName::End,
        keys::KeyName::PageUp,
        keys::KeyName::PageDown,
        keys::KeyName::Enter,
    ] {
        assert!(
            !has(keys::KeySpec::key(k)),
            "{k:?} must be free in a canvas"
        );
    }
    assert!(r("canvas").len() < r("text").len());
}

fn manifest_with_key(shows: ViewKind, key: &str) -> Manifest {
    Manifest::new("canvas-test", "1.0")
        .shows(shows)
        .action(Act::new("go").key(key))
}

#[test]
fn a_canvas_plugin_may_use_keys_a_text_plugin_may_not() {
    use konoma_plugin::testing::ManifestIssue;
    let issues_for = |shows, key: &str| keys::validate(&manifest_with_key(shows, key)).len();
    assert_eq!(issues_for(ViewKind::Canvas, "j"), 0);
    assert!(
        issues_for(ViewKind::Canvas, "R") > 0,
        "R moves out of a canvas"
    );
    assert_eq!(issues_for(ViewKind::Canvas, "x"), 0);
    // The base keys every file preview shares are the host's in a canvas too.
    for k in ["y", "Y", "r", "m", "'", "Ctrl-t", "Ctrl-n", "Ctrl-p"] {
        assert!(issues_for(ViewKind::Canvas, k) > 0, "{k} is the host's");
    }
    // The navigation fixed keys are the plugin's.
    for k in [
        "Enter", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown", "Tab",
    ] {
        assert_eq!(issues_for(ViewKind::Canvas, k), 0, "{k} is free");
    }
    assert!(issues_for(ViewKind::Text, "j") > 0);
    // The host keeps `q`, in a canvas too.
    assert!(issues_for(ViewKind::Canvas, "q") > 0);
    assert!(issues_for(ViewKind::Canvas, "?") > 0);
    let _ = ManifestIssue::ShowsNotDeclared;
}

// ---- Key and Session::key ----

#[test]
fn key_is_plain_data() {
    let k = Key::char('a');
    assert_eq!(k.code, KeyCode::Char('a'));
    assert!(k.mods.is_empty());
    let c = Key::ctrl('c');
    assert!(c.mods.contains(KeyMods::CTRL));
    assert!(!c.mods.contains(KeyMods::ALT));
    let m = Key::new(KeyCode::Enter).with_mods(KeyMods::SHIFT | KeyMods::ALT);
    assert!(m.mods.contains(KeyMods::SHIFT) && m.mods.contains(KeyMods::ALT));
    assert!(!m.mods.contains(KeyMods::CTRL));
    assert_eq!(KeyMods::CTRL.bits() & KeyMods::ALT.bits(), 0);
    assert_eq!(KeyMods::ALT.bits() & KeyMods::SHIFT.bits(), 0);
    assert_eq!(Key::paste("a\nb").code, KeyCode::Paste("a\nb".into()));
    assert_eq!(Key::paste("x"), Key::paste("x"));
    assert_ne!(Key::char('a'), Key::ctrl('a'));
    let all = [
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Tab,
        KeyCode::BackTab,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Esc,
        KeyCode::F(5),
    ];
    assert_eq!(all.len(), 14);
}

struct Echo {
    seen: Vec<Key>,
    typing: bool,
}

impl Session for Echo {
    fn view(&mut self, req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        let mut c = CanvasView::new(req.cols, req.rows).text_input(self.typing);
        let line: String = self
            .seen
            .iter()
            .map(|k| match &k.code {
                KeyCode::Char(ch) => ch.to_string(),
                KeyCode::Paste(p) => p.clone(),
                other => format!("<{other:?}>"),
            })
            .collect();
        c.set_string(0, 0, &line, Style::new());
        Ok(View::Canvas(c))
    }
    fn key(&mut self, key: &Key, _cx: &ActCx) -> Result<Outcome, Failure> {
        self.seen.push(key.clone());
        Ok(Outcome::Redraw)
    }
}

/// Does not override `key`.
struct Plain;
impl Session for Plain {
    fn view(&mut self, _req: &ViewReq, _cx: &ViewCx) -> Result<View, Failure> {
        Ok(View::text(["x"]))
    }
}

struct EchoPlugin;
impl Plugin for EchoPlugin {
    fn manifest(&self) -> Manifest {
        Manifest::new("echo", "1.0").shows(ViewKind::Canvas)
    }
    fn open(&self, _input: Input, _cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        Ok(Box::new(Echo {
            seen: Vec::new(),
            typing: true,
        }))
    }
}

#[test]
fn session_key_defaults_to_unhandled() {
    let mut s = Plain;
    let out = s.key(&Key::char('a'), &ActCx::bare()).unwrap();
    assert_eq!(out, Outcome::Unhandled);
    assert_eq!(
        send_keys(&mut s, &[Key::char('a'), Key::paste("zz")])
            .outcomes()
            .unwrap(),
        vec![Outcome::Unhandled, Outcome::Unhandled]
    );
}

#[test]
fn send_keys_reaches_the_session_in_order() {
    let (mut s, _scratch) = open_bytes(&EchoPlugin, "a.echo", b"").unwrap();
    let outs = send_keys(
        s.as_mut(),
        &[Key::char('h'), Key::char('i'), Key::paste(" there")],
    )
    .outcomes()
    .unwrap();
    assert_eq!(outs, vec![Outcome::Redraw; 3]);
    let c = view_canvas(s.as_mut(), &ViewReq::new(20, 2)).unwrap();
    assert_eq!(
        CanvasText::of(&c).rows,
        vec!["hi there".to_string(), String::new()]
    );
    assert!(c.text_input);
    assert!(send_keys(s.as_mut(), &[]).outcomes().unwrap().is_empty());
}

// ---- testing helpers ----

#[test]
fn canvas_text_joins_symbols_and_trims() {
    let mut c = CanvasView::new(6, 3);
    c.set_string(0, 0, "日a ", Style::new());
    c.set_string(2, 2, "z", Style::new());
    let t = CanvasText::of(&c);
    assert_eq!(t.rows, vec!["日a", "", "  z"]);
    assert_eq!(t.text(), "日a\n\n  z");
    assert_eq!(t.to_string(), t.text());
    assert!(CanvasText::of(&CanvasView::new(0, 3)).rows.is_empty());
}

#[test]
fn view_canvas_rejects_other_views_and_bad_canvases() {
    let mut s = Plain;
    let err = view_canvas(&mut s, &ViewReq::default()).unwrap_err();
    assert!(err.to_string().contains("not a canvas view"));

    struct Bad;
    impl Session for Bad {
        fn view(&mut self, _r: &ViewReq, _c: &ViewCx) -> Result<View, Failure> {
            let mut c = CanvasView::new(2, 2);
            c.cells.clear();
            Ok(View::Canvas(c))
        }
    }
    let err = view_canvas(&mut Bad, &ViewReq::default()).unwrap_err();
    assert!(err.to_string().contains("malformed canvas"));
}

#[test]
fn check_manifest_accepts_canvas() {
    assert!(check_manifest(&EchoPlugin).is_empty());
}

// ---- capabilities ----

#[test]
fn term_caps_colors() {
    assert_eq!(TermCaps::default().colors, ColorDepth::Ansi16);
    assert!(!TermCaps::default().pixels);
    assert!(
        ColorDepth::Ansi16 < ColorDepth::Ansi256 && ColorDepth::Ansi256 < ColorDepth::TrueColor
    );
    let cx = ViewCx::bare().with_term(TermCaps::new().pixels(true).colors(ColorDepth::TrueColor));
    assert!(cx.term.pixels);
    assert_eq!(cx.term.colors, ColorDepth::TrueColor);
}
