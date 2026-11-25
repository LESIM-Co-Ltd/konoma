//! Rule-derived corpus through the project resolver and konoma's config entry.
//! Both adapters share resolution. Independent literal expectations, not their agreement, are
//! the oracle; each group below names the corresponding HEAD rule for review.
use super::*;
use crate::config::{Config, Rule};
use crate::preview::PreviewKind;
use konoma_host_core::project::DiffTarget;
use konoma_plugin::host::file_preview::{FilePreviewPlan, FilePreviewSource};
use std::path::Path;

fn rule(glob: Option<&str>, mime: Option<&str>, builtin: &str) -> Rule {
    Rule {
        glob: glob.map(str::to_owned),
        mime: mime.map(str::to_owned),
        builtin: Some(builtin.into()),
        ..Default::default()
    }
}

fn plugin(id: &str, hint: Option<&str>) -> FilePreviewPlan {
    FilePreviewPlan::Plugin {
        id: id.into(),
        hint: hint.map(str::to_owned),
    }
}

fn host_plan(kind: PreviewKind) -> Result<FilePreviewPlan, PreviewKind> {
    Ok(match kind {
        PreviewKind::Text(_) => FilePreviewPlan::Text,
        PreviewKind::Plugin { id, hint, .. } => FilePreviewPlan::Plugin { id, hint },
        PreviewKind::Command {
            template,
            render_as,
            detached,
            ..
        } => FilePreviewPlan::Command {
            template,
            output: render_as,
            detached,
        },
        PreviewKind::CanNotPreview { .. } => FilePreviewPlan::Unavailable,
        other => return Err(other),
    })
}

