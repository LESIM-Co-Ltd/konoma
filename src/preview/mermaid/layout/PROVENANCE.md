# Provenance and modifications — vendored `dagre`

This directory is a vendored copy of the **`dagre` crate, version 0.1.1**
(<https://crates.io/crates/dagre>, source at <https://github.com/kookyleo/dagre-rs>,
repository sha1 `ad891113d80f5d0305962a0e45e88715e15873f4`; the repository is archived).

- **Copyright (c) kookyleo <kookyleo@gmail.com>**
- **Licensed under the Apache License, Version 2.0** — the full text is in
  `LICENSE-APACHE` in this directory. konoma itself is MIT; this subtree stays
  Apache-2.0 and cannot be relicensed.
- **Upstream released no `NOTICE` file.** The published `dagre-0.1.1.crate` archive
  and the repository both contain only `LICENSE` (checked 2026-08-26), so there is no
  `NOTICE` content to carry forward under Apache-2.0 §4(d).

`dagre` is itself a port of `@dagrejs/dagre` **3.0.1-pre**, commit
`4713b59bfa05af56cf58aa01e2027adf5d2dcf88` (recorded in upstream's
`[package.metadata.upstream]`). `cross-validate/reference_data.json` is the recorded
output of that JavaScript implementation for 30 graphs and is what `cross_validate.rs`
asserts against.

## Apache-2.0 §4(b): statement of changes

Every `.rs` file in this directory has been modified by the konoma authors and carries
a notice to that effect at the top of the file. This section is the complete list of
what those changes are.

### 1. Where the files went

The crate was flattened into a konoma module. Paths are otherwise preserved so that a
diff against a future `dagre` release stays readable.

| upstream | here |
|---|---|
| `src/lib.rs` | `mod.rs` |
| `src/util.rs` | `util.rs` |
| `src/graph/**` | `graph/**` |
| `src/layout/**` | `layout/**` |
| `tests/cross_validate.rs` | `cross_validate.rs` (now a `#[cfg(test)] mod`) |
| `cross-validate/**` | `cross-validate/**` |
| `LICENSE` | `LICENSE-APACHE` |

### 2. Module paths

`crate::` was rewritten to `crate::preview::mermaid::layout::` throughout (91
occurrences). `crate::` now means the konoma binary, not the `dagre` library.

### 3. Let-chains desugared (edition 2024 → edition 2021)

Upstream is `edition = "2024"` and uses let-chains (`if let Some(a) = x && let Some(b)
= y { … }`). konoma is `edition = "2021"`, where those do not parse
(`error: let chains are only allowed in Rust 2024 or later`). Cargo has no per-module
edition, so each chain was **mechanically nested** — `if A && B { X }` became
`if A { if B { X } }`. None of the 30 sites had an `else` branch, so the nesting is
semantically identical; the transformation was applied by script, not by hand.

Sites, by upstream line number:

- `src/graph/mod.rs`: 409, 688, 699, 725, 736
- `src/graph/alg.rs`: 667
- `src/graph/tests.rs`: 3116
- `src/layout/mod.rs`: 76, 84, 351, 400, 414, 519, 563
- `src/layout/coordinate_system.rs`: 62
- `src/layout/order/add_subgraph_constraints.rs`: 40
- `src/layout/order/init_order.rs`: 51
- `src/layout/order/mod.rs`: 154
- `src/layout/order/resolve_conflicts.rs`: 140, 147
- `src/layout/parent_dummy_chains.rs`: 72
- `src/layout/position/bk.rs`: 197, 245, 251, 270, 348, 442, 455, 503, 697
- `src/layout/rank/feasible_tree.rs`: 144
- `src/layout/rank/network_simplex.rs`: 50
- `src/layout/util.rs`: 162, 195, 199, 224, 252
- `tests/cross_validate.rs`: 426

Evidence that the desugaring preserved behaviour: upstream's own suite (538 tests) and
the 30-case cross-validation were run against the edition-2021 form before it was moved
into konoma, and both stayed green.

### 4. Lint allowances

`mod.rs` carries a module-level `#![allow(…)]` covering the whole subtree, with the
reason for each lint written next to it. Nothing was rewritten to satisfy a lint,
because every rewrite spends a piece of the dagre.js cross-validation guarantee.

### 5. Reformatted by konoma's rustfmt

