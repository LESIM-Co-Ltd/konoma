// Vendored from the `dagre` crate v0.1.1 (https://github.com/kookyleo/dagre-rs).
// Copyright (c) kookyleo <kookyleo@gmail.com>. Licensed under the Apache License,
// Version 2.0 -- full text in `src/preview/mermaid/layout/LICENSE-APACHE`.
// Modified by the konoma authors; every change is listed in
// `src/preview/mermaid/layout/PROVENANCE.md`.

//! Graph algorithms: topological sort, DFS traversal, connected components, etc.

use super::Graph;
use std::collections::{HashMap, HashSet, VecDeque};

/// Error returned when a cycle is detected during topological sort.
#[derive(Debug)]
pub struct CycleError;

impl std::fmt::Display for CycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Graph contains a cycle")
    }
}

impl std::error::Error for CycleError {}

/// Topological sort of a directed graph. Returns an error if the graph has a cycle.
pub fn topsort<N, E>(g: &Graph<N, E>) -> Result<Vec<String>, CycleError> {
    let mut visited = HashSet::new();
    let mut stack = HashSet::new();
    let mut result = Vec::new();

    fn visit<N, E>(
        g: &Graph<N, E>,
        v: &str,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) -> Result<(), CycleError> {
        if stack.contains(v) {
            return Err(CycleError);
        }
        if visited.contains(v) {
            return Ok(());
        }
        stack.insert(v.to_string());
        visited.insert(v.to_string());

        if let Some(preds) = g.predecessors(v) {
            for pred in preds {
                visit(g, &pred, visited, stack, result)?;
            }
        }

        stack.remove(v);
        result.push(v.to_string());
        Ok(())
    }

    let nodes = g.nodes();
    for v in &nodes {
        visit(g, v, &mut visited, &mut stack, &mut result)?;
    }

    Ok(result)
}

/// Check if a directed graph is acyclic.
pub fn is_acyclic<N, E>(g: &Graph<N, E>) -> bool {
    topsort(g).is_ok()
}

/// Find all cycles in the graph. Returns a list of cycles, each being a list of node IDs.
pub fn find_cycles<N, E>(g: &Graph<N, E>) -> Vec<Vec<String>> {
    let sccs = tarjan(g);
    sccs.into_iter()
        .filter(|scc| {
            scc.len() > 1 || {
                let v = &scc[0];
                g.has_edge(v, v, None)
            }
        })
        .collect()
}

/// Tarjan's strongly connected components algorithm.
pub fn tarjan<N, E>(g: &Graph<N, E>) -> Vec<Vec<String>> {
    struct TarjanState {
        index: u32,
        stack: Vec<String>,
        on_stack: HashSet<String>,
        indices: HashMap<String, u32>,
        lowlinks: HashMap<String, u32>,
        result: Vec<Vec<String>>,
    }

    fn strongconnect<N, E>(g: &Graph<N, E>, v: &str, state: &mut TarjanState) {
        state.indices.insert(v.to_string(), state.index);
        state.lowlinks.insert(v.to_string(), state.index);
        state.index += 1;
        state.stack.push(v.to_string());
        state.on_stack.insert(v.to_string());

        if let Some(succs) = g.successors(v) {
            for w in succs {
                if !state.indices.contains_key(&w) {
                    strongconnect(g, &w, state);
                    let lw = state.lowlinks[&w];
                    let lv = state.lowlinks.get_mut(v).unwrap();
                    *lv = (*lv).min(lw);
                } else if state.on_stack.contains(&w) {
                    let iw = state.indices[&w];
                    let lv = state.lowlinks.get_mut(v).unwrap();
                    *lv = (*lv).min(iw);
                }
            }
        }

        if state.lowlinks[v] == state.indices[v] {
            let mut scc = Vec::new();
            loop {
                let w = state.stack.pop().unwrap();
                state.on_stack.remove(&w);
                scc.push(w.clone());
                if w == v {
                    break;
                }
            }
            state.result.push(scc);
        }
    }

    let mut state = TarjanState {
        index: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        result: Vec::new(),
    };

    for v in g.nodes() {
        if !state.indices.contains_key(&v) {
            strongconnect(g, &v, &mut state);
        }
    }

    state.result
}

/// Depth-first search traversal.
pub fn dfs<N, E>(g: &Graph<N, E>, roots: &[&str], order: DfsOrder) -> Vec<String> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();

    fn do_dfs<N, E>(
        g: &Graph<N, E>,
        v: &str,
        order: DfsOrder,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) {
        if visited.contains(v) {
            return;
        }
        visited.insert(v.to_string());

        if order == DfsOrder::Pre {
            result.push(v.to_string());
        }

        if let Some(neighbors) = g.successors(v) {
            for w in neighbors {
                do_dfs(g, &w, order, visited, result);
            }
        }

        if order == DfsOrder::Post {
            result.push(v.to_string());
        }
    }

    for root in roots {
        do_dfs(g, root, order, &mut visited, &mut result);
    }

    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfsOrder {
    Pre,
    Post,
}

/// Preorder DFS traversal.
pub fn preorder<N, E>(g: &Graph<N, E>, roots: &[&str]) -> Vec<String> {
    dfs(g, roots, DfsOrder::Pre)
}

/// Postorder DFS traversal.
pub fn postorder<N, E>(g: &Graph<N, E>, roots: &[&str]) -> Vec<String> {
    dfs(g, roots, DfsOrder::Post)
}

/// Find weakly connected components. Returns a list of components,
/// each being a list of node IDs.
pub fn components<N, E>(g: &Graph<N, E>) -> Vec<Vec<String>> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();

    for v in g.nodes() {
        if visited.contains(&v) {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(v.clone());
        visited.insert(v);

        while let Some(node) = queue.pop_front() {
            component.push(node.clone());
            if let Some(neighbors) = g.neighbors(&node) {
                for w in neighbors {
                    if !visited.contains(&w) {
                        visited.insert(w.clone());
                        queue.push_back(w);
                    }
                }
            }
        }
        result.push(component);
    }

    result
}
