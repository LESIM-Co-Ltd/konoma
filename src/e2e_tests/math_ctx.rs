//! In-text LaTeX in the contexts that cannot lift an expression onto a line of its own: table cells,
//! headings, the inside of bold/italic/strikethrough, and quote/alert/`<details>` bodies.
//!
//! Every test here drives the real pipeline (`Sim` + a picker, real RaTeX render, real `math_cells`,
//! real `postprocess_md`) and reads `App::decorated_lines` — the lines the preview draws — against
//! `md_images()`: a placement is only right if the cells under it are blank and the text around it
//! is what the document says. Neither a stub slot (no real sizes) nor the snapshot harness (it masks
//! every column) reaches that.

use super::*;
use crate::preview::markdown::{collect_math_exprs, is_math_url, math_url, ImagePlacement};
use ratatui::style::Color;
use ratatui::text::Line;

fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// `md` opened in a `w`-wide sim whose picker is attached (so `math` is in image mode). The
/// expressions render synchronously there, so the first draw already carries the placements.
fn open_cfg(name: &str, md: &str, w: u16, cfg: Config) -> (Sim, crate::test_support::TmpDir) {
    let dir = sandbox(name);
    std::fs::write(dir.join("d.md"), md).unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, cfg, w, 40).with_picker();
    s.select("d.md");
    s.enter();
    (s, dir)
}

fn open(name: &str, md: &str) -> (Sim, crate::test_support::TmpDir) {
    open_cfg(name, md, 90, Config::default())
}

/// The decorated lines the preview draws, and the placements recorded against them.
fn view(s: &mut Sim) -> (Vec<Line<'static>>, Vec<ImagePlacement>) {
    let w = s.term.size().unwrap().width - 2;
    let lines = s.app.decorated_lines(w);
    (lines, s.app.md_images())
}

fn math_placements(p: &[ImagePlacement]) -> Vec<ImagePlacement> {
    p.iter().filter(|p| is_math_url(&p.url)).cloned().collect()
}

/// The invariant every in-text placement owes: the cells it covers are blank. Returns the decorated
/// line's text split around it — `(before, after)`.
#[track_caller]
fn around(lines: &[Line<'static>], p: &ImagePlacement) -> (String, String) {
    let line = &lines[p.line];
    let (before, reserved, after) = slice_by_display_col(line, p.col, p.cols);
    assert_eq!(
        reserved,
        " ".repeat(p.cols as usize),
        "cells under {p:?} must be blank; line: {:?}",
        plain(line)
    );
    (before, after)
}

/// Every row of the table box that starts at `top` (a `┌` row) has the same display width — the
/// border bars line up, i.e. the reservation took exactly the cells it claimed.
#[track_caller]
fn assert_box_intact(lines: &[Line<'static>]) {
    let mut widths = Vec::new();
    for l in lines {
        let t = plain(l);
        let t = t.trim_start().to_string();
        if t.starts_with('│') || t.starts_with('┌') || t.starts_with('├') || t.starts_with('└')
        {
            widths.push(unicode_width::UnicodeWidthStr::width(plain(l).as_str()));
        }
    }
    assert!(
        !widths.is_empty(),
        "no table in {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(
        widths.windows(2).all(|w| w[0] == w[1]),
        "table rows differ in width {widths:?}: {:#?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------------------------
// Headings
// ---------------------------------------------------------------------------------------------

#[test]
fn heading_every_level_places_the_expression_in_the_heading_line() {
    for level in 1..=6 {
        let md = format!("{} Energy $E=mc^2$ rule\n\ntext\n", "#".repeat(level));
        let (mut s, _d) = open(&format!("mc_h{level}"), &md);
        let (lines, placements) = view(&mut s);
        let m = math_placements(&placements);
        assert_eq!(m.len(), 1, "h{level}: {placements:?}");
        assert_eq!(m[0].rows, 1);
        let (before, after) = around(&lines, &m[0]);
        assert!(before.ends_with("Energy "), "h{level}: {before:?}");
        assert_eq!(after, " rule", "h{level}");
        s.dont_see("E=mc^2");
    }
}

#[test]
fn heading_placements_follow_the_rules_inserted_under_earlier_headings() {
    // An H1/H2 gains a rule line under it during decoration; every placement after one has to be
    // moved down by it (`heading_rule_shift`) or it lands on the wrong row.
    let md = "# One $a$ x\n\n## Two $b$ y\n\n### Three $c$ z\n\npara $d$ end\n";
    let (mut s, _d) = open("mc_h_shift", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 4, "{placements:?}");
    let expect = [
        ("One ", " x"),
        ("Two ", " y"),
        ("Three ", " z"),
        ("para ", " end"),
    ];
    for (p, (b, a)) in m.iter().zip(expect) {
        let (before, after) = around(&lines, p);
        assert!(before.trim_start().ends_with(b), "{before:?} / {p:?}");
        assert_eq!(after, a);
    }
}

#[test]
fn heading_with_two_expressions_bold_and_a_display_one() {
    let (mut s, _d) = open("mc_h_many", "## A $x$ and **$y$** then $$z$$ end\n");
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 3, "{placements:?}");
    assert!(
        m.iter().all(|p| p.line == m[0].line && p.rows == 1),
        "{m:?}"
    );
    assert!(m[0].col + m[0].cols <= m[1].col && m[1].col + m[1].cols <= m[2].col);
    // The `$$z$$` is placed at in-text size: the key is the inline one, not the display one.
    assert_eq!(m[2].url, math_url("z", false));
    let (before, after) = around(&lines, &m[2]);
    assert!(
        before.contains("A ") && before.contains(" and "),
        "{before:?}"
    );
    assert_eq!(after, " end");
    let mid = text_between_cols(&lines[m[0].line], m[0].col + m[0].cols, m[1].col);
    assert_eq!(mid, " and ");
    // The outline reads the source back for every drawn expression, in order.
    assert_eq!(s.app.md_outline()[0].1, "A $x$ and $y$ then $z$ end");
}

#[test]
fn heading_slug_and_outline_text_do_not_depend_on_the_picture() {
    let md = "# Energy $E=mc^2$ rule\n\n[go](#energy-emc2-rule)\n\n## Plain\n";
    // With the picture (the heading line holds blank cells) …
    let (mut s, _d) = open("mc_slug_img", md);
    let (_, placements) = view(&mut s);
    assert_eq!(math_placements(&placements).len(), 1);
    let with_img = s.app.md_outline();
    // … and without any image backend (the heading line holds the source text).
    let (mut t, _d2) = md_preview(Config::default(), "mc_slug_txt", md);
    let _ = view(&mut t);
    let without = t.app.md_outline();
    assert_eq!(
        with_img,
        without
            .iter()
            .map(|(l, x, n)| (*l, x.clone(), *n))
            .collect::<Vec<_>>()
            .as_slice()
            .to_vec()
    );
    assert_eq!(with_img[0].1, "Energy $E=mc^2$ rule");
    assert_eq!(with_img[1].1, "Plain");
    // The anchor written for the text form still resolves with the picture on.
    s.tab();
    s.enter();
    assert!(
        s.app.flash.is_none(),
        "the in-page link to the heading must resolve: {:?}",
        s.app.flash
    );
}

#[test]
fn heading_anchor_that_does_not_exist_still_flashes() {
    // Guards the test above: a flash-free Enter only means something if a wrong slug does flash.
    let (mut s, _d) = open("mc_slug_bad", "# Energy $E=mc^2$ rule\n\n[go](#energy)\n");
    let _ = view(&mut s);
    s.tab();
    s.enter();
    assert!(
        s.app.flash.is_some(),
        "a slug that is not the heading's must not resolve"
    );
}

#[test]
fn heading_outline_lists_the_source_and_jumps_to_it() {
    let mut md = String::new();
    for i in 0..60 {
        md.push_str(&format!("filler {i}\n\n"));
    }
    md.push_str("## Late $E=mc^2$ heading\n\nend\n");
    let (mut s, _d) = open("mc_outline", &md);
    let _ = view(&mut s);
    let items = s.app.md_outline();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].1, "Late $E=mc^2$ heading");
    s.key('o');
    s.see("Late $E=mc^2$ heading");
    s.enter();
    // Jumped: the heading is on screen now (the blank cells + its words), the filler is not.
    s.see("Late");
    s.dont_see("filler 0");
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 1);
    assert!(plain(&lines[m[0].line]).contains("Late"));
}

