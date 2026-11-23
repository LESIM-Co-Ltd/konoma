use std::path::Path;

use konoma_plugin::prelude::*;
use konoma_plugin::proc::{frame, proto::*, wire};

fn hexdump(bytes: &[u8]) -> String {
    bytes
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            format!(
                "{:04x}  {}",
                row * 16,
                chunk
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn documented<'a>(page: &'a str, name: &str) -> &'a str {
    page.split_once(&format!("<!-- vector:{name} -->\n```text\n"))
        .expect("missing vector marker")
        .1
        .split_once("\n```")
        .expect("unclosed vector fence")
        .0
}

#[test]
fn published_handshake_vectors_match_reference_encoder() {
    let hello = Hello {
        versions: vec![1],
        host_version: "0.31.2".into(),
        side_dir: wire::path_to_wire(Path::new("/private/handoff")),
        lang: "en".into(),
    };
    // The minimal M in the worked exchange, not the sample's richer action manifest.
    let manifest = Manifest::new("hex", "0.1.0")
        .name("Hex")
        .bind(Bind::files(["*.bin"]))
        .shows(ViewKind::Text);
    let ack = HelloReply::Ack {
        version: 1,
        manifest: wire::manifest_to_wire(&manifest).unwrap(),
    };
    let mut hello_frame = Vec::new();
    let mut ack_frame = Vec::new();
    frame::send(&mut hello_frame, &hello).unwrap();
    frame::send(&mut ack_frame, &ack).unwrap();
    let vectors = [
        ("hello", hexdump(&hello_frame)),
        ("ack", hexdump(&ack_frame)),
    ];
    for (name, expected) in &vectors {
        println!("{name}:\n{expected}");
    }
    let (mut session, _file) =
        konoma_plugin::testing::open_bytes(&konoma_sample_hexdump::Hexdump, "a.bin", b"A").unwrap();
    let mut view = session
        .view(&ViewReq::new(80, 20), &ViewCx::bare())
        .unwrap();
    let side = wire::Side::new("/private/handoff", 'c');
    let wire::WView::Text(text) =
        wire::view_to_wire(&mut view, &mut wire::Enc::new(&side)).unwrap()
    else {
        panic!("sample must return text");
    };
    assert!(!text.file_lines && text.syntax_name.is_none() && text.note.is_none());
    let wire::WTextSource::Lines(lines) = text.source else {
        panic!("sample must return Lines");
    };
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line.style, wire::WStyle::default());
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].1, wire::WStyle::default());
    let shape = format!(
        "source:{{Lines:[{{spans:[[{text:?},S]],style:S}}]}}",
        text = line.spans[0].0
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/src/content/docs");
    for locale in ["", "ja/"] {
        let page =
            std::fs::read_to_string(root.join(format!("{locale}reference/plugin-protocol.md")))
                .unwrap();
        for (name, expected) in &vectors {
            assert_eq!(
                documented(&page, name),
                expected,
                "{locale}{name} vector drift"
            );
        }
        assert!(
            page.contains(&shape),
            "{locale}worked exchange differs from the sample: {shape}"
        );
        assert!(page.contains("S = {fg:nil,bg:nil,add:0,sub:0}"));
    }
}
