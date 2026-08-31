//! Broad config-parsing coverage (defaults, TOML round-trips, permissive fallbacks). Authored as a
//! batch across several agents and integrated here, kept separate from the inline `mod tests` so the
//! two do not collide. Every assertion is against the real behavior read from `config/mod.rs`.
#![cfg(test)]
// `let mut x = T::default(); x.field = ...` is the idiomatic setup in these tests.
#![allow(clippy::field_reassign_with_default)]

use super::*;
use crate::test_support::unique_tmp;
use std::collections::HashMap;
use std::io::Write as _;

/// Local copy of the `mod tests` temp-file helper (that one is private to the other module).
fn tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    // Preserve `name`'s extension — see the identical comment on `config::tests::tmp`: these tests
    // exercise extension-based preview-rule matching, so the uniqueness suffix must land before the
    // extension, not swallow it.
    let stem = std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let mut p = unique_tmp(&format!("konoma_cfgparity_{stem}"));
    if let Some(ext) = ext {
        p = p.with_extension(ext);
    }
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(bytes).unwrap();
    p
}

// =============================================================================
// UiConfig scalar fields: default + parse + verbatim string storage
// =============================================================================

#[test]
fn cfg_ui_show_hidden_default_and_parse() {
    assert!(!toml::from_str::<Config>("[ui]\n").unwrap().ui.show_hidden);
    assert!(
        toml::from_str::<Config>("[ui]\nshow_hidden = true\n")
            .unwrap()
            .ui
            .show_hidden
    );
}

#[test]
fn cfg_ui_icons_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.icons);
    assert!(
        !toml::from_str::<Config>("[ui]\nicons = false\n")
            .unwrap()
            .ui
            .icons
    );
}

#[test]
fn cfg_ui_wrap_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.wrap);
    assert!(
        !toml::from_str::<Config>("[ui]\nwrap = false\n")
            .unwrap()
            .ui
            .wrap
    );
}

#[test]
fn cfg_ui_line_numbers_default_and_parse() {
    assert!(!toml::from_str::<Config>("[ui]\n").unwrap().ui.line_numbers);
    assert!(
        toml::from_str::<Config>("[ui]\nline_numbers = true\n")
            .unwrap()
            .ui
            .line_numbers
    );
}

#[test]
fn cfg_ui_git_gutter_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.git_gutter);
    assert!(
        !toml::from_str::<Config>("[ui]\ngit_gutter = false\n")
            .unwrap()
            .ui
            .git_gutter
    );
}

#[test]
fn cfg_ui_syntax_highlight_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .syntax_highlight
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nsyntax_highlight = false\n")
            .unwrap()
            .ui
            .syntax_highlight
    );
}

#[test]
fn cfg_ui_confirm_quit_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.confirm_quit);
    assert!(
        !toml::from_str::<Config>("[ui]\nconfirm_quit = false\n")
            .unwrap()
            .ui
            .confirm_quit
    );
}

#[test]
fn cfg_ui_confirm_bookmark_overwrite_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .confirm_bookmark_overwrite
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nconfirm_bookmark_overwrite = false\n")
            .unwrap()
            .ui
            .confirm_bookmark_overwrite
    );
}

#[test]
fn cfg_ui_confirm_jj_sync_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .confirm_jj_sync
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nconfirm_jj_sync = false\n")
            .unwrap()
            .ui
            .confirm_jj_sync
    );
}

#[test]
fn cfg_ui_csv_rainbow_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.csv_rainbow);
    assert!(
        !toml::from_str::<Config>("[ui]\ncsv_rainbow = false\n")
            .unwrap()
            .ui
            .csv_rainbow
    );
}

#[test]
fn cfg_ui_busy_indicator_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .busy_indicator
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nbusy_indicator = false\n")
            .unwrap()
            .ui
            .busy_indicator
    );
}

#[test]
fn cfg_ui_md_autolink_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.md_autolink);
    assert!(
        !toml::from_str::<Config>("[ui]\nmd_autolink = false\n")
            .unwrap()
            .ui
            .md_autolink
    );
}

#[test]
fn cfg_ui_md_alerts_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.md_alerts);
    assert!(
        !toml::from_str::<Config>("[ui]\nmd_alerts = false\n")
            .unwrap()
            .ui
            .md_alerts
    );
}

#[test]
fn cfg_ui_md_emoji_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.md_emoji);
    assert!(
        !toml::from_str::<Config>("[ui]\nmd_emoji = false\n")
            .unwrap()
            .ui
            .md_emoji
    );
}

#[test]
fn cfg_ui_md_frontmatter_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .md_frontmatter
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nmd_frontmatter = false\n")
            .unwrap()
            .ui
            .md_frontmatter
    );
}

#[test]
fn cfg_ui_md_footnotes_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.md_footnotes);
    assert!(
        !toml::from_str::<Config>("[ui]\nmd_footnotes = false\n")
            .unwrap()
            .ui
            .md_footnotes
    );
}

#[test]
fn cfg_ui_md_inline_html_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .md_inline_html
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nmd_inline_html = false\n")
            .unwrap()
            .ui
            .md_inline_html
    );
}

#[test]
fn cfg_ui_restore_tabs_default_and_parse() {
    assert!(toml::from_str::<Config>("[ui]\n").unwrap().ui.restore_tabs);
    assert!(
        !toml::from_str::<Config>("[ui]\nrestore_tabs = false\n")
            .unwrap()
            .ui
            .restore_tabs
    );
}

/// `restore_single_tab`: default true (a lone-tab session is still persisted/restored). Had zero
/// TOML-layer coverage before this — only the `session_actions` unit tests (App::new-level, not
/// TOML parsing) exercised it.
#[test]
fn cfg_ui_restore_single_tab_default_and_parse() {
    assert!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .restore_single_tab
    );
    assert!(
        !toml::from_str::<Config>("[ui]\nrestore_single_tab = false\n")
            .unwrap()
            .ui
            .restore_single_tab
    );
}

/// `md_details`: default `"auto"`; `"open"`/`"closed"` (and any other string) are stored verbatim —
/// `App::details_default_open` (app/md_render.rs, not this parsing layer) is the permissive
/// resolver, matching `filter_mode`'s split between raw storage here and interpretation there. Had
/// zero coverage in this file before this (e2e's `e2e_ui_md_details_config_forces_open_despite_
/// missing_attribute` only covered `"open"`, not the field's own TOML parse/default).
#[test]
fn cfg_ui_md_details_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.md_details,
        "auto"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmd_details = \"open\"\n")
            .unwrap()
            .ui
            .md_details,
        "open"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmd_details = \"closed\"\n")
            .unwrap()
            .ui
            .md_details,
        "closed"
    );
    // Unrecognized value stored verbatim, same convention as every other mode string in this file.
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmd_details = \"sideways\"\n")
            .unwrap()
            .ui
            .md_details,
        "sideways"
    );
}

/// The two shipped example configs must actually parse as a `Config` — they are what users copy to
/// `~/.config/konoma/config.toml`, so a typo in one is a typo in every new install. Read from disk
/// rather than `include_str!`: `config.example.ja.toml` is excluded from the published crate
/// (`Cargo.toml`), so a build from crates.io skips it instead of failing to compile, exactly the way
/// the `samples/` fixtures do.
#[test]
fn shipped_example_configs_parse() {
    for name in ["config.example.toml", "config.example.ja.toml"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("skipping {name} — not present (published-crate build?)");
            continue;
        };
        let cfg: Config = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("{name} が Config としてパースできない: {e}"));
        // Spot-check that the file's own uncommented values really landed (i.e. the parse was not
        // vacuous because every key happened to be commented out).
        assert_eq!(cfg.ui.md_table_align, "left", "{name}: [ui] md_table_align");
        assert_eq!(
            cfg.ui.md_image_align, "center",
            "{name}: [ui] md_image_align"
        );
        assert_eq!(
            cfg.ui.md_block_aligns(),
            crate::preview::markdown::BlockAligns::default(),
            "{name}: 例の既定値が konoma の既定と一致しない"
        );
        // Checklist A5 (mermaid curve test audit, 2026-08-29): the shipped `mermaid_curve = "basis"`
        // is not just a string that happens to land on the field — it has to actually resolve, via
        // `Curve::parse`, to the enum variant every non-flowchart caller and every corpus golden
        // assumes. A plain string-equality check (as `shipped_example_configs_parse`'s siblings do
        // for other keys) would not catch a `Curve::parse` match arm drifting away from this exact
        // spelling; going through the real parser does.
        assert_eq!(cfg.ui.mermaid_curve, "basis", "{name}: [ui] mermaid_curve");
        assert_eq!(
            crate::preview::mermaid::render::edges::Curve::parse(&cfg.ui.mermaid_curve),
            crate::preview::mermaid::render::edges::Curve::Basis,
            "{name}: [ui] mermaid_curve's shipped value must actually resolve to Curve::Basis"
        );
    }
}