// ---------------------------------------------------------------------------------------------
// Bold / italic / strikethrough / link / image alt
// ---------------------------------------------------------------------------------------------

#[test]
fn emphasis_keeps_the_expression_inside_its_run() {
    let md = "**bold $a$ end**\n\n*it $b$ end*\n\n~~strike $c$ end~~\n\nplain $d$ end\n";
    let (mut s, _d) = open("mc_emph", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 4, "{placements:?}");
    for (p, word) in m.iter().zip(["bold ", "it ", "strike ", "plain "]) {
        let (before, after) = around(&lines, p);
        assert_eq!(before, word, "{p:?}");
        assert_eq!(after, " end");
    }
    // The words around a reservation keep their emphasis.
    let bold_line = &lines[m[0].line];
    assert!(
        bold_line.spans.iter().any(|sp| sp.content.contains("bold")
            && sp
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)),
        "{bold_line:?}"
    );
}

#[test]
fn emphasis_with_a_picture_less_expression_is_the_literal_text() {
    // No picture to give (`Loading`): the literal source, exactly as before — never blank cells.
    let dir = sandbox("mc_emph_loading");
    std::fs::write(dir.join("d.md"), "**bold $a$ end**\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.see("bold $a$ end");
    assert!(math_placements(&s.app.md_images()).is_empty());
}

#[test]
fn link_label_with_math_stays_one_literal_span_and_the_link_still_works() {
    let (mut s, _d) = open("mc_link", "[energy $E$ here](u.md) tail $x$ end\n");
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    // Only the expression in the plain text after the link is placed.
    assert_eq!(m.len(), 1, "{placements:?}");
    assert_eq!(m[0].alt, "x");
    let (before, _) = around(&lines, &m[0]);
    assert!(
        before.contains("energy $E$ here") && before.ends_with(" tail "),
        "{before:?}"
    );
    assert_eq!(s.app.md_link_targets(), vec!["u.md".to_string()]);
}

#[test]
fn list_items_place_expressions_inside_emphasis_and_alongside_a_task_checkbox() {
    let md = "- item **b $a$ e** end\n- [ ] task $c$ end\n1. one *i $d$* end\n";
    let (mut s, _d) = open("mc_list", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 3, "{placements:?}");
    for p in &m {
        let (_, after) = around(&lines, p);
        assert!(after.starts_with(' '), "{after:?}");
    }
    let (b, _) = around(&lines, &m[0]);
    assert!(b.ends_with("item b "), "{b:?}");
}

#[test]
fn expressions_with_underscores_and_stars_are_whole_in_every_context() {
    // pulldown-cmark cuts text at an `_`/`*` that did not open emphasis, so `$x_i$` arrives in
    // pieces; the contexts must read it whole (the lifting paragraph path always did).
    let md =
        "## H $\\sum_{i=1}^{n} i$ end\n\n**b $x_i y_j$ e**\n\n> q $a_1 * b_2$ r\n\n- l $c_k$ m\n";
    let (mut s, _d) = open("mc_us", md);
    let (lines, placements) = view(&mut s);
    let alts: Vec<&str> = math_placements(&placements)
        .iter()
        .map(|p| p.alt.clone())
        .collect::<Vec<_>>()
        .iter()
        .map(|a| if a.contains("sum") { "sum" } else { "other" })
        .collect();
    let m = math_placements(&placements);
    assert_eq!(m.len(), 4, "{placements:?}");
    assert_eq!(alts[0], "sum");
    assert_eq!(m[1].alt, "x_i y_j");
    assert_eq!(m[2].alt, "a_1 * b_2");
    assert_eq!(m[3].alt, "c_k");
    for p in &m {
        around(&lines, p);
    }
    s.dont_see("x_i");
}

#[test]
fn an_escaped_dollar_never_opens_an_expression() {
    let md = "# \\$a\\$ and \\$b$ here\n\n**\\$c\\$ bold**\n\n## \\\\$d$ after a backslash\n";
    let (mut s, _d) = open("mc_esc", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    // Only `$d$` — written after an escaped *backslash*, so its `$` is not escaped — is math.
    assert_eq!(m.len(), 1, "{placements:?}");
    assert_eq!(m[0].alt, "d");
    around(&lines, &m[0]);
    let all: Vec<String> = lines.iter().map(plain).collect();
    assert!(
        all.iter().any(|t| t.contains("$a$ and $b$ here")),
        "{all:?}"
    );
    assert!(all.iter().any(|t| t.contains("$c$ bold")), "{all:?}");
}

#[test]
fn image_alt_text_does_not_draw_math() {
    let (mut s, _d) = open("mc_alt", "see ![alt $x$ text](missing.png) end\n");
    let (_, placements) = view(&mut s);
    assert!(math_placements(&placements).is_empty(), "{placements:?}");
}

// ---------------------------------------------------------------------------------------------
// Quote / alert / <details>
// ---------------------------------------------------------------------------------------------

#[test]
fn quote_places_the_expression_after_the_bar() {
    let md = "> quote $a$ end\n\n> > deep $b$ end\n\n> - item $c$ end\n";
    let (mut s, _d) = open("mc_quote", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 3, "{placements:?}");
    let expect = ["> quote ", "> > deep ", "> - item "];
    for (p, e) in m.iter().zip(expect) {
        let (before, after) = around(&lines, p);
        assert_eq!(before, e, "{p:?}");
        assert_eq!(after, " end");
    }
}

#[test]
fn alert_and_open_details_bodies_place_the_expression() {
    let md = "> [!NOTE]\n> alert $a$ body\n\n<details open>\n<summary>s</summary>\n\ndetails $b$ body\n\n</details>\n";
    let (mut s, _d) = open("mc_alert", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 2, "{placements:?}");
    let (b0, a0) = around(&lines, &m[0]);
    assert!(b0.ends_with("alert "), "{b0:?}");
    assert_eq!(a0, " body");
    let (b1, a1) = around(&lines, &m[1]);
    assert!(b1.ends_with("details "), "{b1:?}");
    assert_eq!(a1, " body");
}

#[test]
fn closed_details_body_is_never_asked_for_and_never_drawn() {
    let md = "<details>\n<summary>s</summary>\n\nsecret $zz$ body\n\n</details>\n\nopen $a$ end\n";
    let (mut s, _d) = open("mc_closed", md);
    let (_, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(
        m.len(),
        1,
        "only the expression outside the closed block: {placements:?}"
    );
    assert_eq!(m[0].alt, "a");
    s.dont_see("secret");
}

// ---------------------------------------------------------------------------------------------
// Table cells
// ---------------------------------------------------------------------------------------------

#[test]
fn table_cells_hold_the_expression_in_the_text_with_the_box_intact() {
    let md = "| 名前 | 式 | 中 |\n|:--|--:|:-:|\n| a | $x_i$ | $y$ |\n| 日本 $y$ z | **b $q$** | $a$ and $b$ |\n";
    let (mut s, _d) = open("mc_table", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 6, "{placements:?}");
    assert!(m.iter().all(|p| p.rows == 1));
    assert_box_intact(&lines);
    let t = |p: &ImagePlacement| plain(&lines[p.line]);
    // Left-aligned column, a CJK word and a space before the reservation: 2 (bar+space) + 4 + 1.
    let by_alt = |alt: &str| m.iter().find(|p| p.alt == alt).unwrap().clone();
    let y = m
        .iter()
        .find(|p| p.alt == "y" && t(p).contains("日本"))
        .unwrap();
    let (before, after) = around(&lines, y);
    assert_eq!(before, "│ 日本 ");
    assert!(after.starts_with(" z "), "{after:?}");
    // Right-aligned column: the reservation is flush against the cell's right edge.
    let xi = by_alt("x_i");
    let (before, after) = around(&lines, &xi);
    assert!(
        before.starts_with("│ a ") && before.ends_with(' '),
        "{before:?}"
    );
    assert!(after.starts_with(" │"), "{after:?}");
    // Two expressions in one centered cell keep " and " between them.
    let (a, b) = (by_alt("a"), by_alt("b"));
    assert_eq!(a.line, b.line);
    assert_eq!(
        text_between_cols(&lines[a.line], a.col + a.cols, b.col),
        " and "
    );
    // Inside bold: the words around it are bold, the reservation is not text.
    let q = by_alt("q");
    let (before, _) = around(&lines, &q);
    assert!(before.ends_with("b "), "{before:?}");
}

#[test]
fn table_header_row_and_alignment_variants_all_stay_aligned() {
    for align in ["left", "center", "right"] {
        let md = "| $h_1$ | head |\n|--|--|\n| $c$ | tail $d$ |\n";
        let mut cfg = Config::default();
        cfg.ui.md_table_align = align.into();
        let (mut s, _d) = open_cfg(&format!("mc_talign_{align}"), md, 90, cfg);
        let (lines, placements) = view(&mut s);
        let m = math_placements(&placements);
        assert_eq!(m.len(), 3, "{align}: {placements:?}");
        assert_box_intact(&lines);
        for p in &m {
            around(&lines, p);
        }
    }
}

#[test]
fn table_expression_with_escaped_pipe_star_and_underscore_is_kept_whole() {
    let md = "| a | b |\n|--|--|\n| $a \\| b$ | $a*b*c$ |\n| $x_i y_j$ | [l](u.md) $z$ |\n";
    let (mut s, _d) = open("mc_table_special", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    let alts: Vec<&str> = m.iter().map(|p| p.alt.as_str()).collect();
    assert_eq!(alts, ["a | b", "a*b*c", "x_i y_j", "z"], "{placements:?}");
    assert_box_intact(&lines);
    for p in &m {
        around(&lines, p);
    }
    // The cell's link survives next to an expression.
    assert_eq!(s.app.md_link_targets(), vec!["u.md".to_string()]);
}

#[test]
fn table_currency_and_unclosed_dollars_stay_text() {
    let md = "| a | b |\n|--|--|\n| $5 and $10 | cost $ x |\n";
    let (mut s, _d) = open("mc_table_cur", md);
    let (lines, placements) = view(&mut s);
    assert!(math_placements(&placements).is_empty(), "{placements:?}");
    assert!(lines.iter().map(plain).any(|t| t.contains("$5 and $10")));
}

#[test]
fn table_display_math_is_placed_at_in_text_size() {
    let (mut s, _d) = open("mc_table_disp", "| a |\n|--|\n| $$z$$ |\n");
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 1, "{placements:?}");
    assert_eq!(m[0].url, math_url("z", false));
    assert_eq!(m[0].rows, 1);
    around(&lines, &m[0]);
    assert_box_intact(&lines);
}

#[test]
fn table_narrow_column_falls_back_to_the_literal_text_and_wraps() {
    // 34 wide, three columns of a long expression: the columns are shaved below what the picture
    // needs, so it is the literal text again (which wraps) — never a reservation wider than its own
    // column — and the box still closes. The short `$y$` still fits and is drawn.
    let long = "$\\sum_{i=1}^{n} \\frac{a_i}{b_i} + \\sqrt{x^2+y^2}$";
    let md = format!("| a | b | c |\n|--|--|--|\n| {long} | {long} | $y$ |\n");
    let (mut s, _d) = open_cfg("mc_table_narrow", &md, 34, Config::default());
    let (lines, placements) = view(&mut s);
    assert_box_intact(&lines);
    let m = math_placements(&placements);
    for p in &m {
        around(&lines, p);
    }
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("\\sum") || t.contains("$\\s")),
        "a column too narrow for the picture shows the source: {texts:#?}"
    );
    assert!(m.iter().any(|p| p.alt == "y"), "{placements:?}");
    assert!(
        m.len() < 3,
        "the shaved columns' expressions are not drawn: {m:?}"
    );
}

#[test]
fn table_image_and_expression_on_the_same_row_each_keep_their_own_place() {
    let dir = sandbox("mc_table_img");
    image::RgbaImage::from_pixel(40, 40, image::Rgba([200, 30, 30, 255]))
        .save(dir.join("p.png"))
        .unwrap();
    std::fs::write(
        dir.join("d.md"),
        "| pic | f |\n|--|--|\n| ![p](p.png) | $x$ |\n",
    )
    .unwrap();
    let root = canon(&dir);
    let mut s = Sim::with_config_sized(&root, Config::default(), 90, 40).with_picker();
    s.select("d.md");
    s.enter();
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 1, "{placements:?}");
    assert!(
        placements
            .iter()
            .any(|p| p.url == "p.png" || p.url.ends_with("p.png")),
        "{placements:?}"
    );
    // The picture is on the same physical row as the expression, and the sentinel resolution must
    // not have given the expression the picture's column (or the picture the expression's).
    let (before, after) = around(&lines, &m[0]);
    assert!(before.ends_with("│ "), "{before:?}");
    assert!(after.starts_with(" │"), "{after:?}");
    assert_box_intact(&lines);
}

