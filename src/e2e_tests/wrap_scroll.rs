//! A wrapping text preview moves in screen rows. Everything is read off the drawn screen.

use super::*;
use ratatui::style::Modifier;

/// A 60x20 session previewing `body`.
fn open(cfg: Config, name: &str, body: &str) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    std::fs::write(dir.join("long.txt"), body).unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, 60, 20);
    s.select("long.txt");
    s.enter();
    assert!(s.app.is_windowed());
    (s, dir)
}

/// The rows inside the preview's frame.
fn body_area(s: &Sim) -> (u16, u16, u16) {
    let buf = s.term.backend().buffer();
    let rows: Vec<u16> = (0..buf.area.height)
        .filter(|&y| matches!(buf[(0, y)].symbol(), "┌" | "└"))
        .collect();
    (rows[0] + 1, rows[1], buf.area.width - 1)
}

fn body(s: &Sim) -> Vec<String> {
    let (from, to, right) = body_area(s);
    let buf = s.term.backend().buffer();
    (from..to)
        .map(|y| {
            let mut row = String::new();
            let mut x = 1;
            while x < right {
                let symbol = buf[(x, y)].symbol();
                row.push_str(symbol);
                x += ratatui::text::Span::raw(symbol).width().max(1) as u16;
            }
            row.trim_end().to_string()
        })
        .collect()
}

/// The screen row of the body the caret is drawn on.
fn caret_row(s: &Sim) -> Option<usize> {
    let (from, to, right) = body_area(s);
    let buf = s.term.backend().buffer();
    (from..to)
        .find(|&y| (1..right).any(|x| buf[(x, y)].modifier.contains(Modifier::REVERSED)))
        .map(|y| usize::from(y - from))
}

fn shows(s: &Sim, token: &str) -> bool {
    body(s).iter().any(|row| row.contains(token))
}

/// 60 lines, three of every four longer than the screen is wide. `S07` starts line 7 and `E07`
/// ends it.
fn lines_of(filler: &str) -> String {
    (0..60)
        .map(|i| {
            let n = if i % 4 == 0 { 1 } else { 3 + i % 5 };
            format!("S{i:02} {}E{i:02}\n", filler.repeat(n))
        })
        .collect()
}

fn bodies() -> Vec<(&'static str, String)> {
    vec![
        ("ascii", lines_of("alpha bravo charlie delta ")),
        ("cjk", lines_of("全角の長い文章がここに続きます。")),
        ("tabs", lines_of("\tab\tcdefgh\tijklmnopqrstu\t")),
        ("unbroken", lines_of("abcdefghijklmnopqrstuvwxyz0123456789")),
    ]
}

fn configs() -> Vec<(&'static str, Config)> {
    let mut numbers = Config::default();
    numbers.ui.line_numbers = true;
    vec![("plain", Config::default()), ("numbers", numbers)]
}

#[test]
fn e2e_wrap_end_shows_the_end_of_the_last_line_on_the_bottom_row() {
    for (cname, cfg) in configs() {
        for (name, text) in bodies() {
            let (mut s, _dir) = open(cfg.clone(), &format!("wrap_g_{cname}_{name}"), &text);
            s.key('G');
            let rows = body(&s);
            assert!(
                rows.last().unwrap().ends_with("E59"),
                "{cname}/{name}: {rows:#?}"
            );
            assert!(caret_row(&s).is_some(), "{cname}/{name}: {rows:#?}");
            // A second draw and keys that cannot move stay there.
            s.key('j');
            s.ctrl('f');
            assert_eq!(body(&s), rows, "{cname}/{name}");
            // And `g` is the start again.
            s.key('g');
            assert!(
                body(&s)[0].contains("S00"),
                "{cname}/{name}: {:#?}",
                body(&s)
            );
            assert_eq!(caret_row(&s), Some(0), "{cname}/{name}");
        }
    }
}

