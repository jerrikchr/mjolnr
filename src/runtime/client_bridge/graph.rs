//! Projection of the deterministic code graph to the desktop client (E7).

use std::collections::BTreeMap;
use std::path::Path;

use crate::core::client::graph::{
    ClientGraphDirection, ClientGraphEdge, ClientGraphNode, ClientGraphPage, ClientGraphQuery,
    ClientGraphSummary, ClientGraphSymbol, MAX_GRAPH_NODES, MAX_GRAPH_SYMBOLS_PER_FILE,
};
use crate::graph::{self, CodeGraph, Direction, NodeId};

#[derive(Debug, thiserror::Error)]
pub(super) enum GraphProjectionError {
    #[error("building the code graph failed: {0}")]
    Build(#[from] graph::BuildError),
    #[error("the graph query path was not found in the workspace: {0}")]
    MissingPath(String),
}

pub(super) fn build_page(
    root: &Path,
    query: ClientGraphQuery,
) -> Result<ClientGraphPage, GraphProjectionError> {
    let graph = graph::build(root)?;
    let depth = query.depth.min(graph::MAX_DEPTH);
    let mut selected = select_nodes(&graph, query.path.as_deref(), depth, query.direction)?;
    if let Some(search) = query.search.clone() {
        apply_search_filter(&graph, &mut selected, &search);
    }
    let nodes = selected
        .iter()
        .filter_map(|(id, distance)| project_node(&graph, *id, *distance))
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
        },
        truncated,
    })
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

fn project_node(graph: &CodeGraph, id: NodeId, distance: Option<usize>) -> Option<ClientGraphNode> {
    let node = graph.node(id)?;
    Some(ClientGraphNode {
        path: node.path.to_string_lossy().into_owned(),
        language: node.language.label().to_owned(),
        distance,
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
                    provenance: graph
                        .provenance(from, *to)
                        .map_or_else(|| "ambiguous".to_owned(), |value| value.label().to_owned()),
                })
            })
        })
        .collect()
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
        assert_eq!(page.summary.unresolved_imports, 0);
        assert!(page.nodes.iter().any(|node| {
            node.path == "src/a.rs"
                && node
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == "answer" && symbol.kind == "fn")
        }));
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
