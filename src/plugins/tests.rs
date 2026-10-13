//! The plugin registration step: who shows which file.
//!
//! The guard test keeps a copy of the thirteen default `[[preview.rules]]` konoma had before the
//! plugins and the matcher that read them, and compares it with the new decision over a wide list
//! of names and contents. The copy lives here on purpose: it is the evidence that nothing moved.

use std::path::{Path, PathBuf};

use konoma_plugin::{LaunchMode, Launcher, Verb};

use super::*;
use crate::config::{Config, Rule};
use crate::preview::{archive, PreviewKind};
use crate::test_support::unique_tmp;

// ---- the old implementation (kept as the reference) ----------------------------------------

fn old_rule(glob: Option<&str>, mime: Option<&str>, builtin: &str) -> Rule {
    Rule {
        glob: glob.map(str::to_string),
        mime: mime.map(str::to_string),
        builtin: Some(builtin.to_string()),
        ..Rule::default()
    }
}

/// The thirteen rules `PreviewConfig::default` held before the plugins, in the same order.
fn old_default_rules() -> Vec<Rule> {
    vec![
        old_rule(Some("*.md"), None, "markdown"),
        old_rule(Some("*.{mmd,mermaid}"), None, "mermaid"),
        old_rule(Some("*.svg"), None, "svg"),
        old_rule(None, Some("image/*"), "image"),
        old_rule(Some("*.csv"), None, "csv"),
        old_rule(Some("*.tsv"), None, "tsv"),
        old_rule(
            Some("*.{xlsx,xlsm,xltx,xltm,xlsb,xls,ods}"),
            None,
            "spreadsheet",
        ),
        old_rule(
            Some("*.{docx,docm,dotx,dotm,odt,ott,pptx,pptm,ppsx,ppsm,potx,potm,odp,otp}"),
            None,
            "document",
        ),
        old_rule(Some("*.{zip,tar,tgz}"), None, "archive"),
        old_rule(Some("*.tar.gz"), None, "archive"),
        old_rule(
            Some("*.{rs,ts,tsx,js,py,go,toml,json,sh,yaml,yml,c,cpp,h}"),
            None,
            "code",
        ),
        old_rule(None, Some("video/*"), "video"),
        old_rule(Some("*.pdf"), None, "pdf"),
    ]
}

fn old_mime_glob_match(pattern: &str, mime: &str) -> bool {
    match pattern.strip_suffix("/*") {
        Some(prefix) => mime.starts_with(prefix),
        None => pattern == mime,
    }
}

fn old_rule_matches(rule: &Rule, path: &Path, sniff: Option<&[u8]>) -> bool {
    if let Some(glob) = &rule.glob {
        if let Ok(set) = globset::GlobBuilder::new(glob)
            .case_insensitive(true)
            .build()
            .map(|g| g.compile_matcher())
        {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if set.is_match(name) {
                return true;
            }
        }
    }
    if let Some(mime_pat) = &rule.mime {
        let kind = match sniff {
            Some(bytes) => infer::get(bytes),
            None => infer::get_from_path(path).ok().flatten(),
        };
        if let Some(kind) = kind {
            if old_mime_glob_match(mime_pat, kind.mime_type()) {
                return true;
            }
        }
    }
    false
}

fn old_from_rule(rule: &Rule, path: &Path) -> PreviewKind {
    let p = path.to_path_buf();
    if let Some(builtin) = rule.builtin.as_deref() {
        return match builtin {
            "markdown" => PreviewKind::Markdown(p),
            "mermaid" => PreviewKind::Mermaid(p),
            "image" => PreviewKind::Image(p),
            "svg" => PreviewKind::Svg(p),
            "video" => PreviewKind::Video(p),
            "pdf" => PreviewKind::Pdf(p),
            "code" => PreviewKind::Code(p),
            // The note is the old `builtin` name: the separator settled when the file is decided.
            "csv" | "tsv" => PreviewKind::Plugin {
                path: p,
                id: "csv".to_string(),
                hint: Some(builtin.to_string()),
            },
            "archive" => match archive::ArchiveKind::from_path(path) {
                Some(_) => PreviewKind::Plugin {
                    path: p,
                    id: "archive".to_string(),
                    hint: None,
                },
                None => PreviewKind::can_not_preview(path),
            },
            "spreadsheet" => PreviewKind::Spreadsheet(p),
            "document" => PreviewKind::Document(p),
            "text" => PreviewKind::Text(p),
            _ => PreviewKind::can_not_preview(path),
        };
    }
    if let Some(template) = rule.command.as_deref() {
        return PreviewKind::Command {
            path: p,
            template: template.to_string(),
            render_as: rule.render_as.clone(),
            detached: rule.detached,
        };
    }
    PreviewKind::can_not_preview(path)
}