#[test]
fn e2e_wrap_j_and_k_keep_the_whole_caret_line_on_screen() {
    for (cname, cfg) in configs() {
        for (name, text) in bodies() {
            let (mut s, _dir) = open(cfg.clone(), &format!("wrap_jk_{cname}_{name}"), &text);
            let down = (0..60).map(|i| ('j', i));
            let up = (0..59).rev().map(|i| ('k', i));
            for (step, (key, line)) in down.skip(1).chain(up).enumerate() {
                s.key(key);
                let (start, end) = (format!("S{line:02}"), format!("E{line:02}"));
                assert!(
                    shows(&s, &start) && shows(&s, &end),
                    "{cname}/{name} step {step} {key} line {line}: {:#?}",
                    body(&s)
                );
                assert!(
                    caret_row(&s).is_some(),
                    "{cname}/{name} step {step} {key} line {line}: {:#?}",
                    body(&s)
                );
            }
        }
    }
}

#[test]
fn e2e_wrap_paging_shows_every_line_and_keeps_the_caret_on_screen() {
    for (cname, cfg) in configs() {
        for (name, text) in bodies() {
            for (forward, back) in [('f', 'b'), ('d', 'u')] {
                let tag = format!("wrap_page_{cname}_{name}_{forward}");
                let (mut s, _dir) = open(cfg.clone(), &tag, &text);
                for (key, last) in [(forward, "E59"), (back, "S00")] {
                    let mut seen = body(&s).join("\n");
                    for _ in 0..200 {
                        let before = body(&s);
                        s.ctrl(key);
                        assert!(caret_row(&s).is_some(), "{tag} {key}: {:#?}", body(&s));
                        seen.push('\n');
                        seen.push_str(&body(&s).join("\n"));
                        if body(&s) == before {
                            break;
                        }
                    }
                    assert!(shows(&s, last), "{tag} {key}: {:#?}", body(&s));
                    for i in 0..60 {
                        for t in [format!("S{i:02}"), format!("E{i:02}")] {
                            // A token split by the wrap of an unbroken run is not searched for.
                            if name != "unbroken" {
                                assert!(seen.contains(&t), "{tag} {key}: {t} never shown");
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One line far longer than the screen, between two short ones. `w000`, `w001`, ... are its words.
fn one_huge_line(word: impl Fn(usize) -> String) -> String {
    let long: String = (0..400).map(word).collect();
    format!("FIRST\n{long}\nLAST\n")
}

#[test]
fn e2e_wrap_a_line_longer_than_the_screen_is_read_through_by_paging() {
    for (cname, cfg) in configs() {
        let text = one_huge_line(|i| format!("w{i:03} "));
        let (mut s, _dir) = open(cfg, &format!("wrap_huge_page_{cname}"), &text);
        // `j` onto it shows it from its start.
        s.key('j');
        assert!(body(&s)[0].contains("w000"), "{cname}: {:#?}", body(&s));
        let mut seen = body(&s).join("\n");
        for _ in 0..40 {
            s.ctrl('f');
            assert!(caret_row(&s).is_some(), "{cname}: {:#?}", body(&s));
            seen.push_str(&body(&s).join("\n"));
        }
        for i in 0..400 {
            assert!(seen.contains(&format!("w{i:03}")), "{cname}: w{i:03}");
        }
        assert_eq!(
            body(&s).last().unwrap().trim_start_matches([' ', '3']),
            "LAST"
        );
    }
}

#[test]
fn e2e_wrap_the_column_keys_follow_the_caret_down_a_long_line() {
    for (cname, cfg) in configs() {
        // A small screen: the line takes several of it, in few key presses.
        let long: String = (0..60).map(|i| format!("w{i:03} ")).collect();
        let text = format!("FIRST\n{long}\nLAST\n");
        let (mut s, _dir) = open(cfg, &format!("wrap_huge_col_{cname}"), &text);
        s.resize(30, 10);
        s.key('j');
        s.key('$');
        assert!(shows(&s, "w059"), "{cname}: {:#?}", body(&s));
        assert!(caret_row(&s).is_some(), "{cname}: {:#?}", body(&s));
        s.key('0');
        assert!(shows(&s, "w000"), "{cname}: {:#?}", body(&s));
        assert!(caret_row(&s).is_some(), "{cname}: {:#?}", body(&s));
        // Column by column past the bottom of the screen and back.
        let chars: Vec<char> = long.chars().collect();
        for key in ['l', 'h'] {
            for step in 0..320 {
                s.key(key);
                let col = match key {
                    'l' => (step + 1).min(chars.len() - 1),
                    _ => 298_usize.saturating_sub(step),
                };
                // The word wrap does not draw a space it breaks at, so no caret shows there.
                assert!(
                    caret_row(&s).is_some() || chars[col] == ' ',
                    "{cname} {key} {step}: {:#?}",
                    body(&s)
                );
            }
            // The far end of the walk is on screen.
            let want = if key == 'l' { "w059" } else { "w000" };
            assert!(shows(&s, want), "{cname} {key}: {:#?}", body(&s));
        }
    }
}

#[test]
fn e2e_wrap_search_shows_a_match_deep_in_a_long_line() {
    for (cname, cfg) in configs() {
        let text = one_huge_line(|i| {
            if i % 130 == 100 {
                format!("hit{i:03} ")
            } else {
                format!("w{i:03} ")
            }
        });
        let (mut s, _dir) = open(cfg, &format!("wrap_search_{cname}"), &text);
        s.key('/');
        s.keys("hit");
        s.enter();
        for (key, want) in [
            (None, "hit100"),
            (Some('n'), "hit230"),
            (Some('n'), "hit360"),
            (Some('N'), "hit230"),
            (Some('N'), "hit100"),
        ] {
            if let Some(k) = key {
                s.key(k);
            }
            assert!(shows(&s, want), "{cname} {want}: {:#?}", body(&s));
        }
    }
}

#[test]
fn e2e_wrap_a_jump_to_a_line_shows_the_whole_line() {
    for (cname, cfg) in configs() {
        let text = lines_of("alpha bravo charlie delta ");
        let (mut s, dir) = open(cfg, &format!("wrap_jump_{cname}"), &text);
        let file = canon(&dir).join("long.txt");
        for line in [31usize, 8, 59, 60, 1] {
            s.app.paste_jump_from(&format!("{}:{line}", file.display()));
            s.draw();
            let n = line - 1;
            assert!(
                shows(&s, &format!("S{n:02}")) && shows(&s, &format!("E{n:02}")),
                "{cname} line {line}: {:#?}",
                body(&s)
            );
            assert!(caret_row(&s).is_some(), "{cname} line {line}");
        }
    }
}

#[test]
fn e2e_wrap_a_narrower_terminal_keeps_the_caret_on_screen() {
    let text = lines_of("alpha bravo charlie delta ");
    let (mut s, _dir) = open(Config::default(), "wrap_resize", &text);
    for _ in 0..30 {
        s.key('j');
    }
    s.resize(30, 20);
    assert!(caret_row(&s).is_some(), "{:#?}", body(&s));
    s.key('G');
    assert!(body(&s).last().unwrap().ends_with("E59"), "{:#?}", body(&s));
    // Wider again: the last window is filled, not left short.
    s.resize(120, 20);
    let rows = body(&s);
    assert!(rows.iter().all(|r| !r.is_empty()), "{rows:#?}");
    assert!(rows.last().unwrap().ends_with("E59"), "{rows:#?}");
}

#[test]
fn e2e_wrap_off_and_back_on() {
    let text = lines_of("alpha bravo charlie delta ");
    let mut cfg = Config::default();
    cfg.ui.wrap = false;
    let (mut s, _dir) = open(cfg, "wrap_off", &text);
    s.key('G');
    let rows = body(&s);
    // Without wrapping a line is a row: the last screen is the last lines.
    let first = 60 - rows.len();
    for (i, row) in rows.iter().enumerate() {
        assert!(row.starts_with(&format!("S{:02}", first + i)), "{rows:#?}");
    }
    // Wrapping on: the caret's line (the last) stays on screen.
    s.app.toggle_wrap();
    s.draw();
    assert!(body(&s).last().unwrap().ends_with("E59"), "{:#?}", body(&s));
    s.app.toggle_wrap();
    s.draw();
    assert_eq!(body(&s), rows);
}

#[cfg(feature = "git")]
#[test]
fn e2e_wrap_end_counts_the_git_change_gutter() {
    // Lines that fill the width exactly wrap once the change gutter takes a cell of it.
    let dir = sandbox("wrap_git_gutter");
    init_git_repo(&dir);
    let committed: String = (0..40).map(|i| format!("S{i:02}\n")).collect();
    std::fs::write(dir.join("long.txt"), committed).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "-qm", "base"]);
    let changed: String = (0..40)
        .map(|i| format!("S{i:02}{}E{i:02}\n", "x".repeat(58 - 6)))
        .collect();
    std::fs::write(dir.join("long.txt"), changed).unwrap();
    let mut s = Sim::with_config_sized(&canon(&dir), Config::default(), 60, 20);
    s.select("long.txt");
    s.enter();
    s.key('G');
    let rows = body(&s);
    // The gutter pushes the last char of each line onto a row of its own.
    assert_eq!(rows.last().unwrap(), "9", "{rows:#?}");
    assert!(rows[rows.len() - 2].ends_with("E3"), "{rows:#?}");
}

/// A line number past the end of a file goes to its last line, and the numbers keep matching the
/// lines.
#[test]
fn e2e_wrap_a_pasted_line_past_the_end_lands_on_the_last_line() {
    for wrap in [true, false] {
        for eol in ["\n", ""] {
            let text: Vec<String> = (0..60).map(|i| format!("S{i:02} E{i:02}")).collect();
            let mut cfg = Config::default();
            cfg.ui.wrap = wrap;
            cfg.ui.line_numbers = true;
            let tag = format!("wrap_paste_{wrap}_{}", eol.len());
            let (mut s, dir) = open(cfg, &tag, &format!("{}{eol}", text.join("\n")));
            for asked in [64, 61, 60, 1000] {
                let path = canon(&dir).join("long.txt");
                s.app
                    .paste_jump_from(&format!("{}:{asked}", path.display()));
                s.draw();
                let rows = body(&s);
                for row in rows.iter().filter(|r| r.contains('S')) {
                    let mut words = row.split_whitespace();
                    let number: usize = words.next().unwrap().parse().unwrap();
                    let token = words.find(|w| w.starts_with('S')).unwrap();
                    assert_eq!(
                        token,
                        format!("S{:02}", number - 1),
                        "wrap={wrap} eol={eol:?} :{asked}\n{rows:#?}"
                    );
                }
                assert!(shows(&s, "E59"), "wrap={wrap} :{asked}\n{rows:#?}");
                let caret = caret_row(&s).expect("a caret");
                assert!(
                    rows[caret].contains("S59"),
                    "wrap={wrap} :{asked}\n{rows:#?}"
                );
            }
        }
    }
}

/// Once a frame is drawn, drawing it again reads nothing.
#[test]
fn e2e_wrap_a_second_frame_reads_nothing() {
    let long = format!("a\n{}\nz\n", "x".repeat(3 * 1024 * 1024));
    let mut checked = 0;
    for wrap in [true, false] {
        for numbers in [true, false] {
            for (name, text) in [
                ("long", long.as_str()),
                ("tall", lines_of("alpha ").as_str()),
            ] {
                let mut cfg = Config::default();
                cfg.ui.wrap = wrap;
                cfg.ui.line_numbers = numbers;
                let tag = format!("wrap_reads_{wrap}_{numbers}_{name}");
                let (mut s, _dir) = open(cfg, &tag, text);
                for key in ["", "j", "G", "k"] {
                    for c in key.chars() {
                        s.key(c);
                    }
                    s.draw();
                    s.draw();
                    let before = konoma_widgets::window::disk_reads();
                    for _ in 0..5 {
                        s.draw();
                    }
                    assert_eq!(
                        konoma_widgets::window::disk_reads() - before,
                        0,
                        "wrap={wrap} numbers={numbers} {name} after {key:?}"
                    );
                    checked += 1;
                }
                assert!(
                    konoma_widgets::window::disk_reads() > 0,
                    "the counter counts"
                );
            }
        }
    }
    assert_eq!(checked, 32);
}

/// The text on screen with the blanks taken out: a very narrow view cuts every word.
fn squeezed(s: &Sim) -> String {
    body(s).concat().split_whitespace().collect()
}

/// So narrow that the number gutter takes most or all of a line's first screen row: paging still
/// moves, reaches both ends and passes every line.
#[test]
fn e2e_wrap_paging_in_a_view_the_number_gutter_fills() {
    let text: String = (0..12).map(|i| format!("S{i:02} E{i:02}\n")).collect();
    for (cname, cfg) in configs() {
        for width in 4u16..=10 {
            let (mut s, _dir) = open(cfg.clone(), &format!("wrap_narrow_{cname}_{width}"), &text);
            s.resize(width, 12);
            let what = format!("{cname} width {width}");
            let mut seen = squeezed(&s);
            let mut before = body(&s);
            for _ in 0..200 {
                s.ctrl('d');
                assert!(
                    caret_row(&s).is_some() || width < 6,
                    "{what}: {:#?}",
                    body(&s)
                );
                if body(&s) == before {
                    break;
                }
                before = body(&s);
                seen.push('|');
                seen.push_str(&squeezed(&s));
            }
            if width >= 5 {
                assert!(squeezed(&s).ends_with("E11"), "{what}: {:#?}", body(&s));
                for i in 0..12 {
                    for token in [format!("S{i:02}"), format!("E{i:02}")] {
                        assert!(seen.contains(&token), "{what} {token}: {seen}");
                    }
                }
            }
            let end = body(&s);
            s.key('g');
            for step in 1..12 {
                s.key('j');
                assert!(caret_row(&s).is_some(), "{what} j {step}: {:#?}", body(&s));
            }
            s.key('g');
            s.key('G');
            assert_eq!(body(&s), end, "{what}: G and paging end apart");
            for _ in 0..200 {
                s.ctrl('u');
            }
            assert!(squeezed(&s).contains("S00"), "{what}: {:#?}", body(&s));
        }
    }
}

/// Views too small to show anything useful, and a file with nothing in it: every key is taken.
#[test]
fn e2e_wrap_empty_files_and_tiny_views_take_every_key() {
    for (cname, cfg) in configs() {
        for (name, text) in [
            ("empty", String::new()),
            ("newline", "\n".to_string()),
            ("wide", "全角の長い行\tと🙂\n".repeat(30)),
        ] {
            for size in [(3u16, 5u16), (4, 4), (5, 12), (60, 4)] {
                let dir = sandbox(&format!("wrap_tiny_{cname}_{name}_{}_{}", size.0, size.1));
                std::fs::write(dir.join("long.txt"), &text).unwrap();
                let mut s = Sim::with_config_sized(&canon(&dir), cfg.clone(), 60, 20);
                s.select("long.txt");
                s.enter();
                s.resize(size.0, size.1);
                for key in "Gjkgllll$h0vjjGg".chars() {
                    s.key(key);
                }
                s.esc();
                for key in "VGk".chars() {
                    s.key(key);
                }
                s.esc();
                for key in "fbdu".chars() {
                    s.ctrl(key);
                }
                s.resize(60, 20);
                if name == "wide" {
                    s.key('G');
                    assert!(shows(&s, "🙂"), "{cname}/{name}/{size:?}: {:#?}", body(&s));
                    assert!(caret_row(&s).is_some(), "{cname}/{name}/{size:?}");
                }
            }
        }
    }
}

/// A file rewritten shorter (or longer) from outside while its end is shown: the window comes back
/// inside the text, with the end of the new last line on screen after `G`.
#[test]
fn e2e_wrap_a_file_that_shrinks_outside_stays_in_range() {
    for (cname, cfg) in configs() {
        for (name, kept) in [("some", 20usize), ("one", 1), ("none", 0)] {
            let text = lines_of("alpha bravo charlie delta ");
            let (mut s, dir) = open(cfg.clone(), &format!("wrap_shrink_{cname}_{name}"), &text);
            s.key('G');
            let path = canon(&dir).join("long.txt");
            let shorter: String = text.split_inclusive('\n').take(kept).collect();
            std::fs::write(&path, &shorter).unwrap();
            s.app
                .refresh_fs_changed(false, std::slice::from_ref(&path))
                .unwrap();
            s.draw();
            let what = format!("{cname}/{name}: {:#?}", body(&s));
            if kept > 0 {
                let last = format!("E{:02}", kept - 1);
                assert!(shows(&s, &last), "{what}");
                s.key('G');
                assert!(shows(&s, &last), "{cname}/{name}: {:#?}", body(&s));
                assert!(caret_row(&s).is_some(), "{cname}/{name}: {:#?}", body(&s));
                s.key('k');
                assert!(caret_row(&s).is_some(), "{cname}/{name}: {:#?}", body(&s));
            } else {
                assert!(!shows(&s, "S00"), "{what}");
                s.key('G');
                s.key('k');
            }
            std::fs::write(&path, &text).unwrap();
            s.app
                .refresh_fs_changed(false, std::slice::from_ref(&path))
                .unwrap();
            s.draw();
            s.key('G');
            assert!(
                body(&s).last().unwrap().ends_with("E59"),
                "{cname}/{name}: {:#?}",
                body(&s)
            );
        }
    }
}

/// Another tab and back: the same screen rows, mid-line included.
#[test]
fn e2e_wrap_a_tab_comes_back_to_the_same_screen_rows() {
    for (cname, cfg) in configs() {
        let text = one_huge_line(|i| format!("w{i:04} "));
        let (mut s, _dir) = open(cfg.clone(), &format!("wrap_tabs_{cname}"), &text);
        s.ctrl('f');
        s.ctrl('f');
        let (rows, caret) = (body(&s), caret_row(&s));
        assert!(!rows[0].contains("w0000"), "{cname}: {rows:#?}");
        s.key('t');
        assert_ne!(body_or_none(&s), Some(rows.clone()), "{cname}: no new tab");
        s.key('[');
        assert_eq!(body(&s), rows, "{cname}");
        assert_eq!(caret_row(&s), caret, "{cname}");
    }
}

fn body_or_none(s: &Sim) -> Option<Vec<String>> {
    s.screen().contains('└').then(|| body(s))
}

/// Following an outside change opens the file at its first changed line, below long lines.
#[cfg(feature = "git")]
#[test]
fn e2e_wrap_follow_shows_the_changed_line() {
    for line in [2usize, 31, 59] {
        let dir = sandbox(&format!("wrap_follow_{line}"));
        init_git_repo(&dir);
        // Lines of many rows each: the unchanged lines above a change are more than a screen.
        let text = lines_of(&"alpha bravo charlie delta ".repeat(5));
        std::fs::write(dir.join("long.txt"), &text).unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-qm", "base"]);
        let root = canon(&dir);
        let mut cfg = Config::default();
        cfg.ui.follow_view = "file".into();
        let mut s = Sim::with_config_sized(&root, cfg, 60, 20);
        s.key('F');
        let path = root.join("long.txt");
        let changed = text.replace(&format!("S{line:02} "), &format!("S{line:02} CHANGED "));
        std::fs::write(&path, changed).unwrap();
        assert!(s.app.follow_note_change(&path));
        s.app
            .refresh_fs_changed(false, std::slice::from_ref(&path))
            .unwrap();
        s.app.follow_jump(&path);
        s.draw();
        s.draw();
        assert!(shows(&s, "CHANGED"), "line {line}: {:#?}", body(&s));
        assert!(caret_row(&s).is_some(), "line {line}: {:#?}", body(&s));
    }
}

/// Selecting downwards and upwards keeps the line being extended to on screen.
#[test]
fn e2e_wrap_selections_keep_the_caret_line_on_screen() {
    for (cname, cfg) in configs() {
        for key in ['v', 'V'] {
            let text = lines_of("全角の長い文章がここに続きます。");
            let (mut s, _dir) = open(cfg.clone(), &format!("wrap_select_{cname}_{key}"), &text);
            s.key(key);
            let down = (1..60).map(|i| ('j', i));
            let up = (0..59).rev().map(|i| ('k', i));
            for (k, line) in down.chain(up) {
                s.key(k);
                let (start, end) = (format!("S{line:02}"), format!("E{line:02}"));
                assert!(
                    shows(&s, &start) && shows(&s, &end),
                    "{cname} {key} {k} line {line}: {:#?}",
                    body(&s)
                );
            }
            s.key('G');
            assert!(
                body(&s).last().unwrap().ends_with("E59"),
                "{cname} {key}: {:#?}",
                body(&s)
            );
        }
    }
}

/// With git change marks on a wrapping text, the host and the standalone preview draw the same
/// cells, wherever the keys put the window.
#[cfg(feature = "git")]
#[test]
fn e2e_wrap_with_change_marks_the_standalone_preview_matches_the_host() {
    use konoma_plugin::registry::{PluginOrigin, Registry};
    use ratatui::buffer::Buffer;
    for (case, spec) in ["", "G", "G k k", "<C-f> <C-f>", "G g j j j j j j j j"]
        .into_iter()
        .enumerate()
    {
        let dir = sandbox(&format!("wrap_marks_parity_{case}"));
        init_git_repo(&dir);
        let path = dir.join("file.txt");
        // Every line fills the 30 columns inside the frame exactly, so the mark column wraps it.
        let old: String = (0..40)
            .map(|i| format!("S{i:02}{}E{i:02}\n", "x".repeat(24)))
            .collect();
        let new = old.replace("S25x", "S25y").replace("S30x", "S30y");
        std::fs::write(&path, &old).unwrap();
        run_git(&dir, &["add", "file.txt"]);
        run_git(&dir, &["commit", "-qm", "baseline"]);
        std::fs::write(&path, &new).unwrap();
        let mut cfg = cfg_en();
        cfg.ui.tabbar = "hidden".into();
        cfg.ui.statusbar = "bottom".into();
        cfg.ui.diff_view = "preview".into();
        cfg.ui.wrap = true;
        let mut sim = Sim::with_config_sized(&canon(&dir), cfg, 32, 11);
        sim.select("file.txt");
        sim.key('d');
        assert!(sim.app.preview_from_diff_for_test());
        let presses = super::table_parity::presses(spec);
        for &(code, mods) in &presses {
            sim.press(code, mods);
        }
        sim.term.clear().unwrap();
        sim.draw();
        let host = sim.term.backend().buffer().clone();

        let mut registry = Registry::new();
        for plugin in konoma_file_plugins::plugins() {
            registry.register(plugin, PluginOrigin::Standard).unwrap();
        }
        let source = konoma_plugin::host::file_preview::FilePreviewSource::new(
            std::sync::Arc::new(registry),
            Vec::new(),
            false,
        );
        let mut term = Terminal::new(TestBackend::new(32, 11)).unwrap();
        let shared = konoma_host_core::shared::SharedConfig::parse("[ui]\nwrap=true\n").unwrap();
        let events = presses
            .iter()
            .map(|&(code, mods)| {
                let ev = KeyEvent::new(code, mods);
                konoma_plugin::app::AppEvent::Key(
                    konoma_plugin::app::key_from_crossterm(&ev).expect("a key"),
                )
            })
            .collect();
        let sdk: Buffer = konoma_plugin::app::run_preview_with_landed_gutter_for_test(
            &mut term,
            source,
            path,
            dir.to_path_buf(),
            &shared,
            None,
            crate::vcs::diff_contents(&old, &new),
            events,
        )
        .unwrap();
        // The frame from its top border to its bottom border.
        let mut marked = false;
        for y in 0..10 {
            for x in 0..32 {
                let (a, b) = (&host[(x, y)], &sdk[(x, y)]);
                assert_eq!(
                    (a.symbol(), a.modifier),
                    (b.symbol(), b.modifier),
                    "{spec:?}: cell ({x},{y})\nhost {:#?}\nsdk {:#?}",
                    super::table_parity::rows_of(&sim.term),
                    sdk
                );
                marked |= a.symbol() == "▌";
            }
        }
        // The first change is where the preview lands.
        if spec.is_empty() {
            assert!(marked, "{spec:?}: no change mark on screen");
        }
    }
}