#[test]
fn cfg_ui_md_table_align_and_md_image_align_default_parse_and_resolve() {
    use crate::preview::markdown::{BlockAlign, BlockAligns};
    // Defaults are konoma's own historical layout, and the resolver agrees with them.
    let ui = toml::from_str::<Config>("[ui]\n").unwrap().ui;
    assert_eq!(ui.md_table_align, "left");
    assert_eq!(ui.md_image_align, "center");
    assert_eq!(ui.md_block_aligns(), BlockAligns::default());
    assert_eq!(
        ui.md_block_aligns(),
        BlockAligns {
            table: BlockAlign::Left,
            image: BlockAlign::Center
        }
    );
    // Every accepted value, on both keys.
    for (v, want) in [
        ("left", BlockAlign::Left),
        ("center", BlockAlign::Center),
        ("right", BlockAlign::Right),
    ] {
        let ui = toml::from_str::<Config>(&format!(
            "[ui]\nmd_table_align = \"{v}\"\nmd_image_align = \"{v}\"\n"
        ))
        .unwrap()
        .ui;
        assert_eq!(ui.md_table_align, v);
        assert_eq!(ui.md_image_align, v);
        assert_eq!(ui.md_block_aligns().table, want, "table={v}");
        assert_eq!(ui.md_block_aligns().image, want, "image={v}");
    }
    // Unrecognized values are stored verbatim (same convention as every other mode string here) and
    // resolve back to **their own key's** default — not to a single shared one.
    let ui = toml::from_str::<Config>(
        "[ui]\nmd_table_align = \"sideways\"\nmd_image_align = \"diagonal\"\n",
    )
    .unwrap()
    .ui;
    assert_eq!(ui.md_table_align, "sideways");
    assert_eq!(ui.md_image_align, "diagonal");
    assert_eq!(ui.md_block_aligns(), BlockAligns::default());
    // The two keys are independent: one being set must not move the other.
    let ui = toml::from_str::<Config>("[ui]\nmd_table_align = \"right\"\n")
        .unwrap()
        .ui;
    assert_eq!(ui.md_block_aligns().table, BlockAlign::Right);
    assert_eq!(ui.md_block_aligns().image, BlockAlign::Center);
    let ui = toml::from_str::<Config>("[ui]\nmd_image_align = \"left\"\n")
        .unwrap()
        .ui;
    assert_eq!(ui.md_block_aligns().table, BlockAlign::Left);
    assert_eq!(ui.md_block_aligns().image, BlockAlign::Left);
}

#[test]
fn cfg_ui_tabbar_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.tabbar,
        "auto"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\ntabbar = \"always\"\n")
            .unwrap()
            .ui
            .tabbar,
        "always"
    );
}

#[test]
fn cfg_ui_preview_loading_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .preview_loading,
        "indicator"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\npreview_loading = \"progressive\"\n")
            .unwrap()
            .ui
            .preview_loading,
        "progressive"
    );
}

#[test]
fn cfg_ui_path_style_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.path_style,
        "relative"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\npath_style = \"home\"\n")
            .unwrap()
            .ui
            .path_style,
        "home"
    );
}

#[test]
fn cfg_ui_keys_default_and_parse() {
    assert_eq!(toml::from_str::<Config>("[ui]\n").unwrap().ui.keys, "vim");
    assert_eq!(
        toml::from_str::<Config>("[ui]\nkeys = \"less\"\n")
            .unwrap()
            .ui
            .keys,
        "less"
    );
}

#[test]
fn cfg_ui_lang_default_and_parse() {
    assert_eq!(toml::from_str::<Config>("[ui]\n").unwrap().ui.lang, "auto");
    assert_eq!(
        toml::from_str::<Config>("[ui]\nlang = \"jp\"\n")
            .unwrap()
            .ui
            .lang,
        "jp"
    );
}

#[test]
fn cfg_ui_statusbar_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.statusbar,
        "split"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nstatusbar = \"bottom\"\n")
            .unwrap()
            .ui
            .statusbar,
        "bottom"
    );
}

#[test]
fn cfg_ui_commit_meta_align_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .commit_meta_align,
        "right"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\ncommit_meta_align = \"inline\"\n")
            .unwrap()
            .ui
            .commit_meta_align,
        "inline"
    );
}

#[test]
fn cfg_ui_follow_view_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.follow_view,
        "diff"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nfollow_view = \"file\"\n")
            .unwrap()
            .ui
            .follow_view,
        "file"
    );
}

#[test]
fn cfg_ui_tree_cursor_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.tree_cursor,
        "origin"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\ntree_cursor = \"top\"\n")
            .unwrap()
            .ui
            .tree_cursor,
        "top"
    );
    // No `[ui]` table at all still resolves to the default via `#[serde(default)]`.
    assert_eq!(Config::default().ui.tree_cursor, "origin");
}

/// `filter_mode`: default `"fuzzy"`; `"substring"` selects the legacy path; the raw field stores
/// verbatim (like the other mode strings) but `.filter_mode()` is the permissive accessor — only
/// the literal `"substring"` opts out of fuzzy, anything else (including a typo) is `"fuzzy"`.
#[test]
fn cfg_ui_filter_mode_default_parse_and_permissive_fallback() {
    let d = Config::default();
    assert_eq!(d.ui.filter_mode, "fuzzy");
    assert_eq!(d.ui.filter_mode(), "fuzzy");

    let mk =
        |v: &str| -> Config { toml::from_str(&format!("[ui]\nfilter_mode = \"{v}\"\n")).unwrap() };
    assert_eq!(mk("substring").ui.filter_mode, "substring");
    assert_eq!(mk("substring").ui.filter_mode(), "substring");
    assert_eq!(mk("fuzzy").ui.filter_mode, "fuzzy");
    assert_eq!(mk("fuzzy").ui.filter_mode(), "fuzzy");
    // Unknown value: stored verbatim, but the accessor falls back to "fuzzy" (typo-safe).
    assert_eq!(mk("regex").ui.filter_mode, "regex");
    assert_eq!(mk("regex").ui.filter_mode(), "fuzzy");
}

#[test]
fn cfg_ui_mermaid_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.mermaid,
        "image"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmermaid = \"text\"\n")
            .unwrap()
            .ui
            .mermaid,
        "text"
    );
}

/// `math`: default `"image"`; `"text"` disables the LaTeX-to-raster lift. Had zero coverage in this
/// file before this — `math_color`'s own sanitizer was covered, but the sibling mode switch that
/// gates it (whether math renders as an image at all — `App::math_image_mode`) was never checked at
/// the TOML-parsing layer, only exercised end-to-end via `e2e_ui_math_image_mode_lifts_expression_
/// text_mode_keeps_it_inline` (which never asserts what the *raw config field* itself parsed to).
#[test]
fn cfg_ui_math_default_and_parse() {
    assert_eq!(toml::from_str::<Config>("[ui]\n").unwrap().ui.math, "image");
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmath = \"text\"\n")
            .unwrap()
            .ui
            .math,
        "text"
    );
}

#[test]
fn cfg_ui_mermaid_theme_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.mermaid_theme,
        "dark"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmermaid_theme = \"forest\"\n")
            .unwrap()
            .ui
            .mermaid_theme,
        "forest"
    );
}

/// `mermaid_curve`: default `"basis"`; round-trips any string through the config parser (the
/// permissive fallback to `Curve::Basis` for an unrecognised spelling happens downstream in
/// `preview::mermaid::render::edges::Curve::parse`, not here — this is only the raw string
/// making it from TOML into `UiConfig`, same as `mermaid_theme` above).
#[test]
fn cfg_ui_mermaid_curve_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n").unwrap().ui.mermaid_curve,
        "basis"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmermaid_curve = \"linear\"\n")
            .unwrap()
            .ui
            .mermaid_curve,
        "linear"
    );
}

/// `mermaid_routing`: default `"splines"`; round-trips any string through the config parser (the
/// permissive fallback to `Routing::Splines` for an unrecognised spelling happens downstream in
/// `preview::mermaid::render::orthogonal::Routing::parse`, not here — same shape as
/// `cfg_ui_mermaid_curve_default_and_parse` above).
#[test]
fn cfg_ui_mermaid_routing_default_and_parse() {
    assert_eq!(
        toml::from_str::<Config>("[ui]\n")
            .unwrap()
            .ui
            .mermaid_routing,
        "splines"
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmermaid_routing = \"konoma-orthogonal\"\n")
            .unwrap()
            .ui
            .mermaid_routing,
        "konoma-orthogonal"
    );
}

/// `math_color`: default light gray; validated against usvg's own color parser (`svgtypes::Color`) so a
/// value that passes is one usvg renders — never one it silently falls back to *black* for (invisible on
/// dark = the "blank equation" bug). A real color passes; a typo / `none` / `currentColor` / a
/// fully-transparent color falls back to the default. Regression 2026-07-20: the old alphabetic-only
/// check let `wihte`/`none`/`transparent` through → usvg fell back to black → equations blanked again.
#[test]
fn cfg_ui_math_color_default_and_sanitize() {
    let d = Config::default();
    assert_eq!(d.ui.math_color, "#d0d0d0");
    assert_eq!(d.ui.math_color(), "#d0d0d0");
    let mk =
        |v: &str| -> Config { toml::from_str(&format!("[ui]\nmath_color = \"{v}\"\n")).unwrap() };
    // Real colors pass through verbatim (usvg re-parses them identically).
    assert_eq!(mk("#111").ui.math_color(), "#111"); // 3-digit hex
    assert_eq!(mk("#1a2b3c").ui.math_color(), "#1a2b3c"); // 6-digit hex
    assert_eq!(mk("white").ui.math_color(), "white"); // CSS name
    assert_eq!(mk("black").ui.math_color(), "black"); // valid on a light terminal
    assert_eq!(mk("rgb(0,0,0)").ui.math_color(), "rgb(0,0,0)"); // functional notation
                                                                // Invalid / dangerous values fall back to the default so they can never blank equations.
    assert_eq!(mk("wihte").ui.math_color(), "#d0d0d0"); // typo → usvg would fall back to BLACK
    assert_eq!(mk("gary").ui.math_color(), "#d0d0d0"); // typo → default
    assert_eq!(mk("#gggggg").ui.math_color(), "#d0d0d0"); // non-hex → default
    assert_eq!(mk("none").ui.math_color(), "#d0d0d0"); // paint keyword, not a color → default
    assert_eq!(mk("currentColor").ui.math_color(), "#d0d0d0"); // no DOM context → default
    assert_eq!(mk("transparent").ui.math_color(), "#d0d0d0"); // alpha 0 would blank → default
    assert_eq!(mk("").ui.math_color(), "#d0d0d0"); // empty → default
}

