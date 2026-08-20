//! Unified-diff capture and parsing (Phase D3 producer).
//!
//! One responsibility: turn what `git diff` printed into
//! [`ChangeCapture`](crate::core::change_capture::ChangeCapture). It runs git,
//! splits the output per file, and decodes each section on its own. It applies
//! no wire bounds and grades no trust — both belong to the bridge, and
//! `tests/architecture.rs` makes that structural by forbidding this module from
//! naming the client contract at all.
//!
//! Three decisions here are load-bearing:
//!
//! 1. **Per-file decoding.** git's stdout is taken as bytes and split before
//!    anything is decoded, so one Latin-1 file does not turn its neighbours
//!    into U+FFFD, and an undecodable file is reported as undecodable rather
//!    than rendered as mojibake and called the file (AGENTS.md §1.3).
//! 2. **Paths come from `---`/`+++`, not from `diff --git`.** The `diff --git
//!    a/X b/Y` line is genuinely ambiguous for a path containing a space; the
//!    marker lines carry exactly one path each. `core.quotePath=false` keeps
//!    non-ASCII paths literal, so the only quoting left to undo is git's
//!    C-style escaping of control characters and quotes.
//! 3. **Untracked files are diffed against `/dev/null` with `--no-index`.** The
//!    alternative is `git add --intent-to-add`, which writes to the index — a
//!    mutation on a read path, performed to render a view. Refused.

use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::core::change_capture::{
    ChangeCapture, ChangeStatus, FileChange, Hunk, HunkLine, LineSide,
};

use super::error::RepositoryError;
use super::git;

/// How many untracked files this producer will spend a `git` process on.
///
/// Each untracked file costs one process, so an unbounded loop over a tree with
/// thousands of untracked paths is a fork bomb wearing a review surface. Beyond
/// this bound the paths are still *named*, in
/// [`ChangeCapture::undiffed_untracked`], because dropping them silently is the
/// failure this bound exists to avoid being an excuse for.
pub(super) const MAX_UNTRACKED_DIFFS: usize = 50;

/// The section marker git prints before every file. Splitting on it is what
/// makes per-file decoding possible.
const SECTION_MARKER: &[u8] = b"diff --git ";

/// Capture the working tree's changes as one moment.
///
/// `head` decides which comparison is even possible: with a commit to compare
/// against, `git diff HEAD` covers staged and unstaged work in one pass; on an
/// unborn branch there is no HEAD to name, and `--cached` is the only honest
/// question left to ask (it reports the whole index as added).
pub(super) fn capture(
    work_dir: &Path,
    head: Option<&str>,
    index_revision: Option<&str>,
    untracked: &[String],
    capture_sequence: u32,
) -> Result<ChangeCapture, RepositoryError> {
    let mut hasher = Sha256::new();
    let tracked = tracked_diff(work_dir, head)?;
    hasher.update(&tracked.stdout);

    let mut files = parse_sections(&tracked.stdout);
    let mut output_truncated = tracked.truncated;
    let mut undiffed_untracked = Vec::new();
    let mut diffed = 0_usize;

    for path in untracked {
        // Porcelain reports a wholly untracked directory as `dir/`. There is no
        // file to diff and no bounded way to walk it here, so it is named
        // rather than expanded.
        if path.ends_with('/') || diffed >= MAX_UNTRACKED_DIFFS {
            undiffed_untracked.push(path.clone());
            continue;
        }
        diffed += 1;
        let output = untracked_diff(work_dir, path)?;
        hasher.update(&output.stdout);
        output_truncated |= output.truncated;

        let mut parsed = parse_sections(&output.stdout);
        if parsed.is_empty() {
            // An empty untracked file really does produce no diff text. It is
            // an added file with nothing in it, which is a fact, not a failure.
            files.push(added_placeholder(path));
            continue;
        }
        // `--no-index` names the left side `/dev/null`, and its right-hand path
        // is whatever argv said. The path mjolnr already knows is authoritative;
        // trusting the header here would let a crafted filename rewrite it.
        for file in &mut parsed {
            file.path.clone_from(path);
            file.old_path = None;
            file.status = ChangeStatus::Added;
        }
        files.append(&mut parsed);
    }

    Ok(ChangeCapture {
        base_revision: head.map(ToOwned::to_owned),
        index_revision: index_revision.map(ToOwned::to_owned),
        digest: hex(&hasher.finalize()),
        files,
        output_truncated,
        undiffed_untracked,
        capture_sequence,
    })
}

