//! Bounded traversals over the graph.
//!
//! One reason to change: the questions the graph can answer.
//!
//! Every function here is breadth-first with an explicit depth bound and an
//! explicit node bound. Ordering is by distance, then by path — never by a
//! score, which is the line `AGENTS.md` §11 law 2 draws.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use super::{CodeGraph, MAX_DEPTH, MAX_NODES, NodeId, SymbolSite};

/// Which way to walk the import edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// What this file reaches.
    Imports,
    /// What reaches this file.
    Importers,
    /// Both, which is the question "what is this file entangled with".
    Both,
}

/// A reached node and how far away it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neighbour {
    pub id: NodeId,
    pub distance: usize,
}

/// Files within `depth` hops of `start`, nearest first.
///
/// `start` is never included: the caller already knows where it began, and
/// spending one of [`MAX_NODES`] slots restating it is waste.
#[must_use]
pub fn neighbours(
    graph: &CodeGraph,
    start: NodeId,
    depth: usize,
    direction: Direction,
) -> Vec<Neighbour> {
    reachable(graph, &[start], depth, direction)
}

/// Files within `depth` hops of any of `starts`, nearest first.
///
/// Multi-source by design: blast-radius questions begin at several files at
/// once, and one traversal per source would report the same file twice at two
/// distances. A node is reported once, at its minimum distance. The `starts`
/// themselves are never included. Result is ordered by distance, then by path
/// — never by a score, which is the line `AGENTS.md` §11 law 2 draws.
#[must_use]
pub fn reachable(
    graph: &CodeGraph,
    starts: &[NodeId],
    depth: usize,
    direction: Direction,
) -> Vec<Neighbour> {
    let depth = depth.min(MAX_DEPTH);
    let mut seen: BTreeSet<NodeId> = starts.iter().copied().collect();
    let mut queue: VecDeque<Neighbour> = starts
        .iter()
        .map(|id| Neighbour {
            id: *id,
            distance: 0,
        })
        .collect();
    let mut out = Vec::new();

    while let Some(current) = queue.pop_front() {
        if current.distance >= depth {
            continue;
        }
        for next in step(graph, current.id, direction) {
            if !seen.insert(next) {
                continue;
            }
            let neighbour = Neighbour {
                id: next,
                distance: current.distance.saturating_add(1),
            };
            out.push(neighbour);
            if out.len() >= MAX_NODES {
                break;
            }
            queue.push_back(neighbour);
        }
        if out.len() >= MAX_NODES {
            break;
        }
    }
    out.sort_by_key(|neighbour| {
        (
            neighbour.distance,
            path_of(graph, neighbour.id).to_string_lossy().into_owned(),
        )
    });
    out
}

/// Everything a symbol's definition sites reach when other files import them.
#[derive(Debug, Clone)]
pub struct BlastRadius {
    /// Where the symbol is defined, in graph order.
    pub sites: Vec<SymbolSite>,
    /// Files importing any of those sites, nearest first — the blast radius.
    pub affected: Vec<Neighbour>,
}

/// The files that import any file defining `symbol`, within `depth` hops.
///
/// This is "if this name changes, what breaks", answered with the evidence the
/// graph has: the sites that define it, and everything that imports those
/// sites. `None` when the symbol has no definition site — deliberately
/// distinct from an empty `affected`, which means it is defined and nothing
/// imports it.
#[must_use]
pub fn blast_radius(graph: &CodeGraph, symbol: &str, depth: usize) -> Option<BlastRadius> {
    let sites = graph.definitions(symbol).to_vec();
    if sites.is_empty() {
        return None;
    }
    let starts: Vec<NodeId> = sites.iter().map(|site| site.file).collect();
    let affected = reachable(graph, &starts, depth, Direction::Importers);
    Some(BlastRadius { sites, affected })
}

/// The shortest chain of files connecting `from` to `to`, inclusive of both.
///
/// Edges are followed in **both** directions here. The question this answers is
/// "how are these two related", and an import chain that happens to run the
/// other way is the same relationship seen from the other end. `None` means no
/// chain exists within [`MAX_DEPTH`] — deliberately distinct from an empty
/// list, which would read as adjacency.
#[must_use]
pub fn between(graph: &CodeGraph, from: NodeId, to: NodeId) -> Option<Vec<NodeId>> {
    if from == to {
        return Some(vec![from]);
    }
    let mut seen = BTreeSet::from([from]);
    let mut queue = VecDeque::from([from]);
    // Parent links, so the path can be walked back without storing a full path
    // per frontier entry.
    let mut came_from: Vec<Option<NodeId>> = vec![None; graph.files().len()];
    let mut distance = 0_usize;

    while !queue.is_empty() && distance < MAX_DEPTH {
        for _ in 0..queue.len() {
            let Some(current) = queue.pop_front() else {
                break;
            };
            for next in step(graph, current, Direction::Both) {
                if !seen.insert(next) {
                    continue;
                }
                if let Some(slot) = came_from.get_mut(next.0) {
                    *slot = Some(current);
                }
                if next == to {
                    return Some(unwind(&came_from, from, to));
                }
                queue.push_back(next);
            }
        }
        distance = distance.saturating_add(1);
    }
    None
}

fn unwind(came_from: &[Option<NodeId>], from: NodeId, to: NodeId) -> Vec<NodeId> {
    let mut path = vec![to];
    let mut current = to;
    // Bounded by the number of nodes: a parent chain cannot revisit a node, so
    // this cannot loop, but an explicit bound beats an argument about it.
    for _ in 0..came_from.len() {
        if current == from {
            break;
        }
        let Some(Some(parent)) = came_from.get(current.0).copied() else {
            break;
        };
        path.push(parent);
        current = parent;
    }
    path.reverse();
    path
}

fn step(graph: &CodeGraph, id: NodeId, direction: Direction) -> Vec<NodeId> {
    let Some(node) = graph.node(id) else {
        return Vec::new();
    };
    match direction {
        Direction::Imports => node.imports.iter().copied().collect(),
        Direction::Importers => node.importers.iter().copied().collect(),
        Direction::Both => node
            .imports
            .iter()
            .chain(node.importers.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn path_of(graph: &CodeGraph, id: NodeId) -> &Path {
    graph
        .node(id)
        .map_or_else(|| Path::new(""), |node| node.path.as_path())
}
