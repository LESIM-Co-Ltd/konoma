//! The drawn buffer of a table, a canvas and an image, as literal text plus the styles of the
//! cells (plugin stage 3, step 0: pins what the host draws today, so moving the table, canvas and
//! image drawing into shared crates can be shown not to change it). The dump is
//! `test_support::buffer_dump`: the rows as the terminal shows them, then one line per run of
//! equally styled cells. Regenerate on purpose with
//! `KONOMA_PRINT_SNAPSHOTS=1 cargo nextest run -p konoma render_snapshot --no-capture` and read
//! the output before pasting it in.

use super::canvas::{open_with, Draw, Opened, Painter};
use super::*;

/// The drawn buffer as text. Row 1 is the frame's top border with the file's path in it, and the
/// path holds the sandbox directory name (process id), so the title text is replaced by plain
/// border cells before dumping; the corners and the border style stay.
fn dump(s: &Sim) -> String {
    dump_masked(s, false)
}

/// `dump`, and with `pixels` the cells that carry picture data (a half block, or a coloured
/// background) inside the frame become `#` with no style: the shape of the picture and the frame
/// around it are pinned, the resampled colours (the scaler's business) are not.
fn dump_masked(s: &Sim, pixels: bool) -> String {
    let mut buf = s.term.backend().buffer().clone();
    let w = buf.area.width;
    let row: String = (0..w).map(|x| buf[(x, 1)].symbol().to_string()).collect();
    if row.contains("konoma_e2e_") {
        let style = buf[(0, 1)].style();
        for x in 1..w.saturating_sub(1) {
            buf[(x, 1)].set_symbol("─").set_style(style);
        }
    }
    if pixels {
        let h = buf.area.height;
        for y in 2..h.saturating_sub(2) {
            for x in 1..w.saturating_sub(1) {
                let c = &mut buf[(x, y)];
                if c.symbol() == "▀" || c.bg != ratatui::style::Color::Reset {
                    c.set_symbol("#").set_style(ratatui::style::Style::reset());
                }
            }
        }
    }
    crate::test_support::buffer_dump(&buf)
}

/// Compare with the literal; with `KONOMA_PRINT_SNAPSHOTS` set, print the dump instead.
#[track_caller]
fn check(name: &str, got: &str, want: &str) {
    if std::env::var_os("KONOMA_PRINT_SNAPSHOTS").is_some() {
        println!("=====BEGIN {name}=====\n{got}=====END {name}=====");
        return;
    }
    assert_eq!(got, want, "{name} changed");
}

// ---- table ---------------------------------------------------------------------------------------

/// A CSV wider than a 44-column terminal: a header, ASCII, wide characters and a long value.
const WIDE_CSV: &str = "id,name,city,note,score,extra\n\
1,alpha,Tokyo,first row,10,aaaaaaaa\n\
2,ベータ,東京,日本語のセル,20,bbbbbbbb\n\
3,gamma,Osaka,a rather long value that does not fit,30,cccccccc\n\
4,delta,Kyoto,last,40,dddddddd\n";

/// ASCII under the popup, wide characters only inside the cell that the popup shows.
const POPUP_CSV: &str = "id,name,city,note,score,extra\n\
1,alpha,Tokyo,first row,10,aaaaaaaa\n\
2,beta,Nara,short,20,bbbbbbbb\n\
3,gamma,Osaka,a rather long value ワイド文字 that does not fit,30,cccccccc\n\
4,delta,Kyoto,last,40,dddddddd\n";

fn open_table(lang: &str, w: u16, h: u16) -> (Sim, crate::test_support::TmpDir) {
    open_table_csv(WIDE_CSV, lang, w, h)
}

fn open_table_csv(csv: &str, lang: &str, w: u16, h: u16) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox("render_table");
    std::fs::write(dir.join("t.csv"), csv).unwrap();
    let mut cfg = Config::default();
    cfg.ui.lang = lang.into();
    let mut s = Sim::with_config_sized(&canon(&dir), cfg, w, h);
    s.select("t.csv");
    s.enter();
    assert!(s.app.is_table_preview());
    (s, dir)
}