/// String fields have NO validation/fallback at parse time — an unrecognized value is stored
/// verbatim (the consumer decides how to interpret/fall back).
#[test]
fn cfg_ui_string_fields_stored_verbatim_when_bogus() {
    let cfg: Config = toml::from_str(
        r#"
[ui]
filter_mode = "regex"
tabbar = "bogus"
preview_loading = "nope"
path_style = "sideways"
keys = "dvorak"
lang = "kling on"
statusbar = "diagonal"
commit_meta_align = "center"
follow_view = "hologram"
mermaid = "ascii-art"
mermaid_theme = "chartreuse"
mermaid_curve = "monotoneX"
mermaid_routing = "hyperspace"
math = "vector-art"
md_details = "sideways"
"#,
    )
    .unwrap();
    assert_eq!(cfg.ui.filter_mode, "regex");
    assert_eq!(cfg.ui.tabbar, "bogus");
    assert_eq!(cfg.ui.preview_loading, "nope");
    assert_eq!(cfg.ui.path_style, "sideways");
    assert_eq!(cfg.ui.keys, "dvorak");
    assert_eq!(cfg.ui.lang, "kling on");
    assert_eq!(cfg.ui.statusbar, "diagonal");
    assert_eq!(cfg.ui.commit_meta_align, "center");
    assert_eq!(cfg.ui.follow_view, "hologram");
    assert_eq!(cfg.ui.mermaid, "ascii-art");
    assert_eq!(cfg.ui.mermaid_theme, "chartreuse");
    assert_eq!(cfg.ui.mermaid_curve, "monotoneX");
    assert_eq!(cfg.ui.mermaid_routing, "hyperspace");
    assert_eq!(cfg.ui.math, "vector-art");
    assert_eq!(cfg.ui.md_details, "sideways");
}

#[test]
fn cfg_ui_tab_width_default_and_parse() {
    let ui = toml::from_str::<Config>("[ui]\n").unwrap().ui;
    let _: usize = ui.tab_width;
    assert_eq!(ui.tab_width, 4);
    assert_eq!(
        toml::from_str::<Config>("[ui]\ntab_width = 8\n")
            .unwrap()
            .ui
            .tab_width,
        8
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\ntab_width = 0\n")
            .unwrap()
            .ui
            .tab_width,
        0
    );
}

#[test]
fn cfg_ui_image_render_scale_default_and_parse() {
    let ui = toml::from_str::<Config>("[ui]\n").unwrap().ui;
    let _: f64 = ui.image_render_scale;
    assert_eq!(ui.image_render_scale, 1.0);
    assert_eq!(
        toml::from_str::<Config>("[ui]\nimage_render_scale = 0.5\n")
            .unwrap()
            .ui
            .image_render_scale,
        0.5
    );
}

#[test]
fn cfg_ui_svg_max_px_default_and_parse() {
    let ui = toml::from_str::<Config>("[ui]\n").unwrap().ui;
    let _: u32 = ui.svg_max_px;
    assert_eq!(ui.svg_max_px, 800);
    assert_eq!(
        toml::from_str::<Config>("[ui]\nsvg_max_px = 1600\n")
            .unwrap()
            .ui
            .svg_max_px,
        1600
    );
}

#[test]
fn cfg_ui_mermaid_rows_default_and_parse() {
    // 0/invalid -> 24 normalization lives in App::mermaid_rows_cap() (app.rs); the parser stores
    // the raw value verbatim (0 stays 0 here).
    let ui = toml::from_str::<Config>("[ui]\n").unwrap().ui;
    let _: u16 = ui.mermaid_rows;
    assert_eq!(ui.mermaid_rows, 24);
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmermaid_rows = 40\n")
            .unwrap()
            .ui
            .mermaid_rows,
        40
    );
    assert_eq!(
        toml::from_str::<Config>("[ui]\nmermaid_rows = 0\n")
            .unwrap()
            .ui
            .mermaid_rows,
        0
    );
}

#[test]
fn cfg_ui_empty_toml_yields_all_scalar_defaults() {
    let ui = toml::from_str::<Config>("").unwrap().ui;
    assert!(!ui.show_hidden);
    assert_eq!(ui.filter_mode, "fuzzy");
    assert_eq!(ui.tabbar, "auto");
    assert!(ui.icons);
    assert!(ui.wrap);
    assert!(!ui.line_numbers);
    assert!(ui.git_gutter);
    assert_eq!(ui.tab_width, 4);
    assert!(ui.syntax_highlight);
    assert_eq!(ui.preview_loading, "indicator");
    assert_eq!(ui.path_style, "relative");
    assert_eq!(ui.keys, "vim");
    assert_eq!(ui.lang, "auto");
    assert_eq!(ui.statusbar, "split");
    assert_eq!(ui.image_render_scale, 1.0);
    assert_eq!(ui.svg_max_px, 800);
    assert_eq!(ui.commit_meta_align, "right");
    assert!(ui.confirm_quit);
    assert!(ui.confirm_bookmark_overwrite);
    assert!(ui.csv_rainbow);
    assert!(ui.busy_indicator);
    assert_eq!(ui.mermaid, "image");
    assert_eq!(ui.mermaid_theme, "dark");
    assert_eq!(ui.mermaid_curve, "basis");
    assert_eq!(ui.mermaid_routing, "splines");
    assert_eq!(ui.mermaid_rows, 24);
    assert_eq!(ui.follow_view, "diff");
    assert!(ui.restore_tabs);
    assert!(ui.md_autolink);
    assert!(ui.md_alerts);
    assert!(ui.md_emoji);
}

#[test]
fn cfg_ui_partial_toml_leaves_other_scalars_default() {
    let ui = toml::from_str::<Config>(
        r#"
[ui]
wrap = false
tab_width = 2
mermaid = "text"
"#,
    )
    .unwrap()
    .ui;
    assert!(!ui.wrap);
    assert_eq!(ui.tab_width, 2);
    assert_eq!(ui.mermaid, "text");
    assert!(!ui.show_hidden);
    assert!(ui.icons);
    assert_eq!(ui.mermaid_theme, "dark");
    assert!(ui.md_autolink);
    assert!(ui.md_alerts);
    assert!(ui.md_emoji);
    assert!(ui.restore_tabs);
}

#[test]
fn cfg_ui_unknown_key_is_ignored() {
    let parsed =
        toml::from_str::<Config>("[ui]\nwrap = false\nthis_key_does_not_exist = \"whatever\"\n");
    assert!(parsed.is_ok(), "unknown key must not fail the parse");
    let ui = parsed.unwrap().ui;
    assert!(!ui.wrap);
    assert!(ui.icons);
    assert_eq!(ui.mermaid, "image");
}

#[test]
fn cfg_ui_unknown_key_with_various_types_is_ignored() {
    let parsed = toml::from_str::<Config>(
        r#"
[ui]
bogus_bool = true
bogus_int = 99
bogus_str = "x"
bogus_arr = [1, 2, 3]
mermaid_rows = 30
"#,
    );
    assert!(parsed.is_ok());
    assert_eq!(parsed.unwrap().ui.mermaid_rows, 30);
}

// =============================================================================
// Collection / derived UiConfig fields + SortConfig
// =============================================================================

#[test]
fn cfg_coll_details_default_is_empty() {
    assert!(Config::default().ui.details.is_empty());
    let cfg: Config = toml::from_str("[ui]\nicons = false\n").unwrap();
    assert!(cfg.ui.details.is_empty());
}

#[test]
fn cfg_coll_details_parses_array_verbatim_preserving_order() {
    let cfg: Config =
        toml::from_str("[ui]\ndetails = [\"type\", \"size\", \"modified\"]\n").unwrap();
    assert_eq!(
        cfg.ui.details,
        vec![
            "type".to_string(),
            "size".to_string(),
            "modified".to_string()
        ]
    );
}

#[test]
fn cfg_coll_details_stores_unknown_column_names_verbatim() {
    let cfg: Config = toml::from_str("[ui]\ndetails = [\"size\", \"bogus\", \"perm\"]\n").unwrap();
    assert_eq!(
        cfg.ui.details,
        vec!["size".to_string(), "bogus".to_string(), "perm".to_string()]
    );
}

#[test]
fn cfg_coll_graph_max_branches_default_is_12() {
    assert_eq!(Config::default().ui.graph_max_branches, 12);
}