/// The old decision, with the user rules replacing the defaults the way they used to.
fn old_resolve(
    rules: &[Rule],
    preview_commands: bool,
    path: &Path,
    sniff: Option<&[u8]>,
) -> PreviewKind {
    for rule in rules {
        if old_rule_matches(rule, path, sniff) {
            let kind = old_from_rule(rule, path);
            if matches!(kind, PreviewKind::Command { .. }) && !preview_commands {
                return PreviewKind::can_not_preview(path);
            }
            return kind;
        }
    }
    let is_text = match sniff {
        Some(bytes) => crate::preview::text::is_probably_text_bytes(bytes),
        None => crate::preview::text::is_probably_text(path),
    };
    if is_text {
        PreviewKind::Text(path.to_path_buf())
    } else {
        PreviewKind::can_not_preview(path)
    }
}

// ---- the guard ------------------------------------------------------------------------------

/// Every extension that appears in a default glob, plus near misses and ones nothing claims.
const EXTENSIONS: &[&str] = &[
    "md", "mmd", "mermaid", "svg", "csv", "tsv", "xlsx", "xlsm", "xltx", "xltm", "xlsb", "xls",
    "ods", "docx", "docm", "dotx", "dotm", "odt", "ott", "pptx", "pptm", "ppsx", "ppsm", "potx",
    "potm", "odp", "otp", "zip", "tar", "tgz", "tar.gz", "rs", "ts", "tsx", "js", "py", "go",
    "toml", "json", "sh", "yaml", "yml", "c", "cpp", "h", "pdf",
    // Raster and video names (decided by content, not by name).
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "mp4", "mov", "mkv", "webm", "avi",
    // Near misses and unclaimed names.
    "xlsxx", "doc", "ppt", "rtf", "txt", "rb", "log", "exe", "gz", "mdx", "markdown", "svgz",
    "csv.bak", "tar.bz2", "bin",
];

/// Whole file names with no extension, or that are special.
const BARE_NAMES: &[&str] = &[
    "README",
    "LICENSE",
    "Makefile",
    "Dockerfile",
    ".bashrc",
    ".gitignore",
    "noext",
    "a.",
    ".md",
    ".pdf",
];

/// A few contents: what the file reads as decides the `image/*` and `video/*` rules and the
/// text fallback.
fn contents() -> Vec<(&'static str, Vec<u8>)> {
    let mut mp4 = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom".to_vec();
    mp4.extend(std::iter::repeat_n(0u8, 64));
    let mut mkv = b"\x1a\x45\xdf\xa3\x93\x42\x82\x88matroska".to_vec();
    mkv.extend(std::iter::repeat_n(0u8, 64));
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(std::iter::repeat_n(0u8, 64));
    vec![
        ("empty", Vec::new()),
        ("text", b"hello\nworld\n".to_vec()),
        ("binary", b"\x00\x01\x02\x03binary".to_vec()),
        ("png", png),
        ("jpeg", b"\xff\xd8\xff\xe0\x00\x10JFIF\x00".to_vec()),
        ("gif", b"GIF89a\x01\x00\x01\x00".to_vec()),
        ("mp4", mp4),
        ("mkv", mkv),
        ("pdf", b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec()),
        ("zip", b"PK\x03\x04\x14\x00\x00\x00\x08\x00".to_vec()),
        (
            "svg-text",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
        ),
    ]
}

fn name_variants() -> Vec<String> {
    let mut out = Vec::new();
    for ext in EXTENSIONS {
        out.push(format!("a.{ext}"));
        out.push(format!("A.{}", ext.to_uppercase()));
        out.push(format!("Mixed.{}", {
            let mut s = ext.to_string();
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            s
        }));
    }
    out.extend(BARE_NAMES.iter().map(|s| s.to_string()));
    out
}

fn assert_same(new: &PreviewKind, old: &PreviewKind, what: &str) {
    assert_eq!(format!("{new:?}"), format!("{old:?}"), "{what}");
}

