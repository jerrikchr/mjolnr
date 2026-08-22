//! Projection of the deterministic code graph to the desktop client (E7).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::core::client::graph::{
    ClientGraphDirection, ClientGraphEdge, ClientGraphLanguageCapability, ClientGraphNode,
    ClientGraphPage, ClientGraphQuery, ClientGraphSummary, ClientGraphSymbol, MAX_GRAPH_NODES,
    MAX_GRAPH_SYMBOLS_PER_FILE,
};
use crate::graph::{self, CodeGraph, Direction, NodeId};

#[derive(Debug)]
struct GraphAnalysis {
    community: Vec<usize>,
    community_sizes: Vec<usize>,
    articulation_points: Vec<bool>,
    cycle_nodes: Vec<bool>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum GraphProjectionError {
    #[error("building the code graph failed: {0}")]
    Build(#[from] graph::BuildError),
    #[error("the graph query path was not found in the workspace: {0}")]
    MissingPath(String),
}

#[cfg(test)]
pub(super) fn build_page(
    root: &Path,
    query: ClientGraphQuery,
) -> Result<ClientGraphPage, GraphProjectionError> {
    build_page_with_progress(root, query, |_, _| {})
}

pub(super) fn build_page_with_progress(
    root: &Path,
    query: ClientGraphQuery,
    mut progress: impl FnMut(usize, usize),
) -> Result<ClientGraphPage, GraphProjectionError> {
    let graph = graph::build_with_progress(root, |value| {
        progress(value.files_scanned, value.files_total);
    })?;
    let analysis = analyze_graph(&graph);
    let unsupported_languages = unsupported_languages(root);
    let languages = language_capabilities(&graph);
    let depth = query.depth.min(graph::MAX_DEPTH);
    let mut selected = select_nodes(&graph, query.path.as_deref(), depth, query.direction)?;
    if let Some(search) = query.search.clone() {
        apply_search_filter(&graph, &mut selected, &search);
    }
    let nodes = selected
        .iter()
        .filter_map(|(id, distance)| project_node(&graph, &analysis, *id, *distance))
        .collect::<Vec<_>>();
    let edges = project_edges(&graph, &selected);
    let truncation = graph.truncation();
    let truncated = truncation.is_truncated() || selected.len() >= MAX_GRAPH_NODES;

    Ok(ClientGraphPage {
        query: ClientGraphQuery { depth, ..query },
        nodes,
        edges,
        summary: ClientGraphSummary {
            files_scanned: graph.files().len(),
            external_imports: graph.external(),
            unresolved_imports: graph.unresolved(),
            files_skipped: truncation.files_skipped,
            files_too_large: truncation.files_too_large,
            non_parsed_edges: graph.non_parsed_edges(),
            communities: analysis.community_sizes.len(),
            articulation_points: analysis
                .articulation_points
                .iter()
                .filter(|value| **value)
                .count(),
            cycle_nodes: analysis.cycle_nodes.iter().filter(|value| **value).count(),
            unsupported_languages,
            languages,
        },
        truncated,
    })
}

fn language_capabilities(graph: &CodeGraph) -> Vec<ClientGraphLanguageCapability> {
    let mut counts = BTreeMap::new();
    for file in graph.files() {
        *counts.entry(file.language).or_insert(0_usize) += 1;
    }
    counts
        .into_iter()
        .map(|(language, files)| ClientGraphLanguageCapability {
            language: language.label().to_owned(),
            files,
            imports: true,
            symbols: true,
            call_graph: false,
            resolver: "available".to_owned(),
            extraction: "tree-sitter".to_owned(),
        })
        .collect()
}

fn unsupported_languages(root: &Path) -> Vec<String> {
    const SOURCE_EXTENSIONS: &[(&str, &str)] = &[
        ("java", "Java"),
        ("kt", "Kotlin"),
        ("swift", "Swift"),
        ("c", "C"),
        ("cpp", "C++"),
        ("h", "C/C++ headers"),
        ("rb", "Ruby"),
        ("php", "PHP"),
        ("lua", "Lua"),
    ];
    let mut found = BTreeSet::new();
    collect_unsupported_languages(root, &mut found, SOURCE_EXTENSIONS);
    found.into_iter().collect()
}

fn collect_unsupported_languages(
    directory: &Path,
    found: &mut BTreeSet<String>,
    extensions: &[(&str, &str)],
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if [
                ".git",
                ".venv",
                "node_modules",
                "target",
                "vendor",
                "__pycache__",
            ]
            .contains(&name)
            {
                continue;
            }
            collect_unsupported_languages(&path, found, extensions);
        } else if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            if let Some((_, label)) = extensions.iter().find(|(value, _)| *value == extension) {
                found.insert((*label).to_owned());
            }
        }
    }
}

