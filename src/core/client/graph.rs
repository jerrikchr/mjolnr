//! Frontend-safe projections for the deterministic code graph (E7).
//!
//! The graph is rebuilt from the current workspace for each query. These DTOs
//! expose computed source relationships only; they never carry authority or a
//! model-written explanation of what a change means.

use serde::{Deserialize, Serialize};

pub const MAX_GRAPH_NODES: usize = 64;
pub const MAX_GRAPH_SYMBOLS_PER_FILE: usize = 32;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientGraphDirection {
    Imports,
    Importers,
    #[default]
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientGraphQuery {
    /// A workspace-relative file to focus, or `None` for a bounded overview.
    pub path: Option<String>,
    pub depth: usize,
    pub direction: ClientGraphDirection,
    /// Optional case-insensitive substring over `path` or any `symbols[].name`.
    /// Empty after trimming is treated as absent, so the query stays total —
    /// a blank search never claims "nothing found".
    #[serde(default)]
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientGraphSymbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientGraphNode {
    pub path: String,
    pub language: String,
    pub distance: Option<usize>,
    pub imports: Vec<String>,
    pub importers: Vec<String>,
    pub symbols: Vec<ClientGraphSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientGraphEdge {
    pub from: String,
    pub to: String,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientGraphSummary {
    pub files_scanned: usize,
    pub external_imports: usize,
    pub unresolved_imports: usize,
    pub files_skipped: usize,
    pub files_too_large: usize,
    pub non_parsed_edges: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientGraphPage {
    pub query: ClientGraphQuery,
    pub nodes: Vec<ClientGraphNode>,
    pub edges: Vec<ClientGraphEdge>,
    pub summary: ClientGraphSummary,
    pub truncated: bool,
}
