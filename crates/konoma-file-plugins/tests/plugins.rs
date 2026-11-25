use konoma_plugin::testing::{check_manifest, open_file};
use konoma_plugin::{Input, OpenCx};
use konoma_test_support::unique_tmp;

#[test]
fn standard_file_plugins_have_one_order_and_real_implementations() {
    let plugins = konoma_file_plugins::plugins();
    let ids: Vec<_> = plugins.iter().map(|p| p.manifest().id).collect();
    assert_eq!(
        ids,
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
        ],
        "standard file plugin registration order"
    );

    let dir = unique_tmp("standard_file_plugins");
    std::fs::create_dir_all(&dir).unwrap();
    // Only opening is exercised: lazy media and Office sessions do not need a renderer or an
    // external program. Markdown and tables read here, so give them small valid inputs.
    let files = [
        ("markdown", "a.md", b"# heading\n".as_slice()),
        ("mermaid", "a.mmd", b"graph TD; A-->B;\n"),
        (
            "svg",
            "a.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        ),
        ("image", "a.png", b""),
        ("csv", "a.csv", b"heading\nvalue\n"),
        ("spreadsheet", "a.xlsx", b""),
        ("document", "a.docx", b""),
        ("presentation", "a.pptx", b""),
        ("archive", "a.tar", &[0; 1024]),
        ("code", "a.rs", b"fn main() {}\n"),
        ("video", "a.mp4", b""),
        ("pdf", "a.pdf", b""),
    ];
    for plugin in plugins {
        let id = plugin.manifest().id;
        assert!(check_manifest(&*plugin).is_empty(), "{id}: clean manifest");
        let opened = if id == "math" {
            plugin.open(
                Input::Embed {
                    kind: "math".into(),
                    source: "x^2".into(),
                    display: true,
                },
                &OpenCx::bare(),
            )
        } else {
            let (_, name, contents) = files.iter().find(|(file_id, _, _)| *file_id == id).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, contents).unwrap();
            open_file(&*plugin, &path)
        };
        assert!(
            opened.is_ok(),
            "{id}: real implementation opens its input: {:?}",
            opened.err()
        );
    }
}