#[test]
fn render_snapshot_table_opened() {
    let (s, _d) = open_table("en", 44, 12);
    check(
        "table_opened",
        &dump(&s),
        r####" 0|                            TABLE   path:rel|
 1|┌──────────────────────────────────────────┐|
 2|│id  name   city                           │|
 3|│────────────────                          │|
 4|│1   alpha  Tokyo                          │|
 5|│2   ベータ 東京                           │|
 6|│3   gamma  Osaka                          │|
 7|│4   delta  Kyoto                          │|
 8|│                                          │|
 9|│                                          │|
10|└──────────────────────────────────────────┘|
11|h/j/k/l:cell  ↵:view cell  /:search …|
--- styles ---
r0 c27..33 fg=White bg=Cyan mod=BOLD | TABLE|
r0 c34..43 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=BOLD |id|
r2 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=BOLD |name|
r2 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=BOLD |city|
r3 c1..16 fg=DarkGray bg=Reset mod=NONE |────────────────|
r4 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=REVERSED |1|
r4 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |alpha|
r4 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Tokyo|
r5 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |2|
r5 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |ベータ|
r5 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |東京|
r6 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |3|
r6 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |gamma|
r6 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Osaka|
r7 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |4|
r7 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |delta|
r7 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Kyoto|
r11 c0..36 fg=Reset bg=Reset mod=DIM |h/j/k/l:cell  ↵:view cell  /:search …|
"####,
    );
}

#[test]
fn render_snapshot_table_selection_moved() {
    let (mut s, _d) = open_table("en", 44, 12);
    s.key('j');
    s.key('j');
    s.key('l');
    check(
        "table_moved",
        &dump(&s),
        r####" 0|                            TABLE   path:rel|
 1|┌──────────────────────────────────────────┐|
 2|│id  name   city                           │|
 3|│────────────────                          │|
 4|│1   alpha  Tokyo                          │|
 5|│2   ベータ 東京                           │|
 6|│3   gamma  Osaka                          │|
 7|│4   delta  Kyoto                          │|
 8|│                                          │|
 9|│                                          │|
10|└──────────────────────────────────────────┘|
11|h/j/k/l:cell  ↵:view cell  /:search …|
--- styles ---
r0 c27..33 fg=White bg=Cyan mod=BOLD | TABLE|
r0 c34..43 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=BOLD |id|
r2 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=BOLD |name|
r2 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=BOLD |city|
r3 c1..16 fg=DarkGray bg=Reset mod=NONE |────────────────|
r4 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |1|
r4 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |alpha|
r4 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Tokyo|
r5 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |2|
r5 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |ベータ|
r5 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |東京|
r6 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |3|
r6 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=REVERSED |gamma|
r6 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Osaka|
r7 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |4|
r7 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |delta|
r7 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Kyoto|
r11 c0..36 fg=Reset bg=Reset mod=DIM |h/j/k/l:cell  ↵:view cell  /:search …|
"####,
    );
}

#[test]
fn render_snapshot_table_scrolled_right() {
    let (mut s, _d) = open_table("en", 44, 12);
    s.key('j');
    for _ in 0..4 {
        s.key('l');
    }
    check(
        "table_scrolled",
        &dump(&s),
        r####" 0|                            TABLE   path:rel|
 1|┌──────────────────────────────────────────┐|
 2|│score extra                               │|
 3|│──────────────                            │|
 4|│10    aaaaaaaa                            │|
 5|│20    bbbbbbbb                            │|
 6|│30    cccccccc                            │|
 7|│40    dddddddd                            │|
 8|│                                          │|
 9|│                                          │|
10|└──────────────────────────────────────────┘|
11|h/j/k/l:cell  ↵:view cell  /:search …|
--- styles ---
r0 c27..33 fg=White bg=Cyan mod=BOLD | TABLE|
r0 c34..43 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=BOLD |score|
r2 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=BOLD |extra|
r3 c1..14 fg=DarkGray bg=Reset mod=NONE |──────────────|
r4 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |10|
r4 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |aaaaaaaa|
r5 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=REVERSED |20|
r5 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |bbbbbbbb|
r6 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |30|
r6 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |cccccccc|
r7 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |40|
r7 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |dddddddd|
r11 c0..36 fg=Reset bg=Reset mod=DIM |h/j/k/l:cell  ↵:view cell  /:search …|
"####,
    );
}

