// The size an SVG declares, read without parsing it, lives in the `konoma-svg` crate. This module
// exists for the one test that needs host code (the mermaid renderer and the math previewer produce
// the SVGs it compares); nothing in the host calls the sizing functions through a `crate::` path.

#[cfg(test)]
mod tests {
    use konoma_svg::svg_guard::SvgFail;
    use konoma_svg::svg_size::{declared_size, Declared};
    use resvg::usvg;

    /// The size usvg itself reports for `doc`, rounded up the way konoma rounds it.
    fn usvg_size(doc: &str) -> Option<(u32, u32)> {
        let tree = usvg::Tree::from_str(doc, &usvg::Options::default()).ok()?;
        let s = tree.size();
        Some((s.width().ceil() as u32, s.height().ceil() as u32))
    }

    fn ours(doc: &str) -> Result<Option<(u32, u32)>, SvgFail> {
        Ok(match declared_size(doc.as_bytes())? {
            Declared::Size(w, h) => Some((w.ceil() as u32, h.ceil() as u32)),
            Declared::Unknown => None,
        })
    }

    #[test]
    fn real_files_and_konomas_own_svgs_agree_with_usvg() {
        let mut docs: Vec<(String, String)> = Vec::new();
        for p in ["samples/sample.svg", "site/public/favicon.svg"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(p);
            if let Ok(s) = std::fs::read_to_string(&path) {
                docs.push((p.to_string(), s));
            }
        }
        for (i, code) in [
            "flowchart LR\n  A[Start] --> B{Choice}\n  B -->|yes| C[Done]\n  B -->|no| D[Retry]",
            "sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi",
            "pie title Pets\n  \"Dogs\" : 386\n  \"Cats\" : 85",
            "stateDiagram-v2\n  [*] --> A\n  A --> B\n  B --> [*]",
        ]
        .iter()
        .enumerate()
        {
            if let Some(svg) =
                crate::preview::markdown::mermaid_to_svg_flow(code, "default", "basis", "splines")
            {
                docs.push((format!("mermaid {i}"), svg));
            }
        }
        for latex in ["E = mc^2", "\\frac{a}{b} + \\sum_{i=0}^n i^2", "x"] {
            for display in [false, true] {
                if let Some(svg) = crate::preview::math::latex_to_svg(latex, display, "#d0d0d0") {
                    docs.push((format!("math {latex} {display}"), svg));
                }
            }
        }
        assert!(docs.len() >= 8, "only {} documents", docs.len());
        for (name, d) in docs {
            assert_eq!(ours(&d), Ok(usvg_size(&d)), "{name}");
            assert!(ours(&d).unwrap().is_some(), "{name}: declined");
        }
    }
}
