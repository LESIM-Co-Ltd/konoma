// The Markdown document renderer lives in the `konoma-widgets` crate (the block model and the
// block-level diff in `konoma-markdown-core`); this path stays for the host.

pub use konoma_widgets::markdown::*;

// The one test of this module that has to stay in the host: it reads the manifests and the sources
// of the whole repository (through the host's `CARGO_MANIFEST_DIR`), which a test inside
// `konoma-widgets` could not reach.
#[cfg(test)]
mod tests {
    /// **The crate is gone from the dependency graph**, and this is what says so.
    ///
    /// `docs/FEATURE-MERMAID-RENDERER.md` §8 lists three things the crate cost that nothing in the
    /// program could see: **3.1 MB written to `~/.cache/mmdr/font-cache/`** that no document
    /// mentioned, a **second enumeration of the system fonts** at startup, and ~58,000 lines of
    /// transitive dependency. None of the three is observable from inside a test — a cache konoma
    /// no longer writes leaves nothing behind to assert on — so what is asserted is the one thing
    /// that causes all three: the manifest does not name it, and no source file calls it.
    ///
    /// A source-reading test, the same shape as the one that keeps the jj backend read-only.
    #[test]
    fn the_replaced_renderer_is_no_longer_a_dependency() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // The manifests: the root's and every crate's under `crates/` (the dependency could come
        // back through any of them).
        let mut manifests = vec![root.join("Cargo.toml")];
        let mut source_roots = vec![root.join("src")];
        let crate_dirs: Vec<_> = std::fs::read_dir(root.join("crates"))
            .expect("crates/")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("Cargo.toml").is_file())
            .collect();
        // Safety valve: a broken search of `crates/` must not make this vacuously pass.
        assert!(!crate_dirs.is_empty(), "no crate found under crates/");
        for dir in &crate_dirs {
            manifests.push(dir.join("Cargo.toml"));
            source_roots.push(dir.join("src"));
        }
        for manifest in &manifests {
            let text = std::fs::read_to_string(manifest).expect("Cargo.toml");
            for line in text.lines() {
                let l = line.trim();
                if l.starts_with('#') {
                    continue;
                }
                assert!(
                    !l.contains("mermaid-rs-renderer"),
                    "{} still declares the crate stage 5b removed: {l}",
                    manifest.display()
                );
            }
        }

        // …and nothing calls it. A `use` that survived would not compile, but a stale doc link or
        // a feature-gated call would, so the whole source tree is read.
        let mut offenders: Vec<String> = Vec::new();
        let mut stack = source_roots;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (i, line) in text.lines().enumerate() {
                    // The crate's *name in prose* is fine — several files explain why it went —
                    // but its Rust path is a call. The needle is assembled rather than written out
                    // so that this test does not find itself.
                    if line.contains(&format!("mermaid{}rs{}renderer", '_', '_')) {
                        offenders.push(format!("{}:{}", path.display(), i + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these still reach for the removed renderer: {offenders:?}"
        );
    }
}