#[test]
fn render_snapshot_table_last_cell() {
    let (mut s, _d) = open_table("en", 44, 12);
    s.key('G');
    s.key('$');
    check(
        "table_last",
        &dump(&s),
        r####" 0|                            TABLE   path:rel|
 1|┌──────────────────────────────────────────┐|
 2|│score extra                               │|
 3|│──────────────                            │|
 4|│10    aaaaaaaa                            │|
 5|│20    bbbbbbbb                            │|
 6|│30    cccccccc                            │|
 7|│40    dddddddd                            │|
 8|│                                          │|
 9|│                                          │|
10|└──────────────────────────────────────────┘|
11|h/j/k/l:cell  ↵:view cell  /:search …|
--- styles ---
r0 c27..33 fg=White bg=Cyan mod=BOLD | TABLE|
r0 c34..43 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=BOLD |score|
r2 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=BOLD |extra|
r3 c1..14 fg=DarkGray bg=Reset mod=NONE |──────────────|
r4 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |10|
r4 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |aaaaaaaa|
r5 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |20|
r5 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |bbbbbbbb|
r6 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |30|
r6 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=NONE |cccccccc|
r7 c1..5 fg=Rgb(110, 200, 200) bg=Reset mod=NONE |40|
r7 c7..14 fg=Rgb(226, 138, 138) bg=Reset mod=REVERSED |dddddddd|
r11 c0..36 fg=Reset bg=Reset mod=DIM |h/j/k/l:cell  ↵:view cell  /:search …|
"####,
    );
}

#[test]
fn render_snapshot_table_cell_popup() {
    let (mut s, _d) = open_table_csv(POPUP_CSV, "en", 44, 12);
    for _ in 0..2 {
        s.key('j');
    }
    for _ in 0..3 {
        s.key('l');
    }
    s.enter();
    assert!(s.app.is_table_cell_open());
    check(
        "table_popup",
        &dump(&s),
        r####" 0|                     TABLE   CELL   path:rel|
 1|┌ ko┌ Cell · note  r3/4 c4/6 ─────────┐  r3┐|
 2|│not│a rather long value ワイド文字   │    │|
 3|│───│that does not fit                │──  │|
 4|│fir│                                 │    │|
 5|│sho│                                 │    │|
 6|│a r│                                 │e…  │|
 7|│las│                                 │    │|
 8|│   │                                 │    │|
 9|│   └p/bottom  q/Enter/Esc:close──────┘    │|
10|└──────────────────────────────────────────┘|
11|j/k:scroll  g/G:top/bottom …|
--- styles ---
r0 c20..26 fg=White bg=Cyan mod=BOLD | TABLE|
r0 c28..33 fg=White bg=DarkGray mod=BOLD | CELL|
r0 c34..43 fg=Reset bg=Reset mod=DIM |  path:rel|
r1 c4..38 fg=Cyan bg=Reset mod=NONE |┌ Cell · note  r3/4 c4/6 ─────────┐|
r2 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=BOLD |not|
r2 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r2 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r2 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=BOLD ||
r3 c1..3 fg=DarkGray bg=Reset mod=NONE |───|
r3 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r3 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r3 c39..40 fg=DarkGray bg=Reset mod=NONE |──|
r4 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=NONE |fir|
r4 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r4 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r4 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=NONE ||
r5 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=NONE |sho|
r5 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r5 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r5 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=NONE ||
r6 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=REVERSED |a r|
r6 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r6 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r6 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=REVERSED |e…|
r7 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=NONE |las|
r7 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r7 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r7 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=NONE ||
r8 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r8 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r9 c4..4 fg=Cyan bg=Reset mod=NONE |└|
r9 c5..31 fg=Cyan bg=Reset mod=DIM |p/bottom  q/Enter/Esc:close|
r9 c32..38 fg=Cyan bg=Reset mod=NONE |──────┘|
r11 c0..27 fg=Reset bg=Reset mod=BOLD |j/k:scroll  g/G:top/bottom …|
"####,
    );
}

