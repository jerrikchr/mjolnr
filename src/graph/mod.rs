//! A deterministic code graph over the workspace.
//!
//! One reason to change: what mjolnr considers a node or an edge.
//!
//! # Why this is not the memory layer `AGENTS.md` §11 law 2 rejects
//!
//! Law 2 forbids embeddings, vector stores, bandits, success-rate scoring, and
//! LLM reflection — mechanisms whose output depends on training, history, or
//! chance. This module reads syntax. The same tree produces the same graph,
//! byte for byte, on every machine and every run; ordering is by distance then
//! by path, never by a score. Nothing here learns, and nothing here ranks.
//!
//! # What it is honest about
//!
//! An edge exists only where a `use` or `mod` declaration resolves to a file
//! inside the workspace. External crates, unexpandable statements, and paths
//! that resolve nowhere are **counted** as unresolved rather than dropped, so
//! "this file has no importers" and "the scanner could not tell" are different
//! answers (`AGENTS.md` §1.3).
//!
//! # Dependencies
//!
//! Nothing internal. This module is a leaf: it takes an already-canonicalised
//! root and returns data. Path containment belongs to the caller, which is why
//! [`build`] cannot be handed a path the model chose.

//! # Edge provenance
//!
//! Every edge records how it was made ([`EdgeProvenance`]). An edge runs from
//! the importer to the file it imports ([`CodeGraph::provenance(from, to)`]
//! answers about `from` importing `to`), so a blast-radius walk along
//! importers must query each affected file *as the source* of its edge to the
//! changed file. The scanner only creates [`EdgeProvenance::Parsed`] edges
//! today; the other variants exist so that a derived edge — when one is ever
//! introduced — must render as marked, never as equal to a parsed one
//! (`AGENTS.md` §1.3).

mod build;
mod diffmap;
mod foreign;
mod query;
mod rust;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub use build::{BuildError, build};
pub use diffmap::{
    ChangeMap, ChangedFile, FileImpact, LineRange, MAX_DIFF_BYTES, MAX_DIFF_FILES, ParsedDiff,
    UnmappedFile, map, parse_unified,
};
pub use query::{BlastRadius, Direction, Neighbour, between, blast_radius, neighbours, reachable};
pub use rust::SymbolKind;

/// A source language the graph can parse structurally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceLanguage {
    Rust,
    JavaScript,
    TypeScript,
    TypeScriptReact,
    Python,
    Go,
}

impl SourceLanguage {
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_string_lossy().as_ref() {
            "rs" => Some(Self::Rust),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::TypeScriptReact),
            "py" => Some(Self::Python),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescript-react",
            Self::Python => "python",
            Self::Go => "go",
        }
    }
}

/// Most files one build will scan. A repository larger than this yields a
/// truncated graph that says so, never a subset presented as whole.
pub const MAX_FILES: usize = 4096;

/// Largest source file the scanner will read. Generated files run to megabytes
/// and contribute noise proportional to their size.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Most nodes any one query returns.
pub const MAX_NODES: usize = 64;

/// Deepest traversal any one query performs.
pub const MAX_DEPTH: usize = 4;

/// Most definition sites returned for one symbol name.
pub const MAX_SYMBOL_SITES: usize = 32;

/// Directories never traversed, matching the set `docs/context.md` already
/// excludes from skill-resource discovery.
pub(crate) const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    ".venv",
    "node_modules",
    "target",
    "vendor",
    "__pycache__",
];

/// Index into [`CodeGraph::files`]. Not a bare `usize` at the boundary: a
/// newtype makes a node identity impossible to confuse with a count or a line
/// number (`AGENTS.md` §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) usize);

/// One file, and the edges it participates in.
#[derive(Debug)]
pub struct FileNode {
    /// Workspace-relative, always — an absolute path here would leak the
    /// owner's directory layout into model context.
    pub path: PathBuf,
    /// The grammar used to read this file.
    pub language: SourceLanguage,
    /// The crate-relative module path, or `None` for a file outside `src/`
    /// (a test or example, which is its own crate root and cannot be reached
    /// by a `crate::` import).
    pub module: Option<String>,
    /// Files this one imports.
    pub imports: BTreeSet<NodeId>,
    /// Files that import this one.
    pub importers: BTreeSet<NodeId>,
}