#[test]
fn cfg_coll_graph_max_branches_parses_explicit_and_zero_verbatim() {
    let cfg: Config = toml::from_str("[ui]\ngraph_max_branches = 5\n").unwrap();
    assert_eq!(cfg.ui.graph_max_branches, 5);
    let cfg: Config = toml::from_str("[ui]\ngraph_max_branches = 0\n").unwrap();
    assert_eq!(cfg.ui.graph_max_branches, 0);
}

#[test]
fn cfg_coll_graph_base_branches_default_is_empty() {
    assert!(Config::default().ui.graph_base_branches.is_empty());
}

#[test]
fn cfg_coll_graph_base_branches_parses_array_verbatim_preserving_order() {
    let cfg: Config =
        toml::from_str("[ui]\ngraph_base_branches = [\"main\", \"develop\"]\n").unwrap();
    assert_eq!(
        cfg.ui.graph_base_branches,
        vec!["main".to_string(), "develop".to_string()]
    );
}

#[test]
fn cfg_coll_mermaid_rows_default_is_24() {
    assert_eq!(Config::default().ui.mermaid_rows, 24u16);
}

#[test]
fn cfg_coll_md_task_states_parses_from_toml_and_derives() {
    let cfg: Config = toml::from_str("[ui]\nmd_task_states = [\" \", \"/\", \"x\"]\n").unwrap();
    assert_eq!(
        cfg.ui.md_task_states,
        vec![" ".to_string(), "/".to_string(), "x".to_string()]
    );
    assert_eq!(cfg.ui.md_task_state_chars(), vec![' ', '/', 'x']);
}

#[test]
fn cfg_coll_md_task_state_chars_minimum_two_and_longer_valid() {
    let mut ui = UiConfig::default();
    ui.md_task_states = vec!["a".into(), "b".into()];
    assert_eq!(ui.md_task_state_chars(), vec!['a', 'b']);
    ui.md_task_states = vec![" ".into(), "/".into(), "x".into(), "-".into()];
    assert_eq!(ui.md_task_state_chars(), vec![' ', '/', 'x', '-']);
}

#[test]
fn cfg_coll_md_task_state_chars_empty_string_element_falls_back() {
    let mut ui = UiConfig::default();
    ui.md_task_states = vec!["".into(), "x".into()];
    assert_eq!(ui.md_task_state_chars(), vec![' ', 'x']);
    ui.md_task_states = vec![" ".into(), "".into(), "x".into()];
    assert_eq!(ui.md_task_state_chars(), vec![' ', 'x']);
}

#[test]
fn cfg_coll_md_task_state_chars_all_multichar_falls_back() {
    let mut ui = UiConfig::default();
    ui.md_task_states = vec!["ab".into(), "cd".into()];
    assert_eq!(ui.md_task_state_chars(), vec![' ', 'x']);
}

#[test]
fn cfg_coll_md_task_state_chars_counts_unicode_scalars_not_bytes() {
    let mut ui = UiConfig::default();
    ui.md_task_states = vec!["○".into(), "●".into()];
    assert_eq!(ui.md_task_state_chars(), vec!['○', '●']);
    ui.md_task_states = vec!["☐".into(), "☒".into(), "☑".into()];
    assert_eq!(ui.md_task_state_chars(), vec!['☐', '☒', '☑']);
}

#[test]
fn cfg_sort_defaults() {
    let d = SortConfig::default();
    assert_eq!(d.key, "name");
    assert!(!d.reverse);
    assert!(d.dirs_first);
    let ui_sort = Config::default().ui.sort;
    assert_eq!(ui_sort.key, "name");
    assert!(!ui_sort.reverse);
    assert!(ui_sort.dirs_first);
}

#[test]
fn cfg_sort_parses_each_field_from_toml() {
    let cfg: Config =
        toml::from_str("[ui.sort]\nkey = \"size\"\nreverse = true\ndirs_first = false\n").unwrap();
    assert_eq!(cfg.ui.sort.key, "size");
    assert!(cfg.ui.sort.reverse);
    assert!(!cfg.ui.sort.dirs_first);
}

#[test]
fn cfg_sort_partial_toml_uses_defaults_for_omitted_fields() {
    let cfg: Config = toml::from_str("[ui.sort]\nkey = \"ext\"\n").unwrap();
    assert_eq!(cfg.ui.sort.key, "ext");
    assert!(!cfg.ui.sort.reverse);
    assert!(cfg.ui.sort.dirs_first);
}

#[test]
fn cfg_sort_invalid_key_stored_verbatim_at_parse() {
    let cfg: Config = toml::from_str("[ui.sort]\nkey = \"bogus\"\n").unwrap();
    assert_eq!(cfg.ui.sort.key, "bogus");
    let cfg: Config = toml::from_str("[ui.sort]\nkey = \"\"\n").unwrap();
    assert_eq!(cfg.ui.sort.key, "");
}

#[test]
fn cfg_coll_all_collection_fields_parse_together() {
    let cfg: Config = toml::from_str(
        "[ui]\n\
         details = [\"size\", \"modified\"]\n\
         graph_max_branches = 8\n\
         graph_base_branches = [\"release\", \"main\"]\n\
         mermaid_rows = 30\n\
         md_task_states = [\" \", \"x\", \"-\"]\n",
    )
    .unwrap();
    assert_eq!(
        cfg.ui.details,
        vec!["size".to_string(), "modified".to_string()]
    );
    assert_eq!(cfg.ui.graph_max_branches, 8);
    assert_eq!(
        cfg.ui.graph_base_branches,
        vec!["release".to_string(), "main".to_string()]
    );
    assert_eq!(cfg.ui.mermaid_rows, 30u16);
    assert_eq!(cfg.ui.md_task_state_chars(), vec![' ', 'x', '-']);
}

// =============================================================================
// ThemeConfig + color parsing (parse_color_opt / lighten / theme methods)
// =============================================================================

#[test]
fn cfg_color_parse_hex_lowercase_and_uppercase() {
    assert_eq!(
        parse_color_opt("#ff8800", None),
        Some(Color::Rgb(255, 136, 0))
    );
    assert_eq!(
        parse_color_opt("#FF8800", None),
        Some(Color::Rgb(255, 136, 0))
    );
    assert_eq!(
        parse_color_opt("#AABBCC", None),
        Some(Color::Rgb(170, 187, 204))
    );
    assert_eq!(parse_color_opt("#000000", None), Some(Color::Rgb(0, 0, 0)));
    assert_eq!(
        parse_color_opt("#ffffff", None),
        Some(Color::Rgb(255, 255, 255))
    );
}

#[test]
fn cfg_color_parse_named_colors() {
    assert_eq!(parse_color_opt("black", None), Some(Color::Black));
    assert_eq!(parse_color_opt("red", None), Some(Color::Red));
    assert_eq!(parse_color_opt("green", None), Some(Color::Green));
    assert_eq!(parse_color_opt("yellow", None), Some(Color::Yellow));
    assert_eq!(parse_color_opt("blue", None), Some(Color::Blue));
    assert_eq!(parse_color_opt("magenta", None), Some(Color::Magenta));
    assert_eq!(parse_color_opt("cyan", None), Some(Color::Cyan));
    assert_eq!(parse_color_opt("gray", None), Some(Color::Gray));
    assert_eq!(parse_color_opt("darkgray", None), Some(Color::DarkGray));
    assert_eq!(parse_color_opt("lightblue", None), Some(Color::LightBlue));
    assert_eq!(parse_color_opt("white", None), Some(Color::White));
    assert_eq!(parse_color_opt("reset", None), Some(Color::Reset));
}

#[test]
fn cfg_color_parse_named_normalization() {
    assert_eq!(parse_color_opt("Blue", None), Some(Color::Blue));
    assert_eq!(parse_color_opt("MAGENTA", None), Some(Color::Magenta));
    assert_eq!(parse_color_opt("light blue", None), Some(Color::LightBlue));
    assert_eq!(parse_color_opt("light-blue", None), Some(Color::LightBlue));
    assert_eq!(parse_color_opt("light_blue", None), Some(Color::LightBlue));
    assert_eq!(parse_color_opt("grey", None), Some(Color::Gray));
    assert_eq!(parse_color_opt("silver", None), Some(Color::Gray));
    assert_eq!(parse_color_opt("bright-white", None), Some(Color::White));
}

#[test]
fn cfg_color_parse_indexed() {
    assert_eq!(parse_color_opt("0", None), Some(Color::Indexed(0)));
    assert_eq!(parse_color_opt("8", None), Some(Color::Indexed(8)));
    assert_eq!(parse_color_opt("255", None), Some(Color::Indexed(255)));
}

#[test]
fn cfg_color_none_empty_and_whitespace_ignore_fallback() {
    let fb = Some(Color::Red);
    assert_eq!(parse_color_opt("none", fb), None);
    assert_eq!(parse_color_opt("NONE", fb), None);
    assert_eq!(parse_color_opt("None", fb), None);
    assert_eq!(parse_color_opt("  none  ", fb), None);
    assert_eq!(parse_color_opt("", fb), None);
    assert_eq!(parse_color_opt("   ", fb), None);
}

#[test]
fn cfg_color_invalid_returns_fallback() {
    assert_eq!(
        parse_color_opt("bogus", Some(Color::Rgb(1, 2, 3))),
        Some(Color::Rgb(1, 2, 3))
    );
    assert_eq!(
        parse_color_opt("nope-xyz", Some(Color::Blue)),
        Some(Color::Blue)
    );
    assert_eq!(parse_color_opt("bogus", None), None);
}