`cargo fmt` runs over `src/`, and konoma's edition-2021 rustfmt sorts `use` lists and
wraps lines differently from upstream's edition-2024 style. This is whitespace and
import ordering only; it touched `layout/tests.rs`, `layout/normalize.rs`,
`layout/order/mod.rs`, `layout/position/bk.rs` and
`layout/rank/network_simplex.rs`.

### 6. Code that was dropped

- **`src/graph/json.rs` (127 lines)** and its `pub mod json;` declaration. It
  (de)serialises a `Graph` through serde and is unreachable from the layout pipeline.
  Keeping it would have forced `serde_json` to become a *runtime* dependency of konoma
  for a feature konoma never calls; as a dev-dependency it is limited to the
  cross-validation test.
- **The shortest-path / spanning-tree half of `src/graph/alg.rs`** (upstream lines
  232-686): `PathEntry`, `dijkstra`, `dijkstra_with_edge_fn`, `dijkstra_all`,
  `bellman_ford`, `floyd_warshall`, `ExtractedPath`/`extract_path`, `reduce`/
  `do_reduce`, `prim` and the private `PriorityQueue`. The layout pipeline never calls
  any of them. What is kept — `topsort`, `is_acyclic`, `find_cycles`, `tarjan`, `dfs`,
  `preorder`, `postorder`, `components` — is used by `layout/` or by `layout/tests.rs`.
  The now-unused `EdgeFn` type alias went with them, and `use super::{Edge, Graph}`
  became `use super::Graph` because `Edge` was only referenced by the dropped code.
- **56 tests in `src/graph/tests.rs`** that exercise only the dropped functions
  (`dijkstra*`, `bellman_ford*`, `floyd_warshall*`, `prim*`, `extract_path*`,
  `reduce*`, `priority_queue*`, `all_shortest_paths*`, `json_*`), plus the
  `mod alg_internal` test helper that held a second copy of `PriorityQueue`. 482 of
  upstream's 538 unit tests remain.

### 7. `cross_validate.rs`

- `use dagre::…` became `use super::…`; the file is now `#[cfg(test)] mod
  cross_validate;` rather than an integration test, because konoma is a binary crate
  and `tests/` cannot reach into it.
- `include_str!("../cross-validate/…")` became `include_str!("cross-validate/…")`.
- One let-chain desugared (see §3).
- **A stale comment was corrected.** Upstream's line 473 read *"For now, don't assert —
  just report. We'll tighten this as we fix divergences."* immediately above a `panic!`
  that does assert. The comment now says what the code does.

### 8. Not carried over

Upstream's `Cargo.toml` (the `json` feature and its optional `serde`/`serde_json`
dependencies), `README.md`, `.github/`, `Cargo.lock` and `.gitignore`. konoma picks up
upstream's one runtime dependency, `log`, as a direct dependency instead.
`cross-validate/SETUP.md` and `cross-validate/generate_reference.mjs` **are** carried
over so the baseline can be regenerated; they need Node and are never run from a test.

### Regenerating the baseline from konoma

`LICENSE-APACHE`, `cross-validate/reference_data.json`,
`cross-validate/generate_reference.mjs` and `cross-validate/SETUP.md` are **byte-identical
to upstream 0.1.1** — deliberately, so that a future upstream release diffs cleanly. That
means `SETUP.md` still describes upstream's repository layout. Read it with this mapping:

| `SETUP.md` says | in konoma |
|---|---|
| `tests/cross_validate.rs` | `src/preview/mermaid/layout/cross_validate.rs` |
| `cross-validate/…` (from the repo root) | `src/preview/mermaid/layout/cross-validate/…` |
| `ref/dagre-js/` (clone target) | anywhere; it is not tracked here either |
| `Cargo.toml` → `[package.metadata.upstream]` | this file's header |
| `README.md` → "Compatibility" | this file's header |

`node generate_reference.mjs` must be run with `src/preview/mermaid/layout/` as the
working directory, since the script resolves `cross-validate/` and `ref/` relative to it.

## Diffing against a future `dagre` release

The vendored tree can be compared to an upstream tarball by undoing the two mechanical
edits first:

```sh
# for each .rs file: drop the 5-line provenance header + its blank line,
# then map the module path back to the crate root
tail -n +7 "$f" | sed 's|crate::preview::mermaid::layout::|crate::|g'
```

15 of the 31 files are byte-identical to upstream 0.1.1 after that normalisation.
