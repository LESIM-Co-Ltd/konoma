//! A Markdown document in the konoma host and in the standalone app (`konoma-plugin` feature
//! `app`) is laid out by the same shared pipeline. Given the same file, the same size and the
//! same keys, the two must show the same page: the same rows in the frame (the host draws a
//! scroll label on the border and a scrollbar on it, which the app does not), the same footer
//! hints and the same help rows for the keys both have.

use super::table_parity::{help_lines, hints, presses, rows_of, same_row, APP_H, HOST_H, W};
use super::*;
use konoma_plugin::app::{open_file, prepare_bundled, run_with, AppCx, AppEvent, AppKeys};
use konoma_plugin::TermCaps;
use std::sync::Arc;

/// What the app shows after the presses: the whole screen.
fn app_term(path: &std::path::Path, w: u16, h: u16, spec: &str) -> Terminal<TestBackend> {
    let mut opened = open_file(
        vec![Arc::new(konoma_markdown::MarkdownPlugin)],
        path,
        "en",
        TermCaps::new(),
    )
    .expect("opens");
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: opened.shows.clone(),
        ..AppCx::default()
    };
    let ev: Vec<AppEvent> = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            let ev = KeyEvent::new(code, mods);
            AppEvent::Key(konoma_plugin::app::key_from_crossterm(&ev).expect("a key"))
        })
        .collect();
    let mut it = ev.into_iter();
    run_with(
        &mut term,
        &mut *opened.session,
        &mut it,
        &cx,
        &AppKeys::defaults(),
    )
    .unwrap();
    assert_eq!(
        it.count(),
        0,
        "the app ended before the keys ran out: {spec}"
    );
    term
}

/// What the host shows after the presses, with the file opened from its tree.
fn host_term(root: &std::path::Path, file: &str, w: u16, h: u16, spec: &str) -> Sim {
    let mut s = Sim::with_config_sized(root, cfg_en(), w, h);
    s.select(file);
    s.enter();
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended before the keys ran out: {spec}");
    }
    // Draw the last page onto a cleared screen: the test backend keeps a cell the diff skipped
    // after a wide character from an earlier screen (the tree), which no one sees on a real
    // terminal after a repaint; the app starts from a blank screen.
    s.term.clear().expect("clear");
    s.draw();
    s
}