#[test]
fn cfg_color_hex_wrong_length_or_digit_returns_fallback() {
    let fb = Some(Color::Red);
    assert_eq!(parse_color_opt("#fff", fb), fb);
    assert_eq!(parse_color_opt("#12345678", fb), fb);
    assert_eq!(parse_color_opt("#GGGGGG", fb), fb);
    assert_eq!(parse_color_opt("101820", fb), fb);
}

#[test]
fn cfg_color_index_overflow_returns_fallback() {
    assert_eq!(parse_color_opt("256", Some(Color::Blue)), Some(Color::Blue));
    assert_eq!(parse_color_opt("256", None), None);
}

#[test]
fn cfg_color_success_ignores_fallback() {
    assert_eq!(parse_color_opt("red", Some(Color::White)), Some(Color::Red));
    assert_eq!(
        parse_color_opt("#101820", Some(Color::White)),
        Some(Color::Rgb(16, 24, 32))
    );
    assert_eq!(
        parse_color_opt("8", Some(Color::White)),
        Some(Color::Indexed(8))
    );
}

#[test]
fn cfg_color_trims_before_parsing() {
    assert_eq!(
        parse_color_opt("  #101820  ", None),
        Some(Color::Rgb(16, 24, 32))
    );
    assert_eq!(parse_color_opt("\tblue\n", None), Some(Color::Blue));
}

#[test]
fn cfg_color_lighten_brightens_rgb() {
    assert_eq!(lighten(Color::Rgb(43, 48, 59)), Color::Rgb(70, 78, 99));
    assert_eq!(lighten(DEFAULT_CODE_BG), Color::Rgb(70, 78, 99));
    assert_eq!(lighten(Color::Rgb(0, 0, 0)), Color::Rgb(27, 30, 40));
    assert_eq!(lighten(Color::Rgb(16, 24, 32)), Color::Rgb(43, 54, 72));
}

#[test]
fn cfg_color_lighten_saturates_at_255() {
    assert_eq!(
        lighten(Color::Rgb(255, 255, 255)),
        Color::Rgb(255, 255, 255)
    );
    assert_eq!(
        lighten(Color::Rgb(250, 240, 230)),
        Color::Rgb(255, 255, 255)
    );
    assert_eq!(
        lighten(Color::Rgb(229, 226, 216)),
        Color::Rgb(255, 255, 255)
    );
}

#[test]
fn cfg_color_lighten_channels_never_decrease() {
    for (r, g, b) in [
        (43u8, 48u8, 59u8),
        (0, 0, 0),
        (200, 210, 220),
        (255, 255, 255),
    ] {
        if let Color::Rgb(nr, ng, nb) = lighten(Color::Rgb(r, g, b)) {
            assert!(nr >= r && ng >= g && nb >= b, "input ({r},{g},{b})");
        } else {
            panic!("lighten of Rgb should stay Rgb");
        }
    }
}

#[test]
fn cfg_color_lighten_non_rgb_unchanged() {
    assert_eq!(lighten(Color::Blue), Color::Blue);
    assert_eq!(lighten(Color::Reset), Color::Reset);
    assert_eq!(lighten(Color::White), Color::White);
    assert_eq!(lighten(Color::Black), Color::Black);
    assert_eq!(lighten(Color::Indexed(5)), Color::Indexed(5));
}

#[test]
fn cfg_theme_code_bg_named_indexed_and_uppercase_hex() {
    let named = ThemeConfig {
        code_bg: "blue".into(),
        ..Default::default()
    };
    assert_eq!(named.code_bg(), Some(Color::Blue));
    let indexed = ThemeConfig {
        code_bg: "8".into(),
        ..Default::default()
    };
    assert_eq!(indexed.code_bg(), Some(Color::Indexed(8)));
    let upper = ThemeConfig {
        code_bg: "#AABBCC".into(),
        ..Default::default()
    };
    assert_eq!(upper.code_bg(), Some(Color::Rgb(170, 187, 204)));
}

#[test]
fn cfg_theme_code_bg_whitespace_only_is_none() {
    let t = ThemeConfig {
        code_bg: "   ".into(),
        ..Default::default()
    };
    assert_eq!(t.code_bg(), None);
}

#[test]
fn cfg_theme_bg_named_indexed_and_empty() {
    let named = ThemeConfig {
        bg: "blue".into(),
        ..Default::default()
    };
    assert_eq!(named.bg(), Some(Color::Blue));
    let indexed = ThemeConfig {
        bg: "4".into(),
        ..Default::default()
    };
    assert_eq!(indexed.bg(), Some(Color::Indexed(4)));
    let empty = ThemeConfig {
        bg: "".into(),
        ..Default::default()
    };
    assert_eq!(empty.bg(), None);
}

#[test]
fn cfg_theme_code_label_bg_auto_derives_from_custom_code_bg() {
    let t = ThemeConfig {
        code_bg: "#101820".into(),
        code_label_bg: "auto".into(),
        ..Default::default()
    };
    assert_eq!(t.code_label_bg(), Some(lighten(Color::Rgb(16, 24, 32))));
    assert_eq!(t.code_label_bg(), Some(Color::Rgb(43, 54, 72)));
}

#[test]
fn cfg_theme_code_label_bg_auto_is_case_and_whitespace_insensitive() {
    let t = ThemeConfig {
        code_label_bg: "  Auto  ".into(),
        ..Default::default()
    };
    assert_eq!(t.code_label_bg(), Some(lighten(DEFAULT_CODE_BG)));
    let t = ThemeConfig {
        code_label_bg: "AUTO".into(),
        ..Default::default()
    };
    assert_eq!(t.code_label_bg(), Some(lighten(DEFAULT_CODE_BG)));
}

#[test]
fn cfg_theme_code_label_bg_named_index_and_invalid() {
    let named = ThemeConfig {
        code_label_bg: "blue".into(),
        ..Default::default()
    };
    assert_eq!(named.code_label_bg(), Some(Color::Blue));
    let indexed = ThemeConfig {
        code_label_bg: "5".into(),
        ..Default::default()
    };
    assert_eq!(indexed.code_label_bg(), Some(Color::Indexed(5)));
    let invalid = ThemeConfig {
        code_label_bg: "not-a-color".into(),
        ..Default::default()
    };
    assert_eq!(invalid.code_label_bg(), None);
    let empty = ThemeConfig {
        code_label_bg: "".into(),
        ..Default::default()
    };
    assert_eq!(empty.code_label_bg(), None);
}

#[test]
fn cfg_theme_code_label_right_variants() {
    let right = ThemeConfig {
        code_label_align: "right".into(),
        ..Default::default()
    };
    assert!(right.code_label_right());
    let left_ws = ThemeConfig {
        code_label_align: "  left  ".into(),
        ..Default::default()
    };
    assert!(!left_ws.code_label_right());
    let left_mixed = ThemeConfig {
        code_label_align: "Left".into(),
        ..Default::default()
    };
    assert!(!left_mixed.code_label_right());
    let other = ThemeConfig {
        code_label_align: "center".into(),
        ..Default::default()
    };
    assert!(other.code_label_right());
    let empty = ThemeConfig {
        code_label_align: "".into(),
        ..Default::default()
    };
    assert!(empty.code_label_right());
}

#[test]
fn cfg_theme_full_roundtrip_via_toml() {
    let cfg: Config = toml::from_str(
        "[ui.theme]\n\
         bg = \"#123456\"\n\
         code_bg = \"#0a141e\"\n\
         code_label_align = \"left\"\n\
         code_label_bg = \"auto\"\n\
         code_theme = \"Dracula\"\n",
    )
    .unwrap();
    let th = &cfg.ui.theme;
    assert_eq!(th.bg(), Some(Color::Rgb(18, 52, 86)));
    assert_eq!(th.code_bg(), Some(Color::Rgb(10, 20, 30)));
    assert!(!th.code_label_right());
    assert_eq!(th.code_label_bg(), Some(lighten(Color::Rgb(10, 20, 30))));
    assert_eq!(th.code_label_bg(), Some(Color::Rgb(37, 50, 70)));
    assert_eq!(th.code_theme, "Dracula");
}

#[test]
fn cfg_theme_partial_toml_keeps_defaults_via_serde_default() {
    let cfg: Config = toml::from_str("[ui.theme]\ncode_label_align = \"left\"\n").unwrap();
    let th = &cfg.ui.theme;
    assert!(!th.code_label_right());
    assert_eq!(th.bg(), None);
    assert_eq!(th.code_bg(), Some(DEFAULT_CODE_BG));
    assert_eq!(th.code_label_bg(), Some(lighten(DEFAULT_CODE_BG)));
    assert_eq!(th.code_theme, "TwoDark");
}

#[test]
fn cfg_theme_absent_block_uses_all_defaults() {
    let cfg: Config = toml::from_str("").unwrap();
    let th = &cfg.ui.theme;
    assert_eq!(th.bg(), None);
    assert_eq!(th.code_bg(), Some(DEFAULT_CODE_BG));
    assert!(th.code_label_right());
    assert_eq!(th.code_label_bg(), Some(lighten(DEFAULT_CODE_BG)));
    assert_eq!(th.code_theme, "TwoDark");
}

// =============================================================================
// Preview rules / rule matching / editor line injection / GitConfig
// =============================================================================