#[test]
fn render_snapshot_table_cell_popup_of_wide_text() {
    let (mut s, _d) = open_table_csv(POPUP_CSV, "ja", 44, 12);
    s.key('j');
    s.key('j');
    for _ in 0..3 {
        s.key('l');
    }
    s.enter();
    check(
        "table_popup_wide_ja",
        &dump(&s),
        r####" 0|                 テーブル   セル   パス:相対|
 1|┌ ko┌ セル · note  r3/4 c4/6 ─────────┐  r3┐|
 2|│not│a rather long value ワイド文字   │    │|
 3|│───│that does not fit                │──  │|
 4|│fir│                                 │    │|
 5|│sho│                                 │    │|
 6|│a r│                                 │e…  │|
 7|│las│                                 │    │|
 8|│   │                                 │    │|
 9|│   └/末尾  q/Enter/Esc:閉じる────────┘    │|
10|└──────────────────────────────────────────┘|
11|j/k:スクロール  g/G:先頭/末尾 …|
--- styles ---
r0 c16..25 fg=White bg=Cyan mod=BOLD | テーブル|
r0 c27..32 fg=White bg=DarkGray mod=BOLD | セル|
r0 c33..43 fg=Reset bg=Reset mod=DIM |  パス:相対|
r1 c4..38 fg=Cyan bg=Reset mod=NONE |┌ セル · note  r3/4 c4/6 ─────────┐|
r2 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=BOLD |not|
r2 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r2 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r2 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=BOLD ||
r3 c1..3 fg=DarkGray bg=Reset mod=NONE |───|
r3 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r3 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r3 c39..40 fg=DarkGray bg=Reset mod=NONE |──|
r4 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=NONE |fir|
r4 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r4 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r4 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=NONE ||
r5 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=NONE |sho|
r5 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r5 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r5 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=NONE ||
r6 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=REVERSED |a r|
r6 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r6 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r6 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=REVERSED |e…|
r7 c1..3 fg=Rgb(200, 130, 220) bg=Reset mod=NONE |las|
r7 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r7 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r7 c39..40 fg=Rgb(200, 130, 220) bg=Reset mod=NONE ||
r8 c4..4 fg=Cyan bg=Reset mod=NONE |│|
r8 c38..38 fg=Cyan bg=Reset mod=NONE |│|
r9 c4..4 fg=Cyan bg=Reset mod=NONE |└|
r9 c5..29 fg=Cyan bg=Reset mod=DIM |/末尾  q/Enter/Esc:閉じる|
r9 c30..38 fg=Cyan bg=Reset mod=NONE |────────┘|
r11 c0..30 fg=Reset bg=Reset mod=BOLD |j/k:スクロール  g/G:先頭/末尾 …|
"####,
    );
}

#[test]
fn render_snapshot_table_ja() {
    let (mut s, _d) = open_table("ja", 44, 12);
    s.key('j');
    s.key('j');
    s.key('l');
    check(
        "table_ja",
        &dump(&s),
        r####" 0|                        テーブル   パス:相対|
 1|┌──────────────────────────────────────────┐|
 2|│id  name   city                           │|
 3|│────────────────                          │|
 4|│1   alpha  Tokyo                          │|
 5|│2   ベータ 東京                           │|
 6|│3   gamma  Osaka                          │|
 7|│4   delta  Kyoto                          │|
 8|│                                          │|
 9|│                                          │|
10|└──────────────────────────────────────────┘|
11|h/j/k/l:セル  ↵:セル表示  /:検索  y:コピー …|
--- styles ---
r0 c23..32 fg=White bg=Cyan mod=BOLD | テーブル|
r0 c33..43 fg=Reset bg=Reset mod=DIM |  パス:相対|
r2 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=BOLD |id|
r2 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=BOLD |name|
r2 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=BOLD |city|
r3 c1..16 fg=DarkGray bg=Reset mod=NONE |────────────────|
r4 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |1|
r4 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |alpha|
r4 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Tokyo|
r5 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |2|
r5 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |ベータ|
r5 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |東京|
r6 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |3|
r6 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=REVERSED |gamma|
r6 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Osaka|
r7 c1..3 fg=Rgb(102, 178, 255) bg=Reset mod=NONE |4|
r7 c5..10 fg=Rgb(120, 200, 120) bg=Reset mod=NONE |delta|
r7 c12..16 fg=Rgb(224, 176, 92) bg=Reset mod=NONE |Kyoto|
r11 c0..43 fg=Reset bg=Reset mod=DIM |h/j/k/l:セル  ↵:セル表示  /:検索  y:コピー …|
"####,
    );
}

// ---- canvas --------------------------------------------------------------------------------------

fn canvas_session(name: &str, draw: Draw, w: u16, h: u16) -> Opened {
    open_with(name, Painter::new(name, draw), w, h, "en")
}

#[test]
fn render_snapshot_canvas_pattern() {
    let o = canvas_session("rs_pattern", Draw::Pattern { cols: 20, rows: 6 }, 40, 12);
    check(
        "canvas_pattern",
        &dump(&o.s),
        r####" 0|                      PREVIEW   path:rel|
 1|┌──────────────────────────────────────┐|
 2|│ABC                                   │|
 3|│日本語                                │|
 4|│a日b                                  │|
 5|│n=0 one                               │|
 6|│                                      │|
 7|│                                      │|
 8|│                                      │|
 9|│                                      │|
10|└──────────────────────────────────────┘|
11|q:back  C-n/C-p:next/prev file  ?:help …|
--- styles ---
r0 c21..29 fg=White bg=Blue mod=BOLD | PREVIEW|
r0 c30..39 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..3 fg=Red bg=Reset mod=BOLD |ABC|
r2 c4..4 fg=Reset bg=Blue mod=NONE ||
r3 c1..6 fg=Rgb(10, 20, 30) bg=Reset mod=NONE |日本語|
r4 c1..4 fg=Reset bg=Reset mod=UNDERLINED |a日b|
r11 c0..39 fg=Reset bg=Reset mod=DIM |q:back  C-n/C-p:next/prev file  ?:help …|
"####,
    );
}