#[test]
fn project_preview_resolution_matches_host_rules() {
    let dir = crate::test_support::unique_tmp("project_preview_resolution");
    std::fs::create_dir_all(&dir).unwrap();
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\0";
    let mp4 = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom";
    // HEAD resolve_kind: user assignments, then registry defaults in declaration order,
    // then the text probe and Unavailable. These independent literals name each default branch.
    let defaults: Vec<(&str, &[u8], FilePreviewPlan, DiffTarget)> = vec![
        (
            "a.md",
            b"# title",
            plugin("markdown", None),
            DiffTarget::Markdown,
        ),
        (
            "a.mmd",
            b"graph TD",
            plugin("mermaid", None),
            DiffTarget::Text,
        ),
        ("a.svg", b"<svg/>", plugin("svg", None), DiffTarget::Svg),
        ("picture", png, plugin("image", None), DiffTarget::Picture),
        (
            "a.csv",
            b"a,b",
            plugin("csv", Some("csv")),
            DiffTarget::Other,
        ),
        (
            "a.tsv",
            b"a\tb",
            plugin("csv", Some("tsv")),
            DiffTarget::Other,
        ),
        (
            "a.xlsx",
            b"PK",
            plugin("spreadsheet", None),
            DiffTarget::Other,
        ),
        ("a.docx", b"PK", plugin("document", None), DiffTarget::Other),
        (
            "a.pptx",
            b"PK",
            plugin("presentation", None),
            DiffTarget::Other,
        ),
        ("a.zip", b"PK", plugin("archive", None), DiffTarget::Other),
        (
            "a.tar.gz",
            b"archive",
            plugin("archive", None),
            DiffTarget::Other,
        ),
        (
            "a.rs",
            b"fn main() {}",
            plugin("code", None),
            DiffTarget::Text,
        ),
        ("movie", mp4, plugin("video", None), DiffTarget::Other),
        (
            "a.pdf",
            b"%PDF-1.4",
            plugin("pdf", None),
            DiffTarget::Picture,
        ),
        (
            "binary",
            b"\0binary",
            FilePreviewPlan::Unavailable,
            DiffTarget::Other,
        ),
        ("unknown", b"words", FilePreviewPlan::Text, DiffTarget::Text),
    ];
    let failures = std::cell::RefCell::new(Vec::new());
    let check = |label: &str, cfg: &Config, path: &Path, bytes: Option<&[u8]>, expected, target| {
        let mut registry = Registry::new();
        for implementation in konoma_file_plugins::plugins() {
            registry
                .register(implementation, PluginOrigin::Standard)
                .unwrap();
        }
        for id in cfg.plugin_disabled() {
            registry.set_enabled(&id, false);
        }
        let source = FilePreviewSource::new(
            Arc::new(registry),
            cfg.preview.rules.iter().map(Into::into).collect(),
            cfg.external.preview_commands,
        );
        let resolved = source.resolve(path, bytes);
        let host = match bytes {
            Some(bytes) => cfg.resolve_preview_with(path, bytes),
            None => cfg.resolve_preview(path),
        };
        let host = match host_plan(host) {
            Ok(plan) => format!("{plan:?}"),
            Err(kind) => format!("unexpected non-file preview: {kind:?}"),
        };
        for (field, actual, expected) in [
            (
                "project plan",
                format!("{:?}", resolved.plan),
                format!("{expected:?}"),
            ),
            ("host plan", host, format!("{expected:?}")),
            (
                "diff target",
                format!("{:?}", resolved.diff_target),
                format!("{target:?}"),
            ),
        ] {
            if actual != expected {
                failures.borrow_mut().push(format!(
                    "{label}/{field}: got {actual}, expected {expected}"
                ));
            }
        }
    };
    let mut cfg = Config::default();
    for (name, bytes, expected, target) in defaults {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        check(
            &format!("default/{name}"),
            &cfg,
            &path,
            None,
            expected,
            target,
        );
    }
    // HEAD user_rules: a rule with glob and MIME expands to alternatives (OR), preserving order.
    // A leading user assignment wins over defaults; a miss continues into defaults/text fallback.
    for (glob, mime, name, bytes, expected) in [
        (
            Some("*.md"),
            None,
            "a.md",
            b"text".as_slice(),
            FilePreviewPlan::Text,
        ),
        (
            None,
            Some("image/*"),
            "picture",
            png.as_slice(),
            FilePreviewPlan::Text,
        ),
        (
            Some("*.md"),
            Some("image/*"),
            "a.md",
            b"text",
            FilePreviewPlan::Text,
        ),
        (
            Some("*.md"),
            Some("image/*"),
            "picture",
            png,
            FilePreviewPlan::Text,
        ),
        (
            Some("*.md"),
            Some("image/*"),
            "unknown",
            b"words",
            FilePreviewPlan::Text,
        ),
    ] {
        cfg.preview.rules = vec![rule(glob, mime, "text")];
        check(
            &format!("user-rule/glob={glob:?}/mime={mime:?}/name={name}"),
            &cfg,
            &dir.join(name),
            Some(bytes),
            expected,
            DiffTarget::Text,
        );
    }
    // HEAD kind_of/csv_note: the explicit CSV/TSV hint wins over the opposite path suffix.
    for (name, builtin, hint) in [("a.tsv", "csv", "csv"), ("a.csv", "tsv", "tsv")] {
        cfg.preview.rules = vec![rule(Some("*"), None, builtin)];
        check(
            &format!("delimiter/builtin={builtin}/name={name}"),
            &cfg,
            &dir.join(name),
            None,
            plugin("csv", Some(hint)),
            DiffTarget::Other,
        );
    }
    // HEAD kind_of COMMAND_ID: preserve template/output/detach, gated by preview_commands.
    cfg.preview.rules = vec![Rule {
        glob: Some("*".into()),
        command: Some("viewer {path} {out}".into()),
        render_as: Some("text".into()),
        detached: true,
        ..Default::default()
    }];
    for enabled in [true, false] {
        cfg.external.preview_commands = enabled;
        check(
            &format!("command/gate={enabled}"),
            &cfg,
            &dir.join("a.md"),
            None,
            if enabled {
                FilePreviewPlan::Command {
                    template: "viewer {path} {out}".into(),
                    output: Some("text".into()),
                    detached: true,
                }
            } else {
                FilePreviewPlan::Unavailable
            },
            if enabled {
                DiffTarget::Text
            } else {
                DiffTarget::Other
            },
        );
    }
    // HEAD resolve_kind: registry skips disabled bindings before the plain-text fallback.
    cfg = Config::default();
    cfg.plugins
        .entry("markdown".into())
        .or_default()
        .insert("enabled".into(), toml::Value::Boolean(false));
    check(
        "disabled/markdown",
        &cfg,
        &dir.join("a.md"),
        None,
        FilePreviewPlan::Text,
        DiffTarget::Text,
    );
    cfg = Config::default();
    // HEAD resolve_kind: Some(sniff) uses baseline bytes even for a deleted extensionless path.
    // Without bytes, a missing .md still matches the filename default, without a content probe.
    check(
        "deleted/baseline-image",
        &cfg,
        &dir.join("deleted"),
        Some(png),
        plugin("image", None),
        DiffTarget::Picture,
    );
    check(
        "deleted/markdown-name-only",
        &cfg,
        &dir.join("deleted.md"),
        None,
        plugin("markdown", None),
        DiffTarget::Markdown,
    );
    #[cfg(unix)]
    {
        // HEAD plugin_id_of/preview availability: never open a FIFO when MIME/text needs bytes.
        let fifo = dir.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success(),
            "special-file/fifo: mkfifo succeeds"
        );
        check(
            "special-file/fifo",
            &cfg,
            &fifo,
            None,
            FilePreviewPlan::Unavailable,
            DiffTarget::Other,
        );
    }
    assert!(
        failures.borrow().is_empty(),
        "{}",
        failures.borrow().join("\n")
    );
}