// Minimal PNG signature (8-byte magic + IHDR start). infer 0.16 -> "image/png".
const PNG_MAGIC: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R',
];
// MP4 "ftyp isom" box header. infer 0.16 -> "video/mp4".
const MP4_MAGIC: &[u8] = &[
    0x00, 0x00, 0x00, 0x20, b'f', b't', b'y', b'p', b'i', b's', b'o', b'm', 0x00, 0x00, 0x02, 0x00,
];

#[test]
fn cfg_preview_default_mermaid_extensions() {
    for name in ["cfgp_a.mmd", "cfgp_b.mermaid"] {
        let p = tmp(name, b"graph TD; A-->B\n");
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Mermaid(_)), "{name}: {kind:?}");
        std::fs::remove_file(&p).ok();
    }
}

#[test]
fn cfg_preview_default_svg() {
    let p = tmp(
        "cfgp_pic.svg",
        b"<svg xmlns='http://www.w3.org/2000/svg'/>\n",
    );
    let kind = Config::default().resolve_preview(&p);
    assert!(matches!(kind, PreviewKind::Svg(_)), "got {kind:?}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn cfg_preview_default_csv_tsv_delimiters() {
    let c = tmp("cfgp_data.csv", b"a,b,c\n1,2,3\n");
    match Config::default().resolve_preview(&c) {
        PreviewKind::Table { delimiter, .. } => assert_eq!(delimiter, b','),
        other => panic!("expected Table for csv, got {other:?}"),
    }
    std::fs::remove_file(&c).ok();
    let t = tmp("cfgp_data.tsv", b"a\tb\tc\n1\t2\t3\n");
    match Config::default().resolve_preview(&t) {
        PreviewKind::Table { delimiter, .. } => assert_eq!(delimiter, b'\t'),
        other => panic!("expected Table for tsv, got {other:?}"),
    }
    std::fs::remove_file(&t).ok();
}

#[test]
fn cfg_preview_default_pdf() {
    let p = tmp("cfgp_doc.pdf", b"%PDF-1.4\n");
    let kind = Config::default().resolve_preview(&p);
    assert!(matches!(kind, PreviewKind::Pdf(_)), "got {kind:?}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn cfg_preview_default_image_png_by_content() {
    let p = tmp("cfgp_photo.png", PNG_MAGIC);
    let kind = Config::default().resolve_preview(&p);
    assert!(matches!(kind, PreviewKind::Image(_)), "got {kind:?}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn cfg_preview_default_video_mp4_by_content() {
    let p = tmp("cfgp_clip.mp4", MP4_MAGIC);
    let kind = Config::default().resolve_preview(&p);
    assert!(matches!(kind, PreviewKind::Video(_)), "got {kind:?}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn cfg_preview_default_code_lowercase_exts() {
    for (name, body) in [
        ("cfgp_s.py", &b"print(1)\n"[..]),
        ("cfgp_s.json", &b"{}\n"[..]),
        ("cfgp_s.toml", &b"k = 1\n"[..]),
    ] {
        let p = tmp(name, body);
        let kind = Config::default().resolve_preview(&p);
        assert!(matches!(kind, PreviewKind::Code(_)), "{name}: {kind:?}");
        std::fs::remove_file(&p).ok();
    }
}

#[test]
fn cfg_preview_default_txt_falls_back_to_text() {
    let p = tmp("cfgp_notes.txt", b"just some plain text\nsecond line\n");
    let kind = Config::default().resolve_preview(&p);
    assert!(matches!(kind, PreviewKind::Text(_)), "got {kind:?}");
    std::fs::remove_file(&p).ok();
}

#[test]
fn cfg_preview_glob_svg_wins_over_png_content() {
    let p = tmp("cfgp_weird.svg", PNG_MAGIC);
    let kind = Config::default().resolve_preview(&p);
    assert!(
        matches!(kind, PreviewKind::Svg(_)),
        "svg glob wins: {kind:?}"
    );
    std::fs::remove_file(&p).ok();
}

#[test]
fn cfg_preview_user_rules_replace_defaults() {
    let toml = r#"
[[preview.rules]]
glob = "*.md"
builtin = "code"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    let md = tmp("cfgp_ovr.md", b"# title\n");
    assert!(
        matches!(cfg.resolve_preview(&md), PreviewKind::Code(_)),
        "user rule should win for .md"
    );
    std::fs::remove_file(&md).ok();
    let rs = tmp("cfgp_ovr.rs", b"fn main() {}\n");
    assert!(
        matches!(cfg.resolve_preview(&rs), PreviewKind::Text(_)),
        "defaults must be replaced, not merged"
    );
    std::fs::remove_file(&rs).ok();
}

#[test]
fn cfg_preview_absent_preview_keeps_default_rules() {
    let cfg: Config = toml::from_str("[ui]\nwrap = false\n").unwrap();
    assert!(
        matches!(
            cfg.resolve_preview(Path::new("/x/note.md")),
            PreviewKind::Markdown(_)
        ),
        "missing [preview] keeps builtins"
    );
}

#[test]
fn cfg_preview_command_rule_produces_command_kind() {
    let toml = r#"
[[preview.rules]]
glob = "*.xyz"
command = "mpv {path}"
render_as = "image"
detached = true
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    match cfg.resolve_preview(Path::new("/tmp/whatever.xyz")) {
        PreviewKind::Command {
            template,
            render_as,
            detached,
            ..
        } => {
            assert_eq!(template, "mpv {path}");
            assert_eq!(render_as.as_deref(), Some("image"));
            assert!(detached);
        }
        other => panic!("expected Command, got {other:?}"),
    }
}

#[test]
fn cfg_preview_command_rule_disabled_by_external_falls_back_to_can_not_preview() {
    let toml = r#"
[external]
preview_commands = false

[[preview.rules]]
glob = "*.xyz"
command = "mpv {path}"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    match cfg.resolve_preview(Path::new("/tmp/whatever.xyz")) {
        PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "xyz"),
        other => panic!("preview_commands=false: expected CanNotPreview, got {other:?}"),
    }
    // Builtin renderers matched by other rules are unaffected.
    let toml2 = r#"
[external]
preview_commands = false
"#;
    let cfg2: Config = toml::from_str(toml2).unwrap();
    assert!(
        matches!(
            cfg2.resolve_preview(Path::new("/x/note.md")),
            PreviewKind::Markdown(_)
        ),
        "preview_commands=false leaves builtin rules intact"
    );
}

#[test]
fn cfg_preview_command_rule_explicit_true_still_delegates() {
    // The default-true case is already covered by cfg_preview_command_rule_produces_command_kind
    // (an absent [external] table); this pins the explicitly-written `true` too.
    let toml = r#"
[external]
preview_commands = true

[[preview.rules]]
glob = "*.xyz"
command = "cat {path}"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    assert!(matches!(
        cfg.resolve_preview(Path::new("/tmp/whatever.xyz")),
        PreviewKind::Command { .. }
    ));
}

#[test]
fn cfg_preview_unknown_builtin_falls_back_to_can_not_preview() {
    let toml = r#"
[[preview.rules]]
glob = "*.md"
builtin = "nonsense-renderer"
"#;
    let cfg: Config = toml::from_str(toml).unwrap();
    match cfg.resolve_preview(Path::new("/x/a.md")) {
        PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "md"),
        other => panic!("expected CanNotPreview, got {other:?}"),
    }
}

#[test]
fn cfg_preview_mime_glob_match_additional_cases() {
    assert!(!mime_glob_match("image/*", "text/plain"));
    assert!(mime_glob_match("video/*", "video/mp4"));
    assert!(mime_glob_match("application/pdf", "application/pdf"));
    assert!(!mime_glob_match("application/pdf", "application/zip"));
    assert!(!mime_glob_match("application/pdf", "application/pdfx"));
    assert!(!mime_glob_match("Image/*", "image/png"));
    assert!(!mime_glob_match("APPLICATION/PDF", "application/pdf"));
    // Implementation quirk: the stripped prefix excludes the slash, so a bare category matches.
    assert!(mime_glob_match("image/*", "image"));
}

#[test]
fn cfg_preview_rule_matches_glob_variants() {
    let md = Rule {
        glob: Some("*.md".into()),
        ..Default::default()
    };
    assert!(rule_matches(&md, Path::new("/x/a.md")));
    assert!(!rule_matches(&md, Path::new("/x/a.rs")));
    assert!(rule_matches(&md, Path::new("/deep/nested/note.md")));
    let upper = Rule {
        glob: Some("*.MD".into()),
        ..Default::default()
    };
    assert!(rule_matches(&upper, Path::new("/x/readme.md")));
    let slashed = Rule {
        glob: Some("docs/*.md".into()),
        ..Default::default()
    };
    assert!(!rule_matches(&slashed, Path::new("/x/docs/note.md")));
    let neither = Rule {
        builtin: Some("text".into()),
        ..Default::default()
    };
    assert!(!rule_matches(&neither, Path::new("/x/a.md")));
    let broken = Rule {
        glob: Some("[".into()),
        ..Default::default()
    };
    assert!(!rule_matches(&broken, Path::new("/x/a.md")));
}

#[test]
fn cfg_preview_rule_matches_mime_via_content() {
    let png = tmp("cfgp_rm.png", PNG_MAGIC);
    let img_rule = Rule {
        mime: Some("image/*".into()),
        ..Default::default()
    };
    let vid_rule = Rule {
        mime: Some("video/*".into()),
        ..Default::default()
    };
    assert!(rule_matches(&img_rule, &png));
    assert!(!rule_matches(&vid_rule, &png));
    std::fs::remove_file(&png).ok();
    let txt = tmp("cfgp_rm.txt", b"hello, not an image\n");
    assert!(!rule_matches(&img_rule, &txt));
    std::fs::remove_file(&txt).ok();
}

#[test]
fn cfg_editor_vscode_family_gets_dash_g() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("cursor", path, Some(3)),
        vec!["cursor", "-g", "/tmp/a.rs:3"]
    );
    assert_eq!(
        build_argv("code-insiders", path, Some(5)),
        vec!["code-insiders", "-g", "/tmp/a.rs:5"]
    );
    assert_eq!(
        build_argv("codium -w", path, Some(4)),
        vec!["codium", "-g", "-w", "/tmp/a.rs:4"]
    );
    assert_eq!(
        build_argv("code", path, Some(15)),
        vec!["code", "-g", "/tmp/a.rs:15"]
    );
}