#[test]
fn render_snapshot_canvas_filled() {
    let o = canvas_session("rs_filled", Draw::Filled { cols: 20, rows: 6 }, 40, 12);
    check(
        "canvas_filled",
        &dump(&o.s),
        r####" 0|                      PREVIEW   path:rel|
 1|┌──────────────────────────────────────┐|
 2|│ABC                                   │|
 3|│日本語                                │|
 4|│a日b                                  │|
 5|│n=0 one                               │|
 6|│                                      │|
 7|│                                      │|
 8|│                                      │|
 9|│                                      │|
10|└──────────────────────────────────────┘|
11|q:back  C-n/C-p:next/prev file  ?:help …|
--- styles ---
r0 c21..29 fg=White bg=Blue mod=BOLD | PREVIEW|
r0 c30..39 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..3 fg=Red bg=Reset mod=BOLD |ABC|
r2 c4..20 fg=Reset bg=Blue mod=NONE ||
r3 c1..6 fg=Rgb(10, 20, 30) bg=Reset mod=NONE |日本語|
r3 c7..20 fg=Reset bg=Blue mod=NONE ||
r4 c1..4 fg=Reset bg=Reset mod=UNDERLINED |a日b|
r4 c5..20 fg=Reset bg=Blue mod=NONE ||
r5 c8..20 fg=Reset bg=Blue mod=NONE ||
r6 c1..20 fg=Reset bg=Blue mod=NONE ||
r7 c1..20 fg=Reset bg=Blue mod=NONE ||
r11 c0..39 fg=Reset bg=Reset mod=DIM |q:back  C-n/C-p:next/prev file  ?:help …|
"####,
    );
}

#[test]
fn render_snapshot_canvas_wide_character_in_the_last_column() {
    // 10 columns wide: the wide glyph at column 9 does not fit and must not spill into the frame.
    let o = canvas_session(
        "rs_wide_edge",
        Draw::WideAt {
            cols: 10,
            rows: 3,
            at: 9,
        },
        40,
        10,
    );
    check(
        "canvas_wide_edge",
        &dump(&o.s),
        r####" 0|                      PREVIEW   path:rel|
 1|┌──────────────────────────────────────┐|
 2|│                                      │|
 3|│                                      │|
 4|│                                      │|
 5|│                                      │|
 6|│                                      │|
 7|│                                      │|
 8|└──────────────────────────────────────┘|
 9|q:back  C-n/C-p:next/prev file  ?:help …|
--- styles ---
r0 c21..29 fg=White bg=Blue mod=BOLD | PREVIEW|
r0 c30..39 fg=Reset bg=Reset mod=DIM |  path:rel|
r9 c0..39 fg=Reset bg=Reset mod=DIM |q:back  C-n/C-p:next/prev file  ?:help …|
"####,
    );
}

#[test]
fn render_snapshot_canvas_wide_character_inside() {
    let o = canvas_session(
        "rs_wide_in",
        Draw::WideAt {
            cols: 10,
            rows: 3,
            at: 4,
        },
        40,
        10,
    );
    check(
        "canvas_wide_in",
        &dump(&o.s),
        r####" 0|                      PREVIEW   path:rel|
 1|┌──────────────────────────────────────┐|
 2|│    日                                │|
 3|│                                      │|
 4|│                                      │|
 5|│                                      │|
 6|│                                      │|
 7|│                                      │|
 8|└──────────────────────────────────────┘|
 9|q:back  C-n/C-p:next/prev file  ?:help …|
--- styles ---
r0 c21..29 fg=White bg=Blue mod=BOLD | PREVIEW|
r0 c30..39 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c5..6 fg=Green bg=Reset mod=NONE |日|
r9 c0..39 fg=Reset bg=Reset mod=DIM |q:back  C-n/C-p:next/prev file  ?:help …|
"####,
    );
}

