mod support;

use konoma_plugin::testing::open_bytes;
use konoma_plugin_test::{run, Limits, Options};
use konoma_sample_hexdump::Hexdump;
use support::command::CommandOutput;

#[test]
fn built_program_conforms_to_the_protocol() {
    let (mut command, profile_dir) = support::cargo();
    let output = command
        .args([
            "-p",
            "konoma-sample-hexdump",
            "--bin",
            "konoma-sample-hexdump",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .supervised_output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_session, file) = open_bytes(&Hexdump, "input.bin", b"ABC\0\xff").unwrap();
    let report = run(
        &Options {
            program: profile_dir.join(format!(
                "konoma-sample-hexdump{}",
                std::env::consts::EXE_SUFFIX
            )),
            args: Vec::new(),
            plugin: Some("konoma-sample-hexdump".into()),
            files: vec![file.path().to_path_buf()],
        },
        Limits::default(),
    );
    assert_eq!(report.failed(), 0, "{report:#?}");
    assert_eq!(report.skipped(), 0, "{report:#?}");
}