fn tracked_diff(work_dir: &Path, head: Option<&str>) -> Result<git::RawGitOutput, RepositoryError> {
    let mut arguments = vec![
        "-c",
        "core.quotePath=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--find-renames",
        "--unified=3",
    ];
    if head.is_some() {
        arguments.push("HEAD");
    } else {
        arguments.push("--cached");
    }
    arguments.push("--");

    let output = git::run_raw(work_dir, "diff", &arguments)?;
    if output.success {
        return Ok(output);
    }
    Err(RepositoryError::CommandFailed {
        operation: "diff",
        detail: output.stderr,
    })
}

fn untracked_diff(work_dir: &Path, path: &str) -> Result<git::RawGitOutput, RepositoryError> {
    // `--no-index` exits non-zero *because* the files differ, which is the
    // expected outcome for every call here. Its exit status carries no failure
    // information, so it is deliberately not checked; an unreadable path simply
    // produces no sections and is reported as an empty added file.
    git::run_raw(
        work_dir,
        "diff",
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-index",
            "--unified=3",
            "--",
            "/dev/null",
            path,
        ],
    )
}

fn added_placeholder(path: &str) -> FileChange {
    FileChange {
        path: path.to_owned(),
        old_path: None,
        status: ChangeStatus::Added,
        hunks: Vec::new(),
        binary: false,
        undecodable: false,
        truncated: false,
    }
}

/// Split raw git output into per-file sections and parse each independently.
fn parse_sections(raw: &[u8]) -> Vec<FileChange> {
    section_bounds(raw)
        .into_iter()
        .filter_map(|(start, end)| raw.get(start..end))
        .map(|section| match std::str::from_utf8(section) {
            Ok(text) => parse_file(text),
            // Not decodable as UTF-8: reported with the flag set and no lines.
            // The path is recovered lossily *only* so the file can be named —
            // a name a human can recognize is worth an approximation; content
            // is not.
            Err(_) => undecodable_file(section),
        })
        .collect()
}

/// Byte ranges of each `diff --git ` section.
fn section_bounds(raw: &[u8]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    for index in 0..raw.len() {
        let at_start = index == 0;
        let after_newline = index > 0 && raw.get(index - 1) == Some(&b'\n');
        if (at_start || after_newline)
            && raw.get(index..index + SECTION_MARKER.len()) == Some(SECTION_MARKER)
        {
            starts.push(index);
        }
    }
    let mut bounds = Vec::with_capacity(starts.len());
    for (position, start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(raw.len());
        bounds.push((*start, end));
    }
    bounds
}

fn undecodable_file(section: &[u8]) -> FileChange {
    let first_line = section
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let header = String::from_utf8_lossy(first_line).into_owned();
    let path = header
        .strip_prefix("diff --git ")
        .and_then(|rest| rest.split_once(" b/"))
        .map_or_else(|| "(unnamed)".to_owned(), |(_, right)| right.to_owned());
    FileChange {
        path,
        old_path: None,
        status: ChangeStatus::Modified,
        hunks: Vec::new(),
        binary: false,
        undecodable: true,
        truncated: false,
    }
}