#[test]
fn cfg_editor_sublime_helix_zed_get_path_colon_line() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("subl", path, Some(20)),
        vec!["subl", "/tmp/a.rs:20"]
    );
    assert_eq!(
        build_argv("sublime_text", path, Some(7)),
        vec!["sublime_text", "/tmp/a.rs:7"]
    );
    assert_eq!(
        build_argv("helix", path, Some(2)),
        vec!["helix", "/tmp/a.rs:2"]
    );
    assert_eq!(build_argv("zed", path, Some(9)), vec!["zed", "/tmp/a.rs:9"]);
}

#[test]
fn cfg_editor_vim_family_variants_get_plus_and_zt() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("vi", path, Some(3)),
        vec!["vi", "+3", "+normal! zt", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("gvim", path, Some(3)),
        vec!["gvim", "+3", "+normal! zt", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("neovim", path, Some(3)),
        vec!["neovim", "+3", "+normal! zt", "/tmp/a.rs"]
    );
}

#[test]
fn cfg_editor_plus_n_editors_without_zt() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("micro", path, Some(6)),
        vec!["micro", "+6", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("kak", path, Some(6)),
        vec!["kak", "+6", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("emacsclient", path, Some(6)),
        vec!["emacsclient", "+6", "/tmp/a.rs"]
    );
}

#[test]
fn cfg_editor_full_path_program_recognized_via_file_stem() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("/usr/local/bin/nvim", path, Some(4)),
        vec!["/usr/local/bin/nvim", "+4", "+normal! zt", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("/usr/bin/emacs", path, Some(4)),
        vec!["/usr/bin/emacs", "+4", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("/opt/vscode/code", path, Some(4)),
        vec!["/opt/vscode/code", "-g", "/tmp/a.rs:4"]
    );
}

#[test]
fn cfg_editor_line_token_suppresses_auto_injection() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("vim +{line}", path, Some(5)),
        vec!["vim", "+5", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("vim +{line} {path}", path, Some(5)),
        vec!["vim", "+5", "/tmp/a.rs"]
    );
    assert_eq!(
        build_argv("vim +{line} {path}", path, None),
        vec!["vim", "+1", "/tmp/a.rs"]
    );
}

#[test]
fn cfg_editor_unknown_editor_leaves_argv_unchanged() {
    let path = Path::new("/tmp/a.rs");
    assert_eq!(
        build_argv("some-fancy-editor", path, Some(99)),
        vec!["some-fancy-editor", "/tmp/a.rs"]
    );
}

#[test]
fn cfg_editor_resolve_applies_line_per_editor() {
    let mut ec = EditorConfig {
        command: "nvim".into(),
        ext: HashMap::new(),
    };
    ec.ext.insert("md".into(), "code -w".into());
    assert_eq!(
        ec.resolve(Path::new("/x/a.md"), Some(10)),
        vec!["code", "-g", "-w", "/x/a.md:10"]
    );
    assert_eq!(
        ec.resolve(Path::new("/x/main.rs"), Some(50)),
        vec!["nvim", "+50", "+normal! zt", "/x/main.rs"]
    );
}

#[test]
fn cfg_git_defaults() {
    let d = GitConfig::default();
    assert_eq!(d.tool, "lazygit");
    assert_eq!(d.diff, "unified");
    assert_eq!(d.main_branch, "");
    assert_eq!(d.worktree_dir, "../");
    let g = Config::default().git;
    assert_eq!(g.tool, "lazygit");
    assert_eq!(g.diff, "unified");
    assert_eq!(g.main_branch, "");
    assert_eq!(g.worktree_dir, "../");
}

#[test]
fn cfg_git_parses_from_toml() {
    let cfg: Config = toml::from_str("[git]\ntool = \"tig status\"\ndiff = \"split\"\n").unwrap();
    assert_eq!(cfg.git.tool, "tig status");
    assert_eq!(cfg.git.diff, "split");
    assert_eq!(cfg.git.main_branch, "");
    assert_eq!(cfg.git.worktree_dir, "../");
    let cfg2: Config = toml::from_str("[git]\nmain_branch = \"develop\"\n").unwrap();
    assert_eq!(cfg2.git.main_branch, "develop");
    assert_eq!(cfg2.git.tool, "lazygit");
    assert_eq!(cfg2.git.diff, "unified");
    assert_eq!(cfg2.git.worktree_dir, "../");
    let cfg3: Config = toml::from_str("[git]\nworktree_dir = \"../wt\"\n").unwrap();
    assert_eq!(cfg3.git.worktree_dir, "../wt");
}

// =============================================================================
// [jj] — jj (Jujutsu) settings that have no git counterpart
// =============================================================================

#[test]
fn cfg_jj_tool_default_and_roundtrip() {
    let d = JjConfig::default();
    assert_eq!(d.tool, "lazyjj");
    assert_eq!(Config::default().jj.tool, "lazyjj");

    let cfg: Config = toml::from_str("[jj]\ntool = \"jjui\"\n").unwrap();
    assert_eq!(cfg.jj.tool, "jjui");

    // Absent `[jj]` section entirely keeps the default, same as every other sub-table in this file.
    let cfg: Config = toml::from_str("[ui]\n").unwrap();
    assert_eq!(cfg.jj.tool, "lazyjj");
}

// =============================================================================
// [external] — on/off switches for every external process konoma can launch
// =============================================================================

/// Absent `[external]` (and every field within it) defaults to fully enabled — the toggle changes
/// nothing unless a user opts in to turning something off.
#[test]
fn cfg_external_defaults_all_true() {
    let d = ExternalConfig::default();
    assert!(d.git);
    assert!(d.git_tool);
    assert!(d.pdf);
    assert!(d.video);
    assert!(d.remote_images);
    assert!(d.open_links);
    assert!(d.preview_commands);

    let e = Config::default().external;
    assert!(e.git);
    assert!(e.git_tool);
    assert!(e.pdf);
    assert!(e.video);
    assert!(e.remote_images);
    assert!(e.open_links);
    assert!(e.preview_commands);

    // No `[external]` section at all in the file.
    let cfg: Config = toml::from_str("[ui]\n").unwrap();
    assert!(cfg.external.git);
    assert!(cfg.external.git_tool);
    assert!(cfg.external.pdf);
    assert!(cfg.external.video);
    assert!(cfg.external.remote_images);
    assert!(cfg.external.open_links);
    assert!(cfg.external.preview_commands);

    // An empty `[external]` table (present but nothing set).
    let cfg2: Config = toml::from_str("[external]\n").unwrap();
    assert!(cfg2.external.git);
    assert!(cfg2.external.git_tool);
    assert!(cfg2.external.pdf);
    assert!(cfg2.external.video);
    assert!(cfg2.external.remote_images);
    assert!(cfg2.external.open_links);
    assert!(cfg2.external.preview_commands);
}

/// `[external] vcs`: default `"auto"`; `"git"`/`"jj"` round-trip through TOML verbatim, and so does
/// an unrecognized value — this parsing layer stores the raw string with no validation at all.
/// Deciding what an unrecognized value *means* ("unknown -> auto") is `Preference::parse`'s job, not
/// this one's, same split as `filter_mode`/`md_details`/every other mode string in this file. Had
/// zero coverage before this: `cfg_external_defaults_all_true` and its siblings above only ever
/// exercised the six boolean toggles, never this field — which is exactly how `detect()`'s explicit
/// `"git"`/`"jj"` path went untested end to end (see `src/vcs/mod.rs`'s `detect_with` tests).
#[test]
fn cfg_external_vcs_default_and_roundtrip() {
    assert_eq!(ExternalConfig::default().vcs, "auto");
    assert_eq!(Config::default().external.vcs, "auto");

    let cfg: Config = toml::from_str("[external]\nvcs = \"git\"\n").unwrap();
    assert_eq!(cfg.external.vcs, "git");
    let cfg: Config = toml::from_str("[external]\nvcs = \"jj\"\n").unwrap();
    assert_eq!(cfg.external.vcs, "jj");

    // Absent `[external]` section entirely, and a present-but-empty one, both keep the default.
    let cfg: Config = toml::from_str("[ui]\n").unwrap();
    assert_eq!(cfg.external.vcs, "auto");
    let cfg: Config = toml::from_str("[external]\n").unwrap();
    assert_eq!(cfg.external.vcs, "auto");

    // No validation at this layer: an unrecognized value is stored verbatim...
    let cfg: Config = toml::from_str("[external]\nvcs = \"mercurial\"\n").unwrap();
    assert_eq!(cfg.external.vcs, "mercurial");
    // ...and it is `Preference::parse` — not this struct — that decides an unknown value means auto.
    assert_eq!(
        crate::vcs::Preference::parse(&cfg.external.vcs),
        crate::vcs::Preference::Auto
    );
}