#[test]
fn default_decision_is_the_old_decision_for_every_name_and_content() {
    let dir = unique_tmp("konoma_plugins_guard");
    let old_rules = old_default_rules();
    let cfg = Config::default();
    assert!(
        cfg.preview.rules.is_empty(),
        "the default config carries no user rules any more"
    );
    let mut compared = 0usize;
    let names = name_variants();
    for (label, bytes) in contents() {
        let sub = dir.join(label);
        std::fs::create_dir_all(&sub).unwrap();
        for name in &names {
            let path = sub.join(name);
            std::fs::write(&path, &bytes).unwrap();
            // By path (the file is read when a rule needs it)...
            assert_same(
                &cfg.resolve_preview(&path),
                &old_resolve(&old_rules, true, &path, None),
                &format!("{label}/{name} by path"),
            );
            // ...and by bytes (a deleted file's old content).
            assert_same(
                &cfg.resolve_preview_with(&path, &bytes),
                &old_resolve(&old_rules, true, &path, Some(&bytes)),
                &format!("{label}/{name} by bytes"),
            );
            compared += 2;
        }
    }
    // A name with no file behind it (a deleted file, nothing to sniff).
    for name in &names {
        let path = dir.join("missing").join(name);
        assert_same(
            &cfg.resolve_preview(&path),
            &old_resolve(&old_rules, true, &path, None),
            &format!("missing/{name}"),
        );
        compared += 1;
    }
    // The one directory and the empty path must not behave differently either.
    for path in [dir.to_path_buf(), PathBuf::new(), PathBuf::from("/")] {
        assert_same(
            &cfg.resolve_preview(&path),
            &old_resolve(&old_rules, true, &path, None),
            &format!("{path:?}"),
        );
        compared += 1;
    }
    assert!(compared > 4000, "the guard compared {compared} cases");
}

#[test]
fn user_rules_compose_with_the_defaults_like_the_old_list_with_the_defaults_after_them() {
    // The old shape of "user rules before the defaults": an old config that wrote the thirteen
    // defaults after its own rules. The new shape (own rules only) must decide the same.
    let dir = unique_tmp("konoma_plugins_compose");
    let mine = vec![
        Rule {
            glob: Some("*.md".into()),
            builtin: Some("code".into()),
            ..Rule::default()
        },
        Rule {
            glob: Some("*.weird".into()),
            command: Some("cat {path}".into()),
            render_as: Some("image".into()),
            detached: true,
            ..Rule::default()
        },
        Rule {
            mime: Some("image/*".into()),
            builtin: Some("text".into()),
            ..Rule::default()
        },
    ];
    let mut old_all = mine.clone();
    old_all.extend(old_default_rules());
    let mut cfg = Config::default();
    cfg.preview.rules = mine;
    let names = name_variants();
    for (label, bytes) in contents() {
        let sub = dir.join(label);
        std::fs::create_dir_all(&sub).unwrap();
        for name in names
            .iter()
            .chain(&["x.weird".to_string(), "X.WEIRD".to_string()])
        {
            let path = sub.join(name);
            std::fs::write(&path, &bytes).unwrap();
            assert_same(
                &cfg.resolve_preview(&path),
                &old_resolve(&old_all, true, &path, None),
                &format!("{label}/{name}"),
            );
        }
    }
}

// ---- composition and the switches -----------------------------------------------------------

fn kind_of_name(cfg: &Config, name: &str) -> PreviewKind {
    // Nothing on disk: the decision is by name (and "can not preview" for the rest).
    cfg.resolve_preview(
        Path::new("/nonexistent/konoma-plugins")
            .join(name)
            .as_path(),
    )
}

#[test]
fn one_user_rule_no_longer_replaces_the_standard_assignments() {
    let mut cfg = Config::default();
    cfg.preview.rules.push(Rule {
        glob: Some("*.rs".into()),
        builtin: Some("text".into()),
        ..Rule::default()
    });
    // The user's rule wins for what it names...
    assert!(matches!(kind_of_name(&cfg, "a.rs"), PreviewKind::Text(_)));
    // ...and everything else is still decided by the standard plugins.
    assert!(matches!(
        kind_of_name(&cfg, "a.md"),
        PreviewKind::Markdown(_)
    ));
    assert!(matches!(kind_of_name(&cfg, "a.pdf"), PreviewKind::Pdf(_)));
    assert!(kind_of_name(&cfg, "a.zip").is_plugin("archive"));
}

