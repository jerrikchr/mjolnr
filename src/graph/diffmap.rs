//! Mapping a unified diff onto the graph: which definitions a change touches,
//! and which files reach the changed file (change mapping).
//!
//! One reason to change: how a diff's hunks become touched symbols and
//! affected files.
//!
//! # Boundary
//!
//! Paths read from the diff are **keys into the graph index, never filesystem
//! paths.** Nothing here opens a file or resolves a path against the disk; a
//! hostile diff can name anything, and this module will only ever compare
//! strings against the nodes a build produced. Containment belongs to the
//! caller, as everywhere else in `graph`.
//!
//! The parser reads the shape git and diff tools agree on: `--- a/x`,
//! `+++ b/x`, and `@@ -a[,n] +c[,m] @@` headers, where a trailing section
//! heading after the closing `@@` (`@@ -1,3 +1,4 @@ fn main()`) is context,
//! not a range. Hunk bodies are consumed against the counts their headers
//! declare, so an added or removed line whose text begins `++` or `--`
//! (rendered `+++ …`/`--- …`) is content while a hunk is open, never a header.
//! It is deliberately narrow — headers it cannot read are counted
//! ([`ParsedDiff::unparsed`]), never guessed at, and a guessed range would be
//! a lie (`AGENTS.md` §1.3).

use std::path::{Path, PathBuf};

use super::query::{Direction, Neighbour, reachable};
use super::{CodeGraph, MAX_DEPTH, NodeId, SymbolSite};

/// Upper bound on the diff text one call will parse. The tool's schema enforces
/// it too; this is the library-side statement of the same bound.
pub const MAX_DIFF_BYTES: usize = 512 * 1024;

/// Most files one diff may name. Beyond this, files are counted, not parsed:
/// a dropped count is metadata the caller must surface.
pub const MAX_DIFF_FILES: usize = 64;

/// One contiguous touched region on the new-file side. 1-based and inclusive,
/// matching what editors and the graph's own line numbers report. `start == 0`
/// names a hunk that touches nothing on the new side (a pure deletion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

/// A file the diff changes, and where, on the new-file side.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    /// Workspace-relative, as the diff's headers named it.
    pub path: PathBuf,
    pub ranges: Vec<LineRange>,
}

/// What the parser made of a diff's text.
#[derive(Debug, Default)]
pub struct ParsedDiff {
    pub files: Vec<ChangedFile>,
    /// Hunk headers this parser could not read, or could not attach to a
    /// placeable file. Read as "the diff said more than this mapping knows",
    /// never as context that was mapped.
    pub unparsed: usize,
    /// Files beyond [`MAX_DIFF_FILES`] that were counted and not parsed.
    pub files_dropped: usize,
}

/// What one changed file's hunks mean to the graph.
#[derive(Debug, Clone)]
pub struct FileImpact {
    pub file: NodeId,
    /// Definitions whose line falls inside the file's changed ranges, in line
    /// order.
    pub touched: Vec<SymbolSite>,
    /// Files that import this one, nearest first, bounded like every query.
    pub affected: Vec<Neighbour>,
}