/// Where one named item is defined.
#[derive(Debug, Clone)]
pub struct SymbolSite {
    pub file: NodeId,
    /// 1-based, matching what `read_file` and every editor report.
    pub line: usize,
    pub kind: SymbolKind,
    /// The item's name — the name it is indexed under. Self-contained so a
    /// site found through [`symbols_in`](CodeGraph::symbols_in), where no
    /// query key exists to echo, still says what it is.
    pub name: String,
}

/// How much evidence backs an edge.
///
/// Every edge the scanner creates is [`Parsed`](Self::Parsed) — a `use` or
/// `mod` declaration that resolved to a workspace file. The other variants
/// exist so the rendering contract is in place before any derived edge does:
/// an edge that is not fully parsed must render as marked, never as equal to
/// one that is (`AGENTS.md` §1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EdgeProvenance {
    /// Read from a declaration that resolved — a proved edge.
    Parsed,
    /// Derived from parsed edges (a projection), not read from source.
    Inferred,
    /// Exists, but no declaration can be shown for it.
    Ambiguous,
}

impl EdgeProvenance {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::Inferred => "inferred",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// Which bound, if any, cut the scan short.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Truncation {
    /// Files beyond [`MAX_FILES`] that were never opened.
    pub files_skipped: usize,
    /// Files larger than [`MAX_FILE_BYTES`] that were never read.
    pub files_too_large: usize,
}

impl Truncation {
    #[must_use]
    pub fn is_truncated(self) -> bool {
        self.files_skipped > 0 || self.files_too_large > 0
    }
}

#[derive(Debug)]
pub struct CodeGraph {
    files: Vec<FileNode>,
    by_module: BTreeMap<String, NodeId>,
    by_path: BTreeMap<PathBuf, NodeId>,
    symbols: BTreeMap<String, Vec<SymbolSite>>,
    symbols_by_file: Vec<Vec<SymbolSite>>,
    edges: BTreeMap<(NodeId, NodeId), EdgeProvenance>,
    external: usize,
    unresolved: usize,
    truncation: Truncation,
}