#[test]
fn html_table_cell_holds_the_expression_too() {
    let md = "<table><tr><th>h $a$</th></tr><tr><td>c <b>$b$</b> d</td></tr></table>\n";
    let (mut s, _d) = open("mc_html_table", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 2, "{placements:?}");
    assert_box_intact(&lines);
    for p in &m {
        around(&lines, p);
    }
}

#[test]
fn table_inside_a_quote_is_rebased_and_still_blank_under_the_picture() {
    let md = "> | a | b |\n> |--|--|\n> | $x$ | t $y$ |\n";
    let (mut s, _d) = open("mc_table_quote", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 2, "{placements:?}");
    for p in &m {
        let (before, _) = around(&lines, p);
        assert!(before.starts_with("> │"), "{before:?}");
    }
}

#[test]
fn table_in_alert_and_in_a_list_item() {
    let md = "> [!TIP]\n> | a |\n> |--|\n> | $x$ |\n\n- item\n\n  | a |\n  |--|\n  | $y$ |\n";
    let (mut s, _d) = open("mc_table_alert", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert!(!m.is_empty(), "{placements:?}");
    for p in &m {
        around(&lines, p);
    }
}

// ---------------------------------------------------------------------------------------------
// Without a picture: exactly what these contexts drew before
// ---------------------------------------------------------------------------------------------

#[test]
fn without_an_image_backend_every_context_keeps_the_literal_source() {
    let md = "# Head $h$\n\n| a |\n|--|\n| cell $c$ |\n\n**b $b$**\n\n> q $q$\n";
    let (mut s, _d) = md_preview(Config::default(), "mc_text_mode", md);
    let _ = view(&mut s);
    assert!(math_placements(&s.app.md_images()).is_empty());
    for needle in ["Head $h$", "cell $c$", "b $b$", "q $q$"] {
        s.see(needle);
    }
}

#[test]
fn a_picture_that_is_still_rendering_shows_the_source_not_blank_cells() {
    let dir = sandbox("mc_loading");
    std::fs::write(dir.join("d.md"), "# H $h$\n\n| a |\n|--|\n| $c$ |\n").unwrap();
    let root = canon(&dir);
    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.see("H $h$");
    s.see("$c$");
    // …and once they arrive, they are pictures.
    s.drain_md_images();
    s.drain_md_images();
    let (lines, placements) = view(&mut s);
    assert_eq!(math_placements(&placements).len(), 2, "{placements:?}");
    for p in math_placements(&placements) {
        around(&lines, &p);
    }
}

// ---------------------------------------------------------------------------------------------
// What gets rendered: the extraction the app starts renders from
// ---------------------------------------------------------------------------------------------

#[test]
fn extraction_finds_expressions_in_cells_headings_emphasis_and_quotes() {
    let md =
        "# H $h$\n\n| a |\n|--|\n| $c$ and $$d$$ |\n\n**b $b$**\n\n> q $q$\n\n[l $no$](u.md)\n";
    let found = collect_math_exprs(md);
    let got: Vec<(&str, bool)> = found.iter().map(|(l, d)| (l.as_str(), *d)).collect();
    // Document order; all at inline size (`false`), including the `$$d$$` in a cell; the one inside a
    // link label is never asked for (it stays text).
    assert_eq!(
        got,
        [
            ("h", false),
            ("c", false),
            ("d", false),
            ("b", false),
            ("q", false)
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// A heading the expression wraps; the width a row has once its links are folded
// ---------------------------------------------------------------------------------------------

#[test]
fn a_heading_wrapped_by_an_expression_keeps_picture_rule_and_style_on_the_right_rows() {
    // `Energy ` and the 8-cell expression do not fit one 14-cell row: the heading goes on over two
    // rows. The rule of an H1/H2 belongs under the *last* of them, the expression's picture is on the
    // second one, and both rows are the heading's (colour, outline, anchor).
    for level in 1..=6usize {
        let md = format!(
            "{} Energy $E=mc^2$ rule\n\n[go](#energy-emc2-rule)\n",
            "#".repeat(level)
        );
        let (mut s, _d) = open_cfg(&format!("mc_wrap_h{level}"), &md, 16, Config::default());
        let (lines, placements) = view(&mut s);
        let m = math_placements(&placements);
        assert_eq!(m.len(), 1, "h{level}: {placements:?}");
        let rows: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(
            m[0].line, 1,
            "h{level}: the picture is on the second row: {rows:?}"
        );
        let (before, after) = around(&lines, &m[0]);
        assert_eq!(before, "", "h{level}: {rows:?}");
        assert_eq!(after, " rule", "h{level}");
        assert_eq!(rows[0].trim_end(), "Energy", "h{level}: {rows:?}");
        for row in [0, 1] {
            assert_eq!(
                lines[row].style.fg,
                Some(Color::Cyan),
                "h{level}: row {row} keeps the heading colour: {rows:?}"
            );
        }
        if level <= 2 {
            let ch = if level == 1 { '━' } else { '─' };
            assert!(
                rows[2].chars().all(|c| c == ch) && !rows[2].is_empty(),
                "h{level}: the rule is right under the heading's last row: {rows:?}"
            );
        } else {
            assert!(
                !rows[2].contains('━') && !rows[2].contains('─'),
                "h{level}: {rows:?}"
            );
        }
        // One heading, not two: one outline entry holding the whole text, and its anchor resolves.
        let outline = s.app.md_outline();
        assert_eq!(outline.len(), 1, "h{level}: {outline:?}");
        assert_eq!(outline[0].1, "Energy $E=mc^2$ rule", "h{level}");
        assert_eq!(
            outline[0].2, 0,
            "h{level}: the outline points at the first row"
        );
        assert_eq!(
            outline[0].0,
            level.min(4) as u8,
            "h{level}: the level is read from the rule under the *last* row"
        );
        s.tab();
        s.enter();
        assert!(s.app.flash.is_none(), "h{level}: {:?}", s.app.flash);
    }
}

#[test]
fn a_heading_wrapped_several_times_puts_every_picture_on_its_own_heading_row() {
    let md = "## One $a$ two $b$ three $c$ four $d$ five\n\nafter\n";
    let (mut s, _d) = open_cfg("mc_wrap_many", md, 14, Config::default());
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 4, "{placements:?}");
    let rows: Vec<String> = lines.iter().map(plain).collect();
    for p in &m {
        around(&lines, p);
        assert!(
            crate::preview::markdown::is_heading_continuation(&lines[p.line]) || p.line == 0,
            "{rows:?}"
        );
        assert_eq!(lines[p.line].style.fg, Some(Color::Cyan), "{rows:?}");
    }
    // The rule is below the row of the last picture (and of its trailing text).
    let last = m.iter().map(|p| p.line).max().unwrap();
    assert!(rows[last + 1].starts_with('─'), "{rows:?}");
    assert_eq!(
        rows.iter().filter(|r| r.starts_with('─')).count(),
        1,
        "{rows:?}"
    );
    let outline = s.app.md_outline();
    assert_eq!(outline.len(), 1, "{outline:?}");
    assert_eq!(outline[0].1, "One $a$ two $b$ three $c$ four $d$ five");
}

#[test]
fn a_heading_after_a_wrapped_one_is_still_placed_on_its_own_row() {
    // The rule of the first heading comes after its *last* row; every later placement is counted
    // against exactly one rule.
    let md = "# Energy $E=mc^2$ rule\n\n## Next $a$ end\n\npara $b$ end\n";
    let (mut s, _d) = open_cfg("mc_wrap_next", md, 16, Config::default());
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 3, "{placements:?}");
    let expect = [("", " rule"), ("Next ", " end"), ("para ", " end")];
    for (p, (b, a)) in m.iter().zip(expect) {
        let (before, after) = around(&lines, p);
        assert_eq!((before.as_str(), after.as_str()), (b, a), "{p:?}");
    }
}

#[test]
fn a_link_before_an_expression_does_not_count_its_folded_url_towards_the_row() {
    // The row holds `label (URL)` while the document is rendered and `label` once the link is
    // folded. The expression fits in what the folded row leaves, so it stays on the row — in a
    // heading and in a paragraph alike.
    let url =
        "https://example.com/a/very/long/path/that/goes/on/and/on/and/on/and/on/and/on/forever/x";
    let md = format!(
        "# Heading with [a link]({url}) and $y$ tail\n\nPara with [a link]({url}) and $y$ tail\n"
    );
    let (mut s, _d) = open_cfg("mc_fold_width", &md, 102, Config::default());
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 2, "{placements:?}");
    let rows: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(m[0].line, 0, "the heading is one row: {rows:?}");
    assert!(
        rows[1].starts_with('━'),
        "the rule is right under it: {rows:?}"
    );
    for p in &m {
        let (before, after) = around(&lines, p);
        assert!(before.ends_with("and "), "{before:?} {rows:?}");
        assert_eq!(after, " tail");
    }
}

#[test]
fn an_expression_that_does_not_fit_the_folded_row_still_wraps() {
    // The other side of the test above: the folded row is measured, not ignored.
    let md = "Para with [a link](https://example.com/x) and then some more words $y$ tail\n";
    let (mut s, _d) = open_cfg("mc_fold_wrap", md, 34, Config::default());
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 1, "{placements:?}");
    let (before, after) = around(&lines, &m[0]);
    assert_eq!(
        before,
        "",
        "{:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert_eq!(after, " tail");
}

// ---------------------------------------------------------------------------------------------
// What an expression is: the source between its delimiters, in every context
// ---------------------------------------------------------------------------------------------

/// Expressions whose source holds what CommonMark would read as an escape or as emphasis.
const SOURCE_EXPRS: [&str; 9] = [
    r"\{a,b\}",
    r"a\,b",
    r"a\\b",
    r"\%",
    r"x\_i",
    "a*b*c",
    "f^*(x)+g^*(y)",
    "x_i*y_j",
    r"\left\{a,b\right\}",
];

/// One document per context, the expression written between the same words.
fn contexts(e: &str) -> Vec<(&'static str, String)> {
    vec![
        ("h1", format!("# h ${e}$ end\n")),
        ("h3", format!("### h ${e}$ end\n")),
        ("bold", format!("**b ${e}$ end**\n")),
        ("quote", format!("> q ${e}$ end\n")),
        ("nested quote", format!("> > q ${e}$ end\n")),
        ("alert", format!("> [!NOTE]\n> a ${e}$ end\n")),
        (
            "details",
            format!("<details open>\n<summary>s</summary>\n\nd ${e}$ end\n\n</details>\n"),
        ),
        ("list", format!("- l ${e}$ end\n")),
        ("loose list", format!("- l ${e}$ end\n\n- m\n")),
        ("task", format!("- [ ] t ${e}$ end\n")),
        ("paragraph", format!("p ${e}$ end\n")),
        ("table", format!("| a |\n|--|\n| c ${e}$ end |\n")),
    ]
}

#[test]
fn an_expression_is_its_source_in_every_context() {
    // Heading, bold, quote, alert, `<details>`, list item, paragraph and table cell alike: the
    // LaTeX handed to the renderer is exactly what is between the `$`s — backslashes and stars
    // included. (Contexts used to read it from the text after CommonMark had run, so `\{` lost its
    // backslash and `a*b*c` became `abc`.)
    for (i, e) in SOURCE_EXPRS.iter().enumerate() {
        for (name, md) in contexts(e) {
            let (mut s, _d) = open(&format!("mc_src_{i}_{}", name.replace(' ', "_")), &md);
            let (lines, placements) = view(&mut s);
            let m = math_placements(&placements);
            let rows: Vec<String> = lines.iter().map(plain).collect();
            assert_eq!(m.len(), 1, "{name} {e}: {placements:?} {rows:?}");
            assert_eq!(m[0].alt, *e, "{name}: {rows:?}");
            let (_, after) = around(&lines, &m[0]);
            assert!(after.starts_with(" end"), "{name} {e}: {after:?} {rows:?}");
            assert!(
                rows.iter().all(|r| !r.contains('$')),
                "{name} {e}: no source left on screen: {rows:?}"
            );
        }
    }
}

#[test]
fn an_expression_with_delimiters_other_than_dollars_is_cut_the_same_way() {
    let md = "# h \\(x^2\\) and $$z$$ end\n\n**b \\[y_1\\] end**\n\n> q \\(a\\,b\\) end\n";
    let (mut s, _d) = open("mc_src_delims", md);
    let (lines, placements) = view(&mut s);
    let alts: Vec<String> = math_placements(&placements)
        .iter()
        .map(|p| p.alt.clone())
        .collect();
    assert_eq!(alts, ["x^2", "z", "y_1", r"a\,b"], "{placements:?}");
    for p in math_placements(&placements) {
        around(&lines, &p);
    }
}

#[test]
fn text_around_an_expression_keeps_its_entities_escapes_and_cjk() {
    let md = "# a &amp; $x$ &lt; \\*b\\* と $y$ です\n";
    let (mut s, _d) = open("mc_src_text", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 2, "{placements:?}");
    let (before, _) = around(&lines, &m[0]);
    assert!(before.ends_with("a & "), "{before:?}");
    let (before, after) = around(&lines, &m[1]);
    assert!(before.ends_with("< *b* と "), "{before:?}");
    assert_eq!(after, " です");
}

#[test]
fn an_expression_the_events_cannot_hold_whole_stays_literal() {
    // `*a $b* c$`: the emphasis closes inside the expression, so the expression cannot be cut out
    // without unbalancing it. Left as the text it was — never half cut.
    for md in ["# *a $b* c$ d\n", "> *a $b* c$ d\n", "**a $b** c$ d\n"] {
        let (mut s, _d) = open("mc_src_straddle", md);
        let (lines, placements) = view(&mut s);
        let rows: Vec<String> = lines.iter().map(plain).collect();
        assert!(math_placements(&placements).is_empty(), "{md:?}: {rows:?}");
        assert!(rows.iter().any(|r| r.contains('d')), "{md:?}: {rows:?}");
    }
}

#[test]
fn an_expression_holding_a_link_is_math_and_one_inside_a_link_label_is_not() {
    let md = "# h $[x](https://e.example/a)$ end\n\n# [l $y$ m](https://e.example/b)\n";
    let (mut s, _d) = open("mc_src_link", md);
    let (lines, placements) = view(&mut s);
    let m = math_placements(&placements);
    assert_eq!(m.len(), 1, "{placements:?}");
    assert_eq!(m[0].alt, "[x](https://e.example/a)");
    around(&lines, &m[0]);
    s.see("l $y$ m");
}

#[test]
fn currency_and_unclosed_dollars_stay_text_in_every_context() {
    for text in [
        "pay $5 or $10 now",
        "$3 to $4",
        "a $ b and $ c",
        "cost $5 and $x",
        "ends with a dollar$",
    ] {
        for (name, md) in [
            ("h1", format!("# {text}\n")),
            ("bold", format!("**{text}**\n")),
            ("quote", format!("> {text}\n")),
            ("list", format!("- {text}\n")),
            ("table", format!("| a |\n|--|\n| {text} |\n")),
        ] {
            let (mut s, _d) = open("mc_src_cur", &md);
            let (lines, placements) = view(&mut s);
            let rows: Vec<String> = lines.iter().map(plain).collect();
            assert!(
                math_placements(&placements).is_empty(),
                "{name} {text:?}: {rows:?}"
            );
            assert!(
                rows.iter().any(|r| r.contains(text)),
                "{name} {text:?}: {rows:?}"
            );
        }
    }
}

#[test]
fn an_expression_that_is_not_drawn_yet_keeps_its_markup_literal() {
    // The picture is still rendering (`Loading`): the cell, the heading and the bold run show the
    // source — what is between the `$`s is never link or bold markup of the surrounding text.
    let dir = sandbox("mc_lit_loading");
    let md = "| a |\n|--|\n| $[x](https://evil.example/a)$ and $**b**$ and $\\{c\\}$ |\n\n# H $[x](https://evil.example/a)$ and $a*b*c$\n\n**s $**b**$ t**\n";
    std::fs::write(dir.join("d.md"), md).unwrap();
    let root = canon(&dir);
    let mut s = Sim::new(&root).with_media();
    s.select("d.md");
    s.enter();
    s.see("$[x](https://evil.example/a)$ and $**b**$ and $\\{c\\}$");
    s.see("H $[x](https://evil.example/a)$ and $a*b*c$");
    s.see("s $**b**$ t");
    let (_, _) = view(&mut s);
    // No link was made of any of it. (The bare URL inside the source is still autolinked, as it is
    // in any text — but with the `)$` that follows it, not as the destination of `[x](…)`.)
    let targets = s.app.md_link_targets();
    assert!(
        !targets.iter().any(|t| t == "https://evil.example/a"),
        "{targets:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The renderer on its own, with a stub slot
// ---------------------------------------------------------------------------------------------

mod stub {
    use crate::preview::markdown::{
        render_markdown_with_images, ImagePlacement, ImageSlot, MathSlot, MermaidSlot,
    };

    fn render(
        md: &str,
        width: u16,
        math: &dyn Fn(&str, bool) -> MathSlot,
        on: bool,
    ) -> (Vec<String>, Vec<ImagePlacement>) {
        render_icons(md, width, math, on, false)
    }

    fn render_icons(
        md: &str,
        width: u16,
        math: &dyn Fn(&str, bool) -> MathSlot,
        on: bool,
        icons: bool,
    ) -> (Vec<String>, Vec<ImagePlacement>) {
        let slot_of = |_: &str, _: Option<u16>| ImageSlot::Unavailable;
        let ms = |_: &str| MermaidSlot::Text;
        let (lines, pl, _) = render_markdown_with_images(
            md,
            width,
            Default::default(),
            "TwoDark",
            icons,
            &[' ', 'x'],
            &slot_of,
            &ms,
            "mermaid",
            true,
            math,
            on,
        );
        let text = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        (text, pl)
    }

    fn image(cols: u16) -> impl Fn(&str, bool) -> MathSlot {
        move |_, _| MathSlot::Image { cols, rows: 1 }
    }

    #[test]
    fn heading_cell_emphasis_and_quote_each_record_one_placement_at_the_right_column() {
        let md = "# Head $h$ x\n\n| a | b |\n|--|--|\n| $c$ | y |\n\n**s $e$ t**\n\n> q $f$ r\n";
        let (text, pl) = render(md, 60, &image(3), true);
        assert_eq!(pl.len(), 4, "{pl:?}");
        let cols: Vec<(String, usize, u16)> =
            pl.iter().map(|p| (p.alt.clone(), p.line, p.col)).collect();
        // Heading: after "Head " (decoration has already stripped the `# ` marker).
        assert_eq!(cols[0].0, "h");
        assert_eq!(text[cols[0].1], "Head     x");
        // The recorded column is provisional — measured before decoration, so it still counts the
        // `# ` marker (2) — and `App::resolve_inline_math_cols` corrects it (tested above).
        assert_eq!(cols[0].2, 7);
        // Cell: the bar and one space, then the 3 reserved cells.
        assert_eq!(cols[1].0, "c");
        assert_eq!(text[cols[1].1], "│     │ y │");
        assert_eq!(cols[1].2, 2);
        // Bold run and quote.
        assert_eq!(text[cols[2].1], "s     t");
        assert_eq!(cols[2].2, 2);
        assert_eq!(text[cols[3].1], "> q     r");
        assert_eq!(cols[3].2, 4);
    }

    #[test]
    fn nothing_is_placed_when_math_is_off_or_not_drawable() {
        let md = "# Head $h$\n\n| a |\n|--|\n| $c$ |\n\n**s $e$**\n\n> q $f$\n";
        let (base, none) = render(md, 60, &image(3), false);
        assert!(none.is_empty());
        for slot in [MathSlot::Loading, MathSlot::Raw] {
            let s = slot.clone();
            let (text, pl) = render(md, 60, &move |_, _| s.clone(), true);
            assert!(pl.is_empty(), "{slot:?}");
            assert_eq!(
                text, base,
                "{slot:?}: the literal source, byte for byte what math-off draws"
            );
        }
    }

    #[test]
    fn a_tall_slot_is_not_placed_in_text() {
        // rows != 1 cannot sit in a line: it stays text (display size is never asked for here).
        let md = "# Head $h$\n\n| a |\n|--|\n| $c$ |\n";
        let (text, pl) = render(md, 60, &|_, _| MathSlot::Image { cols: 3, rows: 2 }, true);
        assert!(pl.is_empty());
        assert!(text.iter().any(|l| l.contains("$h$")) && text.iter().any(|l| l.contains("$c$")));
    }

    #[test]
    fn an_expression_wider_than_the_line_stays_text() {
        let (text, pl) = render("# H $h$\n", 10, &image(30), true);
        assert!(pl.is_empty());
        assert!(text.iter().any(|l| l.contains("$h$")));
    }

    /// The row holding `needle`, and the row the (first) placement is on.
    fn rows_apart(text: &[String], pl: &[ImagePlacement], needle: &str) -> (usize, usize) {
        let at = text
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not in {text:?}"));
        (at, pl[0].line)
    }

    #[test]
    fn a_body_wraps_an_expression_at_its_own_narrower_width() {
        // 20 cells wide; a quote, an alert or a `<details>` body leaves 18 after its bar (16 for a quote in
        // a quote). `k` letters, a space and the 3-cell expression fit while `k + 4 <= inner`.
        type Shape = fn(&str) -> String;
        let shapes: [(&str, Shape, u16); 4] = [
            ("quote", |t| format!("> {t} $h$ r\n"), 18),
            ("nested quote", |t| format!("> > {t} $h$ r\n"), 16),
            ("alert", |t| format!("> [!NOTE]\n> {t} $h$ r\n"), 18),
            (
                "details",
                |t| format!("<details open>\n<summary>s</summary>\n\n{t} $h$ r\n\n</details>\n"),
                18,
            ),
        ];
        for (name, shape, inner) in shapes {
            let fits = "a".repeat(inner as usize - 4);
            let (text, pl) = render(&shape(&fits), 20, &image(3), true);
            assert_eq!(pl.len(), 1, "{name}: {pl:?}");
            let (at, on) = rows_apart(&text, &pl, &fits);
            assert_eq!(at, on, "{name}: exactly fitting stays on the row: {text:?}");
            let wraps = "a".repeat(inner as usize - 3);
            let (text, pl) = render(&shape(&wraps), 20, &image(3), true);
            let (at, on) = rows_apart(&text, &pl, &wraps);
            assert_eq!(on, at + 1, "{name}: one cell too many wraps: {text:?}");
        }
    }

    #[test]
    fn a_link_in_a_cell_or_heading_keeps_a_dollar_pair_in_its_label_and_alt_text() {
        // The text of a label is never math: it comes back as written, `$` pairs and all.
        let md = "| a |\n|--|\n| [x $h$ y](u.md) and ![p $h$ q](i.png) |\n";
        for slot in [
            MathSlot::Image { cols: 3, rows: 1 },
            MathSlot::Loading,
            MathSlot::Raw,
        ] {
            let s = slot.clone();
            let (text, pl) = render(md, 60, &move |_, _| s.clone(), true);
            let cell = text.iter().find(|l| l.contains("and")).unwrap();
            assert!(cell.contains("x $h$ y"), "{slot:?}: {text:?}");
            assert!(cell.contains("p $h$ q"), "{slot:?}: {text:?}");
            // The two expressions inside labels are the only ones; none of them is drawn.
            assert!(pl.iter().all(|p| p.alt != "h"), "{slot:?}: {pl:?}");
        }
    }

    #[test]
    fn display_math_that_is_not_drawn_in_text_keeps_both_dollar_pairs() {
        let md = "# H $$z$$ e\n\n**b $$z$$ e**\n\n> q $$z$$ e\n\n| a |\n|--|\n| c $$z$$ e |\n";
        for slot in [
            MathSlot::Loading,
            MathSlot::Raw,
            MathSlot::Image { cols: 3, rows: 2 },
            MathSlot::Image { cols: 90, rows: 1 },
        ] {
            let s = slot.clone();
            let (text, pl) = render(md, 60, &move |_, _| s.clone(), true);
            assert!(pl.is_empty(), "{slot:?}: {pl:?}");
            for lead in ["H ", "b ", "q ", "c "] {
                let row = text.iter().find(|l| l.contains(lead)).unwrap();
                assert!(row.contains("$$z$$ e"), "{slot:?} {lead:?}: {text:?}");
            }
        }
    }

    #[test]
    fn an_expression_alone_in_a_block_leaves_the_same_blank_rows_as_a_word_would() {
        // An expression that is a block's only content writes no text, so it has to clear the owed
        // blank row itself — or a nested block after it gets a blank row the same document with a
        // word in its place does not have.
        for shape in [
            "intro\n\n- {c}\n  - child\n\nafter\n",
            "intro\n\n1. {c}\n   - child\n",
            "intro\n\n- [ ] {c}\n  - child\n",
            "intro\n\n> {c}\n> - child\n",
            "intro\n\n- **{c}**\n  - child\n",
            "intro\n\n- {c}\n\n  - child\n",
        ] {
            let blanks = |c: &str| -> Vec<usize> {
                let (text, _) = render(&shape.replace("{c}", c), 40, &image(3), true);
                text.iter()
                    .enumerate()
                    .filter(|(_, l)| l.is_empty())
                    .map(|(i, _)| i)
                    .collect()
            };
            assert_eq!(blanks("$h$"), blanks("hhh"), "{shape:?}");
        }
    }

    #[test]
    fn markup_inside_an_expression_that_is_not_drawn_is_never_markup() {
        let md = "| a |\n|--|\n| $[x](https://evil.example/a)$ and **$**b**$** and $\\{c\\}$ |\n\n# H $[x](https://evil.example/a)$ and $**b**$\n";
        for slot in [
            MathSlot::Loading,
            MathSlot::Raw,
            MathSlot::Image { cols: 3, rows: 2 },
        ] {
            let s = slot.clone();
            let (text, _) = render(md, 80, &move |_, _| s.clone(), true);
            let cell = text.iter().find(|l| l.contains("and")).unwrap();
            assert!(
                cell.contains("$[x](https://evil.example/a)$ and $**b**$ and $\\{c\\}$")
                    || cell.contains("$[x](https://evil.example/a)$ and $**b**$"),
                "{slot:?}: {text:?}"
            );
            assert!(cell.contains("$\\{c\\}$"), "{slot:?}: {text:?}");
            let head = text.iter().find(|l| l.contains("H ")).unwrap();
            assert!(
                head.contains("$[x](https://evil.example/a)$ and $**b**$"),
                "{slot:?}: {text:?}"
            );
        }
    }

    #[test]
    fn a_row_is_measured_with_its_links_folded_up_to_the_icon() {
        // The row `Para [l](URL) …` is `Para l …` once the link is folded (`l` with the link icon and a
        // space in front of it with `ui.icons`). `n` letters after it: the 3-cell expression fits the
        // 30-cell row while the folded row is at most 27 wide.
        let url = "https://example.com/some/long/path";
        for icons in [false, true] {
            // folded width = "Para " (5) + label (1) [+ icon and space (2)] + " " + n letters + " "
            let line_with = |n: usize| format!("Para [l]({url}) {} $h$ r\n", "x".repeat(n));
            let fold = if icons { 2 } else { 0 };
            let fits = 30 - 3 - (5 + 1 + fold + 1 + 1);
            let (text, pl) = render_icons(&line_with(fits), 30, &image(3), true, icons);
            let (at, on) = rows_apart(&text, &pl, "Para");
            assert_eq!(at, on, "icons={icons}: fits exactly: {text:?}");
            let (text, pl) = render_icons(&line_with(fits + 1), 30, &image(3), true, icons);
            let (at, on) = rows_apart(&text, &pl, "Para");
            assert_eq!(on, at + 1, "icons={icons}: one cell more wraps: {text:?}");
        }
    }
}
