mod support;

use std::path::Path;
use std::process::Command;
use support::command::CommandOutput;

// Deliberately strict: an unknown/misspelled Liquid placeholder must fail, even in a README.
fn render(text: &str) -> String {
    let mut rest = text;
    let mut result = String::new();
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let end = rest.find("}}").expect("unclosed template placeholder");
        result.push_str(match rest[..end].trim() {
            "project-name" => "rendered-hexdump",
            "crate_name" => "rendered_hexdump",
            unknown => panic!("unknown template placeholder: {unknown}"),
        });
        rest = &rest[end + 2..];
    }
    assert!(!rest.contains("}}"), "unmatched closing placeholder");
    result.push_str(rest);
    result
}

fn render_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let output = destination.join(entry.file_name());
        if path.is_dir() {
            render_tree(&path, &output);
        } else {
            let text = render(&std::fs::read_to_string(path).unwrap());
            if entry.file_name() != "cargo-generate.toml" {
                std::fs::write(output, text).unwrap();
            }
        }
    }
}

#[test]
fn template_builds_offline_and_declares_the_rendered_id() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    // Source and build artifacts stay below the caller's target, never the user's config.
    let target = support::target_root();
    let generated = target.join(format!("hexdump-template-{}", std::process::id()));
    render_tree(&workspace.join("templates/konoma-plugin"), &generated);
    let manifest = generated.join("Cargo.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str(&format!(
        "\n[patch.crates-io]\nkonoma-plugin = {{ path = {:?} }}\nmermaid-text = {{ path = {:?} }}\n",
        workspace.join("crates/konoma-plugin"),
        workspace.join("vendor/mermaid-text")
    ));
    std::fs::write(&manifest, text).unwrap();
    // Reuse cached, resolved dependency versions, including yanked versions already locked.
    std::fs::copy(workspace.join("Cargo.lock"), generated.join("Cargo.lock")).unwrap();
    let (mut command, profile_dir) = support::cargo();
    let build_target = target.join("hexdump-template-build");
    let output = command
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&build_target)
        .supervised_output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let binary = build_target
        .join(profile_dir.strip_prefix(&target).unwrap())
        .join(format!("rendered-hexdump{}", std::env::consts::EXE_SUFFIX));
    // --manifest does not start a terminal or read standalone-app settings.
    let output = Command::new(binary)
        .arg("--manifest")
        .supervised_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let manifest =
        konoma_plugin::proc::manifest_from_json(std::str::from_utf8(&output.stdout).unwrap())
            .unwrap();
    assert_eq!(manifest.id, "rendered-hexdump");
    assert_eq!(manifest.actions[0].key.as_deref(), Some("w"));
    std::fs::remove_dir_all(generated).unwrap();
}
