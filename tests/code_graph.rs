//! The properties the code graph has.
//!
//! Each test here is one line of the graph's verification checklist. The
//! ones that matter most are the *negative* properties: that an external crate
//! is not an edge, that an empty graph is distinguishable from an absent one,
//! and that a bound which bites is reported rather than silently applied.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fs;
use std::path::{Path, PathBuf};

use smed::graph::{self, Direction, SourceLanguage};
use tempfile::TempDir;

/// A workspace with `src/lib.rs` declaring `a` and `b`, where `a` imports `b`
/// and `b` imports an external crate.
fn fixture() -> TempDir {
    let temp = TempDir::new().expect("fixture");
    write(
        temp.path(),
        "src/lib.rs",
        "mod a;\nmod b;\npub struct Root;\n",
    );
    write(
        temp.path(),
        "src/a.rs",
        "use crate::b::Helper;\nuse serde_json::Value;\n\npub fn from_a() -> Helper { todo!() }\n",
    );
    write(temp.path(), "src/b.rs", "pub struct Helper;\n");
    temp
}

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, contents).expect("write");
}

fn node_named(graph: &smed::graph::CodeGraph, relative: &str) -> smed::graph::NodeId {
    graph
        .find(Path::new(relative))
        .unwrap_or_else(|| panic!("{relative} is not a node"))
}

fn path_of(graph: &smed::graph::CodeGraph, id: smed::graph::NodeId) -> String {
    graph
        .node(id)
        .map_or_else(|| "<unknown>".to_owned(), |node| render_path(&node.path))
}

fn render_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[test]
fn two_builds_of_one_tree_agree_exactly() {
    let temp = fixture();
    let first = graph::build(temp.path()).expect("first build");
    let second = graph::build(temp.path()).expect("second build");

    // Rendered rather than compared field-by-field: the claim is that the whole
    // structure is reproducible, not that one accessor happens to match.
    let render = |graph: &smed::graph::CodeGraph| {
        graph
            .files()
            .iter()
            .map(|file| {
                format!(
                    "{}|{:?}|{:?}|{:?}|{:?}",
                    render_path(&file.path),
                    file.language,
                    file.module,
                    file.imports,
                    file.importers
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(render(&first), render(&second));
    assert_eq!(first.unresolved(), second.unresolved());
}

#[test]
fn an_external_crate_is_counted_as_external_and_never_an_edge() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let a = node_named(&graph, "src/a.rs");
    let b = node_named(&graph, "src/b.rs");

    let node = graph.node(a).expect("node a");
    assert!(
        node.imports.contains(&b),
        "a imports b through `use crate::b::Helper`"
    );
    assert_eq!(
        node.imports.len(),
        1,
        "`use serde_json::Value` must not become an edge"
    );
    assert_eq!(
        graph.external(),
        1,
        "the external import is counted as external, not dropped"
    );
    assert_eq!(
        graph.unresolved(),
        0,
        "an ordinary dependency import is understood, not a scanner failure — \
         counting it as unresolved would bury real misses"
    );
}

#[test]
fn a_crate_path_naming_no_module_is_unresolved_not_external() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "src/lib.rs", "mod a;\n");
    write(temp.path(), "src/a.rs", "use crate::nowhere::Thing;\n");
    let graph = graph::build(temp.path()).expect("build");

    assert_eq!(
        graph.unresolved(),
        1,
        "a `crate::` path that matches nothing is a real miss"
    );
    assert_eq!(graph.external(), 0);
}

#[test]
fn a_mod_declaration_with_no_file_is_unresolved() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "src/lib.rs", "mod missing;\n");
    let graph = graph::build(temp.path()).expect("build");

    assert_eq!(graph.unresolved(), 1);
    assert_eq!(graph.external(), 0);
}

#[test]
fn a_mod_declaration_is_an_edge_to_the_child_file() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let root = node_named(&graph, "src/lib.rs");
    let found = graph::neighbours(&graph, root, 1, Direction::Imports);
    let reached: Vec<_> = found
        .iter()
        .filter_map(|neighbour| graph.node(neighbour.id))
        .map(|node| render_path(&node.path))
        .collect();
    assert_eq!(reached, vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()]);
}

