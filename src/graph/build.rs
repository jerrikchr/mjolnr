//! Walking the tree and resolving declarations into edges.
//!
//! One reason to change: how a declaration becomes an edge.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    BuildProgress, CodeGraph, EdgeProvenance, FileNode, MAX_FILE_BYTES, MAX_FILES,
    MAX_SYMBOL_SITES, NodeId, SKIPPED_DIRECTORIES, SourceLanguage, SymbolSite, Truncation, foreign,
    rust,
};

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("workspace root is not a directory: {path}")]
    NotADirectory { path: String },
    #[error("reading {path} failed: {detail}")]
    Io { path: String, detail: String },
}

enum ExtractedFile {
    Rust(rust::Extracted),
    Foreign(foreign::Extracted),
}

/// Build the graph for an **already canonicalised** root.
///
/// Blocking by design: the caller runs it on a blocking thread, the same way
/// every other filesystem tool in mjolnr does (`AGENTS.md` §4).
pub fn build(root: &Path) -> Result<CodeGraph, BuildError> {
    build_with_progress(root, |_| {})
}

pub fn build_with_progress(
    root: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<CodeGraph, BuildError> {
    if !root.is_dir() {
        return Err(BuildError::NotADirectory {
            path: root.display().to_string(),
        });
    }

    let mut relative_paths = Vec::new();
    let mut truncation = Truncation::default();
    collect(root, root, &mut relative_paths, &mut truncation)?;

    let mut files = Vec::new();
    let mut by_module = BTreeMap::new();
    let mut by_path = BTreeMap::new();
    let mut extracted = Vec::new();

    let files_total = relative_paths.len();
    for (index, path) in relative_paths.into_iter().enumerate() {
        progress(BuildProgress {
            files_scanned: index.saturating_add(1),
            files_total,
        });
        let absolute = root.join(&path);
        match std::fs::metadata(&absolute) {
            Ok(metadata) if metadata.len() > MAX_FILE_BYTES => {
                truncation.files_too_large = truncation.files_too_large.saturating_add(1);
                continue;
            }
            Ok(_) => {}
            Err(error) => {
                return Err(BuildError::Io {
                    path: path.display().to_string(),
                    detail: error.to_string(),
                });
            }
        }
        // A source file that is not UTF-8 is not Rust. Skipping it is a fact
        // about the file, not a scanner limit, so it does not set truncation.
        let Ok(source) = std::fs::read_to_string(&absolute) else {
            continue;
        };

        let Some(language) = SourceLanguage::from_path(&path) else {
            continue;
        };
        let module = (language == SourceLanguage::Rust)
            .then(|| module_path(&path))
            .flatten();
        let id = NodeId(files.len());
        if let Some(module) = module.clone() {
            by_module.insert(module, id);
        }
        by_path.insert(path.clone(), id);
        files.push(FileNode {
            path,
            language,
            module,
            imports: BTreeSet::new(),
            importers: BTreeSet::new(),
        });
        extracted.push(match language {
            SourceLanguage::Rust => ExtractedFile::Rust(rust::extract(&source)),
            _ => ExtractedFile::Foreign(foreign::extract(language, &source)),
        });
    }

    let mut graph = CodeGraph {
        files,
        by_module,
        by_path,
        symbols: BTreeMap::new(),
        symbols_by_file: Vec::new(),
        edges: BTreeMap::new(),
        external: 0,
        unresolved: 0,
        truncation,
    };
    link(&mut graph, &extracted);
    index_symbols(&mut graph, &extracted);
    Ok(graph)
}

/// Depth-first, name-sorted, so two builds of one tree agree exactly.
fn collect(
    root: &Path,
    directory: &Path,
    out: &mut Vec<PathBuf>,
    truncation: &mut Truncation,
) -> Result<(), BuildError> {
    let entries = std::fs::read_dir(directory).map_err(|error| BuildError::Io {
        path: directory.display().to_string(),
        detail: error.to_string(),
    })?;
    let mut names: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    names.sort();

    for path in names {
        let name = path.file_name().map(std::ffi::OsStr::to_string_lossy);
        let Some(name) = name else { continue };
        if path.is_dir() {
            if SKIPPED_DIRECTORIES.contains(&name.as_ref()) {
                continue;
            }
            collect(root, &path, out, truncation)?;
        } else if SourceLanguage::from_path(&path).is_some() {
            if out.len() >= MAX_FILES {
                truncation.files_skipped = truncation.files_skipped.saturating_add(1);
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
    }
    Ok(())
}

/// The crate-relative module path for a file, or `None` outside `src/`.
fn module_path(relative: &Path) -> Option<String> {
    let mut components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if components.first().map(String::as_str) != Some("src") {
        return None;
    }
    components.remove(0);
    let last = components.pop()?;
    let stem = last.strip_suffix(".rs")?;
    if components.is_empty() && (stem == "lib" || stem == "main") {
        return Some("crate".to_owned());
    }
    if stem != "mod" {
        components.push(stem.to_owned());
    }
    let mut path = String::from("crate");
    for component in components {
        path.push_str("::");
        path.push_str(&component);
    }
    Some(path)
}

fn link(graph: &mut CodeGraph, extracted: &[ExtractedFile]) {
    let mut edges = Vec::new();
    // (external, unresolved)
    let mut counts = (0_usize, 0_usize);

    for (index, declarations) in extracted.iter().enumerate() {
        let source = NodeId(index);
        let Some(node) = graph.files.get(index) else {
            continue;
        };
        match declarations {
            ExtractedFile::Rust(declarations) => {
                let module = node.module.clone();
                for child in &declarations.child_modules {
                    // A `mod x;` naming no file is a genuine miss, never
                    // external: the declaration asserts a file in this crate.
                    let resolution = module
                        .as_ref()
                        .map(|module| format!("{module}::{child}"))
                        .and_then(|candidate| graph.by_module.get(&candidate).copied())
                        .map_or(Resolution::Unresolved, Resolution::Local);
                    record(resolution, source, &mut edges, &mut counts);
                }
                for path in &declarations.uses {
                    let resolution = resolve(graph, module.as_deref(), path);
                    record(resolution, source, &mut edges, &mut counts);
                }
                counts.1 = counts.1.saturating_add(declarations.unparsed_uses);
            }
            ExtractedFile::Foreign(declarations) => {
                for path in &declarations.uses {
                    let resolution = resolve_foreign(graph, &node.path, node.language, path);
                    record(resolution, source, &mut edges, &mut counts);
                }
                counts.1 = counts.1.saturating_add(declarations.unparsed_uses);
            }
        }
    }

    for (source, target) in edges {
        // Every edge built from source is parsed by construction. The
        // provenance contract is about the *other* variants, which nothing
        // creates yet — this line is where a derived edge would have to stop
        // being parsed and start being marked.
        graph.record_provenance(source, target, EdgeProvenance::Parsed);
        if let Some(node) = graph.files.get_mut(source.0) {
            node.imports.insert(target);
        }
        if let Some(node) = graph.files.get_mut(target.0) {
            node.importers.insert(source);
        }
    }
    graph.external = counts.0;
    graph.unresolved = counts.1;
}

/// What a declaration turned out to be.
///
/// The three-way split is the whole point. `use std::fmt` is **not** a scanner
/// failure — it is a fully understood import of something outside this
/// workspace, and counting it as "unresolved" would bury the handful of genuine
/// failures under hundreds of ordinary lines and teach a reader to ignore the
/// number (`AGENTS.md` §1.3).
#[derive(Clone, Copy)]
enum Resolution {
    Local(NodeId),
    External,
    Unresolved,
}

fn record(
    resolution: Resolution,
    source: NodeId,
    edges: &mut Vec<(NodeId, NodeId)>,
    counts: &mut (usize, usize),
) {
    match resolution {
        // A self-edge is not information; a file importing its own module is
        // what `self::` means.
        Resolution::Local(target) if target != source => edges.push((source, target)),
        Resolution::Local(_) => {}
        Resolution::External => counts.0 = counts.0.saturating_add(1),
        Resolution::Unresolved => counts.1 = counts.1.saturating_add(1),
    }
}

/// Resolve one `use` path against the module index, longest prefix first.
///
/// Longest-prefix is what makes `use crate::a::B` — an item inside a module —
/// reach the file that defines it rather than resolving nowhere.
fn resolve(graph: &CodeGraph, module: Option<&str>, path: &str) -> Resolution {
    let mut segments = path.split("::").filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return Resolution::Unresolved;
    };
    let rest: Vec<&str> = segments.collect();

    let mut base: Vec<String> = match first {
        "crate" => vec!["crate".to_owned()],
        // `self`/`super` outside `src/` is a relative import in a file that has
        // no module path — a test binary. Unresolved, not external: it names
        // something local that this graph cannot place.
        "self" => match module {
            Some(module) => split_module(module),
            None => return Resolution::Unresolved,
        },
        "super" => {
            let Some(module) = module else {
                return Resolution::Unresolved;
            };
            let mut owner = split_module(module);
            if owner.pop().is_none() {
                return Resolution::Unresolved;
            }
            owner
        }
        // Any other head is another crate — `std`, a dependency, or this crate
        // seen from a test binary. Fully understood, and deliberately not an
        // edge in a graph whose nodes are files in this workspace.
        _ => return Resolution::External,
    };

    let mut tail = rest.as_slice();
    // Successive `super::` segments each climb one module.
    while tail.first() == Some(&"super") {
        if base.pop().is_none() {
            return Resolution::Unresolved;
        }
        match tail.get(1..) {
            Some(remainder) => tail = remainder,
            None => return Resolution::Unresolved,
        }
    }

    // Only the last segment may be an item name; every earlier one must be a
    // module that exists. Falling further back would let `use crate::nowhere::Thing`
    // resolve to the crate root — the root always matches — and report a proved
    // edge for a path that names nothing. That is the guessing this module
    // exists to refuse, so a gap in the middle is a miss.
    let shortest = tail.len().saturating_sub(1);
    for taken in (shortest..=tail.len()).rev() {
        let Some(prefix) = tail.get(..taken) else {
            continue;
        };
        let mut candidate = base.clone();
        candidate.extend(prefix.iter().map(|part| (*part).to_owned()));
        if let Some(id) = graph.by_module.get(&candidate.join("::")) {
            return Resolution::Local(*id);
        }
    }
    Resolution::Unresolved
}

fn split_module(module: &str) -> Vec<String> {
    module.split("::").map(str::to_owned).collect()
}

fn resolve_foreign(
    graph: &CodeGraph,
    source: &Path,
    language: SourceLanguage,
    import: &str,
) -> Resolution {
    match language {
        SourceLanguage::JavaScript
        | SourceLanguage::TypeScript
        | SourceLanguage::TypeScriptReact => {
            if !import.starts_with('.') {
                return Resolution::External;
            }
            let Some(base) = relative_target(source, import) else {
                return Resolution::Unresolved;
            };
            let extensions = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"];
            let mut candidates = vec![base.clone()];
            if base.extension().is_none() {
                candidates.extend(extensions.iter().map(|extension| {
                    let mut candidate = base.clone();
                    candidate.set_extension(extension.trim_start_matches('.'));
                    candidate
                }));
                candidates.extend(extensions.iter().map(|extension| {
                    let mut candidate = base.join("index");
                    candidate.set_extension(extension.trim_start_matches('.'));
                    candidate
                }));
            }
            candidates
                .into_iter()
                .find_map(|candidate| graph.by_path.get(&candidate).copied())
                .map_or(Resolution::Unresolved, Resolution::Local)
        }
        SourceLanguage::Python => resolve_python(graph, source, import),
        SourceLanguage::Go => resolve_go(graph, source, import),
        SourceLanguage::Dart => resolve_dart(graph, source, import),
        SourceLanguage::Rust => Resolution::Unresolved,
    }
}

fn resolve_dart(graph: &CodeGraph, source: &Path, import: &str) -> Resolution {
    if import.starts_with("dart:")
        || import.starts_with("package:") && import.split('/').count() < 2
    {
        return Resolution::External;
    }
    let candidate = if let Some(package_path) = import.strip_prefix("package:") {
        let mut parts = package_path.splitn(2, '/');
        let _package = parts.next();
        let Some(rest) = parts.next() else {
            return Resolution::External;
        };
        PathBuf::from("lib").join(rest)
    } else if let Some(base) = relative_target(source, import) {
        base
    } else {
        return Resolution::Unresolved;
    };
    let candidate = if candidate.extension().is_none() {
        candidate.with_extension("dart")
    } else {
        candidate
    };
    graph
        .by_path
        .get(&candidate)
        .copied()
        .map_or(Resolution::Unresolved, Resolution::Local)
}

fn resolve_python(graph: &CodeGraph, source: &Path, import: &str) -> Resolution {
    let (base, module) = if import.starts_with('.') {
        let dots = import
            .chars()
            .take_while(|character| *character == '.')
            .count();
        let Some(mut base) = source.parent().map(Path::to_path_buf) else {
            return Resolution::Unresolved;
        };
        for _ in 1..dots {
            if !base.pop() {
                return Resolution::Unresolved;
            }
        }
        (base, import.get(dots..).unwrap_or_default())
    } else {
        (PathBuf::new(), import)
    };
    let module_path = module.replace('.', "/");
    if module_path.is_empty() {
        return Resolution::Unresolved;
    }
    let candidate = base.join(module_path);
    let candidates = [
        candidate.with_extension("py"),
        candidate.join("__init__.py"),
    ];
    let local = candidates
        .into_iter()
        .find_map(|candidate| graph.by_path.get(&candidate).copied());
    if local.is_some() {
        return local.map_or(Resolution::Unresolved, Resolution::Local);
    }
    if import.starts_with('.') {
        Resolution::Unresolved
    } else {
        Resolution::External
    }
}

fn resolve_go(graph: &CodeGraph, source: &Path, import: &str) -> Resolution {
    if !import.starts_with('.') {
        return Resolution::External;
    }
    let Some(base) = relative_target(source, import) else {
        return Resolution::Unresolved;
    };
    let exact = graph.by_path.get(&base).copied();
    if exact.is_some() {
        return exact.map_or(Resolution::Unresolved, Resolution::Local);
    }
    let matches: Vec<NodeId> = graph
        .by_path
        .iter()
        .filter(|(path, id)| {
            let _ = id;
            path.parent() == Some(base.as_path())
                && path.extension().is_some_and(|extension| extension == "go")
        })
        .map(|(_, id)| *id)
        .collect();
    match matches.as_slice() {
        [id] => Resolution::Local(*id),
        _ => Resolution::Unresolved,
    }
}

fn relative_target(source: &Path, import: &str) -> Option<PathBuf> {
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    normalise_relative(&parent.join(import))
}

fn normalise_relative(path: &Path) -> Option<PathBuf> {
    let mut normalised = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalised.push(part),
            std::path::Component::ParentDir => {
                if !normalised.pop() {
                    return None;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(normalised)
}

/// Index every named item by name, ordered by path then line, and by file, in
/// line order.
fn index_symbols(graph: &mut CodeGraph, extracted: &[ExtractedFile]) {
    let mut symbols: BTreeMap<String, Vec<SymbolSite>> = BTreeMap::new();
    let mut by_file: Vec<Vec<SymbolSite>> = Vec::with_capacity(extracted.len());
    for (index, declarations) in extracted.iter().enumerate() {
        let mut file_sites = Vec::new();
        let file_symbols = match declarations {
            ExtractedFile::Rust(declarations) => &declarations.symbols,
            ExtractedFile::Foreign(declarations) => &declarations.symbols,
        };
        for (name, kind, line) in file_symbols {
            let site = SymbolSite {
                file: NodeId(index),
                line: *line,
                kind: *kind,
                name: name.clone(),
            };
            symbols.entry(name.clone()).or_default().push(site.clone());
            file_sites.push(site);
        }
        file_sites.sort_by_key(|site| site.line);
        by_file.push(file_sites);
    }
    for sites in symbols.values_mut() {
        sites.sort_by_key(|site| (site.file, site.line));
        sites.truncate(MAX_SYMBOL_SITES);
    }
    graph.symbols = symbols;
    graph.symbols_by_file = by_file;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_of(path: &str) -> Option<String> {
        module_path(Path::new(path))
    }

    #[test]
    fn module_paths_follow_the_crate_layout() {
        // Invented module names throughout. `tests/architecture.rs` scans source
        // text for import edges, and a fixture naming a real module reads as one
        // — a false positive that would fail the build for a string literal.
        assert_eq!(module_of("src/lib.rs"), Some("crate".to_owned()));
        assert_eq!(module_of("src/main.rs"), Some("crate".to_owned()));
        assert_eq!(
            module_of("src/alpha/mod.rs"),
            Some("crate::alpha".to_owned())
        );
        assert_eq!(
            module_of("src/alpha/beta.rs"),
            Some("crate::alpha::beta".to_owned())
        );
        assert_eq!(
            module_of("src/alpha/beta/gamma.rs"),
            Some("crate::alpha::beta::gamma".to_owned())
        );
    }

    #[test]
    fn a_file_outside_src_has_no_module_path() {
        // A test binary is its own crate root: nothing can reach it by
        // `crate::`, and pretending otherwise would invent edges.
        assert_eq!(module_of("tests/architecture.rs"), None);
        assert_eq!(module_of("build.rs"), None);
    }
}