#[test]
fn render_snapshot_canvas_larger_than_the_frame() {
    let o = canvas_session("rs_big", Draw::Pattern { cols: 80, rows: 40 }, 30, 10);
    check(
        "canvas_big",
        &dump(&o.s),
        r####" 0|            PREVIEW   path:rel|
 1|┌────────────────────────────┐|
 2|│ABC                         │|
 3|│日本語                      │|
 4|│a日b                        │|
 5|│n=0 one                     │|
 6|│                            │|
 7|│                            │|
 8|└────────────────────────────┘|
 9|q:back …|
--- styles ---
r0 c11..19 fg=White bg=Blue mod=BOLD | PREVIEW|
r0 c20..29 fg=Reset bg=Reset mod=DIM |  path:rel|
r2 c1..3 fg=Red bg=Reset mod=BOLD |ABC|
r2 c4..4 fg=Reset bg=Blue mod=NONE ||
r3 c1..6 fg=Rgb(10, 20, 30) bg=Reset mod=NONE |日本語|
r4 c1..4 fg=Reset bg=Reset mod=UNDERLINED |a日b|
r9 c0..7 fg=Reset bg=Reset mod=DIM |q:back …|
"####,
    );
}

// ---- image ---------------------------------------------------------------------------------------

fn open_image(w: u16, h: u16, png: (u32, u32)) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox("render_image");
    crate::test_support::write_solid_png(&dir.join("pic.png"), png.0, png.1, [200, 40, 40]);
    let mut s = Sim::with_config_sized(&canon(&dir), cfg_en(), w, h)
        .with_picker()
        .with_media();
    // A still image paints through a resize worker: wire a real one (as the PDF/mermaid e2e tests
    // do), draw once to request the resize, apply the answer, draw again.
    let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel();
    let (resp_tx, mut resp_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || crate::resize_worker(req_rx, resp_tx));
    s.app
        .attach_image_backend(ratatui_image::picker::Picker::halfblocks(), req_tx);
    s.select("pic.png");
    s.enter();
    s.drain_media();
    s.draw();
    let resp = resp_rx.blocking_recv().expect("the resize worker answers");
    assert!(s.app.apply_image_resize(resp));
    s.draw();
    assert!(s.app.is_image_preview());
    (s, dir)
}

#[test]
fn render_snapshot_image_square() {
    let (s, _d) = open_image(40, 14, (200, 200));
    check(
        "image_square",
        &dump_masked(&s, true),
        r####" 0|                 IMAGE   x1.00  path:rel|
 1|┌──────────────────────────────────────┐|
 2|│         ####################         │|
 3|│         ####################         │|
 4|│         ####################         │|
 5|│         ####################         │|
 6|│         ####################         │|
 7|│         ####################         │|
 8|│         ####################         │|
 9|│         ####################         │|
10|│         ####################         │|
11|│         ####################         │|
12|└──────────────────────────────────────┘|
13|+/-:zoom  =:fit  h/j/k/l:pan …|
--- styles ---
r0 c16..22 fg=Black bg=LightBlue mod=BOLD | IMAGE|
r0 c23..29 fg=Reset bg=Reset mod=BOLD |  x1.00|
r0 c30..39 fg=Reset bg=Reset mod=DIM |  path:rel|
r13 c0..29 fg=Reset bg=Reset mod=DIM |+/-:zoom  =:fit  h/j/k/l:pan …|
"####,
    );
}

#[test]
fn render_snapshot_image_wide() {
    let (s, _d) = open_image(40, 14, (400, 100));
    check(
        "image_wide",
        &dump_masked(&s, true),
        r####" 0|                 IMAGE   x1.00  path:rel|
 1|┌──────────────────────────────────────┐|
 2|│                                      │|
 3|│                                      │|
 4|│######################################│|
 5|│######################################│|
 6|│######################################│|
 7|│######################################│|
 8|│######################################│|
 9|│                                      │|
10|│                                      │|
11|│                                      │|
12|└──────────────────────────────────────┘|
13|+/-:zoom  =:fit  h/j/k/l:pan …|
--- styles ---
r0 c16..22 fg=Black bg=LightBlue mod=BOLD | IMAGE|
r0 c23..29 fg=Reset bg=Reset mod=BOLD |  x1.00|
r0 c30..39 fg=Reset bg=Reset mod=DIM |  path:rel|
r13 c0..29 fg=Reset bg=Reset mod=DIM |+/-:zoom  =:fit  h/j/k/l:pan …|
"####,
    );
}