/// Parse one decoded `diff --git` section.
fn parse_file(text: &str) -> FileChange {
    let mut file = FileChange {
        path: String::new(),
        old_path: None,
        status: ChangeStatus::Modified,
        hunks: Vec::new(),
        binary: false,
        undecodable: false,
        truncated: false,
    };
    let mut left_path: Option<String> = None;
    let mut right_path: Option<String> = None;
    let mut rename_from: Option<String> = None;
    let mut rename_to: Option<String> = None;
    let mut current: Option<HunkBuilder> = None;

    for line in text.lines() {
        if let Some(builder) = HunkBuilder::start(line) {
            if let Some(finished) = current.replace(builder) {
                file.hunks.push(finished.finish());
            }
            continue;
        }
        if let Some(builder) = current.as_mut() {
            builder.push(line);
            continue;
        }
        read_header_line(
            line,
            &mut file,
            &mut left_path,
            &mut right_path,
            &mut rename_from,
            &mut rename_to,
        );
    }
    if let Some(finished) = current {
        file.hunks.push(finished.finish());
    }

    // `+++ /dev/null` means the file is gone, so its identity is the left path.
    //
    // The `diff --git` header is the last resort, not the first, and it exists
    // for a case the marker lines genuinely cannot cover: a binary file's
    // section has no `---`/`+++` at all. Preferring the markers everywhere else
    // is what keeps a path containing a space intact — the header cannot be
    // split unambiguously, and a name recovered imperfectly still beats
    // "(unnamed)" when there is no other source.
    file.path = right_path
        .or_else(|| rename_to.clone())
        .or_else(|| left_path.clone())
        .or_else(|| header_path(text))
        .unwrap_or_else(|| "(unnamed)".to_owned());
    if file.status == ChangeStatus::Renamed {
        file.old_path = rename_from.or(left_path);
    }
    file
}

fn read_header_line(
    line: &str,
    file: &mut FileChange,
    left_path: &mut Option<String>,
    right_path: &mut Option<String>,
    rename_from: &mut Option<String>,
    rename_to: &mut Option<String>,
) {
    if line.starts_with("new file mode") {
        file.status = ChangeStatus::Added;
    } else if line.starts_with("deleted file mode") {
        file.status = ChangeStatus::Deleted;
    } else if let Some(rest) = line.strip_prefix("rename from ") {
        *rename_from = Some(unquote(rest));
        file.status = ChangeStatus::Renamed;
    } else if let Some(rest) = line.strip_prefix("rename to ") {
        // Capturing the path, not merely the status. A pure rename has 100%
        // similarity, so git emits no `---`/`+++` lines and no hunks — this is
        // the only place the new path appears at all.
        *rename_to = Some(unquote(rest));
        file.status = ChangeStatus::Renamed;
    } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
        file.binary = true;
    } else if let Some(rest) = line.strip_prefix("--- ") {
        *left_path = marker_path(rest);
    } else if let Some(rest) = line.strip_prefix("+++ ") {
        *right_path = marker_path(rest);
    }
}

/// The right-hand path from a `diff --git a/X b/Y` header.
///
/// Only ever a fallback, for the one case the `---`/`+++` markers cannot cover:
/// a binary file's section has neither line.
///
/// The header is genuinely ambiguous — a path containing ` b/` makes any single
/// split wrong from both ends — so the split point is not searched for at all.
/// Instead: git writes `a/X b/X` with the *same* path on both sides for
/// everything except a rename, so the halves have equal length and the boundary
/// is arithmetic rather than a guess. When the halves do not match (a rename),
/// there is no sound answer and this yields `None` rather than a plausible
/// wrong one; a renamed file reaches its path through `rename to` anyway.
fn header_path(text: &str) -> Option<String> {
    let header = text.lines().next()?.strip_prefix("diff --git ")?;
    let inner = header.strip_prefix("a/")?;
    // `X b/X` splits at exactly the midpoint of what remains after ` b/`.
    let path_len = inner.len().checked_sub(3)?.checked_div(2)?;
    let left = inner.get(..path_len)?;
    let right = inner.get(path_len..)?.strip_prefix(" b/")?;
    (left == right).then(|| unquote(right))
}

/// A `---`/`+++` marker path, with git's `a/` or `b/` prefix removed.
/// `/dev/null` is git's way of saying "this side does not exist" and yields
/// `None` so the other side supplies the identity.
fn marker_path(raw: &str) -> Option<String> {
    if raw == "/dev/null" {
        return None;
    }
    let unquoted = unquote(raw);
    Some(
        unquoted
            .strip_prefix("a/")
            .or_else(|| unquoted.strip_prefix("b/"))
            .map_or(unquoted.clone(), ToOwned::to_owned),
    )
}

/// Undo git's C-style quoting, which it applies to paths containing control
/// characters, a double quote, or a backslash. `core.quotePath=false` means
/// non-ASCII no longer triggers it, so the escapes left are few and known.
fn unquote(raw: &str) -> String {
    let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return raw.to_owned();
    };
    let mut out = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            // Anything else escaped is the literal character, including `\\`
            // and `\"`. An unterminated escape at the end drops nothing that
            // was there.
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Accumulates one hunk's lines while tracking both line numbers.
struct HunkBuilder {
    hunk: Hunk,
    old_line: u32,
    new_line: u32,
}

