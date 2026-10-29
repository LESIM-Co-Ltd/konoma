//! A file shown as text (the `code` plugin) in the konoma host and in the standalone app
//! (`konoma-plugin` feature `app`). Given the same file, the same size and the same keys, the two
//! must draw the same frame: the path title, the position label, the scrollbar on the right
//! border and the rows inside, wrapped or cut and scrolled sideways.
//!
//! Not compared (known differences): `j`/`k` (the host moves a caret and the window follows it;
//! the app has no caret), `h` `l` and `$` after the caret has moved (they follow the caret), the
//! colors.

use super::table_parity::{presses, rows_of};
use super::*;
use konoma_plugin::app::{open_file, run_with, AppCx, AppEvent, AppKeys, Display};
use konoma_plugin::TermCaps;
use std::sync::Arc;

/// What the app shows after the presses.
fn app_text(
    path: &std::path::Path,
    wrap: bool,
    tab_width: usize,
    (w, h): (u16, u16),
    spec: &str,
) -> Terminal<TestBackend> {
    let mut opened = open_file(
        vec![Arc::new(konoma_code::CodePlugin)],
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
        path: opened.path.clone(),
        open_dir: opened.path.parent().map(|p| p.to_path_buf()),
        display: Display {
            wrap,
            tab_width,
            ..Display::default()
        },
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
    assert_eq!(it.count(), 0, "the app ended early: {spec}");
    term
}

/// What the host shows after the presses, once the grammar is warm (so the coloring is not
/// pending: the state is checked, not a clock).
fn host_text(
    root: &std::path::Path,
    file: &str,
    wrap: bool,
    tab_width: usize,
    (w, h): (u16, u16),
    spec: &str,
) -> Sim {
    let ext = file.rsplit('.').next().unwrap_or("");
    crate::preview::code::warm_file(ext, &root.join(file));
    let mut cfg = cfg_en();
    cfg.ui.wrap = wrap;
    cfg.ui.tab_width = tab_width;
    let mut s = Sim::with_config_sized(root, cfg, w, h);
    s.select(file);
    s.enter();
    for (code, mods) in presses(spec) {
        s.press(code, mods);
        assert!(!s.quit, "the host ended early: {spec}");
    }
    assert!(!s.app.is_highlight_pending(), "{file}: still coloring");
    s.term.clear().expect("clear");
    s.draw();
    s
}

/// The frame: from the top border to the bottom border, the whole width.
fn frame_rows(term: &Terminal<TestBackend>) -> Vec<String> {
    let rows = rows_of(term);
    let top = rows
        .iter()
        .position(|l| l.starts_with('┌'))
        .expect("a frame");
    let bottom = rows
        .iter()
        .rposition(|l| l.starts_with('└'))
        .expect("a frame end");
    rows[top..=bottom].to_vec()
}

fn fixtures() -> Vec<(String, String)> {
    let code: String = (0..70)
        .map(|i| format!("fn f{i}() {{\n    let x = {i};\n}}\n"))
        .collect();
    let long = format!(
        "// {}\nlet short = 1;\n{}\n",
        "word ".repeat(80),
        "x".repeat(300)
    );
    let cjk: String = (0..40)
        .map(|i| format!("// 日本語のコメント行 {i} です。これは長い行で折り返しを確かめます、とても長い。\nlet s{i} = \"全角\";\n"))
        .collect();
    vec![
        ("code.rs".into(), code),
        ("long.rs".into(), long),
        ("cjk.rs".into(), cjk),
        (
            "tabs.rs".into(),
            // The first line is the widest (`$` puts the host's caret at its end).
            format!("{}z{}\n", "\t".repeat(12), "x".repeat(40))
                + &(0..60)
                    .map(|i| {
                        format!(
                            "\t\tfn f{i}() {{\tlet x = {i};\t// note\tend\n{}ab\tc\tdef\t\tg\n",
                            "\t".repeat(i % 4)
                        )
                    })
                    .collect::<String>(),
        ),
        // A wide character before a tab: the column the tab stop counts from.
        (
            "tabswide.rs".into(),
            (0..60)
                .map(|i| format!("\t日本\tx{i}\t語語\ty\n{}あ\tz\n", "\t".repeat(i % 3)))
                .collect(),
        ),
        ("plain.sh".into(), "# one\n# two\necho hi\n".into()),
        ("noeol.toml".into(), "a = 1\nb = 2\nc = 3".to_string()),
        ("empty.json".into(), String::new()),
    ]
}

const SPECS: &[&str] = &[
    "",
    "G",
    "G g",
    "<C-f>",
    "<C-f> <C-f>",
    "<C-f> <C-b>",
    "<C-d>",
    "<C-d> <C-d> <C-u>",
    "<PgDn>",
    "<PgDn> <PgUp>",
    "<End>",
    "<End> <Home>",
    "$",
    "$ 0",
];

#[test]
fn the_text_frame_agrees_with_the_host() {
    let dir = sandbox("text-parity");
    for (name, body) in fixtures() {
        std::fs::write(dir.join(&name), body).unwrap();
    }
    let root = canon(&dir);
    let (mut checked, mut cells, mut labels, mut pcts, mut thumbs, mut marks) =
        (0, 0usize, 0, 0, 0, 0);
    for (name, _) in fixtures() {
        for wrap in [true, false] {
            // `[ui] tab_width` matters to the file with tabs only (the others have none).
            let tab_widths: &[usize] = if name.starts_with("tabs") {
                &[0, 1, 2, 4, 8, 16]
            } else {
                &[4]
            };
            for &tab_width in tab_widths {
                for (w, h) in [(60u16, 12u16), (200, 26), (24, 8)] {
                    for spec in SPECS {
                        // `$` puts the host's caret on the last character and the window follows
                        // the caret's first cell: a last wide character is cut by one cell, where the
                        // app (no caret) shows the end of the line.
                        if name.starts_with("cjk") && spec.contains('$') {
                            continue;
                        }
                        // The test backend keeps a stale cell where a wide character was drawn
                        // in an earlier frame (the host repaints before its last frame, the app
                        // does not): with wide characters, one press is compared.
                        if name == "tabswide.rs" && spec.split(' ').count() > 1 {
                            continue;
                        }
                        let what =
                            format!("{name} wrap={wrap} tab_width={tab_width} {w}x{h} [{spec}]");
                        let host = host_text(&root, &name, wrap, tab_width, (w, h), spec);
                        let app = app_text(&root.join(&name), wrap, tab_width, (w, h - 1), spec);
                        let (hr, ar) = (frame_rows(&host.term), frame_rows(&app));
                        assert_eq!(
                            hr,
                            ar,
                            "{what}: the frame differs\nhost:\n{}\napp:\n{}",
                            rows_of(&host.term).join("\n"),
                            rows_of(&app).join("\n")
                        );
                        checked += 1;
                        cells += hr.iter().map(|r| r.chars().count()).sum::<usize>();
                        labels += usize::from(
                            hr[0].contains("[Top]")
                                || hr[0].contains('%')
                                || hr[0].contains("[Bot]"),
                        );
                        pcts += usize::from(hr[0].contains('%'));
                        thumbs += usize::from(hr.iter().skip(1).any(|r| r.ends_with('█')));
                        marks += usize::from(hr.iter().any(|r| r.contains('→')));
                    }
                }
            }
        }
    }
    assert!(checked >= 1000, "{checked}");
    assert!(cells > 100_000, "{cells}");
    assert!(labels > 100, "{labels}");
    assert!(pcts > 20, "{pcts}");
    assert!(thumbs > 100, "{thumbs}");
    assert!(marks > 100, "frames with a tab mark: {marks}");
}

/// The title is the path relative to the folder the app was opened in, a framed text carries no
/// "line N" in its footer (the label on the frame says where it is), and nothing is named
/// "preview" while a path is known.
#[test]
fn a_framed_text_is_titled_by_its_relative_path_and_the_footer_has_no_line_number() {
    let dir = sandbox("text-title");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let body: String = (0..80).map(|i| format!("fn f{i}() {{}}\n")).collect();
    std::fs::write(dir.join("sub/nested.rs"), body).unwrap();
    let root = canon(&dir);
    let app = app_text(&root.join("sub/nested.rs"), true, 4, (60, 11), "<C-f>");
    let rows = rows_of(&app);
    assert!(rows[0].contains("nested.rs"), "{}", rows[0]);
    assert!(!rows[0].contains("preview"), "{}", rows[0]);
    let foot = rows.last().unwrap();
    assert!(!foot.contains("line "), "{foot}");
    assert!(rows.iter().any(|r| r.contains('%')), "{rows:?}");
}