impl CodeGraph {
    #[must_use]
    pub fn files(&self) -> &[FileNode] {
        &self.files
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&FileNode> {
        self.files.get(id.0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Resolve a workspace-relative path to its node.
    #[must_use]
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        self.by_path.get(path).copied()
    }

    /// Number of indexed files by parser language, ordered by display label.
    #[must_use]
    pub fn language_counts(&self) -> BTreeMap<SourceLanguage, usize> {
        let mut counts = BTreeMap::new();
        for file in &self.files {
            let count = counts.entry(file.language).or_insert(0_usize);
            *count = (*count).saturating_add(1);
        }
        counts
    }

    /// Definition sites for a name, ordered by path then line, bounded by
    /// [`MAX_SYMBOL_SITES`].
    #[must_use]
    pub fn definitions(&self, name: &str) -> &[SymbolSite] {
        self.symbols
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Definition sites inside one file, in line order.
    #[must_use]
    pub fn symbols_in(&self, file: NodeId) -> &[SymbolSite] {
        self.symbols_by_file
            .get(file.0)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// How the edge from `from` to `to` came to be. `None` when no edge exists
    /// — deliberately distinct from [`EdgeProvenance::Ambiguous`], which names
    /// an edge that exists without a placeable declaration.
    #[must_use]
    pub fn provenance(&self, from: NodeId, to: NodeId) -> Option<EdgeProvenance> {
        self.edges.get(&(from, to)).copied()
    }

    /// Edges whose provenance is not [`EdgeProvenance::Parsed`]. Zero today, by
    /// construction — the number exists so a renderer can state "all edges
    /// parsed" as a fact it checked rather than a claim it repeated.
    #[must_use]
    pub fn non_parsed_edges(&self) -> usize {
        self.edges
            .values()
            .filter(|provenance| **provenance != EdgeProvenance::Parsed)
            .count()
    }

    /// Record how one edge came to be. [`build`] marks every edge it creates as
    /// [`Parsed`](EdgeProvenance::Parsed); the setter exists so tests can prove
    /// the rendering contract for the other variants.
    pub(crate) fn record_provenance(
        &mut self,
        from: NodeId,
        to: NodeId,
        provenance: EdgeProvenance,
    ) {
        self.edges.insert((from, to), provenance);
    }

    /// Imports of other crates — `std`, dependencies, and this crate seen from a
    /// test binary. Understood, not failed: they have no node here because
    /// nodes are files in this workspace.
    #[must_use]
    pub fn external(&self) -> usize {
        self.external
    }

    /// Declarations naming something local that the scanner could not place: a
    /// `crate::` path matching no module, a `mod x;` with no file, a `use` it
    /// could not expand. This is the number worth reading — kept separate from
    /// [`external`](Self::external) precisely so that hundreds of ordinary
    /// `use std::…` lines cannot bury a handful of real misses.
    #[must_use]
    pub fn unresolved(&self) -> usize {
        self.unresolved
    }

    #[must_use]
    pub fn truncation(&self) -> Truncation {
        self.truncation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_with(two_nodes: EdgeProvenance) -> (CodeGraph, NodeId, NodeId) {
        let mut graph = CodeGraph {
            files: vec![
                FileNode {
                    path: PathBuf::from("src/a.rs"),
                    language: SourceLanguage::Rust,
                    module: Some("crate::a".to_owned()),
                    imports: BTreeSet::new(),
                    importers: BTreeSet::new(),
                },
                FileNode {
                    path: PathBuf::from("src/b.rs"),
                    language: SourceLanguage::Rust,
                    module: Some("crate::b".to_owned()),
                    imports: BTreeSet::new(),
                    importers: BTreeSet::new(),
                },
            ],
            by_module: BTreeMap::new(),
            by_path: BTreeMap::new(),
            symbols: BTreeMap::new(),
            symbols_by_file: vec![Vec::new(), Vec::new()],
            edges: BTreeMap::new(),
            external: 0,
            unresolved: 0,
            truncation: Truncation::default(),
        };
        graph.record_provenance(NodeId(0), NodeId(1), two_nodes);
        (graph, NodeId(0), NodeId(1))
    }

    #[test]
    fn parsed_edges_never_need_a_mark() {
        let (graph, a, b) = graph_with(EdgeProvenance::Parsed);
        assert_eq!(graph.provenance(a, b), Some(EdgeProvenance::Parsed));
        assert_eq!(graph.non_parsed_edges(), 0);
        assert_eq!(EdgeProvenance::Parsed.label(), "parsed");
    }

    #[test]
    fn inferred_and_ambiguous_edges_are_counted_and_render_distinctly() {
        let (mut graph, a, b) = graph_with(EdgeProvenance::Inferred);
        assert_eq!(graph.non_parsed_edges(), 1);
        graph.record_provenance(a, b, EdgeProvenance::Ambiguous);
        assert_eq!(graph.provenance(a, b), Some(EdgeProvenance::Ambiguous));
        assert_eq!(EdgeProvenance::Inferred.label(), "inferred");
        assert_eq!(EdgeProvenance::Ambiguous.label(), "ambiguous");
        assert_eq!(EdgeProvenance::Parsed.label(), "parsed");
        assert_ne!(
            EdgeProvenance::Inferred.label(),
            EdgeProvenance::Parsed.label(),
            "a marked edge must never render as equal to a parsed one"
        );
    }

    #[test]
    fn a_missing_edge_is_none_not_ambiguous() {
        let (graph, a, _b) = graph_with(EdgeProvenance::Parsed);
        assert_eq!(
            graph.provenance(a, NodeId(3)),
            None,
            "no edge must not read as an existing one whose declaration is missing"
        );
    }
}