fn select_nodes(
    graph: &CodeGraph,
    path: Option<&str>,
    depth: usize,
    direction: ClientGraphDirection,
) -> Result<BTreeMap<NodeId, Option<usize>>, GraphProjectionError> {
    let mut selected = BTreeMap::new();
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        for index in 0..graph.files().len().min(MAX_GRAPH_NODES) {
            selected.insert(NodeId(index), None);
        }
        return Ok(selected);
    };
    let Some(start) = graph.find(Path::new(path)) else {
        return Err(GraphProjectionError::MissingPath(path.to_owned()));
    };
    selected.insert(start, Some(0));
    let direction = match direction {
        ClientGraphDirection::Imports => Direction::Imports,
        ClientGraphDirection::Importers => Direction::Importers,
        ClientGraphDirection::Both => Direction::Both,
    };
    for neighbour in graph::neighbours(graph, start, depth, direction) {
        selected.insert(neighbour.id, Some(neighbour.distance));
    }
    Ok(selected)
}

fn apply_search_filter(
    graph: &CodeGraph,
    selected: &mut BTreeMap<NodeId, Option<usize>>,
    search: &str,
) {
    let trimmed = search.trim();
    if trimmed.is_empty() {
        return;
    }
    let needle = trimmed.to_ascii_lowercase();
    selected.retain(|id, _| {
        let Some(node) = graph.node(*id) else {
            return false;
        };
        if node
            .path
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(&needle)
        {
            return true;
        }
        graph
            .symbols_in(*id)
            .iter()
            .any(|symbol| symbol.name.to_ascii_lowercase().contains(&needle))
    });
}

fn project_node(
    graph: &CodeGraph,
    analysis: &GraphAnalysis,
    id: NodeId,
    distance: Option<usize>,
) -> Option<ClientGraphNode> {
    let node = graph.node(id)?;
    let community = analysis.community.get(id.0).copied();
    Some(ClientGraphNode {
        path: node.path.to_string_lossy().into_owned(),
        language: node.language.label().to_owned(),
        distance,
        degree: node.imports.len().saturating_add(node.importers.len()),
        community,
        community_size: community
            .and_then(|value| analysis.community_sizes.get(value).copied())
            .unwrap_or(0),
        is_articulation_point: analysis
            .articulation_points
            .get(id.0)
            .copied()
            .unwrap_or(false),
        in_cycle: analysis.cycle_nodes.get(id.0).copied().unwrap_or(false),
        imports: node
            .imports
            .iter()
            .take(MAX_GRAPH_NODES)
            .filter_map(|target| graph.node(*target))
            .map(|target| target.path.to_string_lossy().into_owned())
            .collect(),
        importers: node
            .importers
            .iter()
            .take(MAX_GRAPH_NODES)
            .filter_map(|source| graph.node(*source))
            .map(|source| source.path.to_string_lossy().into_owned())
            .collect(),
        symbols: graph
            .symbols_in(id)
            .iter()
            .take(MAX_GRAPH_SYMBOLS_PER_FILE)
            .map(|symbol| ClientGraphSymbol {
                name: symbol.name.clone(),
                kind: symbol.kind.label().to_owned(),
                line: symbol.line,
            })
            .collect(),
    })
}

fn project_edges(
    graph: &CodeGraph,
    selected: &BTreeMap<NodeId, Option<usize>>,
) -> Vec<ClientGraphEdge> {
    selected
        .keys()
        .filter_map(|id| graph.node(*id).map(|node| (*id, node)))
        .flat_map(|(from, node)| {
            node.imports.iter().filter_map(move |to| {
                if !selected.contains_key(to) {
                    return None;
                }
                Some(ClientGraphEdge {
                    from: node.path.to_string_lossy().into_owned(),
                    to: graph
                        .node(*to)
                        .map(|target| target.path.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    relation: "imports".to_owned(),
                    provenance: edge_provenance(graph, from, *to).0,
                    confidence_bps: edge_provenance(graph, from, *to).1,
                })
            })
        })
        .collect()
}

fn edge_provenance(graph: &CodeGraph, from: NodeId, to: NodeId) -> (String, u16) {
    match graph.provenance(from, to) {
        Some(crate::graph::EdgeProvenance::Parsed) => ("parsed".to_owned(), 1000),
        Some(crate::graph::EdgeProvenance::Inferred) => ("inferred".to_owned(), 500),
        Some(crate::graph::EdgeProvenance::Ambiguous) | None => ("ambiguous".to_owned(), 200),
    }
}

#[allow(
    clippy::indexing_slicing,
    reason = "all indices come from the same graph-owned adjacency and NodeId vectors"
)]
fn analyze_graph(graph: &CodeGraph) -> GraphAnalysis {
    let mut undirected = vec![Vec::new(); graph.files().len()];
    let mut directed = vec![Vec::new(); graph.files().len()];
    let mut reverse = vec![Vec::new(); graph.files().len()];
    for (index, file) in graph.files().iter().enumerate() {
        for target in &file.imports {
            directed[index].push(target.0);
            reverse[target.0].push(index);
            undirected[index].push(target.0);
            undirected[target.0].push(index);
        }
    }
    for neighbours in [&mut undirected, &mut directed, &mut reverse] {
        for values in neighbours.iter_mut() {
            values.sort_unstable();
            values.dedup();
        }
    }
    let (community, community_sizes) = weak_components(&undirected);
    let articulation_points = articulation_points(&undirected);
    let cycle_nodes = strongly_connected_cycle_nodes(&directed, &reverse);
    GraphAnalysis {
        community,
        community_sizes,
        articulation_points,
        cycle_nodes,
    }
}