#[test]
fn depth_two_reaches_what_depth_one_does_not() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "src/lib.rs", "mod a;\n");
    write(temp.path(), "src/a.rs", "use crate::b::X;\nmod b;\n");
    write(temp.path(), "src/b.rs", "pub struct X;\n");
    let graph = graph::build(temp.path()).expect("build");
    let root = node_named(&graph, "src/lib.rs");

    let shallow = graph::neighbours(&graph, root, 1, Direction::Imports);
    let deep = graph::neighbours(&graph, root, 2, Direction::Imports);
    assert_eq!(shallow.len(), 1);
    assert_eq!(deep.len(), 2);
    assert!(
        deep.iter().all(|neighbour| neighbour.distance <= 2),
        "no neighbour may be reported beyond the requested depth"
    );
}

#[test]
fn unconnected_files_report_no_path_rather_than_an_empty_one() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "src/lib.rs", "pub struct Root;\n");
    write(temp.path(), "src/lonely.rs", "pub struct Lonely;\n");
    write(temp.path(), "src/other.rs", "pub struct Other;\n");
    let graph = graph::build(temp.path()).expect("build");

    let from = node_named(&graph, "src/lonely.rs");
    let to = node_named(&graph, "src/other.rs");
    assert!(
        graph::between(&graph, from, to).is_none(),
        "no chain exists, and `Some(vec![])` would read as adjacency"
    );
}

#[test]
fn between_returns_the_chain_inclusive_of_both_ends() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let from = node_named(&graph, "src/a.rs");
    let to = node_named(&graph, "src/b.rs");

    let path = graph::between(&graph, from, to).expect("a reaches b");
    assert_eq!(path.first().copied(), Some(from));
    assert_eq!(path.last().copied(), Some(to));
}

#[test]
fn a_workspace_with_no_supported_source_is_empty_not_merely_edgeless() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "README.md", "# not rust\n");
    write(temp.path(), "notes.txt", "not source\n");
    let graph = graph::build(temp.path()).expect("build");

    assert!(
        graph.is_empty(),
        "`is_empty` is what lets the tool say 'nothing to look in' instead of 'found nothing'"
    );
    assert_eq!(graph.unresolved(), 0);
}

#[test]
fn javascript_imports_and_definitions_are_syntax_backed() {
    let temp = TempDir::new().expect("fixture");
    write(
        temp.path(),
        "src/util.js",
        "export function helper() { return 1; }\n",
    );
    write(
        temp.path(),
        "src/app.js",
        "import { helper } from './util';\nimport React from 'react';\nexport class App {}\n",
    );
    let graph = graph::build(temp.path()).expect("build");
    let app = node_named(&graph, "src/app.js");
    let util = node_named(&graph, "src/util.js");

    assert_eq!(
        graph.node(app).map(|node| node.language),
        Some(SourceLanguage::JavaScript)
    );
    assert!(
        graph
            .node(app)
            .is_some_and(|node| node.imports.contains(&util))
    );
    assert_eq!(graph.external(), 1);
    assert_eq!(graph.unresolved(), 0);
    assert_eq!(graph.definitions("App").len(), 1);
    assert_eq!(graph.definitions("helper").len(), 1);
}

#[test]
fn typescript_uses_the_typescript_grammar_for_imports_and_interfaces() {
    let temp = TempDir::new().expect("fixture");
    write(
        temp.path(),
        "src/types.ts",
        "export interface User { id: string; }\n",
    );
    write(
        temp.path(),
        "src/app.ts",
        "import type { User } from './types';\nexport function load(): User { throw new Error(); }\n",
    );
    let graph = graph::build(temp.path()).expect("build");
    let app = node_named(&graph, "src/app.ts");
    let types = node_named(&graph, "src/types.ts");

    assert_eq!(
        graph.node(app).map(|node| node.language),
        Some(SourceLanguage::TypeScript)
    );
    assert!(
        graph
            .node(app)
            .is_some_and(|node| node.imports.contains(&types))
    );
    assert_eq!(graph.external(), 0);
    assert_eq!(graph.unresolved(), 0);
    assert_eq!(graph.definitions("User").len(), 1);
    assert_eq!(graph.definitions("load").len(), 1);
}

