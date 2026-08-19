//! Querying the workspace's code graph (§E3).
//!
//! One reason to change: how the graph is exposed to a model.
//!
//! `search_text` answers "where does this string appear". This answers "what
//! reaches this file, what does it reach, where is this name defined, what
//! would a change to it break, and which definitions does a diff touch" — the
//! questions a model currently rebuilds one grep at a time.
//!
//! [`ToolTier::Read`] is not a concession. Every fact this returns is derivable
//! from `read_file` over the same paths; returning it in one bounded call
//! changes the cost, not the authority. The graph is advisory context, exactly
//! like project instructions: an edge is not permission, and knowing where a
//! symbol lives is not consent to write to it.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::graph::{self, CodeGraph, Direction, EdgeProvenance, MAX_DEPTH, NodeId};
use crate::policy::paths;
use crate::tools::files;

/// Default traversal depth. One hop answers "what does this touch"; the deeper
/// walks are opt-in because their result grows fast.
const DEFAULT_DEPTH: usize = 1;

#[derive(Debug)]
pub struct QueryGraph;

impl QueryGraph {
    pub const NAME: &'static str = "query_graph";
}

#[async_trait]
impl Tool for QueryGraph {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Query the workspace's source graph: what a file imports, what imports it, where a symbol is defined, how two files connect, what a symbol change would reach (impact), or which definitions a diff touches and which files it affects (map_diff). Built from syntax on every call."
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["neighbors", "define", "between", "impact", "map_diff"],
                    "description": "neighbors: files around `path`. define: definition sites for `symbol`. between: shortest chain from `path` to `other_path`. impact: what reaches `symbol`'s definition files. map_diff: definitions `diff` touches and files it affects."
                },
                "path": {
                    "type": ["string", "null"],
                    "minLength": 1,
                    "description": "Workspace-relative file. Required for neighbors and between."
                },
                "other_path": {
                    "type": ["string", "null"],
                    "minLength": 1,
                    "description": "The far end, for between."
                },
                "symbol": {
                    "type": ["string", "null"],
                    "minLength": 1,
                    "maxLength": 256,
                    "description": "Item name, for define and impact."
                },
                "diff": {
                    "type": ["string", "null"],
                    "minLength": 1,
                    "maxLength": 524_288,
                    "description": "Unified diff text, for map_diff. Paths inside it are matched against the graph only — never the filesystem."
                },
                "direction": {
                    "type": ["string", "null"],
                    "enum": ["imports", "importers", "both", null],
                    "description": "For neighbors. Defaults to both."
                },
                "depth": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "maximum": MAX_DEPTH,
                    "description": "For neighbors, impact, and map_diff. Defaults to 1."
                }
            },
            "required": ["mode"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let mode = super::arguments::required_string(arguments, "mode")?;
        Ok(match mode.as_str() {
            "define" => format!(
                "graph: where is {:?} defined",
                string_or_empty(arguments, "symbol")
            ),
            "between" => format!(
                "graph: how {:?} connects to {:?}",
                string_or_empty(arguments, "path"),
                string_or_empty(arguments, "other_path")
            ),
            "impact" => format!(
                "graph: blast radius of {:?}",
                string_or_empty(arguments, "symbol")
            ),
            "map_diff" => "graph: diff → touched definitions and affected files".to_owned(),
            _ => format!(
                "graph: files around {:?}",
                string_or_empty(arguments, "path")
            ),
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let mode = super::arguments::required_string(&arguments, "mode")?;
        let root = context.workspace_root.clone();

        let built = files::blocking(move || Ok(graph::build(&root))).await?;
        let graph = match built {
            Ok(graph) => graph,
            Err(error) => return Ok(refusal(&error.to_string())),
        };

        if graph.is_empty() {
            // Distinct from "found nothing": there was nothing to look in.
            return Ok(refusal(
                "no Rust sources under this workspace root — the code graph covers Rust only",
            ));
        }

        match mode.as_str() {
            "define" => Ok(define(&graph, &arguments)),
            "between" => between(&graph, &arguments, &context),
            "impact" => Ok(impact(&graph, &arguments)),
            "map_diff" => Ok(map_diff(&graph, &arguments)),
            _ => neighbors(&graph, &arguments, &context),
        }
    }
}

fn depth_arg(arguments: &serde_json::Value) -> usize {
    usize::try_from(super::arguments::optional_u64(
        arguments,
        "depth",
        DEFAULT_DEPTH as u64,
    ))
    .unwrap_or(DEFAULT_DEPTH)
    .clamp(1, MAX_DEPTH)
}

fn neighbors(
    graph: &CodeGraph,
    arguments: &serde_json::Value,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let Some(start) = resolve_node(graph, arguments, "path", context)? else {
        return Ok(refusal("neighbors needs `path`"));
    };
    let depth = depth_arg(arguments);
    let direction = match arguments
        .get("direction")
        .and_then(serde_json::Value::as_str)
    {
        Some("imports") => Direction::Imports,
        Some("importers") => Direction::Importers,
        _ => Direction::Both,
    };

    let found = graph::neighbours(graph, start, depth, direction);
    let mut lines = vec![format!(
        "{} · depth {depth} · {}",
        display(graph, start),
        match direction {
            Direction::Imports => "imports",
            Direction::Importers => "importers",
            Direction::Both => "both directions",
        }
    )];
    if found.is_empty() {
        lines.push("  (no connected files within this depth)".to_owned());
    }
    for neighbour in &found {
        let node = graph.node(neighbour.id);
        let relation = node.map_or("", |node| {
            if node.importers.contains(&start) {
                "imported by"
            } else {
                "imports"
            }
        });
        lines.push(format!(
            "  {} hop{} · {relation} · {}",
            neighbour.distance,
            if neighbour.distance == 1 { "" } else { "s" },
            display(graph, neighbour.id)
        ));
    }
    Ok(success(graph, lines, found.len() >= graph::MAX_NODES))
}

/// What a change to one symbol would reach: its definition sites, then
/// everything that imports them.
fn impact(graph: &CodeGraph, arguments: &serde_json::Value) -> ToolResult {
    let Some(symbol) = arguments.get("symbol").and_then(serde_json::Value::as_str) else {
        return refusal("impact needs `symbol`");
    };
    let depth = depth_arg(arguments);
    let mut lines = vec![format!("{symbol} · blast radius, depth {depth}")];

    let Some(radius) = graph::blast_radius(graph, symbol, depth) else {
        lines.push(
            "  (no definition found; it may be in a macro, a dependency, or not Rust)".to_owned(),
        );
        return success(graph, lines, false);
    };
    for site in &radius.sites {
        lines.push(format!(
            "  defines: {}:{} · {}",
            display(graph, site.file),
            site.line,
            site.kind.label()
        ));
    }
    if radius.affected.is_empty() {
        lines.push("  (nothing imports the defining file(s) within this depth)".to_owned());
    }
    for neighbour in &radius.affected {
        lines.push(format!(
            "  reaches: {} · {} hop{}",
            display(graph, neighbour.id),
            neighbour.distance,
            if neighbour.distance == 1 { "" } else { "s" },
        ));
    }
    success(graph, lines, radius.affected.len() >= graph::MAX_NODES)
}

/// Which definitions a diff touches, and which files reach each changed file.
///
/// The diff's paths are keys into the graph index; nothing here touches the
/// filesystem (`graph::diffmap` documents the boundary).
fn map_diff(graph: &CodeGraph, arguments: &serde_json::Value) -> ToolResult {
    let Some(diff) = arguments.get("diff").and_then(serde_json::Value::as_str) else {
        return refusal("map_diff needs `diff`");
    };
    if diff.len() > graph::MAX_DIFF_BYTES {
        return refusal("diff exceeds the parse bound — split the change into smaller diffs");
    }
    let depth = depth_arg(arguments);
    let parsed = graph::parse_unified(diff);
    let mapped = graph::map(graph, &parsed.files, depth);

    let mut lines = vec![format!(
        "diff → touched definitions and affected files, depth {depth}"
    )];
    for entry in &mapped.entries {
        lines.push(format!(
            "  {} · {} definition(s) touched · {} file(s) affected",
            display(graph, entry.file),
            entry.touched.len(),
            entry.affected.len(),
        ));
        for site in entry.touched.iter().take(graph::MAX_SYMBOL_SITES) {
            lines.push(format!(
                "      touched: {} · {} · line {}",
                site.name,
                site.kind.label(),
                site.line
            ));
        }
        if entry.touched.len() > graph::MAX_SYMBOL_SITES {
            lines.push(format!(
                "      …and {} more definition(s)",
                entry.touched.len() - graph::MAX_SYMBOL_SITES
            ));
        }
        for neighbour in &entry.affected {
            lines.push(format!(
                "      affected: {} · {} hop{}{}",
                display(graph, neighbour.id),
                neighbour.distance,
                if neighbour.distance == 1 { "" } else { "s" },
                provenance_mark(graph, neighbour.id, entry.file)
            ));
        }
    }
    for unmapped in &mapped.unmapped {
        lines.push(format!(
            "  not mapped: {} (no graph node — not Rust, or beyond scan bounds)",
            display_path(&unmapped.path)
        ));
    }
    if mapped.entries.is_empty() && mapped.unmapped.is_empty() {
        lines.push("  (the diff names no files this parser could read)".to_owned());
    }
    if parsed.unparsed > 0 {
        lines.push(format!(
            "— {} hunk header(s) could not be read",
            parsed.unparsed
        ));
    }
    if parsed.files_dropped > 0 {
        lines.push(format!(
            "— {} file(s) beyond the diff bound were dropped",
            parsed.files_dropped
        ));
    }
    let non_parsed = graph.non_parsed_edges();
    lines.push(if non_parsed == 0 {
        "— provenance: all edges parsed".to_owned()
    } else {
        format!("— provenance: {non_parsed} edge(s) not parsed — ~ inferred, ? ambiguous")
    });
    let truncated = parsed.files_dropped > 0
        || mapped
            .entries
            .iter()
            .any(|entry| entry.affected.len() >= graph::MAX_NODES);
    success(graph, lines, truncated)
}

/// The mark an edge's provenance earns on the direct edge from an affected
/// file to the changed file it imports. An edge runs importer → imported
/// (`from` → `to`), so the affected file is the *source* of the edge being
/// marked — the caller hands over the neighbour as `from` and the changed file
/// as `to`. Parsed edges earn nothing: they are the default state, and the
/// header line states the count when anything differs.
fn provenance_mark(graph: &CodeGraph, from: NodeId, to: NodeId) -> String {
    match graph.provenance(from, to) {
        Some(EdgeProvenance::Inferred) => " · ~ inferred".to_owned(),
        Some(EdgeProvenance::Ambiguous) => " · ? ambiguous".to_owned(),
        _ => String::new(),
    }
}

fn define(graph: &CodeGraph, arguments: &serde_json::Value) -> ToolResult {
    let Some(symbol) = arguments.get("symbol").and_then(serde_json::Value::as_str) else {
        return refusal("define needs `symbol`");
    };
    let sites = graph.definitions(symbol);
    let mut lines = vec![format!("{symbol} · {} definition site(s)", sites.len())];
    for site in sites {
        lines.push(format!(
            "  {}:{} · {}",
            display(graph, site.file),
            site.line,
            site.kind.label()
        ));
    }
    if sites.is_empty() {
        lines.push(
            "  (no definition found; it may be in a macro, a dependency, or not Rust)".to_owned(),
        );
    }
    success(graph, lines, sites.len() >= graph::MAX_SYMBOL_SITES)
}

fn between(
    graph: &CodeGraph,
    arguments: &serde_json::Value,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let (Some(from), Some(to)) = (
        resolve_node(graph, arguments, "path", context)?,
        resolve_node(graph, arguments, "other_path", context)?,
    ) else {
        return Ok(refusal("between needs `path` and `other_path`"));
    };

    let lines = match graph::between(graph, from, to) {
        Some(path) => {
            let mut lines = vec![format!("{} step(s)", path.len().saturating_sub(1))];
            for (position, id) in path.iter().enumerate() {
                lines.push(format!(
                    "  {}. {}",
                    position.saturating_add(1),
                    display(graph, *id)
                ));
            }
            lines
        }
        None => vec![format!(
            "no connection within {MAX_DEPTH} steps between {} and {}",
            display(graph, from),
            display(graph, to)
        )],
    };
    Ok(success(graph, lines, false))
}

/// Resolve an argument path to a node, applying containment first.
fn resolve_node(
    graph: &CodeGraph,
    arguments: &serde_json::Value,
    key: &str,
    context: &ToolContext,
) -> Result<Option<NodeId>, ToolError> {
    let Some(raw) = arguments.get(key).and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let requested = PathBuf::from(raw);
    let contained =
        paths::existing(&context.workspace_root, &requested).map_err(files::preview_path_error)?;
    let relative = contained
        .strip_prefix(&context.workspace_root)
        .unwrap_or(&contained);
    Ok(graph.find(relative))
}

fn display(graph: &CodeGraph, id: NodeId) -> String {
    graph
        .node(id)
        .map_or_else(|| "<unknown>".to_owned(), |node| display_path(&node.path))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Every result carries the graph's own limits, so a model never reads a
/// bounded answer as an exhaustive one.
fn success(graph: &CodeGraph, mut lines: Vec<String>, hit_cap: bool) -> ToolResult {
    let truncation = graph.truncation();
    let languages = graph
        .language_counts()
        .into_iter()
        .map(|(language, count)| format!("{}={count}", language.label()))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!(
        "— graph: {} files · languages: {languages} · {} external import(s) · {} unresolved",
        graph.files().len(),
        graph.external(),
        graph.unresolved()
    ));
    if truncation.is_truncated() {
        lines.push(format!(
            "— truncated: {} file(s) beyond the scan limit, {} too large",
            truncation.files_skipped, truncation.files_too_large
        ));
    }
    let mut result = ToolResult::ok(lines.join("\n"));
    result.truncated = hit_cap || truncation.is_truncated();
    result
}

/// Refusals carry [`ReasonCode::ToolExecution`]: the call was well-formed and
/// the graph answered, but the arguments named nothing this tool can act on.
fn refusal(detail: &str) -> ToolResult {
    ToolResult::refused(ReasonCode::ToolExecution, detail)
}

fn string_or_empty(arguments: &serde_json::Value, key: &str) -> String {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::core::message::ToolOutcome;
    use crate::graph::EdgeProvenance;

    /// The fixture `tests/code_graph.rs` builds: `a` imports `b`, `lib`
    /// declares both modules and therefore imports both too.
    fn fixture() -> (tempfile::TempDir, CodeGraph) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        std::fs::write(src.join("lib.rs"), "mod a;\nmod b;\npub struct Root;\n").expect("lib");
        std::fs::write(
            src.join("a.rs"),
            "use crate::b::Helper;\nuse serde_json::Value;\n\npub fn from_a() -> Helper { todo!() }\n",
        )
        .expect("a");
        std::fs::write(src.join("b.rs"), "pub struct Helper;\n").expect("b");
        let graph = graph::build(temp.path()).expect("build");
        (temp, graph)
    }

    #[test]
    fn a_mark_is_read_on_the_importer_to_imported_edge() {
        let (_temp, mut graph) = fixture();
        let a = graph.find(Path::new("src/a.rs")).expect("a node");
        let b = graph.find(Path::new("src/b.rs")).expect("b node");
        assert!(graph.provenance(a, b).is_some(), "a imports b");
        // The scanner only ever records Parsed edges; mark the importer edge
        // the way a derived edge would be, to prove the renderer reads that
        // side and not the reverse.
        graph.record_provenance(a, b, EdgeProvenance::Ambiguous);

        assert_eq!(
            provenance_mark(&graph, a, b),
            " · ? ambiguous",
            "the mark resides on the edge the importer owns"
        );
        assert_eq!(
            provenance_mark(&graph, b, a),
            "",
            "the reverse side has no edge and must not borrow the mark"
        );
    }

    #[test]
    fn a_mark_renders_on_the_affected_line_of_the_importer() {
        let (_temp, mut graph) = fixture();
        let a = graph.find(Path::new("src/a.rs")).expect("a node");
        let b = graph.find(Path::new("src/b.rs")).expect("b node");
        graph.record_provenance(a, b, EdgeProvenance::Ambiguous);

        let diff = "--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1,2 @@\n pub struct Helper;\n+pub struct Helper2;\n";
        let result = map_diff(&graph, &serde_json::json!({ "diff": diff, "depth": 1 }));
        assert_eq!(result.outcome, ToolOutcome::Ok);
        let lines = result.content;
        assert!(
            lines.contains("affected: src/a.rs · 1 hop · ? ambiguous"),
            "the mark belongs on a.rs's line, the file whose edge to b.rs is ambiguous:\n{lines}"
        );
        assert!(
            !lines.contains("affected: src/lib.rs · 1 hop · ? ambiguous"),
            "lib.rs's edge to b.rs is parsed; the mark must not leak onto it:\n{lines}"
        );
    }

    #[test]
    fn map_diff_discloses_an_affected_walk_that_hits_the_cap() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let src = temp.path().join("src");
        std::fs::create_dir_all(&src).expect("src dir");
        let mut lib = String::from("mod hub;\n");
        let mut modules = Vec::new();
        for i in 0..graph::MAX_NODES {
            let name = format!("f{i:02}");
            std::fs::write(src.join(format!("{name}.rs")), "use crate::hub::Hub;\n")
                .expect("importer");
            modules.push(format!("mod {name};\n"));
        }
        lib.push_str(&modules.join(""));
        std::fs::write(src.join("lib.rs"), lib).expect("lib");
        std::fs::write(src.join("hub.rs"), "pub struct Hub;\n").expect("hub");
        let graph = graph::build(temp.path()).expect("build");

        let diff = "--- a/src/hub.rs\n+++ b/src/hub.rs\n@@ -1 +1,2 @@\n pub struct Hub;\n+pub struct Hub2;\n";
        let result = map_diff(&graph, &serde_json::json!({ "diff": diff, "depth": 1 }));
        assert!(graph::MAX_NODES < graph.files().len());
        assert!(
            result.truncated,
            "65 importers, 64 admitted: the cap must be disclosed, not silent:\n{}",
            result.content
        );
    }
}