#[test]
fn a_rule_with_neither_builtin_nor_command_or_an_unknown_name_stops_the_search() {
    for rule in [
        Rule {
            glob: Some("*.md".into()),
            ..Rule::default()
        },
        Rule {
            glob: Some("*.md".into()),
            builtin: Some("no-such-viewer".into()),
            ..Rule::default()
        },
    ] {
        let mut cfg = Config::default();
        cfg.preview.rules.push(rule);
        match kind_of_name(&cfg, "a.md") {
            PreviewKind::CanNotPreview { ext } => assert_eq!(ext, "md"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn old_builtin_names_read_as_plugins() {
    let one = |builtin: &str, name: &str| {
        let mut cfg = Config::default();
        cfg.preview.rules.push(Rule {
            glob: Some("*.q".into()),
            builtin: Some(builtin.into()),
            ..Rule::default()
        });
        let _ = name;
        cfg.resolve_preview(Path::new("/nonexistent/x.q"))
    };
    assert!(matches!(one("markdown", ""), PreviewKind::Markdown(_)));
    assert!(matches!(one("mermaid", ""), PreviewKind::Mermaid(_)));
    assert!(matches!(one("image", ""), PreviewKind::Image(_)));
    assert!(matches!(one("svg", ""), PreviewKind::Svg(_)));
    assert!(matches!(one("video", ""), PreviewKind::Video(_)));
    assert!(matches!(one("pdf", ""), PreviewKind::Pdf(_)));
    assert!(matches!(one("code", ""), PreviewKind::Code(_)));
    assert!(matches!(
        one("spreadsheet", ""),
        PreviewKind::Spreadsheet(_)
    ));
    assert!(matches!(one("document", ""), PreviewKind::Document(_)));
    assert!(matches!(one("text", ""), PreviewKind::Text(_)));
    let separator = |kind: PreviewKind| match kind {
        PreviewKind::Plugin { id, hint, path } if id == "csv" => {
            konoma_table::delimiter(hint.as_deref(), &path)
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(separator(one("csv", "")), b',');
    assert_eq!(separator(one("tsv", "")), b'\t');
    // `archive` on a name that is not an archive is the safe "can not preview", as before.
    assert!(matches!(
        one("archive", ""),
        PreviewKind::CanNotPreview { .. }
    ));
}

#[test]
fn builtin_document_follows_the_extension_to_document_or_presentation() {
    let rules = vec![Rule {
        glob: Some("*".into()),
        builtin: Some("document".into()),
        ..Rule::default()
    }];
    let plugin_for = |name: &str| {
        user_rules(&rules, Path::new(name))
            .into_iter()
            .map(|u| u.plugin)
            .collect::<Vec<_>>()
    };
    for w in [
        "a.docx",
        "a.DOCM",
        "a.dotx",
        "a.dotm",
        "a.odt",
        "a.ott",
        "a.unknown",
    ] {
        assert_eq!(plugin_for(w), ["document"], "{w}");
    }
    for p in [
        "a.pptx", "a.pptm", "a.ppsx", "a.ppsm", "a.potx", "a.potm", "a.odp", "a.OTP",
    ] {
        assert_eq!(plugin_for(p), ["presentation"], "{p}");
    }
}

#[test]
fn a_rule_with_a_glob_and_a_mime_matches_on_either() {
    let dir = unique_tmp("konoma_plugins_both");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("pic.dat");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    let mut cfg = Config::default();
    cfg.preview.rules.push(Rule {
        glob: Some("*.zzz".into()),
        mime: Some("image/*".into()),
        builtin: Some("code".into()),
        ..Rule::default()
    });
    assert!(matches!(cfg.resolve_preview(&png), PreviewKind::Code(_)));
    assert!(matches!(kind_of_name(&cfg, "a.zzz"), PreviewKind::Code(_)));
}

#[test]
fn a_command_rule_carries_its_template_output_kind_and_detach_flag() {
    let mut cfg = Config::default();
    cfg.preview.rules = vec![
        Rule {
            glob: Some("*.a".into()),
            command: Some("one {path}".into()),
            ..Rule::default()
        },
        Rule {
            glob: Some("*.b".into()),
            command: Some("two {path} {out}".into()),
            render_as: Some("image".into()),
            detached: true,
            ..Rule::default()
        },
    ];
    match kind_of_name(&cfg, "x.a") {
        PreviewKind::Command {
            template,
            render_as,
            detached,
            ..
        } => {
            assert_eq!(template, "one {path}");
            assert_eq!(render_as, None);
            assert!(!detached);
        }
        other => panic!("{other:?}"),
    }
    match kind_of_name(&cfg, "x.b") {
        PreviewKind::Command {
            template,
            render_as,
            detached,
            ..
        } => {
            assert_eq!(template, "two {path} {out}");
            assert_eq!(render_as.as_deref(), Some("image"));
            assert!(detached);
        }
        other => panic!("{other:?}"),
    }
    // The gate: `[external] preview_commands = false` makes the matched rule "can not preview".
    cfg.external.preview_commands = false;
    assert!(matches!(
        kind_of_name(&cfg, "x.b"),
        PreviewKind::CanNotPreview { .. }
    ));
    // And a non-command file is unaffected.
    assert!(matches!(
        kind_of_name(&cfg, "x.md"),
        PreviewKind::Markdown(_)
    ));
}

fn cfg_from(toml_text: &str) -> Config {
    toml::from_str::<Config>(toml_text).unwrap()
}

#[test]
fn enabled_false_hands_the_file_to_the_next_step() {
    let dir = unique_tmp("konoma_plugins_disabled");
    std::fs::create_dir_all(&dir).unwrap();
    let md = dir.join("n.md");
    std::fs::write(&md, "# hi\n").unwrap();
    let png = dir.join("p.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n\0\0\0").unwrap();
    let csv = dir.join("t.csv");
    std::fs::write(&csv, "a,b\n").unwrap();
    let cfg = cfg_from("[plugins.markdown]\nenabled = false\n\n[plugins.image]\nenabled = false\n");
    // A text file with its plugin off is plain text.
    assert!(matches!(cfg.resolve_preview(&md), PreviewKind::Text(_)));
    // A binary file with its plugin off is "can not preview".
    assert!(matches!(
        cfg.resolve_preview(&png),
        PreviewKind::CanNotPreview { .. }
    ));
    // Others are untouched.
    assert!(cfg.resolve_preview(&csv).is_plugin("csv"));
    // `enabled = true` and absent behave the same.
    let on = cfg_from("[plugins.markdown]\nenabled = true\n");
    assert!(matches!(on.resolve_preview(&md), PreviewKind::Markdown(_)));
}

#[test]
fn a_user_rule_aimed_at_a_disabled_plugin_is_skipped() {
    let mut cfg = cfg_from("[plugins.code]\nenabled = false\n");
    cfg.preview.rules.push(Rule {
        glob: Some("*.md".into()),
        builtin: Some("code".into()),
        ..Rule::default()
    });
    assert!(matches!(
        kind_of_name(&cfg, "a.md"),
        PreviewKind::Markdown(_)
    ));
}

#[test]
fn disabling_one_of_two_office_plugins_leaves_the_other() {
    let cfg = cfg_from("[plugins.presentation]\nenabled = false\n");
    assert!(matches!(
        kind_of_name(&cfg, "a.pptx"),
        PreviewKind::CanNotPreview { .. }
    ));
    assert!(matches!(
        kind_of_name(&cfg, "a.docx"),
        PreviewKind::Document(_)
    ));
}

#[test]
fn the_host_internal_pieces_cannot_be_disabled() {
    let mut cfg = cfg_from("[plugins.text]\nenabled = false\n[plugins.command]\nenabled = false\n");
    assert!(matches!(
        cfg.plugin_report(crate::i18n::Lang::En),
        Some(m) if m.contains("`text` cannot be disabled") && m.contains("`command` cannot be disabled")
    ));
    cfg.preview.rules.push(Rule {
        glob: Some("*.q".into()),
        command: Some("echo {path}".into()),
        ..Rule::default()
    });
    // The command rule still works.
    assert!(matches!(
        kind_of_name(&cfg, "a.q"),
        PreviewKind::Command { .. }
    ));
}

#[test]
fn unknown_plugin_ids_and_a_bad_enabled_are_reported() {
    use crate::i18n::Lang;
    let cfg = cfg_from(
        "[plugins.nope]\nenabled = false\n\n[plugins.csv]\nenabled = \"no\"\nrainbow = false\n",
    );
    let en = cfg.plugin_report(Lang::En).unwrap();
    assert!(en.contains("unknown plugin `nope`"), "{en}");
    assert!(
        en.contains("[plugins.csv] enabled must be true or false"),
        "{en}"
    );
    let ja = cfg.plugin_report(Lang::Jp).unwrap();
    assert!(ja.contains("未知のプラグイン `nope`"), "{ja}");
    // A bad `enabled` does not switch anything off.
    assert_eq!(cfg.plugin_disabled(), ["nope"]);
    assert!(kind_of_name(&cfg, "a.csv").is_plugin("csv"));
    // No problem: no message.
    assert_eq!(Config::default().plugin_report(Lang::En), None);
    assert_eq!(
        cfg_from("[plugins.markdown]\nalerts = false\n").plugin_report(Lang::En),
        None
    );
}

#[test]
fn other_keys_of_a_plugin_table_become_settings() {
    let cfg = cfg_from(
        "[plugins.markdown]\nenabled = false\nalerts = false\ntask_states = [\" \", \"/\", \"x\"]\n\
         details = \"closed\"\n[plugins.mermaid]\nrows = 10\n",
    );
    let md = cfg.plugin_settings("markdown");
    assert!(!md.bool("alerts", true));
    assert_eq!(md.str("details", "auto"), "closed");
    assert_eq!(md.list_str("task_states"), [" ", "/", "x"]);
    assert!(md.get("enabled").is_none(), "`enabled` is the host's");
    assert_eq!(cfg.plugin_settings("mermaid").int("rows", 24), 10);
    assert!(cfg.plugin_settings("pdf").is_empty());
    assert_eq!(cfg.plugin_disabled(), ["markdown"]);
}

// ---- the declarations -----------------------------------------------------------------------

#[test]
fn the_standard_declarations_register_and_keep_the_expected_order() {
    let r = build_registry(&[]);
    assert_eq!(
        r.ids(),
        [
            "markdown",
            "mermaid",
            "math",
            "svg",
            "image",
            "csv",
            "spreadsheet",
            "document",
            "presentation",
            "archive",
            "code",
            "video",
            "pdf",
            "text",
            "command",
            "can-not-preview",
        ]
    );
    // The two orders that carry meaning: svg before `image/*`, and the mime plugins (image,
    // video) in the places the old rules had them.
    let ids = r.ids();
    let at = |id: &str| ids.iter().position(|x| *x == id).unwrap();
    assert!(at("svg") < at("image"));
    assert!(at("image") < at("csv"));
    assert!(at("code") < at("video"));
    assert!(at("spreadsheet") < at("document"));
}

/// The standard plugins that are wired to the display: really opened, not declarations only.
const WIRED: &[&str] = &["csv", "archive"];

#[test]
fn the_declared_plugins_refuse_to_open_until_they_are_wired() {
    use konoma_plugin::testing::open_file;
    let r = build_registry(&[]);
    for id in r.ids() {
        if WIRED.contains(&id) {
            continue;
        }
        let plugin = r.plugin(id).unwrap();
        let err = open_file(&*plugin, Path::new("/nonexistent/x")).err();
        assert!(
            matches!(err, Some(f) if f.kind == konoma_plugin::FailureKind::Unsupported),
            "{id}"
        );
    }
}

#[test]
fn the_wired_plugins_are_the_real_ones_and_open_their_files() {
    use konoma_plugin::testing::open_file;
    let r = build_registry(&[]);
    let dir = unique_tmp("konoma_plugins_wired");
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("a.csv");
    std::fs::write(&csv, "h\n1\n").unwrap();
    let session = open_file(&*r.plugin("csv").unwrap(), &csv);
    assert!(session.is_ok(), "csv opens a csv");
    // A file that is not an archive is refused by the archive plugin on its own terms (a
    // declaration-only plugin would say "not opened through the plugin API yet").
    let txt = dir.join("a.txt");
    std::fs::write(&txt, "x").unwrap();
    let err = open_file(&*r.plugin("archive").unwrap(), &txt)
        .err()
        .unwrap();
    assert!(!err.message.en().contains("declared"), "{err}");
    for id in WIRED {
        assert!(r.ids().contains(id), "{id} is registered");
    }
}

#[test]
fn every_standard_manifest_is_clean() {
    for m in standard_manifests() {
        let issues = konoma_plugin::testing::check_manifest(&Declared(m.clone()));
        assert!(issues.is_empty(), "{}: {issues:?}", m.id);
        assert_eq!(m.api, konoma_plugin::ApiVersion::CURRENT);
    }
}

#[test]
fn verbs_follow_the_decided_chains() {
    for m in standard_manifests() {
        let verb = |v: Verb| m.verbs.iter().find(|d| d.verb == v);
        match m.id.as_str() {
            // Embedded only: no file to edit or open.
            "math" => assert!(m.verbs.is_empty()),
            "spreadsheet" | "document" | "presentation" => {
                for v in [Verb::Edit, Verb::Open] {
                    let d = verb(v).unwrap();
                    assert_eq!(d.mode, LaunchMode::Gui, "{}", m.id);
                    // Microsoft app, LibreOffice, then the OS default.
                    assert!(
                        matches!(&d.chain[0], Launcher::MacApp { bundle_id } if bundle_id.starts_with("com.microsoft.")),
                        "{}",
                        m.id
                    );
                    assert!(matches!(
                        &d.chain[1],
                        Launcher::MacApp { bundle_id } if bundle_id == "org.libreoffice.script"
                    ));
                    assert_eq!(d.chain.last(), Some(&Launcher::OsDefault));
                }
                let want = match m.id.as_str() {
                    "spreadsheet" => "com.microsoft.Excel",
                    "document" => "com.microsoft.Word",
                    _ => "com.microsoft.Powerpoint",
                };
                assert!(matches!(
                    &verb(Verb::Edit).unwrap().chain[0],
                    Launcher::MacApp { bundle_id } if bundle_id == want
                ));
            }
            _ => {
                let e = verb(Verb::Edit).unwrap();
                assert_eq!(e.chain, [Launcher::Editor], "{}", m.id);
                assert_eq!(e.mode, LaunchMode::Terminal);
                assert!(e.line_token);
                let o = verb(Verb::Open).unwrap();
                assert_eq!(o.chain, [Launcher::OsDefault], "{}", m.id);
                assert_eq!(o.mode, LaunchMode::Gui);
            }
        }
    }
}

#[test]
fn declared_setting_defaults_equal_the_ui_defaults_they_will_replace() {
    use konoma_plugin::Value;
    let ui = crate::config::UiConfig::default();
    let manifests = standard_manifests();
    let default_of = |plugin: &str, name: &str| -> Value {
        manifests
            .iter()
            .find(|m| m.id == plugin)
            .unwrap_or_else(|| panic!("no plugin {plugin}"))
            .settings
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no setting {plugin}.{name}"))
            .default
            .clone()
    };
    let st = |x: &str| Value::Str(x.to_string());
    let list = |xs: &[String]| Value::List(xs.iter().map(|x| st(x)).collect());
    assert_eq!(
        default_of("markdown", "task_states"),
        list(&ui.md_task_states)
    );
    assert_eq!(
        default_of("markdown", "autolink"),
        Value::Bool(ui.md_autolink)
    );
    assert_eq!(default_of("markdown", "alerts"), Value::Bool(ui.md_alerts));
    assert_eq!(default_of("markdown", "emoji"), Value::Bool(ui.md_emoji));
    assert_eq!(
        default_of("markdown", "frontmatter"),
        Value::Bool(ui.md_frontmatter)
    );
    assert_eq!(
        default_of("markdown", "footnotes"),
        Value::Bool(ui.md_footnotes)
    );
    assert_eq!(
        default_of("markdown", "inline_html"),
        Value::Bool(ui.md_inline_html)
    );
    assert_eq!(default_of("markdown", "details"), st(&ui.md_details));
    assert_eq!(
        default_of("markdown", "table_align"),
        st(&ui.md_table_align)
    );
    assert_eq!(
        default_of("markdown", "image_align"),
        st(&ui.md_image_align)
    );
    assert_eq!(default_of("mermaid", "mode"), st(&ui.mermaid));
    assert_eq!(default_of("mermaid", "theme"), st(&ui.mermaid_theme));
    assert_eq!(default_of("mermaid", "curve"), st(&ui.mermaid_curve));
    assert_eq!(default_of("mermaid", "routing"), st(&ui.mermaid_routing));
    assert_eq!(
        default_of("mermaid", "rows"),
        Value::Int(i64::from(ui.mermaid_rows))
    );
    assert_eq!(default_of("math", "mode"), st(&ui.math));
    assert_eq!(default_of("math", "color"), st(&ui.math_color));
    assert_eq!(default_of("csv", "rainbow"), Value::Bool(ui.csv_rainbow));
    assert_eq!(
        default_of("code", "syntax_highlight"),
        Value::Bool(ui.syntax_highlight)
    );
    assert_eq!(
        default_of("svg", "max_px"),
        Value::Int(i64::from(ui.svg_max_px))
    );
}

#[test]
fn the_default_assignments_are_the_thirteen_old_conditions() {
    // Every glob and mime of the old list appears as a binding of the plugin it named.
    let rows = build_registry(&[]).describe();
    let has = |target: konoma_plugin::Target, plugin: &str| {
        rows.iter()
            .any(|r| r.target == target && r.plugin == plugin)
    };
    use konoma_plugin::Target::{Glob, Mime};
    assert!(has(Glob("*.md".into()), "markdown"));
    assert!(has(Glob("*.{mmd,mermaid}".into()), "mermaid"));
    assert!(has(Glob("*.svg".into()), "svg"));
    assert!(has(Mime("image/*".into()), "image"));
    assert!(has(Glob("*.csv".into()), "csv"));
    assert!(has(Glob("*.tsv".into()), "csv"));
    assert!(has(Glob("*.{zip,tar,tgz}".into()), "archive"));
    assert!(has(Glob("*.tar.gz".into()), "archive"));
    assert!(has(Mime("video/*".into()), "video"));
    assert!(has(Glob("*.pdf".into()), "pdf"));
}

// ---- konoma --plugins -----------------------------------------------------------------------

#[test]
fn the_plugins_table_lists_user_rules_first_and_marks_disabled_plugins() {
    let mut cfg = cfg_from("[plugins.pdf]\nenabled = false\n");
    cfg.preview.rules = vec![
        Rule {
            glob: Some("*.log".into()),
            command: Some("tail {path}".into()),
            ..Rule::default()
        },
        Rule {
            glob: Some("*.q".into()),
            mime: Some("application/x-q".into()),
            builtin: Some("document".into()),
            ..Rule::default()
        },
    ];
    let out = plugins_table(&cfg);
    let lines: Vec<&str> = out.lines().collect();
    let head = lines
        .iter()
        .position(|l| l.starts_with("ASSIGNMENT"))
        .unwrap();
    // User rules come first, in order, then the built-in assignments.
    assert!(lines[head + 1].starts_with("glob *.log") && lines[head + 1].contains("command"));
    assert!(lines[head + 1].ends_with("[[preview.rules]] #1"));
    assert!(lines[head + 2].starts_with("glob *.q") && lines[head + 2].ends_with("#2"));
    assert!(lines[head + 3].starts_with("mime application/x-q"));
    assert!(lines[head + 4].starts_with("glob *.md") && lines[head + 4].ends_with("built-in"));
    let pdf = lines.iter().find(|l| l.starts_with("glob *.pdf")).unwrap();
    let cols: Vec<&str> = pdf.split_whitespace().collect();
    assert_eq!(&cols[2..], ["pdf", "yes", "no", "built-in"]);
    // Columns line up.
    let col = |l: &str| l.find("markdown").or(l.find("command")).unwrap();
    assert_eq!(col(lines[head + 1]), col(lines[head + 4]));
}

#[test]
fn the_plugins_table_shows_problems_with_the_plugin_tables() {
    let out = plugins_table(&cfg_from("[plugins.bogus]\nx = 1\n"));
    assert!(
        out.contains("warning: unknown plugin `bogus` (ignored)"),
        "{out}"
    );
}

// ---- one resolution reads the file at most once ---------------------------------------------

#[test]
fn a_decision_asks_for_the_content_only_when_a_rule_needs_it() {
    let dir = unique_tmp("konoma_plugins_lazy");
    let cfg = Config::default();
    // A name rule decides without touching the file (it does not even exist).
    let (_, _) = crate::test_support::count_resolve_preview_calls(|| {
        assert!(matches!(
            cfg.resolve_preview(&dir.join("gone.md")),
            PreviewKind::Markdown(_)
        ));
    });
    // The facts closures are the only readers: count them through the registry directly.
    use konoma_plugin::registry::{FileFacts, Resolution};
    use std::cell::Cell;
    let mime_calls = Cell::new(0);
    let text_calls = Cell::new(0);
    let mime = || {
        mime_calls.set(mime_calls.get() + 1);
        None
    };
    let text = || {
        text_calls.set(text_calls.get() + 1);
        true
    };
    let path = dir.join("README");
    let facts = FileFacts {
        path: &path,
        sniff_mime: &mime,
        is_text: &text,
    };
    let res = with_registry(&[], |r| r.resolve_file(&[], &facts));
    assert_eq!(res, Resolution::Text);
    // image/* and video/* both ask; the sniff still runs once.
    assert_eq!(mime_calls.get(), 1);
    assert_eq!(text_calls.get(), 1);
    let named = dir.join("a.md");
    let facts = FileFacts {
        path: &named,
        sniff_mime: &mime,
        is_text: &text,
    };
    let _ = with_registry(&[], |r| r.resolve_file(&[], &facts));
    assert_eq!(mime_calls.get(), 1, "a name rule needs no sniff");
    assert_eq!(text_calls.get(), 1);
}