/// (field name, its accessor) — used by `cfg_external_each_field_parses_independently`.
type ExternalFieldCase = (&'static str, fn(&ExternalConfig) -> bool);

/// Each field parses independently — flipping one to `false` leaves the rest at their default `true`.
#[test]
fn cfg_external_each_field_parses_independently() {
    let cases: &[ExternalFieldCase] = &[
        ("git", |e| e.git),
        ("git_tool", |e| e.git_tool),
        ("pdf", |e| e.pdf),
        ("video", |e| e.video),
        ("remote_images", |e| e.remote_images),
        ("open_links", |e| e.open_links),
        ("preview_commands", |e| e.preview_commands),
    ];
    for (field, getter) in cases {
        let toml = format!("[external]\n{field} = false\n");
        let cfg: Config = toml::from_str(&toml).unwrap();
        assert!(
            !getter(&cfg.external),
            "{field} = false should parse as false"
        );
        // The other six fields stay at their default `true`.
        let others_true = cases
            .iter()
            .filter(|(f, _)| f != field)
            .all(|(_, g)| g(&cfg.external));
        assert!(others_true, "{field}=false must not disturb other fields");
    }
}

/// All seven flipped to `false` at once.
#[test]
fn cfg_external_all_false_parses() {
    let toml = "\
[external]
git = false
git_tool = false
pdf = false
video = false
remote_images = false
open_links = false
preview_commands = false
";
    let cfg: Config = toml::from_str(toml).unwrap();
    let e = &cfg.external;
    assert!(
        !e.git
            && !e.git_tool
            && !e.pdf
            && !e.video
            && !e.remote_images
            && !e.open_links
            && !e.preview_commands
    );
}

/// A type-mismatched value (a string where a bool is expected) fails the **whole** config parse —
/// the same as every other plain bool field in this config (e.g. `[ui] icons = "nope"`), not a
/// silent per-field fallback. `Config::load_reporting` already turns that failure into
/// `Config::default()` (so `external.*` stays `true`) plus a startup message, same as any other
/// broken config file.
#[test]
fn cfg_external_field_wrong_type_fails_whole_parse_like_other_bool_fields() {
    assert!(toml::from_str::<Config>("[external]\ngit = \"nope\"\n").is_err());
    // Sanity: the same failure mode already exists for an established bool field.
    assert!(toml::from_str::<Config>("[ui]\nicons = \"nope\"\n").is_err());
}

// =============================================================================
// Shipped example configs: every key must land on the struct that owns it
// =============================================================================
//
// Regression coverage for a0dbe2f: a `[jj]` header sitting in the middle of `[external]`'s keys is
// still valid TOML, so `shipped_example_configs_parse` (which only checks that the files parse as a
// `Config`, plus a couple of `[ui]` spot-checks) stayed green while five `[external]` switches
// silently parsed into `JjConfig` — a table with no such fields — and were dropped by serde. This
// test asserts, for every top-level and nested section in both example configs, that each key
// written there is actually a field of the struct that section deserializes into.

/// Extracts the `pub <name>:` field names of `pub struct <struct_name> { ... }`, in declaration
/// order, straight from `source`. Deliberately NOT a hard-coded list: a field added to the struct
/// and forgotten in the example config must show up here as a new key to check, not silently drop
/// out of coverage. Mirrors the technique `.claude/skills/konoma-docs/SKILL.md` uses for its
/// config-vs-reference cross-check (find the struct marker, find its closing `\n}`, then take every
/// line whose first token is `pub`).
fn struct_pub_field_names(source: &str, struct_name: &str) -> Vec<String> {
    let marker = format!("pub struct {struct_name} {{");
    let start = source.find(&marker).unwrap_or_else(|| {
        panic!("`{marker}` not found in src/config/mod.rs — struct renamed or moved?")
    });
    let body_start = start + marker.len();
    let end = source[body_start..]
        .find("\n}")
        .map(|p| body_start + p)
        .unwrap_or_else(|| panic!("no closing `}}` found for struct {struct_name}"));
    let body = &source[body_start..end];
    body.lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("pub ")?;
            let colon = rest.find(':')?;
            let name = rest[..colon].trim();
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| name.to_string())
        })
        .collect()
}

/// Looks up a dotted TOML path (e.g. `"ui.sort"`) from the root table, returning the sub-table only
/// if every segment exists and is itself a table.
fn toml_subtable<'a>(root: &'a toml::Table, dotted: &str) -> Option<&'a toml::Table> {
    let mut cur = root;
    for part in dotted.split('.') {
        cur = cur.get(part)?.as_table()?;
    }
    Some(cur)
}

#[test]
fn shipped_example_configs_keys_land_on_the_struct_that_owns_them() {
    let src_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/config/mod.rs");
    let source = std::fs::read_to_string(&src_path)
        .unwrap_or_else(|e| panic!("cannot read src/config/mod.rs: {e}"));

    // (TOML path, owning struct). The field *list* for each is read live from `source` above, but
    // which struct a nested path belongs to (`ui.sort` -> `SortConfig`) is structural knowledge a
    // field-name scan can't derive on its own, so it's named here.
    //
    // Deliberately NOT covered below, with reasons (do not add a path here without one):
    //   - `[keys]` / `[keys.<surface>]`: `KeysConfig::surfaces` is `#[serde(flatten)]`, so every
    //     key not matching a named field is legal there (captured as a keymap surface/chord) — there
    //     is no fixed field list a key could be "misplaced" against.
    //   - `[editor.ext]`: `EditorConfig::ext` is an open `HashMap<extension, command>` — any
    //     extension name is a legal key by design (see its doc comment in config/mod.rs).
    //   - `[[preview.rules]]`: not a plain table but a TOML array of tables, so it can't be walked
    //     via `toml_subtable` like the others; it is checked separately below against `Rule`'s
    //     fields directly.
    let sections: &[(&str, &str)] = &[
        ("ui", "UiConfig"),
        ("ui.sort", "SortConfig"),
        ("ui.theme", "ThemeConfig"),
        ("preview", "PreviewConfig"),
        ("git", "GitConfig"),
        ("jj", "JjConfig"),
        ("external", "ExternalConfig"),
        ("editor", "EditorConfig"),
    ];

    let fields_by_section: HashMap<&str, Vec<String>> = sections
        .iter()
        .map(|&(path, s)| (path, struct_pub_field_names(&source, s)))
        .collect();
    assert!(
        fields_by_section.values().all(|f| !f.is_empty()),
        "a struct's field extraction came back empty (extraction likely broke): {fields_by_section:?}"
    );

    // Reverse index (field name -> owning section path(s)) so a failure can name where a misplaced
    // key actually belongs, not just that it doesn't belong where it sits.
    let mut owner_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (&path, fields) in &fields_by_section {
        for f in fields {
            owner_of.entry(f.as_str()).or_default().push(path);
        }
    }

    let rule_fields = struct_pub_field_names(&source, "Rule");
    assert!(!rule_fields.is_empty());

    for name in ["config.example.toml", "config.example.ja.toml"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("skipping {name} — not present (published-crate build?)");
            continue;
        };
        let root: toml::Table = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("{name} does not parse as raw TOML: {e}"));

        for &(section_path, struct_name) in sections {
            let Some(sub) = toml_subtable(&root, section_path) else {
                continue; // section absent (fully commented out) — nothing to check
            };
            let owned = &fields_by_section[section_path];
            for key in sub.keys() {
                if owned.iter().any(|f| f == key) {
                    continue;
                }
                let elsewhere: Vec<&str> = owner_of
                    .get(key.as_str())
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|p| *p != section_path)
                    .collect();
                let hint = if elsewhere.is_empty() {
                    "no struct covered by this test declares it either — likely a typo".to_string()
                } else {
                    format!("it belongs under [{}]", elsewhere.join("] / ["))
                };
                panic!(
                    "{name}: key `{key}` is written in [{section_path}] but `{struct_name}` \
                     (which [{section_path}] deserializes into) has no such field — {hint}. This is \
                     the a0dbe2f bug shape: a misplaced section header lets keys parse into the wrong \
                     table, where serde drops them silently instead of erroring."
                );
            }
        }

        // `[[preview.rules]]`: a TOML array of tables, checked against `Rule`'s fields directly
        // rather than through `toml_subtable` (which only understands plain sub-tables).
        if let Some(rules) = root
            .get("preview")
            .and_then(|v| v.as_table())
            .and_then(|t| t.get("rules"))
            .and_then(|v| v.as_array())
        {
            for (i, rule) in rules.iter().enumerate() {
                let Some(rt) = rule.as_table() else {
                    panic!("{name}: [[preview.rules]] #{i} is not a table");
                };
                for key in rt.keys() {
                    assert!(
                        rule_fields.iter().any(|f| f == key),
                        "{name}: [[preview.rules]] #{i} has key `{key}` but `Rule` has no such \
                         field (Rule's fields: {rule_fields:?})"
                    );
                }
            }
        }
    }
}