#[allow(
    clippy::indexing_slicing,
    reason = "adjacency entries are constructed from validated graph node indices"
)]
fn weak_components(adjacency: &[Vec<usize>]) -> (Vec<usize>, Vec<usize>) {
    let mut ids = vec![usize::MAX; adjacency.len()];
    let mut sizes = Vec::new();
    for start in 0..adjacency.len() {
        if ids[start] != usize::MAX {
            continue;
        }
        let id = sizes.len();
        let mut stack = vec![start];
        let mut size = 0;
        ids[start] = id;
        while let Some(node) = stack.pop() {
            size += 1;
            for next in &adjacency[node] {
                if ids[*next] == usize::MAX {
                    ids[*next] = id;
                    stack.push(*next);
                }
            }
        }
        sizes.push(size);
    }
    (ids, sizes)
}

#[allow(
    clippy::indexing_slicing,
    reason = "adjacency entries are constructed from validated graph node indices"
)]
fn articulation_points(adjacency: &[Vec<usize>]) -> Vec<bool> {
    let mut discovered = vec![usize::MAX; adjacency.len()];
    let mut low = vec![usize::MAX; adjacency.len()];
    let mut parent = vec![usize::MAX; adjacency.len()];
    let mut result = vec![false; adjacency.len()];
    let mut time = 0;
    for root in 0..adjacency.len() {
        if discovered[root] == usize::MAX {
            articulation_visit(
                root,
                adjacency,
                &mut discovered,
                &mut low,
                &mut parent,
                &mut time,
                &mut result,
            );
        }
    }
    result
}

#[allow(
    clippy::indexing_slicing,
    reason = "recursive traversal only follows validated adjacency indices"
)]
fn articulation_visit(
    node: usize,
    adjacency: &[Vec<usize>],
    discovered: &mut [usize],
    low: &mut [usize],
    parent: &mut [usize],
    time: &mut usize,
    result: &mut [bool],
) {
    discovered[node] = *time;
    low[node] = *time;
    *time += 1;
    let mut children = 0;
    for next in &adjacency[node] {
        if discovered[*next] == usize::MAX {
            children += 1;
            parent[*next] = node;
            articulation_visit(*next, adjacency, discovered, low, parent, time, result);
            low[node] = low[node].min(low[*next]);
            if parent[node] == usize::MAX && children > 1 {
                result[node] = true;
            }
            if parent[node] != usize::MAX && low[*next] >= discovered[node] {
                result[node] = true;
            }
        } else if *next != parent[node] {
            low[node] = low[node].min(discovered[*next]);
        }
    }
}

#[allow(
    clippy::indexing_slicing,
    reason = "SCC vectors have one slot per graph node and follow validated indices"
)]
fn strongly_connected_cycle_nodes(directed: &[Vec<usize>], reverse: &[Vec<usize>]) -> Vec<bool> {
    let mut visited = vec![false; directed.len()];
    let mut order = Vec::with_capacity(directed.len());
    for node in 0..directed.len() {
        if !visited[node] {
            finish_visit(node, directed, &mut visited, &mut order);
        }
    }
    let mut component = vec![usize::MAX; directed.len()];
    let mut sizes = Vec::new();
    for node in order.into_iter().rev() {
        if component[node] != usize::MAX {
            continue;
        }
        let id = sizes.len();
        let mut stack = vec![node];
        let mut size = 0;
        component[node] = id;
        while let Some(current) = stack.pop() {
            size += 1;
            for next in &reverse[current] {
                if component[*next] == usize::MAX {
                    component[*next] = id;
                    stack.push(*next);
                }
            }
        }
        sizes.push(size);
    }
    directed
        .iter()
        .enumerate()
        .map(|(node, edges)| sizes[component[node]] > 1 || edges.contains(&node))
        .collect()
}

