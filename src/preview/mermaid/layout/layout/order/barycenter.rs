// Vendored from the `dagre` crate v0.1.1 (https://github.com/kookyleo/dagre-rs).
// Copyright (c) kookyleo <kookyleo@gmail.com>. Licensed under the Apache License,
// Version 2.0 -- full text in `src/preview/mermaid/layout/LICENSE-APACHE`.
// Modified by the konoma authors; every change is listed in
// `src/preview/mermaid/layout/PROVENANCE.md`.

//! Barycenter heuristic for crossing reduction.
//!
//! Computes barycenter values for a set of "movable" nodes based on the
//! weighted average position of their in-edge sources.

use crate::preview::mermaid::layout::graph::Graph;
use crate::preview::mermaid::layout::layout::types::{EdgeLabel, NodeLabel};

/// A single barycenter entry for a node.
#[derive(Debug, Clone)]
pub(crate) struct BarycenterEntry {
    pub v: String,
    pub barycenter: Option<f64>,
    pub weight: Option<i32>,
}

/// Compute barycenter values for each movable node.
///
/// For each node, the barycenter is the weighted average of the `order` of its
/// in-edge source nodes, where the weight is the edge weight.
/// Nodes with no in-edges get `None` for barycenter and weight.
pub(crate) fn barycenter(
    g: &Graph<NodeLabel, EdgeLabel>,
    movable: &[String],
) -> Vec<BarycenterEntry> {
    movable
        .iter()
        .map(|v| {
            let in_edges = g.in_edges(v, None).unwrap_or_default();
            if in_edges.is_empty() {
                return BarycenterEntry {
                    v: v.clone(),
                    barycenter: None,
                    weight: None,
                };
            }

            let mut sum = 0.0_f64;
            let mut weight = 0_i32;
            for e in &in_edges {
                let edge_weight = g.edge_by_obj(e).map_or(1, |l| l.weight);
                let node_order = g.node(&e.v).and_then(|n| n.order).unwrap_or(0) as f64;
                sum += edge_weight as f64 * node_order;
                weight += edge_weight;
            }

            BarycenterEntry {
                v: v.clone(),
                barycenter: if weight > 0 {
                    Some(sum / weight as f64)
                } else {
                    None
                },
                weight: Some(weight),
            }
        })
        .collect()
}