#[test]
fn python_relative_imports_and_external_packages_are_split() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "pkg/models.py", "class User:\n    pass\n");
    write(
        temp.path(),
        "pkg/app.py",
        "from .models import User\nimport requests\n\ndef run():\n    return User\n",
    );
    let graph = graph::build(temp.path()).expect("build");
    let app = node_named(&graph, "pkg/app.py");
    let models = node_named(&graph, "pkg/models.py");

    assert!(
        graph
            .node(app)
            .is_some_and(|node| node.imports.contains(&models))
    );
    assert_eq!(graph.external(), 1);
    assert_eq!(graph.unresolved(), 0);
    assert_eq!(graph.definitions("User").len(), 1);
    assert_eq!(graph.definitions("run").len(), 1);
}

#[test]
fn go_relative_package_imports_resolve_without_guessing_external_modules() {
    let temp = TempDir::new().expect("fixture");
    write(
        temp.path(),
        "cmd/main.go",
        "package main\nimport \"./internal\"\n",
    );
    write(
        temp.path(),
        "cmd/internal/model.go",
        "package internal\ntype Model struct{}\n",
    );
    write(
        temp.path(),
        "cmd/other.go",
        "package main\nimport \"fmt\"\n",
    );
    let graph = graph::build(temp.path()).expect("build");
    let main = node_named(&graph, "cmd/main.go");
    let model = node_named(&graph, "cmd/internal/model.go");

    assert!(
        graph
            .node(main)
            .is_some_and(|node| node.imports.contains(&model))
    );
    assert_eq!(graph.external(), 1);
    assert_eq!(graph.unresolved(), 0);
    assert_eq!(graph.definitions("Model").len(), 1);
}

#[test]
fn skipped_directories_are_never_scanned() {
    let temp = fixture();
    write(
        temp.path(),
        "target/debug/build.rs",
        "pub struct Generated;\n",
    );
    write(
        temp.path(),
        "node_modules/x/index.rs",
        "pub struct Vendored;\n",
    );
    let graph = graph::build(temp.path()).expect("build");

    assert!(
        graph.definitions("Generated").is_empty(),
        "build output is not source"
    );
    assert!(graph.definitions("Vendored").is_empty());
}

#[test]
fn definitions_carry_the_file_and_a_one_based_line() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");

    let sites = graph.definitions("Helper");
    assert_eq!(sites.len(), 1);
    let site = sites.first().expect("one site");
    assert_eq!(
        graph.node(site.file).map(|node| render_path(&node.path)),
        Some("src/b.rs".to_owned())
    );
    assert_eq!(site.line, 1);
    assert_eq!(site.kind.label(), "struct");
}

#[test]
fn a_file_outside_src_is_a_node_but_reaches_nothing_by_crate_path() {
    let temp = fixture();
    // A test binary is its own crate root; `use smed::…` is an external crate
    // from the graph's point of view, and inventing an edge into `src/` here
    // would be the scanner guessing.
    write(temp.path(), "tests/it.rs", "use smed::graph;\n");
    let graph = graph::build(temp.path()).expect("build");

    let node = graph.node(node_named(&graph, "tests/it.rs")).expect("node");
    assert!(node.module.is_none());
    assert!(node.imports.is_empty());
}

#[test]
fn the_file_bound_is_reported_rather_than_silently_applied() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "src/lib.rs", "pub struct Root;\n");
    // One file over the cap is enough: the claim is that the bound is disclosed,
    // and writing MAX_FILES + 1 real files would trade minutes for nothing.
    let oversized = "x".repeat(usize::try_from(graph::MAX_FILE_BYTES).unwrap_or(usize::MAX) + 1);
    write(temp.path(), "src/huge.rs", &format!("// {oversized}\n"));
    let graph = graph::build(temp.path()).expect("build");

    let truncation = graph.truncation();
    assert_eq!(truncation.files_too_large, 1);
    assert!(truncation.is_truncated());
    assert!(
        graph.find(Path::new("src/huge.rs")).is_none(),
        "an unread file must not appear as an empty node"
    );
}