#[allow(
    clippy::indexing_slicing,
    reason = "DFS only follows validated adjacency indices"
)]
fn finish_visit(
    node: usize,
    adjacency: &[Vec<usize>],
    visited: &mut [bool],
    order: &mut Vec<usize>,
) {
    visited[node] = true;
    for next in &adjacency[node] {
        if !visited[*next] {
            finish_visit(*next, adjacency, visited, order);
        }
    }
    order.push(node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_projects_only_parsed_edges_and_bounded_symbols() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.rs"), "pub fn answer() {}\n").unwrap();
        std::fs::write(source.join("b.rs"), "use crate::a;\nfn caller() {}\n").unwrap();

        let page = build_page(
            root.path(),
            ClientGraphQuery {
                path: None,
                depth: 2,
                direction: ClientGraphDirection::Both,
                search: None,
            },
        )
        .unwrap();

        assert_eq!(page.nodes.len(), 2);
        assert_eq!(page.edges.len(), 1);
        assert_eq!(
            page.edges.first().map(|edge| edge.provenance.as_str()),
            Some("parsed")
        );
        assert_eq!(
            page.edges.first().map(|edge| edge.relation.as_str()),
            Some("imports")
        );
        assert_eq!(
            page.edges.first().map(|edge| edge.confidence_bps),
            Some(1000)
        );
        assert_eq!(page.summary.unresolved_imports, 0);
        assert_eq!(page.summary.communities, 1);
        assert_eq!(page.summary.articulation_points, 0);
        assert_eq!(page.summary.cycle_nodes, 0);
        assert!(page.nodes.iter().any(|node| {
            node.path == "src/a.rs"
                && node.degree == 1
                && node.community == Some(0)
                && node.community_size == 2
                && !node.is_articulation_point
                && !node.in_cycle
                && node
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == "answer" && symbol.kind == "fn")
        }));
    }

    #[test]
    fn overview_marks_import_cycles_and_articulation_points_deterministically() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.rs"), "use crate::b;\n").unwrap();
        std::fs::write(source.join("b.rs"), "use crate::c;\n").unwrap();
        std::fs::write(source.join("c.rs"), "use crate::a;\n").unwrap();

        let page = build_page(
            root.path(),
            ClientGraphQuery {
                path: None,
                depth: 2,
                direction: ClientGraphDirection::Both,
                search: None,
            },
        )
        .unwrap();

        assert_eq!(page.summary.communities, 1);
        assert_eq!(page.summary.cycle_nodes, 3);
        assert!(page.nodes.iter().all(|node| node.in_cycle));
        assert!(page.nodes.iter().all(|node| !node.is_articulation_point));
    }

    #[test]
    fn search_filters_to_paths_or_symbols_containing_the_needle() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("alpha.rs"), "pub fn apple() {}\n").unwrap();
        std::fs::write(source.join("beta.rs"), "pub fn banana() {}\n").unwrap();

        let page = build_page(
            root.path(),
            ClientGraphQuery {
                path: None,
                depth: 1,
                direction: ClientGraphDirection::Both,
                search: Some("alpha".to_owned()),
            },
        )
        .unwrap();
        assert!(
            page.nodes.iter().any(|node| node.path == "src/alpha.rs"),
            "path match survives"
        );
        assert!(
            !page.nodes.iter().any(|node| node.path == "src/beta.rs"),
            "non-matching path is filtered"
        );

        let page = build_page(
            root.path(),
            ClientGraphQuery {
                path: None,
                depth: 1,
                direction: ClientGraphDirection::Both,
                search: Some("BANANA".to_owned()),
            },
        )
        .unwrap();
        assert!(
            page.nodes.iter().any(|node| node.path == "src/beta.rs"),
            "symbol match is case-insensitive"
        );
        assert!(
            !page.nodes.iter().any(|node| node.path == "src/alpha.rs"),
            "non-matching symbol is filtered"
        );

        let page = build_page(
            root.path(),
            ClientGraphQuery {
                path: None,
                depth: 1,
                direction: ClientGraphDirection::Both,
                search: Some("   ".to_owned()),
            },
        )
        .unwrap();
        assert_eq!(page.nodes.len(), 2, "blank search is treated as absent");
    }

    #[test]
    fn a_missing_focus_path_is_a_refused_graph_question() {
        let root = tempfile::tempdir().unwrap();
        let error = build_page(
            root.path(),
            ClientGraphQuery {
                path: Some("src/missing.rs".to_owned()),
                depth: 1,
                direction: ClientGraphDirection::Both,
                search: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("not found"));
    }
}