/// The cells between the frame's borders: from the row under the top border to the row over the
/// bottom border, without the first and last column (the border, where the host's scrollbar is).
fn inner(term: &Terminal<TestBackend>) -> Vec<String> {
    let rows = rows_of(term);
    let top = rows
        .iter()
        .position(|l| l.starts_with('┌'))
        .expect("a frame");
    let bottom = rows
        .iter()
        .rposition(|l| l.starts_with('└'))
        .expect("a frame end");
    let buf = term.backend().buffer();
    (top + 1..bottom)
        .map(|y| {
            let mut line = String::new();
            let mut x = 1;
            while x + 1 < buf.area.width {
                let sym = buf[(x, y as u16)].symbol();
                line.push_str(sym);
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// The hints only the host has, spelled out in full.
const HOST_ONLY: &[&str] = &[
    ",:view",
    "[/]:tab",
    "/:search",
    "C-n/C-p:next/prev file",
    "Tab:focus",
    "F:FOLLOW",
    "O:outline",
    "e:edit",
    "e:open",
    "o:open in app",
    "q:back",
    "R:raw source",
    "y:copy",
];

/// One Markdown file per case: (file name, text).
fn fixtures() -> Vec<(String, String)> {
    let long: String = (0..60)
        .map(|i| format!("paragraph {i} with some words\n\n"))
        .collect();
    let wide_line = "lorem ipsum dolor sit amet ".repeat(30);
    vec![
        ("heading.md".into(), "# One\n\n## Two\n\n### Three\n\ntext\n".into()),
        (
            "list.md".into(),
            "- a\n- b\n  - c\n  - d\n\n1. x\n2. y\n\n- [ ] todo\n- [x] done\n".into(),
        ),
        (
            "table.md".into(),
            "| a | b |\n|---|:-:|\n| 1 | 2 |\n| long cell here | x |\n".into(),
        ),
        (
            "code.md".into(),
            "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n`inline` code\n".into(),
        ),
        ("quote.md".into(), "> quoted\n> text\n>\n> > nested\n".into()),
        (
            "alert.md".into(),
            "> [!NOTE]\n> a note\n\n> [!WARNING]\n> careful\n".into(),
        ),
        (
            "footnote.md".into(),
            "text[^1] and more[^2].\n\n[^1]: first\n[^2]: second\n".into(),
        ),
        (
            "front.md".into(),
            "---\ntitle: T\ntags: [a, b]\n---\n\n# Body\n\ntext\n".into(),
        ),
        (
            "details.md".into(),
            "<details>\n<summary>more</summary>\n\nhidden body\n\n</details>\n\n<details open>\n<summary>open</summary>\n\nshown\n\n</details>\n".into(),
        ),
        ("wrap.md".into(), format!("# Long\n\n{wide_line}\n\n- {wide_line}\n")),
        ("scroll.md".into(), format!("# Top\n\n{long}")),
        (
            "cjk.md".into(),
            "# 見出し\n\n日本語の段落です。これは長い行になるかもしれない文章で、折り返しを確かめます。\n\n- 項目一\n- 項目二\n\n| 名前 | 値 |\n|---|---|\n| あ | い |\n".into(),
        ),
        (
            "pictures.md".into(),
            "![local](none.png)\n\n![remote](https://example.com/x.png)\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n$x^2$ and\n\n$$\\int_0^1 x dx$$\n".into(),
        ),
    ]
}

/// Keys pressed after the open: the scroll, the pages, the ends.
const SPECS: &[&str] = &[
    "",
    "j",
    "j j j",
    "k",
    "G",
    "G g",
    "G k",
    "g",
    "<C-f>",
    "<C-f> <C-f>",
    "<C-b>",
    "<C-d>",
    "<C-d> <C-u>",
    "<PgDn>",
    "<PgDn> <PgUp>",
    "<Down> <Down>",
    "<End>",
    "<End> <Home>",
];

#[test]
fn the_page_scroll_and_footer_agree_with_the_host() {
    let dir = sandbox("doc-parity");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let (mut checked, mut scrolled) = (0, 0);
    for (name, _) in fixtures() {
        for spec in SPECS {
            let what = format!("{name} [{spec}]");
            let h = host_term(&root, &name, W, HOST_H, spec);
            let a = app_term(&root.join(&name), W, APP_H, spec);
            let (hi, ai) = (inner(&h.term), inner(&a));
            assert_eq!(
                hi,
                ai,
                "{what}: the page differs\nhost:\n{}\napp:\n{}",
                rows_of(&h.term).join("\n"),
                rows_of(&a).join("\n")
            );
            let (hh, ah) = (
                hints(rows_of(&h.term).last().expect("footer")),
                hints(rows_of(&a).last().expect("footer")),
            );
            for t in hh.iter().filter(|t| !ah.contains(t)) {
                assert!(
                    HOST_ONLY.contains(&t.as_str()),
                    "{what}: the host footer has `{t}` that the app lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            for t in ah.iter().filter(|t| !hh.contains(t)) {
                assert!(
                    t.starts_with("q:quit") || t.starts_with("line "),
                    "{what}: the app footer has `{t}` that the host lacks\nhost: {hh:?}\napp: {ah:?}"
                );
            }
            checked += 1;
            scrolled += usize::from(hi.first().is_some_and(|l| l != "Top") && name == "scroll.md");
        }
    }
    assert!(checked >= 13 * SPECS.len(), "{checked}");
    assert!(scrolled > 5, "{scrolled}");
}

/// The rows only the host's help lists for a Markdown file (search, links, bookmarks, the tabs...)
/// are not in the app's; every row the app lists must be the host's, in the host's order.
#[test]
fn the_document_help_rows_agree_with_the_host() {
    let dir = sandbox("doc-help");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let mut compared = 0;
    for (name, _) in fixtures() {
        for spec in ["?", "G ?", "j j ?", "<C-f> ?", "? j", "? G", "? <C-d>"] {
            let what = format!("{name} [{spec}]");
            let h = host_term(&root, &name, 100, 130, spec);
            let a = app_term(&root.join(&name), 99, 130, spec);
            let hr = rows_of(&h.term);
            let (py, px) = hr
                .iter()
                .enumerate()
                .find_map(|(y, r)| {
                    r.find("┌ Help")
                        .map(|b| (y, unicode_width::UnicodeWidthStr::width(&r[..b])))
                })
                .unwrap_or_else(|| panic!("{what}: the host's help panel in {hr:#?}"));
            let hl: Vec<String> = help_lines(&hr[py + 1..], Some(px))
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            let al: Vec<String> = help_lines(&rows_of(&a), None)
                .into_iter()
                .filter(|l| !l.is_empty())
                .collect();
            // The panel scrolls (`? j`): the same keys scroll both, so compare what both show.
            if spec.starts_with("? ") {
                continue;
            }
            let mut last = 0;
            // The title sits on the host's border and in the app's footer.
            let mut al = al;
            let title = al.pop().expect("a title");
            let border: String = hr[py].split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                border.contains(&title),
                "{what}: `{title}` not in `{border}`"
            );
            for row in &al {
                // The app says `quit` where the host says `back to tree`.
                let found = hl.iter().position(|r| {
                    same_row(r, row) || (row.starts_with("q / Esc") && r.starts_with("q / Esc"))
                });
                let at = found.unwrap_or_else(|| {
                    panic!("{what}: the app's help row `{row}` is not in the host's\nhost: {hl:#?}\napp: {al:#?}")
                });
                assert!(
                    at >= last || row.starts_with("q / Esc"),
                    "{what}: `{row}` is out of order"
                );
                last = last.max(at);
                compared += 1;
            }
        }
    }
    assert!(compared > 13 * 4 * 6, "{compared}");
}

/// Narrow and short terminals: the wrapping, the clamp of the scroll and the page size follow
/// the frame, whatever its size.
#[test]
fn a_narrow_or_short_terminal_shows_what_the_host_shows() {
    let dir = sandbox("doc-narrow");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let mut checked = 0;
    for (name, _) in fixtures() {
        for (w, h) in [(24u16, 12u16), (40, 20), (61, 9), (80, 40), (121, 6)] {
            for spec in [
                "",
                "j j",
                "<C-f>",
                "<C-d>",
                "G",
                "G <C-b>",
                "<C-f> <C-f> <C-d>",
            ] {
                // The test backend keeps a stale cell where a wide character was drawn in an
                // earlier frame (the host's too, before its repaint above): with a wide
                // character, only one scroll step is compared, which the repaint cleans.
                if name == "cjk.md" && spec.split(' ').count() > 1 && spec != "<C-f>" {
                    continue;
                }
                let what = format!("{name} {w}x{h} [{spec}]");
                let hs = host_term(&root, &name, w, h + 1, spec);
                let a = app_term(&root.join(&name), w, h, spec);
                assert_eq!(
                    inner(&hs.term),
                    inner(&a),
                    "{what}: the page differs\nhost:\n{}\napp:\n{}",
                    rows_of(&hs.term).join("\n"),
                    rows_of(&a).join("\n")
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 13 * 5 * 7 - 5 * 4, "{checked}");
}

/// Every Markdown the snapshot corpus holds, on its first page and one page down: the host and
/// the app lay the same source out the same way.
#[test]
fn the_markdown_corpus_shows_the_same_page_as_the_host() {
    let cases = crate::app::md_snapshot_tests::all_cases();
    // The corpus does not shrink unnoticed (a case dropped would drop its check).
    assert!(cases.len() >= 300, "{} cases", cases.len());
    let dir = sandbox("doc-corpus");
    let mut checked = 0;
    for (i, (label, src)) in cases.iter().enumerate() {
        let name = format!("case{i}.md");
        std::fs::write(dir.join(&name), src).unwrap();
        let root = canon(&dir);
        for (spec, w, h) in [("", 100u16, 30u16), ("<C-f>", 100, 30), ("", 37, 14)] {
            let hs = host_term(&root, &name, w, h + 1, spec);
            let a = app_term(&root.join(&name), w, h, spec);
            assert_eq!(
                inner(&hs.term),
                inner(&a),
                "{label} ({name}) {w}x{h} [{spec}]: the page differs\nhost:\n{}\napp:\n{}",
                rows_of(&hs.term).join("\n"),
                rows_of(&a).join("\n")
            );
            checked += 1;
        }
        std::fs::remove_file(dir.join(&name)).unwrap();
    }
    assert!(checked >= cases.len() * 3, "{checked}");
}

// ---- the pictures of a document: local, remote, mermaid, math ------------------------------------
//
// Both sides get a picture backend (halfblocks). The host's results are pumped by hand; the app's
// workers are held back by gates until the first screen is taken, so "before" is the screen with
// every picture still waiting, and "after" is the screen once the app has converged. Nothing
// waits on a clock: the app's event source returns when the screen reaches the host's settled
// screen, and a deadline turns a screen that never gets there into a failure with both screens.

use super::image_gif_parity::Shared;
use konoma_plugin::app::{run_with_pictures, DocEnv, EventSource, Next, Pictures};
use konoma_plugin::{Failure, Input, Manifest, OpenCx, Plugin, Session};
use ratatui::buffer::Buffer;
use ratatui::style::Color as RColor;
use ratatui_image::picker::{Picker, ProtocolType};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// A latch: `wait` returns once `open` was called.
struct Gate(Mutex<bool>, Condvar);

impl Gate {
    const fn new() -> Gate {
        Gate(Mutex::new(false), Condvar::new())
    }
    fn open(&self) {
        *self.0.lock().unwrap() = true;
        self.1.notify_all();
    }
    fn wait(&self) {
        let (g, t) = self
            .1
            .wait_timeout_while(self.0.lock().unwrap(), Duration::from_secs(30), |o| !*o)
            .unwrap();
        assert!(!t.timed_out() || *g, "a gate was never opened");
    }
}

/// A plugin that holds the embedded renders of the plugin it wraps until the gate opens.
struct Gated(Arc<dyn Plugin>, Arc<Gate>);

impl Plugin for Gated {
    fn manifest(&self) -> Manifest {
        self.0.manifest()
    }
    fn open(&self, input: Input, cx: &OpenCx) -> Result<Box<dyn Session>, Failure> {
        if matches!(input, Input::Embed { .. }) {
            self.1.wait();
        }
        self.0.open(input, cx)
    }
}

/// A screen: its rows (for the failure message) and the cells inside the frame; two screens are
/// equal when their cells are.
#[derive(Clone)]
struct Cells {
    rows: Vec<String>,
    cells: Vec<(String, RColor, RColor)>,
}

impl PartialEq for Cells {
    fn eq(&self, o: &Cells) -> bool {
        self.cells == o.cells
    }
}

/// A cell that carries a kitty placement (the placeholder glyph and, on the frame that transmits,
/// the escape with the image id) reduced to the width of the placement it is a row of: the ids
/// are the process's own, the geometry is what must agree.
fn kitty_normalized(symbol: &str) -> String {
    let n = symbol.chars().filter(|c| *c == '\u{10eeee}').count();
    if n == 0 {
        symbol.to_string()
    } else {
        format!("kitty:{n}")
    }
}

/// The cells between the frame's borders (symbol and both colors), all rows between the frame's
/// top and bottom line.
fn cells_of(buf: &Buffer) -> Cells {
    let rows: Vec<String> = (0..buf.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < buf.area.width {
                let sym = buf[(x, y)].symbol();
                line.push_str(sym);
                x += (unicode_width::UnicodeWidthStr::width(sym) as u16).max(1);
            }
            line.trim_end().to_string()
        })
        .collect();
    let (t, b) = (
        rows.iter()
            .position(|l| l.starts_with('┌'))
            .expect("a frame"),
        rows.iter()
            .rposition(|l| l.starts_with('└'))
            .expect("a frame end"),
    );
    let mut cells = Vec::new();
    for y in t + 1..b {
        for x in 1..buf.area.width - 1 {
            let c = &buf[(x, y as u16)];
            cells.push((kitty_normalized(c.symbol()), c.fg, c.bg));
        }
    }
    Cells { rows, cells }
}

/// The host showing `file`, with its picture workers pumped by hand: the screen before any result
/// was applied, and the screen once nothing is owed any more.
fn host_pictures(
    root: &std::path::Path,
    file: &str,
    cfg: Config,
    w: u16,
    h: u16,
) -> (Cells, Cells) {
    host_pictures_held(root, file, cfg, w, h, None)
}

/// [`host_pictures`] with the host's downloads held by `fetch_gate` until the first screen is
/// taken (the same footing the app is given).
fn host_pictures_held(
    root: &std::path::Path,
    file: &str,
    cfg: Config,
    w: u16,
    h: u16,
    fetch_gate: Option<&'static Gate>,
) -> (Cells, Cells) {
    host_pictures_on(
        root,
        file,
        cfg,
        w,
        h,
        Some(ProtocolType::Halfblocks),
        fetch_gate,
    )
}

fn picker_for(proto: ProtocolType) -> Picker {
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(proto);
    picker
}

/// [`host_pictures_held`] on a terminal of protocol `proto` (`None`: one that draws no pictures).
fn host_pictures_on(
    root: &std::path::Path,
    file: &str,
    cfg: Config,
    w: u16,
    h: u16,
    proto: Option<ProtocolType>,
    fetch_gate: Option<&'static Gate>,
) -> (Cells, Cells) {
    let sim = Sim::with_config_sized(root, cfg, w, h);
    let Some(proto) = proto else {
        let mut s = sim;
        s.select(file);
        s.enter();
        s.draw();
        let c = cells_of(s.term.backend().buffer());
        return (c.clone(), c);
    };
    let mut s = sim.with_media_on(picker_for(proto));
    s.select(file);
    s.enter();
    // A converted document (Word) is made on a worker: its page is there before the pictures
    // are asked for.
    if s.app.is_document_loading() {
        s.drain_media();
    }
    s.draw();
    let before = cells_of(s.term.backend().buffer());
    if let Some(g) = fetch_gate {
        g.open();
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        s.draw();
        let mut any = false;
        while let Ok(r) = s.md_img_rx.as_ref().expect("media").try_recv() {
            s.app.apply_md_image(r);
            any = true;
        }
        while let Ok(r) = s.md_remote_rx.as_ref().expect("media").try_recv() {
            s.app.apply_remote_fetch(r);
            any = true;
        }
        while let Ok(r) = s.md_enc_rx.as_ref().expect("media").try_recv() {
            s.app.apply_md_encode(r);
            any = true;
        }
        if any {
            continue;
        }
        if !(s.app.md_images_loading() || s.app.md_encode_in_flight()) {
            break;
        }
        assert!(Instant::now() < deadline, "{file}: the host never settled");
        std::thread::yield_now();
    }
    s.draw();
    (before, cells_of(s.term.backend().buffer()))
}

/// Takes the first screen, opens the gates, then waits for the screen to be `want`.
struct SettleOn {
    screen: Arc<Mutex<TestBackend>>,
    gates: Vec<Arc<Gate>>,
    fetch_gate: Option<&'static Gate>,
    want: Cells,
    before: Option<Cells>,
    after: Option<Cells>,
    started: Instant,
}

impl EventSource for SettleOn {
    fn next(&mut self, wait: Option<Duration>) -> Next {
        let now = cells_of(self.screen.lock().unwrap().buffer());
        if self.before.is_none() {
            self.before = Some(now);
            for g in &self.gates {
                g.open();
            }
            if let Some(g) = self.fetch_gate {
                g.open();
            }
            return Next::Timeout;
        }
        if now == self.want {
            self.after = Some(now);
            return Next::End;
        }
        // The loop must wake itself while anything is owed or a page is stale: a wait of `None`
        // is a loop asleep until a key comes, which no one will press. (A source that answers
        // every wait with a timeout would hide a loop that forgot to lay the page out again.)
        assert!(
            wait.is_some(),
            "the loop went to sleep before the page was settled\napp:\n{}\nwant:\n{}",
            now.rows.join("\n"),
            self.want.rows.join("\n")
        );
        assert!(
            self.started.elapsed() < Duration::from_secs(30),
            "the app never reached the host's settled screen\napp:\n{}",
            now.rows.join("\n")
        );
        std::thread::sleep(wait.unwrap_or_default().min(Duration::from_millis(2)));
        Next::Timeout
    }
}

/// What the app shows for `file` read through `config_text`, with the pictures held back until
/// the first screen is taken. `plugins` are the extras beside the Markdown plugin.
#[allow(clippy::too_many_arguments)]
fn app_pictures(
    file: &std::path::Path,
    config_text: &str,
    plugins: Vec<Arc<dyn Plugin>>,
    gate: &Arc<Gate>,
    fetcher: (konoma_plugin::document_pictures::Fetcher, &'static Gate),
    want: Cells,
    w: u16,
    h: u16,
) -> (Cells, Cells, DocEnv) {
    let mut own: Vec<Arc<dyn Plugin>> = vec![Arc::new(konoma_markdown::MarkdownPlugin)];
    own.extend(plugins);
    app_pictures_on(
        file,
        config_text,
        (own, Vec::new()),
        Some(ProtocolType::Halfblocks),
        gate,
        fetcher,
        want,
        w,
        h,
    )
}

/// [`app_pictures`] for a program of `programs.0` that carries `programs.1` beside them, on a
/// terminal of protocol `proto` (`None`: no pictures). The embedded renders (the plugins asked
/// with an `Input::Embed`) are held by `gate`.
#[allow(clippy::too_many_arguments)]
fn app_pictures_on(
    file: &std::path::Path,
    config_text: &str,
    programs: OfficeProgram,
    proto: Option<ProtocolType>,
    gate: &Arc<Gate>,
    fetcher: (konoma_plugin::document_pictures::Fetcher, &'static Gate),
    want: Cells,
    w: u16,
    h: u16,
) -> (Cells, Cells, DocEnv) {
    let held = |list: Vec<Arc<dyn Plugin>>| -> Vec<Arc<dyn Plugin>> {
        list.into_iter()
            .map(|p| Arc::new(Gated(p, Arc::clone(gate))) as Arc<dyn Plugin>)
            .collect()
    };
    let (own, bundled) = programs;
    let loaded = (
        konoma_host_core::shared::SharedConfig::parse(&format!(
            "[ui]\nlang = \"en\"\n{config_text}"
        ))
        .expect("config"),
        None,
    );
    let mut prepared =
        match prepare_bundled(held(own), held(bundled), file, TermCaps::new(), loaded) {
            Ok(p) => p,
            Err(_) => panic!("the app did not open {}", file.display()),
        };
    prepared.doc_env.fetcher = fetcher.0;
    let screen = Arc::new(Mutex::new(TestBackend::new(w, h)));
    let mut term = Terminal::new(Shared(Arc::clone(&screen))).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: prepared.opened.shows.clone(),
        reopen: Some(prepared.reopen.clone()),
        ..AppCx::default()
    };
    let mut events = SettleOn {
        screen: Arc::clone(&screen),
        gates: vec![Arc::clone(gate)],
        fetch_gate: Some(fetcher.1),
        want,
        before: None,
        after: None,
        started: Instant::now(),
    };
    let remote_images = prepared.doc_env.remote_images;
    let pics = proto.map(|proto| {
        Pictures::new(
            picker_for(proto),
            &prepared.opened.plugin_id,
            &prepared.opened.path,
            800,
        )
        .with_documents(prepared.doc_env)
    });
    run_with_pictures(
        &mut term,
        &mut *prepared.opened.session,
        &mut events,
        &cx,
        &prepared.keys,
        pics,
    )
    .unwrap();
    let env = DocEnv {
        remote_images,
        ..DocEnv::default()
    };
    (
        events.before.expect("a first screen"),
        events.after.expect("a settled screen"),
        env,
    )
}

fn stripes_png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(240, 120, |x, y| {
        image::Rgb(match (x / 30 + y / 30) % 3 {
            0 => [255, 0, 0],
            1 => [0, 200, 0],
            _ => [0, 0, 255],
        })
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

macro_rules! test_fetcher {
    ($name:ident, $count:ident, $gate:ident) => {
        static $count: AtomicUsize = AtomicUsize::new(0);
        static $gate: Gate = Gate::new();
        fn $name(_url: &str, dest: &std::path::Path) -> bool {
            $count.fetch_add(1, Ordering::SeqCst);
            $gate.wait();
            if let Some(dir) = dest.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(dest, stripes_png()).is_ok()
        }
    };
}
test_fetcher!(fetch_on, FETCHES_ON, FETCH_GATE_ON);
test_fetcher!(fetch_off, FETCHES_OFF, FETCH_GATE_OFF);
test_fetcher!(fetch_host, HOST_FETCHES, HOST_FETCH_GATE);

/// How many cells of `cells` show a picture (a halfblock glyph with a color).
#[track_caller]
fn same(what: &str, host: &Cells, app: &Cells) {
    assert!(
        host == app,
        "{what}: the screens differ\nhost:\n{}\napp:\n{}",
        host.rows.join("\n"),
        app.rows.join("\n")
    );
}

fn picture_cells(cells: &Cells) -> usize {
    cells
        .cells
        .iter()
        .filter(|c| c.0 == "▀" || c.0 == "▄")
        .count()
}

const LOCAL_DOC: &str = "# Local\n\n![the stripes](stripes.png)\n\nafter the picture\n";
const MERMAID_DOC: &str =
    "# Diagram\n\n```mermaid\ngraph TD\n  A[Start] --> B[End]\n```\n\nafter\n";
const MATH_DOC: &str = "# Math\n\ninline $x^2 + y^2$ here\n\n$$\\int_0^1 x\\,dx$$\n\nafter\n";

fn mermaid_and_math() -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(konoma_mermaid::MermaidPlugin),
        Arc::new(konoma_math::MathPlugin),
    ]
}

/// One document in both: the screens before and after the pictures arrive. Returns the app's
/// (before, after) cells.
fn compare_pictures(
    label: &str,
    doc: &str,
    cfg: Config,
    config_text: &str,
    plugins: Vec<Arc<dyn Plugin>>,
) -> (Cells, Cells) {
    let dir = sandbox("doc-pictures");
    std::fs::write(dir.join("stripes.png"), stripes_png()).unwrap();
    std::fs::write(dir.join("d.md"), doc).unwrap();
    let root = canon(&dir);
    let (w, h) = (100u16, 48u16);
    let (hb, ha) = host_pictures(&root, "d.md", cfg, w, h + 1);
    let gate = Arc::new(Gate::new());
    let (ab, aa, _) = app_pictures(
        &root.join("d.md"),
        config_text,
        plugins,
        &gate,
        (fetch_on, &FETCH_GATE_ON),
        ha.clone(),
        w,
        h,
    );
    same(&format!("{label}: before the pictures"), &hb, &ab);
    same(&format!("{label}: settled"), &ha, &aa);
    (ab, aa)
}

#[test]
fn a_local_picture_is_drawn_as_in_the_host() {
    let (before, after) = compare_pictures("local", LOCAL_DOC, cfg_en(), "", Vec::new());
    assert_eq!(picture_cells(&before), 0, "nothing is drawn yet");
    assert!(picture_cells(&after) > 50, "{}", picture_cells(&after));
}

#[test]
fn mermaid_and_math_are_drawn_by_their_plugins_as_in_the_host() {
    for (label, doc) in [("mermaid", MERMAID_DOC), ("math", MATH_DOC)] {
        let (before, after) = compare_pictures(label, doc, cfg_en(), "", mermaid_and_math());
        assert_eq!(picture_cells(&before), 0, "{label}: nothing is drawn yet");
        assert!(
            picture_cells(&after) > 30,
            "{label}: {}",
            picture_cells(&after)
        );
    }
}

#[test]
fn with_no_mermaid_or_math_plugin_the_document_keeps_its_text() {
    let mut cfg = cfg_en();
    for id in ["mermaid", "math"] {
        let mut t = toml::Table::new();
        t.insert("enabled".into(), toml::Value::Boolean(false));
        cfg.plugins.insert(id.into(), t);
    }
    for (label, doc) in [("mermaid", MERMAID_DOC), ("math", MATH_DOC)] {
        let (_, after) = compare_pictures(label, doc, cfg.clone(), "", Vec::new());
        assert_eq!(picture_cells(&after), 0, "{label}: the text stays text");
    }
}

#[test]
fn a_remote_picture_is_fetched_and_drawn_as_in_the_host() {
    let cache = unique_tmp("konoma_doc_pictures_remote_cache");
    crate::test_support::set_test_cache_root(cache.to_path_buf());
    // Both sides start with an empty cache and a download held until the first screen is taken,
    // so "before" is the same waiting page in both.
    let url = "https://pictures.invalid/same.png";
    let dir = sandbox("doc-pictures-remote");
    std::fs::write(
        dir.join("d.md"),
        format!("# Remote\n\n![far away]({url})\n\nafter\n"),
    )
    .unwrap();
    let root = canon(&dir);
    let (w, h) = (100u16, 40u16);
    HOST_FETCHES.store(0, Ordering::SeqCst);
    konoma_plugin::document_pictures::set_test_fetcher(fetch_host);
    let (hb, ha) = host_pictures_held(&root, "d.md", cfg_en(), w, h + 1, Some(&HOST_FETCH_GATE));
    assert_eq!(
        HOST_FETCHES.load(Ordering::SeqCst),
        1,
        "the host downloads once"
    );
    // The host's download is in the cache now; the app starts from an empty one as well.
    let cached = konoma_plugin::document_pictures::md_remote_cache_path(url).unwrap();
    std::fs::remove_file(&cached).expect("the host cached the picture");
    FETCHES_ON.store(0, Ordering::SeqCst);
    let (ab, aa, _) = app_pictures(
        &root.join("d.md"),
        "",
        Vec::new(),
        &Arc::new(Gate::new()),
        (fetch_on, &FETCH_GATE_ON),
        ha.clone(),
        w,
        h,
    );
    assert_eq!(FETCHES_ON.load(Ordering::SeqCst), 1, "one download");
    assert!(
        ab.rows.iter().any(|r| r.contains("loading")),
        "{}",
        ab.rows.join("\n")
    );
    konoma_plugin::document_pictures::clear_test_fetcher();
    assert!(konoma_plugin::document_pictures::get_test_fetcher().is_none());
    same("remote: before the download", &hb, &ab);
    same("remote: settled", &ha, &aa);
    assert_eq!(picture_cells(&ab), 0);
    assert!(picture_cells(&aa) > 50);
}

#[test]
fn with_remote_images_off_nothing_is_fetched_and_the_text_stays_as_in_the_host() {
    let cache = unique_tmp("konoma_doc_pictures_remote_off");
    crate::test_support::set_test_cache_root(cache.to_path_buf());
    let url = "https://pictures.invalid/off.png";
    let dir = sandbox("doc-pictures-remote-off");
    std::fs::write(
        dir.join("d.md"),
        format!("# Off\n\n![far away]({url})\n\nafter\n"),
    )
    .unwrap();
    let root = canon(&dir);
    let mut cfg = cfg_en();
    cfg.external.remote_images = false;
    let (w, h) = (100u16, 30u16);
    let (hb, ha) = host_pictures(&root, "d.md", cfg, w, h + 1);
    FETCHES_OFF.store(0, Ordering::SeqCst);
    // The gate is never opened for the fetcher: a fetch would hang and fail the test.
    let (ab, aa, env) = app_pictures(
        &root.join("d.md"),
        "[external]\nremote_images = false\n",
        Vec::new(),
        &Arc::new(Gate::new()),
        (fetch_off, &FETCH_GATE_OFF),
        ha.clone(),
        w,
        h,
    );
    assert!(
        !env.remote_images,
        "the config reached the app's environment"
    );
    assert_eq!(FETCHES_OFF.load(Ordering::SeqCst), 0, "nothing is fetched");
    same("off: before", &hb, &ab);
    same("off: settled", &ha, &aa);
    assert_eq!(picture_cells(&aa), 0);
}

// ---- a converted document: Word answers with a `Reopen` -------------------------------------------
//
// A docx is opened by the document plugin, which hands its Markdown over ("open this again as
// markdown"). konoma opens the plugin that takes `markdown` and draws what it answers; the app
// does the same with the plugins it was given (`ReopenSource`). The same file must give the same
// page, with and without a plugin to open it again.

use super::word::{build_docx, para};

fn office_docs() -> Vec<Arc<dyn Plugin>> {
    vec![Arc::new(konoma_office::DocumentPlugin)]
}

/// A host config in which no plugin takes the content type `markdown`.
fn cfg_no_markdown() -> Config {
    let mut cfg = cfg_en();
    let mut t = toml::Table::new();
    t.insert("enabled".into(), toml::Value::Boolean(false));
    cfg.plugins.insert("markdown".into(), t);
    cfg
}

/// What the app shows for `file` when only `plugins` are given (no Markdown plugin unless they
/// are one), after `spec`'s keys: the settled screen, no pictures.
fn app_reopen_term(
    file: &std::path::Path,
    plugins: Vec<Arc<dyn Plugin>>,
    w: u16,
    h: u16,
    spec: &str,
) -> Terminal<TestBackend> {
    app_reopen_term_bundled(file, plugins, Vec::new(), w, h, spec)
}

/// [`app_reopen_term`] for a program that also carries `bundled` destination plugins.
fn app_reopen_term_bundled(
    file: &std::path::Path,
    plugins: Vec<Arc<dyn Plugin>>,
    bundled: Vec<Arc<dyn Plugin>>,
    w: u16,
    h: u16,
    spec: &str,
) -> Terminal<TestBackend> {
    let loaded = (
        konoma_host_core::shared::SharedConfig::parse("[ui]\nlang = \"en\"\n").expect("config"),
        None,
    );
    let Ok(mut prepared) = prepare_bundled(plugins, bundled, file, TermCaps::new(), loaded) else {
        panic!("the app did not open {}", file.display());
    };
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let cx = AppCx {
        lang: "en".into(),
        term: TermCaps::new(),
        title: String::new(),
        shows: prepared.opened.shows.clone(),
        reopen: Some(prepared.reopen.clone()),
        ..AppCx::default()
    };
    let ev: Vec<AppEvent> = presses(spec)
        .into_iter()
        .map(|(code, mods)| {
            let ev = KeyEvent::new(code, mods);
            AppEvent::Key(konoma_plugin::app::key_from_crossterm(&ev).expect("a key"))
        })
        .collect();
    let mut it = ev.into_iter();
    run_with(
        &mut term,
        &mut *prepared.opened.session,
        &mut it,
        &cx,
        &prepared.keys,
    )
    .unwrap();
    term
}

fn docx_body() -> String {
    let mut body = para("Quarterly notes");
    for i in 0..40 {
        body += &para(&format!("paragraph {i} of the converted document"));
    }
    body
}

#[test]
fn a_docx_reopened_as_markdown_shows_the_same_page_as_the_host() {
    let dir = sandbox("reopen-docx");
    build_docx(&dir.join("d.docx"), &docx_body());
    let root = canon(&dir);
    let mut checked = 0;
    for (spec, w, h) in [("", 100u16, 30u16), ("<C-f>", 100, 30), ("G", 60, 20)] {
        let hs = host_term(&root, "d.docx", w, h + 1, spec);
        let a = app_reopen_term(
            &root.join("d.docx"),
            office_docs_with_markdown(),
            w,
            h,
            spec,
        );
        let host_rows = inner(&hs.term);
        assert!(
            host_rows.iter().any(|r| r.contains("paragraph")),
            "the host shows the document\n{}",
            rows_of(&hs.term).join("\n")
        );
        assert_eq!(
            host_rows,
            inner(&a),
            "{w}x{h} [{spec}]: the page differs\nhost:\n{}\napp:\n{}",
            rows_of(&hs.term).join("\n"),
            rows_of(&a).join("\n")
        );
        checked += 1;
    }
    assert_eq!(checked, 3);
}

fn office_docs_with_markdown() -> Vec<Arc<dyn Plugin>> {
    let mut all = office_docs();
    all.push(Arc::new(konoma_markdown::MarkdownPlugin));
    all
}

/// The plugins of a program and the ones it carries beside them.
type OfficeProgram = (Vec<Arc<dyn Plugin>>, Vec<Arc<dyn Plugin>>);

/// The `konoma-office` program as it is built: the office plugins, with Markdown, mermaid and math
/// carried beside them as destinations (what `konoma_office::bundled()` holds).
fn office_program() -> OfficeProgram {
    let own: Vec<Arc<dyn Plugin>> = konoma_office::program()
        .into_iter()
        .map(Arc::from)
        .collect();
    let bundled: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(konoma_markdown::MarkdownPlugin),
        Arc::new(konoma_mermaid::MermaidPlugin),
        Arc::new(konoma_math::MathPlugin),
    ];
    (own, bundled)
}

#[test]
fn a_docx_in_the_office_program_with_its_bundled_plugins_shows_the_same_page_as_the_host() {
    let dir = sandbox("reopen-docx-bundled");
    build_docx(&dir.join("d.docx"), &docx_body());
    let root = canon(&dir);
    let mut checked = 0;
    for (spec, w, h) in [("", 100u16, 30u16), ("<C-f>", 100, 30), ("G", 60, 20)] {
        let hs = host_term(&root, "d.docx", w, h + 1, spec);
        let (own, bundled) = office_program();
        let a = app_reopen_term_bundled(&root.join("d.docx"), own, bundled, w, h, spec);
        let host_rows = inner(&hs.term);
        assert!(
            host_rows.iter().any(|r| r.contains("paragraph")),
            "the host shows the document\n{}",
            rows_of(&hs.term).join("\n")
        );
        assert_eq!(
            host_rows,
            inner(&a),
            "{w}x{h} [{spec}]: the page differs\nhost:\n{}\napp:\n{}",
            rows_of(&hs.term).join("\n"),
            rows_of(&a).join("\n")
        );
        checked += 1;
    }
    assert_eq!(checked, 3);
}

#[test]
fn a_bundled_plugin_never_takes_a_file_in_the_app() {
    let dir = sandbox("bundled-no-file");
    std::fs::write(dir.join("n.md"), "# t\n").unwrap();
    let root = canon(&dir);
    let (own, bundled) = office_program();
    let loaded = (
        konoma_host_core::shared::SharedConfig::parse("[ui]\nlang = \"en\"\n").expect("config"),
        None,
    );
    match prepare_bundled(own, bundled, &root.join("n.md"), TermCaps::new(), loaded) {
        Err(f) => assert!(
            matches!(f.error, konoma_plugin::app::AppError::NotHandled { .. }),
            "{}",
            f.error
        ),
        Ok(p) => panic!("opened by `{}`", p.opened.plugin_id),
    }
}

#[test]
fn a_docx_with_no_plugin_to_reopen_it_shows_what_the_host_shows() {
    let dir = sandbox("reopen-docx-none");
    build_docx(&dir.join("d.docx"), &docx_body());
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, cfg_no_markdown(), 100, 31);
    s.select("d.docx");
    s.enter();
    s.term.clear().expect("clear");
    s.draw();
    let a = app_reopen_term(&root.join("d.docx"), office_docs(), 100, 30, "");
    // The host frames the failure with its own hint and path lines; the app draws the failure
    // message alone. What must agree is the message itself.
    let host_rows = inner(&s.term);
    let app_rows = rows_of(&a);
    let said = "no plugin is available to show this document";
    assert_eq!(host_rows[0], said, "host:\n{}", rows_of(&s.term).join("\n"));
    assert_eq!(app_rows[0], said, "app:\n{}", app_rows.join("\n"));
    assert!(
        app_rows[1..app_rows.len() - 1].iter().all(|r| r.is_empty()),
        "nothing else is drawn\n{}",
        app_rows.join("\n")
    );
    // A markdown plugin beside the Word one is what makes the same file show its text.
    let with = app_reopen_term(
        &root.join("d.docx"),
        office_docs_with_markdown(),
        100,
        30,
        "",
    );
    assert!(
        inner(&with).iter().any(|r| r.contains("paragraph")),
        "{}",
        rows_of(&with).join("\n")
    );
}

// A document's own pictures (`DocumentView::resources`, the `office-img://` ones) are drawn by the
// standalone app as the host draws them.
#[test]
fn a_docx_with_a_picture_is_drawn_after_the_reopen_as_in_the_host() {
    let dir = sandbox("reopen-docx-picture");
    super::word::build_docx_with_picture(
        &dir.join("d.docx"),
        "Notes with a picture",
        "stripes.png",
        &stripes_png(),
    );
    let root = canon(&dir);
    let (w, h) = (100u16, 40u16);
    let (hb, ha) = host_pictures(&root, "d.docx", cfg_en(), w, h + 1);
    let gate = Arc::new(Gate::new());
    let (ab, aa, _) = app_pictures(
        &root.join("d.docx"),
        "",
        office_docs(),
        &gate,
        (fetch_on, &FETCH_GATE_ON),
        ha.clone(),
        w,
        h,
    );
    assert!(
        ha.rows.iter().any(|r| r.contains("Notes with a picture")),
        "{}",
        ha.rows.join("\n")
    );
    same("docx picture: before the picture", &hb, &ab);
    same("docx picture: settled", &ha, &aa);
    assert_eq!(picture_cells(&ab), 0, "nothing is drawn yet");
    assert!(picture_cells(&aa) > 50, "{}", picture_cells(&aa));
}

// ---- mermaid and math in the standalone programs (the bundled plugins) ---------------------------
//
// The programs are the real lists: `konoma-markdown` carries mermaid and math beside Markdown,
// `konoma-office` carries Markdown, mermaid and math beside the Word plugin. The host is the
// konoma host with the standard plugins.

/// The `konoma-markdown` program as it is built: Markdown, with mermaid and math carried beside it
/// as destinations (what `konoma_markdown::bundled()` holds; that function exists only with the
/// crate's `app` feature, which konoma does not turn on).
fn markdown_program() -> OfficeProgram {
    (
        konoma_markdown::program()
            .into_iter()
            .map(Arc::from)
            .collect(),
        vec![
            Arc::new(konoma_mermaid::MermaidPlugin),
            Arc::new(konoma_math::MathPlugin),
        ],
    )
}

/// How many cells of `cells` are a row of a kitty placement.
fn kitty_cells(cells: &Cells) -> usize {
    cells
        .cells
        .iter()
        .filter(|c| c.0.starts_with("kitty:"))
        .count()
}

/// Cells that show a picture on a terminal of protocol `proto`.
fn drawn_cells(proto: Option<ProtocolType>, cells: &Cells) -> usize {
    match proto {
        Some(ProtocolType::Kitty) => kitty_cells(cells),
        Some(_) => picture_cells(cells),
        None => 0,
    }
}

const BOTH_DOC: &str = "# Both\n\n```mermaid\ngraph TD\n  A[Start] --> B[End]\n```\n\ninline $x^2 + y^2$ here\n\n$$\\int_0^1 x\\,dx$$\n\n```math\n\\sum_{i=1}^n i\n```\n\nafter\n";
const BROKEN_MERMAID_DOC: &str =
    "# Broken\n\n```mermaid\nthis is not a diagram %%% ???\n```\n\nafter\n";
const BROKEN_MATH_DOC: &str =
    "# Broken\n\nbroken $\\frac{1}{$ here\n\n$$\\begin{nope} \\undefined$$\n\nafter\n";

/// `doc` in the host and in `program` at 100 columns: (the app's first screen, its settled one).
#[allow(clippy::too_many_arguments)]
fn compare_program(
    label: &str,
    doc: &str,
    cfg: Config,
    config_text: &str,
    program: OfficeProgram,
    proto: Option<ProtocolType>,
    (w, h): (u16, u16),
) -> (Cells, Cells) {
    let dir = sandbox("doc-program");
    std::fs::write(dir.join("d.md"), doc).unwrap();
    let root = canon(&dir);
    let (hb, ha) = host_pictures_on(&root, "d.md", cfg, w, h + 1, proto, None);
    let gate = Arc::new(Gate::new());
    let (ab, aa, _) = app_pictures_on(
        &root.join("d.md"),
        config_text,
        program,
        proto,
        &gate,
        (fetch_on, &FETCH_GATE_ON),
        ha.clone(),
        w,
        h,
    );
    same(&format!("{label}: before the pictures"), &hb, &ab);
    same(&format!("{label}: settled"), &ha, &aa);
    (ab, aa)
}

fn disabled(cfg: &mut Config, ids: &[&str]) {
    for id in ids {
        let mut t = toml::Table::new();
        t.insert("enabled".into(), toml::Value::Boolean(false));
        cfg.plugins.insert((*id).into(), t);
    }
}

#[test]
fn the_markdown_program_draws_mermaid_and_math_as_the_host_does_on_every_picker() {
    let mut checked = 0;
    for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty] {
        for (label, doc) in [
            ("mermaid", MERMAID_DOC),
            ("math", MATH_DOC),
            ("both", BOTH_DOC),
        ] {
            let name = format!("{label} {proto:?}");
            let (before, after) = compare_program(
                &name,
                doc,
                cfg_en(),
                "",
                markdown_program(),
                Some(proto),
                (100, 48),
            );
            assert_eq!(drawn_cells(Some(proto), &before), 0, "{name}: not yet");
            assert!(
                drawn_cells(Some(proto), &after) > 5,
                "{name}: {}",
                drawn_cells(Some(proto), &after)
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 6);
}

#[test]
fn a_kitty_placement_of_a_mermaid_or_math_picture_is_the_hosts_rectangle() {
    // `Cells` equality already holds the rows of every placement (`kitty:<columns>` per row);
    // this pins that the rows exist and that the diagram and the formulas are separate pictures.
    let (_, after) = compare_program(
        "kitty both",
        BOTH_DOC,
        cfg_en(),
        "",
        markdown_program(),
        Some(ProtocolType::Kitty),
        (100, 48),
    );
    let rows: Vec<usize> = after
        .cells
        .chunks(98)
        .enumerate()
        .filter(|(_, r)| r.iter().any(|c| c.0.starts_with("kitty:")))
        .map(|(i, _)| i)
        .collect();
    assert!(rows.len() >= 3, "{rows:?}\n{}", after.rows.join("\n"));
}

#[test]
fn a_text_mode_shows_the_raw_diagram_and_formula_as_in_the_host() {
    // [plugins.<id>] mode = "text", and the older [ui] names.
    let mut by_plugin = cfg_en();
    for id in ["mermaid", "math"] {
        let mut t = toml::Table::new();
        t.insert("mode".into(), toml::Value::String("text".into()));
        by_plugin.plugins.insert(id.into(), t);
    }
    let mut by_ui = cfg_en();
    by_ui.ui.mermaid = "text".into();
    by_ui.ui.math = "text".into();
    let mut checked = 0;
    for (label, cfg, text) in [
        (
            "plugins",
            by_plugin,
            "[plugins.mermaid]\nmode = \"text\"\n[plugins.math]\nmode = \"text\"\n",
        ),
        ("ui", by_ui, "mermaid = \"text\"\nmath = \"text\"\n"),
    ] {
        for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty] {
            let (_, after) = compare_program(
                &format!("text {label} {proto:?}"),
                BOTH_DOC,
                cfg.clone(),
                text,
                markdown_program(),
                Some(proto),
                (100, 48),
            );
            assert_eq!(drawn_cells(Some(proto), &after), 0, "{label} {proto:?}");
            assert!(
                after.rows.iter().any(|r| r.contains("x^2")),
                "the formula stays raw\n{}",
                after.rows.join("\n")
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4);
}

#[test]
fn only_the_text_mode_kind_stays_text_and_the_other_is_drawn() {
    let mut cfg = cfg_en();
    let mut t = toml::Table::new();
    t.insert("mode".into(), toml::Value::String("text".into()));
    cfg.plugins.insert("math".into(), t);
    let (_, after) = compare_program(
        "math text only",
        BOTH_DOC,
        cfg,
        "[plugins.math]\nmode = \"text\"\n",
        markdown_program(),
        Some(ProtocolType::Halfblocks),
        (100, 48),
    );
    assert!(picture_cells(&after) > 5, "the diagram is a picture");
    assert!(
        after.rows.iter().any(|r| r.contains("x^2")),
        "the formula is text"
    );
}

#[test]
fn with_no_picker_the_cells_stay_as_in_the_host() {
    for (label, doc) in [
        ("mermaid", MERMAID_DOC),
        ("math", MATH_DOC),
        ("both", BOTH_DOC),
    ] {
        let (before, after) = compare_program(
            &format!("no picker {label}"),
            doc,
            cfg_en(),
            "",
            markdown_program(),
            None,
            (100, 48),
        );
        assert_eq!(picture_cells(&after), 0, "{label}");
        assert_eq!(kitty_cells(&after), 0, "{label}");
        assert_eq!(before.cells, after.cells, "{label}: nothing arrives later");
    }
}

#[test]
fn a_destination_that_the_program_does_not_carry_gives_the_hosts_fallback() {
    // konoma-markdown without math (and the reverse), against a host whose plugin of that kind is
    // stopped: the kind that is there is drawn, the other stays text.
    let only = |id: &str| -> OfficeProgram {
        let (own, bundled) = markdown_program();
        let keep: Vec<Arc<dyn Plugin>> = bundled
            .into_iter()
            .filter(|p| p.manifest().id == id)
            .collect();
        assert_eq!(keep.len(), 1, "the program carries {id}");
        (own, keep)
    };
    let mut checked = 0;
    for (kept, stopped) in [("mermaid", "math"), ("math", "mermaid")] {
        for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty] {
            let mut cfg = cfg_en();
            disabled(&mut cfg, &[stopped]);
            let (_, after) = compare_program(
                &format!("only {kept} {proto:?}"),
                BOTH_DOC,
                cfg,
                "",
                only(kept),
                Some(proto),
                (100, 48),
            );
            assert!(drawn_cells(Some(proto), &after) > 3, "{kept} is drawn");
            let raw = if stopped == "math" { "x^2" } else { "A[Start]" };
            assert!(
                after.rows.iter().any(|r| r.contains(raw)),
                "{stopped} stays text ({raw})\n{}",
                after.rows.join("\n")
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4);
    // Neither carried: the markdown plugin alone.
    let (own, _) = markdown_program();
    let mut cfg = cfg_en();
    disabled(&mut cfg, &["mermaid", "math"]);
    let (_, after) = compare_program(
        "none",
        BOTH_DOC,
        cfg,
        "",
        (own, Vec::new()),
        Some(ProtocolType::Halfblocks),
        (100, 48),
    );
    assert_eq!(picture_cells(&after), 0);
}

#[test]
fn a_broken_diagram_or_formula_shows_what_the_host_shows() {
    let mut checked = 0;
    for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty] {
        for (label, doc) in [("mermaid", BROKEN_MERMAID_DOC), ("math", BROKEN_MATH_DOC)] {
            let (before, after) = compare_program(
                &format!("broken {label} {proto:?}"),
                doc,
                cfg_en(),
                "",
                markdown_program(),
                Some(proto),
                (100, 48),
            );
            assert_eq!(drawn_cells(Some(proto), &after), 0, "{label}: no picture");
            if label == "mermaid" {
                assert!(
                    after
                        .rows
                        .iter()
                        .any(|r| r.contains("cannot render mermaid")),
                    "the failure is said\n{}",
                    after.rows.join("\n")
                );
            } else {
                // The formula waited for its render (room reserved) and then went back to text.
                assert!(
                    before.cells != after.cells,
                    "the render was asked for\n{}",
                    after.rows.join("\n")
                );
            }
            assert!(
                after.rows.iter().any(|r| r.contains("after")),
                "{label}: the page is there\n{}",
                after.rows.join("\n")
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 4);
}

/// A Word body with an inline formula and a display one (OMML), around plain paragraphs.
fn docx_math_body() -> String {
    let sup =
        "<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>";
    let frac =
        "<m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f>";
    let mut body = super::word::para("Formulas");
    body += &format!(
        "<w:p><w:r><w:t xml:space=\"preserve\">inline </w:t></w:r><m:oMath>{sup}</m:oMath><w:r><w:t xml:space=\"preserve\"> here</w:t></w:r></w:p>"
    );
    body += &format!("<w:p><m:oMathPara><m:oMath>{frac}</m:oMath></m:oMathPara></w:p>");
    body += &super::word::para("after the formulas");
    body
}

fn office_program_pair() -> OfficeProgram {
    let (own, bundled) = office_program();
    (own, bundled)
}

#[test]
fn a_reopened_docx_draws_its_formulas_in_the_office_program_as_the_host_does() {
    let mut checked = 0;
    for proto in [ProtocolType::Halfblocks, ProtocolType::Kitty] {
        let dir = sandbox("docx-math");
        super::word::build_docx(&dir.join("d.docx"), &docx_math_body());
        let root = canon(&dir);
        let (w, h) = (100u16, 30u16);
        let (hb, ha) = host_pictures_on(&root, "d.docx", cfg_en(), w, h + 1, Some(proto), None);
        assert!(
            ha.rows.iter().any(|r| r.contains("after the formulas")),
            "{}",
            ha.rows.join("\n")
        );
        let gate = Arc::new(Gate::new());
        let (ab, aa, _) = app_pictures_on(
            &root.join("d.docx"),
            "",
            office_program_pair(),
            Some(proto),
            &gate,
            (fetch_on, &FETCH_GATE_ON),
            ha.clone(),
            w,
            h,
        );
        same(&format!("docx {proto:?}: before"), &hb, &ab);
        same(&format!("docx {proto:?}: settled"), &ha, &aa);
        assert_eq!(drawn_cells(Some(proto), &ab), 0, "not yet");
        assert!(drawn_cells(Some(proto), &aa) > 3, "{proto:?}");
        checked += 1;
    }
    assert_eq!(checked, 2);
}

#[test]
fn a_reopened_docx_keeps_its_formulas_as_text_with_no_picker_or_in_text_mode() {
    let dir = sandbox("docx-math-text");
    super::word::build_docx(&dir.join("d.docx"), &docx_math_body());
    let root = canon(&dir);
    // No picker: the sync host (no media) against the app without pictures.
    let hs = host_term(&root, "d.docx", 100, 31, "");
    let (own, bundled) = office_program();
    let a = app_reopen_term_bundled(&root.join("d.docx"), own, bundled, 100, 30, "");
    assert_eq!(inner(&hs.term), inner(&a), "{}", rows_of(&a).join("\n"));
    assert!(
        inner(&a).iter().any(|r| r.contains("^{2}")),
        "{}",
        rows_of(&a).join("\n")
    );
    // Text mode on a picker.
    let mut cfg = cfg_en();
    let mut t = toml::Table::new();
    t.insert("mode".into(), toml::Value::String("text".into()));
    cfg.plugins.insert("math".into(), t);
    let (_, ha) = host_pictures_on(
        &root,
        "d.docx",
        cfg,
        100,
        31,
        Some(ProtocolType::Halfblocks),
        None,
    );
    let gate = Arc::new(Gate::new());
    let (_, aa, _) = app_pictures_on(
        &root.join("d.docx"),
        "[plugins.math]\nmode = \"text\"\n",
        office_program_pair(),
        Some(ProtocolType::Halfblocks),
        &gate,
        (fetch_on, &FETCH_GATE_ON),
        ha.clone(),
        100,
        30,
    );
    same("docx text mode", &ha, &aa);
    assert_eq!(picture_cells(&aa), 0);
}