// --- E3 slice 1: provenance, blast radius, change mapping ---

#[test]
fn every_edge_built_from_source_is_parsed() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let a = node_named(&graph, "src/a.rs");
    let b = node_named(&graph, "src/b.rs");

    assert_eq!(
        graph.provenance(a, b),
        Some(graph::EdgeProvenance::Parsed),
        "the a→b edge is a resolved declaration"
    );
    assert_eq!(
        graph.provenance(b, a),
        None,
        "no edge exists in that direction, which must not read as ambiguous"
    );
    assert_eq!(graph.non_parsed_edges(), 0);
}

#[test]
fn symbols_in_are_a_files_definition_sites_in_line_order() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let a = node_named(&graph, "src/a.rs");

    let sites = graph.symbols_in(a);
    assert_eq!(sites.len(), 1, "a `use` line is an edge, not a definition");
    assert_eq!(sites[0].name, "from_a");
    assert_eq!(sites[0].line, 4);
    assert_eq!(sites[0].kind.label(), "fn");
}

#[test]
fn blast_radius_is_every_importer_of_every_definition_site() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");

    let radius = graph::blast_radius(&graph, "Helper", 1).expect("Helper is defined");
    assert_eq!(radius.sites.len(), 1);
    assert_eq!(radius.sites[0].name, "Helper");
    let reached: Vec<String> = radius
        .affected
        .iter()
        .map(|neighbour| path_of(&graph, neighbour.id))
        .collect();
    assert_eq!(
        reached,
        vec!["src/a.rs".to_owned(), "src/lib.rs".to_owned()],
        "a.rs imports Helper; lib.rs declares the module"
    );
    assert!(radius.affected.iter().all(|n| n.distance == 1));
}

#[test]
fn blast_radius_deepens_with_depth() {
    let temp = TempDir::new().expect("fixture");
    write(temp.path(), "src/lib.rs", "mod a;\nmod b;\nmod c;\n");
    write(temp.path(), "src/a.rs", "use crate::b::Helper;\n");
    write(temp.path(), "src/b.rs", "pub struct Helper;\n");
    write(temp.path(), "src/c.rs", "use crate::a::X;\n");
    let graph = graph::build(temp.path()).expect("build");

    let shallow = graph::blast_radius(&graph, "Helper", 1).expect("radius");
    let deep = graph::blast_radius(&graph, "Helper", 2).expect("radius");
    assert_eq!(shallow.affected.len(), 2, "a.rs and lib.rs import b.rs");
    assert_eq!(deep.affected.len(), 3);
    let c = deep
        .affected
        .iter()
        .find(|neighbour| path_of(&graph, neighbour.id) == "src/c.rs")
        .expect("c.rs is two hops away");
    assert_eq!(c.distance, 2);
}

#[test]
fn an_unknown_symbol_has_no_blast_radius() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    assert!(
        graph::blast_radius(&graph, "NoSuchThing", 1).is_none(),
        "`None` is the 'no evidence' answer, distinct from an empty reach list"
    );
}

#[test]
fn reachable_from_several_sources_reports_each_node_once_at_its_nearest_distance() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let a = node_named(&graph, "src/a.rs");
    let b = node_named(&graph, "src/b.rs");

    // Starting nodes are never re-reported; lib.rs is reached by two routes
    // (via a.rs's `mod a;` and b.rs's `mod b;`) and must appear exactly once.
    let reached = graph::reachable(&graph, &[a, b], 1, Direction::Importers);
    assert_eq!(reached.len(), 1, "lib.rs is the only node reached, once");
    assert_eq!(reached[0].distance, 1);
    assert_eq!(path_of(&graph, reached[0].id), "src/lib.rs");

    let at_depth_two = graph::reachable(&graph, &[a, b], 2, Direction::Importers);
    let paths: Vec<String> = at_depth_two
        .iter()
        .map(|neighbour| path_of(&graph, neighbour.id))
        .collect();
    assert_eq!(
        paths,
        vec!["src/lib.rs".to_owned()],
        "deeper still reports lib.rs once; no node repeats"
    );
}

