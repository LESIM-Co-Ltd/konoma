//! `konoma --plugins`: which plugin shows which files.

use konoma_plugin::registry::PluginOrigin;
use konoma_plugin::{Priority, Target};
use unicode_width::UnicodeWidthStr;

use super::{build_registry, disabled_ids, is_internal, table_problems, user_rules};
use crate::config::Config;

fn target_text(t: &Target) -> String {
    match t {
        Target::Glob(g) => format!("glob {g}"),
        Target::FileName(n) => format!("name {n}"),
        Target::Mime(m) => format!("mime {m}"),
        Target::Content(c) => format!("content {c}"),
        Target::Embed(e) => format!("embed {e}"),
    }
}

/// `text` without control characters. The table is printed straight to the terminal and holds
/// strings that come from the config (a rule's glob, a plugin name, a key in a `[plugins.*]`
/// table): an ESC in one must not reach the terminal as a control sequence.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// The table `konoma --plugins` prints, in the order a file is decided: the user's
/// `[[preview.rules]]` first, then the standard plugins in registration order.
pub(crate) fn plugins_table(cfg: &Config) -> String {
    let mut disabled = disabled_ids(&cfg.plugins);
    // The programs of their own that were loaded (`command`, `--plugin`) are part of the registry.
    let external = cfg.external_plugins.as_deref();
    if let Some(token) = external {
        disabled.push(super::external::marker(token));
    }
    let external_ids: Vec<String> = external
        .map(|t| {
            super::external::plugins_of(t)
                .iter()
                .map(|p| konoma_plugin::Plugin::manifest(p.as_ref()).id)
                .collect()
        })
        .unwrap_or_default();
    let registry = build_registry(&disabled);
    let mut rows: Vec<[String; 5]> = Vec::new();
    // The path only matters for `builtin = "document"`, which covers Word and presentations.
    let probe = std::path::Path::new("");
    for (i, rule) in cfg.preview.rules.iter().enumerate() {
        let one = std::slice::from_ref(rule);
        for u in user_rules(one, probe) {
            let plugin = if rule.builtin.as_deref() == Some("document") {
                "document/presentation".to_string()
            } else {
                u.plugin.clone()
            };
            let enabled = if plugin.contains('/') {
                registry.is_enabled("document") || registry.is_enabled("presentation")
            } else {
                registry.is_enabled(&u.plugin)
            };
            rows.push([
                target_text(&u.target),
                plugin,
                yes(is_internal(&u.plugin) || registry.manifest(&u.plugin).is_some()).into(),
                yes(enabled).into(),
                format!("[[preview.rules]] #{}", i + 1),
            ]);
        }
    }
    for r in registry.describe() {
        if r.priority != Priority::Default {
            continue;
        }
        rows.push([
            target_text(&r.target),
            r.plugin.clone(),
            yes(r.origin == PluginOrigin::Standard).into(),
            yes(r.enabled).into(),
            if external_ids.contains(&r.plugin) {
                "external".into()
            } else {
                "built-in".into()
            },
        ]);
    }
    for row in &mut rows {
        for cell in row.iter_mut() {
            *cell = clean(cell);
        }
    }
    let head = ["ASSIGNMENT", "PLUGIN", "STANDARD", "ENABLED", "SOURCE"];
    let mut widths = head.map(str::len);
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(UnicodeWidthStr::width(cell.as_str()));
        }
    }
    let line = |cells: [&str; 5]| {
        let mut s = String::new();
        for (i, (cell, w)) in cells.iter().zip(widths).enumerate() {
            if i > 0 {
                s.push_str("  ");
            }
            if i == 4 {
                s.push_str(cell);
            } else {
                // Padded by display width (a wide character takes two columns), not by `char`s.
                s.push_str(cell);
                s.push_str(&" ".repeat(w.saturating_sub(UnicodeWidthStr::width(*cell))));
            }
        }
        s.push('\n');
        s
    };
    let mut out = String::from(
        "A file is shown by the first assignment that matches, top to bottom.\n\
         No match: plain text if the file reads as text, otherwise [can not preview].\n\n",
    );
    out.push_str(&line(head));
    for row in &rows {
        out.push_str(&line([&row[0], &row[1], &row[2], &row[3], &row[4]]));
    }
    if let Some(token) = external {
        let lines = super::external::describe(token);
        if !lines.is_empty() {
            out.push_str("\nExternal plugins (ID  VERSION  COMMAND  SOURCE):\n");
            for l in lines {
                out.push_str(&format!("  {}\n", clean(&l)));
            }
        }
        for problem in super::external::problems(token, crate::i18n::Lang::En) {
            out.push_str(&format!("warning: {}\n", clean(&problem)));
        }
    }
    out.push_str(
        "\nTurn a standard plugin off with `[plugins.<id>] enabled = false` in the config.\n",
    );
    for problem in table_problems(&cfg.plugins, crate::i18n::Lang::En) {
        out.push_str(&format!("warning: {}\n", clean(&problem)));
    }
    out
}