impl HunkBuilder {
    /// `Some` when `line` is a hunk header. A body line can never be mistaken
    /// for one: every body line carries a ` `, `+`, `-`, or `\` prefix, so a
    /// literal `@@` at column zero only ever appears as a header.
    fn start(line: &str) -> Option<Self> {
        let rest = line.strip_prefix("@@ ")?;
        let (ranges, _) = rest.split_once(" @@")?;
        let (old, new) = ranges.split_once(' ')?;
        let (old_start, old_lines) = parse_range(old.strip_prefix('-')?)?;
        let (new_start, new_lines) = parse_range(new.strip_prefix('+')?)?;
        Some(Self {
            hunk: Hunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                header: line.to_owned(),
                lines: Vec::new(),
            },
            old_line: old_start,
            new_line: new_start,
        })
    }

    fn push(&mut self, line: &str) {
        // git's "no newline at end of file" marker describes the previous
        // line's terminator rather than being a line of the file. It carries no
        // line number on either side, so it is not projected as one.
        if line.starts_with('\\') {
            return;
        }
        let (kind, old_line_number, new_line_number) = match line.chars().next() {
            Some('+') => {
                let number = self.new_line;
                self.new_line = self.new_line.saturating_add(1);
                (LineSide::Added, None, Some(number))
            }
            Some('-') => {
                let number = self.old_line;
                self.old_line = self.old_line.saturating_add(1);
                (LineSide::Removed, Some(number), None)
            }
            // A space prefix, or a bare empty line, which some git versions
            // emit for an empty context line. Both advance both sides.
            _ => {
                let old = self.old_line;
                let new = self.new_line;
                self.old_line = self.old_line.saturating_add(1);
                self.new_line = self.new_line.saturating_add(1);
                (LineSide::Unchanged, Some(old), Some(new))
            }
        };
        self.hunk.lines.push(HunkLine {
            kind,
            // The marker character is dropped, not kept: `kind` already carries
            // it as a type, and a renderer that draws its own +/- gutter beside
            // content that still starts with `+` shows the marker twice and
            // indents every line of code by one column.
            content: strip_marker(line),
            old_line_number,
            new_line_number,
        });
    }

    fn finish(self) -> Hunk {
        self.hunk
    }
}

/// Drop the leading ` `, `+`, or `-` a diff body line carries.
///
/// A bare empty line has no marker to drop and must not lose a character it
/// never had.
fn strip_marker(line: &str) -> String {
    match line.chars().next() {
        Some(' ' | '+' | '-') => line.get(1..).unwrap_or_default().to_owned(),
        _ => line.to_owned(),
    }
}