#[test]
fn a_diff_maps_to_touched_definitions_and_affected_files() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let diff = "\
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1,2 @@
 pub struct Helper;
+pub struct Helper2;
";
    let parsed = graph::parse_unified(diff);
    assert_eq!(parsed.files.len(), 1);
    assert_eq!(parsed.unparsed, 0);
    assert_eq!(parsed.files_dropped, 0);
    let file = parsed.files.first().expect("one file");
    assert_eq!(file.ranges, vec![graph::LineRange { start: 1, end: 2 }]);

    let mapped = graph::map(&graph, &parsed.files, 1);
    assert!(mapped.unmapped.is_empty());
    assert_eq!(mapped.entries.len(), 1);
    let entry = &mapped.entries[0];
    assert_eq!(entry.touched.len(), 1);
    assert_eq!(entry.touched[0].name, "Helper");
    assert_eq!(entry.touched[0].line, 1);
    let affected: Vec<String> = entry
        .affected
        .iter()
        .map(|neighbour| path_of(&graph, neighbour.id))
        .collect();
    assert_eq!(
        affected,
        vec!["src/a.rs".to_owned(), "src/lib.rs".to_owned()]
    );
}

#[test]
fn a_diff_range_skips_untouched_definitions() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let diff = "\
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,2 @@
 use crate::b::Helper;
 use serde_json::Value;
";
    let parsed = graph::parse_unified(diff);
    let mapped = graph::map(&graph, &parsed.files, 1);
    assert_eq!(mapped.entries.len(), 1);
    assert!(
        mapped.entries[0].touched.is_empty(),
        "the change covers lines 1–2; from_a lives on line 3"
    );
    let affected: Vec<String> = mapped.entries[0]
        .affected
        .iter()
        .map(|neighbour| path_of(&graph, neighbour.id))
        .collect();
    assert_eq!(
        affected,
        vec!["src/lib.rs".to_owned()],
        "lib.rs declares `mod a;`, so it imports a.rs"
    );
}

#[test]
fn a_path_the_graph_has_no_node_for_is_unmapped_not_dropped() {
    let temp = fixture();
    let graph = graph::build(temp.path()).expect("build");
    let diff = "--- a/Cargo.toml\n+++ b/Cargo.toml\n@@ -1 +1 @@\nx\n";
    let parsed = graph::parse_unified(diff);
    let mapped = graph::map(&graph, &parsed.files, 1);

    assert!(mapped.entries.is_empty());
    assert_eq!(mapped.unmapped.len(), 1);
    assert_eq!(mapped.unmapped[0].path, PathBuf::from("Cargo.toml"));
}

#[test]
fn a_deleted_file_maps_to_the_old_path_with_nothing_touched() {
    let temp = fixture();
    write(temp.path(), "src/gone.rs", "pub struct Gone;\n");
    let graph = graph::build(temp.path()).expect("build");
    let diff = "--- a/src/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n";
    let parsed = graph::parse_unified(diff);
    let file = parsed.files.first().expect("one file");
    assert_eq!(file.path, PathBuf::from("src/gone.rs"));
    assert_eq!(file.ranges, vec![graph::LineRange { start: 0, end: 0 }]);

    let mapped = graph::map(&graph, &parsed.files, 1);
    assert_eq!(mapped.entries.len(), 1);
    assert!(
        mapped.entries[0].touched.is_empty(),
        "a deleted file has no new-side definitions"
    );
}

#[test]
fn malformed_hunk_headers_are_counted_not_guessed() {
    let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ not a hunk @@\n";
    let parsed = graph::parse_unified(diff);
    assert_eq!(parsed.unparsed, 1);
    assert!(
        parsed.files.is_empty(),
        "a file whose only hunk was unreadable has no ranges to map"
    );
}