/// A changed path the graph has no node for.
#[derive(Debug, Clone)]
pub struct UnmappedFile {
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct ChangeMap {
    /// Changed files the graph knows, ordered by path.
    pub entries: Vec<FileImpact>,
    /// Changed files the graph has no node for: not Rust, or a Rust file the
    /// scanner skipped (too large, beyond the file bound, or not UTF-8).
    pub unmapped: Vec<UnmappedFile>,
}

/// Parse unified-diff text into changed files and new-side ranges.
///
/// The parser is stateful where the format is ambiguous: while a hunk still
/// owes the removed/added lines its header declared, a body line whose text
/// begins `++` (rendered `+++ …`) is added content, never a file header.
#[must_use]
pub fn parse_unified(text: &str) -> ParsedDiff {
    let mut out = ParsedDiff::default();
    let mut current: Option<ChangedFile> = None;
    let mut old_path: Option<PathBuf> = None;
    // Body lines the open hunk still owes, `(removed, added)`. While either
    // is non-zero, `-`/`+` lines are content, not `--- `/`+++ ` headers.
    let mut hunk: Option<(usize, usize)> = None;

    for line in text.lines() {
        if hunk == Some((0, 0)) {
            hunk = None;
        }
        if let Some((removed, added)) = hunk.as_mut() {
            if consume_hunk_body_line(line, removed, added) {
                if *removed == 0 && *added == 0 {
                    hunk = None;
                }
                continue;
            }
            hunk = None;
        }
        if let Some(header) = line.strip_prefix("--- ") {
            old_path = header_path(header);
        } else if let Some(header) = line.strip_prefix("+++ ") {
            flush(&mut out, current.take());
            let new_path = header_path(header);
            let path = match (&new_path, &old_path) {
                // A deletion names the old file: `+++ /dev/null` says the new
                // side is gone, and the path worth mapping is the old one.
                (Some(new), Some(old)) if new.as_os_str() == "/dev/null" => Some(old.clone()),
                _ => new_path,
            };
            old_path = None;
            if out.files.len() >= MAX_DIFF_FILES {
                out.files_dropped = out.files_dropped.saturating_add(1);
            } else if let Some(path) = path {
                current = Some(ChangedFile {
                    path,
                    ranges: Vec::new(),
                });
            }
        } else if let Some(header) = line.strip_prefix("@@ ") {
            match parse_hunk(header) {
                Some((removed, added, range)) => {
                    hunk = Some((removed, added));
                    match current.as_mut() {
                        Some(file) => file.ranges.push(range),
                        None => out.unparsed = out.unparsed.saturating_add(1),
                    }
                }
                None => out.unparsed = out.unparsed.saturating_add(1),
            }
        }
    }
    flush(&mut out, current.take());
    out
}

fn consume_hunk_body_line(line: &str, removed: &mut usize, added: &mut usize) -> bool {
    match line.as_bytes().first().copied() {
        Some(b' ') => {
            *removed = removed.saturating_sub(1);
            *added = added.saturating_sub(1);
            true
        }
        Some(b'\\') => true,
        Some(b'-') => {
            *removed = removed.saturating_sub(1);
            true
        }
        Some(b'+') => {
            *added = added.saturating_sub(1);
            true
        }
        _ => false,
    }
}

/// Map parsed changes onto the graph.
///
/// `depth` bounds how far the affected-files walk reaches (clamped to
/// [`MAX_DEPTH`]); the walk is along **importers**, because the question is
/// what reaches the changed file, not what it reaches.
#[must_use]
pub fn map(graph: &CodeGraph, files: &[ChangedFile], depth: usize) -> ChangeMap {
    let depth = depth.clamp(1, MAX_DEPTH);
    let mut out = ChangeMap::default();
    for changed in files {
        let Some(file) = graph.find(&changed.path) else {
            out.unmapped.push(UnmappedFile {
                path: changed.path.clone(),
            });
            continue;
        };
        let touched: Vec<SymbolSite> = graph
            .symbols_in(file)
            .iter()
            .filter(|site| {
                changed
                    .ranges
                    .iter()
                    .any(|range| range.start <= site.line && site.line <= range.end)
            })
            .cloned()
            .collect();
        let affected = reachable(graph, &[file], depth, Direction::Importers);
        out.entries.push(FileImpact {
            file,
            touched,
            affected,
        });
    }
    out.entries
        .sort_by(|a, b| path_of(graph, a.file).cmp(path_of(graph, b.file)));
    out
}

/// Push a finished file, unless its hunks mapped to nothing.
fn flush(out: &mut ParsedDiff, file: Option<ChangedFile>) {
    let Some(file) = file else { return };
    if file.ranges.is_empty() {
        return;
    }
    out.files.push(file);
}

/// `a/x\t2026-…` or `b/x` → `x`. Takes the first tab (git appends a timestamp
/// in some modes), strips a leading `a/`/`b/` rename prefix, and refuses
/// headers that name nothing.
fn header_path(header: &str) -> Option<PathBuf> {
    let trimmed = header.split('\t').next()?.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let stripped = trimmed
        .strip_prefix("a/")
        .or_else(|| trimmed.strip_prefix("b/"))
        .unwrap_or(trimmed);
    Some(PathBuf::from(stripped))
}

/// `@@ -a[,b] +c[,d] @@` → the removed/added counts the hunk body owes and
/// the new-side range `c..=c + d - 1`. Parsing runs through the *closing*
/// `@@`; anything after it (the section heading, `fn main()`) is context and
/// ignored. `None` when the header does not have the shape git writes. A
/// length of zero (the `+0,0` of a pure deletion) yields `start == 0`, which
/// covers nothing.
fn parse_hunk(header: &str) -> Option<(usize, usize, LineRange)> {
    let closing = header.find("@@")?;
    let mut tokens = header[..closing].split_whitespace();
    let old = tokens.next()?;
    let new = tokens.next()?;
    let (_, removed) = parse_count(old.strip_prefix('-')?)?;
    let (start, added) = parse_count(new.strip_prefix('+')?)?;
    Some((
        removed,
        added,
        LineRange {
            start,
            end: start.saturating_add(added.saturating_sub(1)),
        },
    ))
}

/// `a` or `a,b` → `(start, length)`, defaulting a missing length to 1.
fn parse_count(spec: &str) -> Option<(usize, usize)> {
    let (start_text, length_text) = spec
        .split_once(',')
        .map_or((spec, None), |(start, length)| (start, Some(length)));
    let start: usize = start_text.parse().ok()?;
    let length: usize = length_text.map_or(Ok(1), str::parse).ok()?;
    Some((start, length))
}

fn path_of(graph: &CodeGraph, id: NodeId) -> &Path {
    graph
        .node(id)
        .map_or_else(|| Path::new(""), |node| node.path.as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> ParsedDiff {
        parse_unified(line)
    }

    #[test]
    fn ranges_attach_to_the_file_they_follow() {
        let diff = "\
--- a/src/one.rs
+++ b/src/one.rs
@@ -1,2 +1,2 @@
x
@@ -5 +6 @@
y
--- a/src/two.rs
+++ b/src/two.rs
@@ -10 +11 @@
z
";
        let parsed = parse(diff);
        assert_eq!(parsed.unparsed, 0);
        assert_eq!(parsed.files.len(), 2);
        let (first, second) = (
            parsed.files.first().expect("first file"),
            parsed.files.get(1).expect("second file"),
        );
        assert_eq!(first.path, Path::new("src/one.rs"));
        assert_eq!(
            first.ranges,
            vec![
                LineRange { start: 1, end: 2 },
                LineRange { start: 6, end: 6 }
            ]
        );
        assert_eq!(second.path, Path::new("src/two.rs"));
        assert_eq!(second.ranges, vec![LineRange { start: 11, end: 11 }]);
    }

    #[test]
    fn a_deletion_maps_to_the_old_path_with_a_range_covering_nothing() {
        let parsed = parse("--- a/src/gone.rs\n+++ /dev/null\n@@ -1,4 +0,0 @@\n...\n");
        assert_eq!(parsed.files.len(), 1);
        let file = parsed.files.first().expect("one file");
        assert_eq!(file.path, Path::new("src/gone.rs"));
        assert_eq!(file.ranges, vec![LineRange { start: 0, end: 0 }]);
        assert_eq!(parsed.unparsed, 0);
    }

    #[test]
    fn a_new_file_with_no_old_path_maps_to_the_new_path() {
        let parsed = parse("--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,3 @@\na\nb\nc\n");
        assert_eq!(parsed.files.len(), 1);
        let file = parsed.files.first().expect("one file");
        assert_eq!(file.path, Path::new("src/new.rs"));
        assert_eq!(file.ranges, vec![LineRange { start: 1, end: 3 }]);
    }

    #[test]
    fn prefixes_and_timestamps_are_stripped() {
        let parsed = parse(
            "--- a/src/x.rs\t2026-01-01 00:00:00\n+++ b/src/x.rs\t2026-01-01 00:00:01\n@@ -1 +1 @@\n",
        );
        let file = parsed.files.first().expect("one file");
        assert_eq!(file.path, Path::new("src/x.rs"));
    }

    #[test]
    fn a_hunk_with_no_file_is_unparsed_not_guessed() {
        let parsed = parse("@@ -1 +1 @@\n");
        assert_eq!(parsed.unparsed, 1);
        assert!(parsed.files.is_empty());
    }

    #[test]
    fn a_malformed_hunk_is_unparsed_not_guessed() {
        let parsed = parse("--- a/src/x.rs\n+++ b/src/x.rs\n@@ not a hunk @@\n@@ -1,2 + @@\n");
        assert_eq!(parsed.unparsed, 2);
        assert!(parsed.files.is_empty(), "no usable ranges survived");
    }

    #[test]
    fn a_hunk_header_with_a_section_heading_still_parses() {
        // The repository fixture shape: `git diff -p` keeps the enclosing
        // function after the closing `@@`. It is context, not a range.
        let parsed = parse(
            "--- a/src/db.rs\n+++ b/src/db.rs\n@@ -1,3 +1,4 @@ fn connect() {\n x\n-y\n+y\n z\n",
        );
        assert_eq!(parsed.unparsed, 0);
        let file = parsed.files.first().expect("one file");
        assert_eq!(file.path, Path::new("src/db.rs"));
        assert_eq!(file.ranges, vec![LineRange { start: 1, end: 4 }]);
    }

    #[test]
    fn a_hunk_body_line_rendered_as_plus_plus_plus_is_not_a_new_file() {
        // An added line whose text begins `++` renders as `+++ b/…` in the
        // patch. While the hunk still owes the lines it declared, that is
        // content; only after the counts are consumed may a `+++ ` line open
        // a new file.
        let parsed = parse(
            "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+++ b/src/ghost.rs\n+end\n@@ -10 +11 @@\n-tail\n+tail\n",
        );
        assert_eq!(parsed.unparsed, 0);
        assert_eq!(parsed.files.len(), 1, "no phantom file from hunk content");
        let file = parsed.files.first().expect("the changed file");
        assert_eq!(file.path, Path::new("src/a.rs"));
        assert_eq!(
            file.ranges,
            vec![
                LineRange { start: 1, end: 3 },
                LineRange { start: 11, end: 11 },
            ]
        );
    }

    #[test]
    fn a_completed_hunk_releases_consecutive_file_headers() {
        let parsed = parse(
            "--- a/src/one.rs\n+++ b/src/one.rs\n@@ -1,3 +1,3 @@\n first\n-old\n+new\n last\n--- a/src/two.rs\n+++ b/src/two.rs\n@@ -4 +4 @@\n-old2\n+new2\n",
        );
        assert_eq!(parsed.unparsed, 0);
        assert_eq!(parsed.files.len(), 2);
        let first = parsed.files.first().expect("first file");
        assert_eq!(first.path, Path::new("src/one.rs"));
        assert_eq!(first.ranges, vec![LineRange { start: 1, end: 3 }]);
        let second = parsed.files.get(1).expect("second file");
        assert_eq!(second.path, Path::new("src/two.rs"));
        assert_eq!(second.ranges, vec![LineRange { start: 4, end: 4 }]);
    }

    #[test]
    fn files_beyond_the_bound_are_counted_and_dropped() {
        // Only the file count matters, so every repeated block can name the
        // same file.
        let block = "--- a/src/f0.rs\n+++ b/src/f0.rs\n@@ -1 +1 @@\nx\n";
        let diff = block.repeat(MAX_DIFF_FILES + 1);
        let parsed = parse(&diff);
        assert_eq!(parsed.files.len(), MAX_DIFF_FILES);
        assert_eq!(parsed.files_dropped, 1);
    }
}
