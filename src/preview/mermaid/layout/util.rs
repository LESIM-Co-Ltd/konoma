// Vendored from the `dagre` crate v0.1.1 (https://github.com/kookyleo/dagre-rs).
// Copyright (c) kookyleo <kookyleo@gmail.com>. Licensed under the Apache License,
// Version 2.0 -- full text in `src/preview/mermaid/layout/LICENSE-APACHE`.
// Modified by the konoma authors; every change is listed in
// `src/preview/mermaid/layout/PROVENANCE.md`.

/// Generate a unique ID string with the given prefix.
pub fn unique_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}{}", prefix, id)
}