/// `12,7` or the single-line shorthand `12`, which means one line.
fn parse_range(raw: &str) -> Option<(u32, u32)> {
    match raw.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((raw.parse().ok()?, 1)),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFIED: &str = "diff --git a/src/main.rs b/src/main.rs\n\
index 111..222 100644\n\
--- a/src/main.rs\n\
+++ b/src/main.rs\n\
@@ -1,3 +1,4 @@ fn main()\n\
 fn main() {\n\
-    old();\n\
+    new();\n\
+    extra();\n\
 }\n";

    #[test]
    fn a_modified_file_keeps_both_line_numbers_in_step() {
        let files = parse_sections(MODIFIED.as_bytes());
        let file = files.first().expect("one file");
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.status, ChangeStatus::Modified);

        let hunk = file.hunks.first().expect("one hunk");
        assert_eq!((hunk.old_start, hunk.old_lines), (1, 3));
        assert_eq!((hunk.new_start, hunk.new_lines), (1, 4));
        // The property that matters for anchoring a review note: a removed line
        // advances only the old side, an added line only the new side, and the
        // trailing context lands on the right number on both.
        let numbers: Vec<_> = hunk
            .lines
            .iter()
            .map(|line| (line.kind, line.old_line_number, line.new_line_number))
            .collect();
        assert_eq!(
            numbers,
            vec![
                (LineSide::Unchanged, Some(1), Some(1)),
                (LineSide::Removed, Some(2), None),
                (LineSide::Added, None, Some(2)),
                (LineSide::Added, None, Some(3)),
                (LineSide::Unchanged, Some(3), Some(4)),
            ]
        );
    }

    #[test]
    fn the_hunk_header_survives_verbatim_including_its_function_context() {
        let files = parse_sections(MODIFIED.as_bytes());
        let hunk = files
            .first()
            .and_then(|file| file.hunks.first())
            .expect("one hunk");
        assert_eq!(hunk.header, "@@ -1,3 +1,4 @@ fn main()");
    }

    #[test]
    fn a_rename_carries_both_paths() {
        let raw = "diff --git a/old.rs b/new.rs\n\
similarity index 95%\n\
rename from old.rs\n\
rename to new.rs\n\
--- a/old.rs\n\
+++ b/new.rs\n\
@@ -1 +1 @@\n\
-a\n\
+b\n";
        let files = parse_sections(raw.as_bytes());
        let file = files.first().expect("one file");
        assert_eq!(file.status, ChangeStatus::Renamed);
        assert_eq!(file.path, "new.rs");
        assert_eq!(file.old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn a_deleted_file_is_identified_by_its_left_hand_path() {
        let raw = "diff --git a/gone.rs b/gone.rs\n\
deleted file mode 100644\n\
--- a/gone.rs\n\
+++ /dev/null\n\
@@ -1,2 +0,0 @@\n\
-one\n\
-two\n";
        let file = parse_sections(raw.as_bytes()).remove(0);
        assert_eq!(file.status, ChangeStatus::Deleted);
        assert_eq!(file.path, "gone.rs");
    }

    #[test]
    fn a_binary_file_is_flagged_and_carries_no_lines() {
        let raw = "diff --git a/logo.png b/logo.png\n\
index 1..2 100644\n\
Binary files a/logo.png and b/logo.png differ\n";
        let file = parse_sections(raw.as_bytes()).remove(0);
        assert!(file.binary);
        assert!(file.hunks.is_empty());
        // A binary section carries no `---`/`+++` at all, so the header is the
        // only place its name exists. Naming it "(unnamed)" would make every
        // binary change unreviewable.
        assert_eq!(file.path, "logo.png");
    }

    /// The header is ambiguous from both ends for a path containing ` b/`, so
    /// the fallback splits at the arithmetic midpoint and *verifies* the halves
    /// match. Searching for a separator gets this wrong whichever end it
    /// searches from — the first draft did, and named the file `lib.rs`.
    #[test]
    fn the_header_fallback_survives_a_path_containing_the_separator() {
        let raw = "diff --git a/src b/lib.rs b/src b/lib.rs\n\
index 1..2 100644\n\
Binary files a/src b/lib.rs and b/src b/lib.rs differ\n";
        let file = parse_sections(raw.as_bytes()).remove(0);
        assert_eq!(file.path, "src b/lib.rs");
    }

    /// When the halves disagree there is no sound answer, so the fallback
    /// declines rather than returning a plausible wrong path. A renamed binary
    /// file still gets its name from `rename to`.
    #[test]
    fn the_header_fallback_declines_rather_than_guessing_on_a_rename() {
        assert_eq!(header_path("diff --git a/old.png b/new.png\n"), None);
        assert_eq!(
            header_path("diff --git a/same.png b/same.png\n").as_deref(),
            Some("same.png")
        );
    }

    /// The guard the byte-level split exists for: a file whose diff is not
    /// UTF-8 is reported as undecodable, and — critically — the *neighbouring*
    /// file is still parsed normally instead of being dragged into the failure.
    #[test]
    fn an_undecodable_file_is_flagged_without_corrupting_its_neighbour() {
        let mut raw = b"diff --git a/latin.txt b/latin.txt\n--- a/latin.txt\n+++ b/latin.txt\n@@ -1 +1 @@\n-\xff\xfe caf\xe9\n+ok\n".to_vec();
        raw.extend_from_slice(MODIFIED.as_bytes());

        let files = parse_sections(&raw);
        assert_eq!(files.len(), 2);

        let bad = files.first().expect("first");
        assert!(bad.undecodable);
        assert!(
            bad.hunks.is_empty(),
            "no content is projected from bytes mjolnr could not decode"
        );
        assert_eq!(bad.path, "latin.txt");

        let good = files.get(1).expect("second");
        assert!(!good.undecodable);
        assert_eq!(good.path, "src/main.rs");
        assert_eq!(good.hunks.len(), 1);
    }

    #[test]
    fn a_path_with_a_space_is_not_split_at_the_space() {
        let raw = "diff --git a/my docs/a b.md b/my docs/a b.md\n\
--- a/my docs/a b.md\n\
+++ b/my docs/a b.md\n\
@@ -1 +1 @@\n\
-x\n\
+y\n";
        let file = parse_sections(raw.as_bytes()).remove(0);
        assert_eq!(file.path, "my docs/a b.md");
    }

    #[test]
    fn a_quoted_path_is_unquoted() {
        assert_eq!(unquote(r#""a/we\"ird.rs""#), "a/we\"ird.rs");
        assert_eq!(unquote(r#""tab\there""#), "tab\there");
        assert_eq!(unquote("plain.rs"), "plain.rs");
    }

    #[test]
    fn multiple_hunks_in_one_file_are_each_captured() {
        let raw = "diff --git a/a.rs b/a.rs\n\
--- a/a.rs\n\
+++ b/a.rs\n\
@@ -1,1 +1,1 @@\n\
-a\n\
+b\n\
@@ -10,1 +10,2 @@\n\
 c\n\
+d\n";
        let file = parse_sections(raw.as_bytes()).remove(0);
        assert_eq!(file.hunks.len(), 2);
        let second = file.hunks.get(1).expect("second hunk");
        assert_eq!(second.new_start, 10);
        assert_eq!(second.lines.len(), 2);
    }

    /// The marker belongs to `kind`, not to the text. A renderer draws its own
    /// gutter; content that still carried `+` would show the marker twice and
    /// shift every line of code one column right.
    #[test]
    fn line_content_carries_no_diff_marker() {
        let file = parse_sections(MODIFIED.as_bytes()).remove(0);
        let hunk = file.hunks.first().expect("hunk");
        let contents: Vec<_> = hunk
            .lines
            .iter()
            .map(|line| line.content.as_str())
            .collect();
        assert_eq!(
            contents,
            vec![
                "fn main() {",
                "    old();",
                "    new();",
                "    extra();",
                "}"
            ]
        );
    }

    #[test]
    fn an_empty_context_line_survives_as_an_empty_string() {
        assert_eq!(strip_marker(""), "");
        assert_eq!(strip_marker(" "), "");
        assert_eq!(strip_marker("+x"), "x");
        assert_eq!(strip_marker("-x"), "x");
    }

    #[test]
    fn a_no_newline_marker_is_not_projected_as_a_line() {
        let raw = "diff --git a/a.rs b/a.rs\n\
--- a/a.rs\n\
+++ b/a.rs\n\
@@ -1 +1 @@\n\
-a\n\
\\ No newline at end of file\n\
+b\n";
        let file = parse_sections(raw.as_bytes()).remove(0);
        let hunk = file.hunks.first().expect("hunk");
        assert_eq!(hunk.lines.len(), 2);
        let contents: Vec<_> = hunk
            .lines
            .iter()
            .map(|line| line.content.as_str())
            .collect();
        assert_eq!(contents, vec!["a", "b"]);
    }

    #[test]
    fn empty_output_produces_no_files() {
        assert!(parse_sections(b"").is_empty());
    }

    /// A single-line hunk header omits the count. Reading that as zero would
    /// silently drop a one-line change from every range calculation.
    #[test]
    fn a_single_line_range_means_one_line_not_zero() {
        assert_eq!(parse_range("12"), Some((12, 1)));
        assert_eq!(parse_range("12,7"), Some((12, 7)));
        assert_eq!(parse_range("nonsense"), None);
    }

    #[test]
    fn a_malformed_hunk_header_is_not_treated_as_a_hunk() {
        assert!(HunkBuilder::start("@@ garbage @@").is_none());
        assert!(HunkBuilder::start(" @@ -1 +1 @@").is_none());
        assert!(HunkBuilder::start("+@@ -1 +1 @@").is_none());
    }
}
