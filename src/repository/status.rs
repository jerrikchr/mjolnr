//! Parsing `git status --porcelain` into bounded local projections (Phase D5).
//!
//! The porcelain v1 format is a documented stable contract, which is why no
//! git library dependency was needed. Parsing lives here, away from process
//! invocation, so it can be tested against fixture text with no repository.

/// Largest number of paths kept in one projection. Beyond this the projection
/// reports truncation rather than growing without bound (AGENTS.md §5).
pub(super) const MAX_PROJECTED_PATHS: usize = 2_000;

/// What `git` reports about the repository right now.
///
/// Deliberately *not* the `core::client::workspace::RepositoryState` DTO. That
/// type is the wire contract and carries a `TrustClass`, which is runtime-owned
/// (ADR 0006): a module that runs git operations must not also decide how its
/// output is trusted. The bridge projects this into the DTO and applies the
/// trust label.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RepositoryStatus {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty_count: u32,
    /// True when `dirty_count` is a saturated or bounded figure rather than a
    /// complete one.
    pub dirty_count_truncated: bool,
}

/// What the index would commit.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexProjection {
    pub staged_files: Vec<String>,
    pub truncated: bool,
}

/// What the working tree holds beyond the index.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorktreeProjection {
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
    pub unmerged_files: Vec<String>,
    pub truncated: bool,
}

/// Split porcelain v1 output into index and worktree projections.
///
/// Unmerged paths are reported separately rather than folded into "modified":
/// a conflicted file is not a change awaiting staging, and calling it one is
/// how a UI ends up offering to stage a conflict marker.
pub(super) fn parse_porcelain(output: &str) -> (IndexProjection, WorktreeProjection) {
    let mut staged = Vec::new();
    let mut modified = Vec::new();
    let mut untracked = Vec::new();
    let mut unmerged = Vec::new();
    let mut truncated = false;

    for line in output.lines() {
        let mut characters = line.chars();
        let (Some(index), Some(worktree)) = (characters.next(), characters.next()) else {
            continue;
        };
        let path = line.get(3..).map(str::trim).unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        // Rename and copy entries are `XY old -> new`; the destination is the
        // path a caller can act on.
        let path = path.rsplit(" -> ").next().unwrap_or(path).to_owned();

        if is_unmerged(index, worktree) {
            truncated |= push_bounded(&mut unmerged, path);
            continue;
        }
        if index == '?' && worktree == '?' {
            truncated |= push_bounded(&mut untracked, path);
            continue;
        }
        if index != ' ' {
            truncated |= push_bounded(&mut staged, path.clone());
        }
        if worktree != ' ' {
            truncated |= push_bounded(&mut modified, path);
        }
    }

    (
        IndexProjection {
            staged_files: staged,
            truncated,
        },
        WorktreeProjection {
            modified_files: modified,
            untracked_files: untracked,
            unmerged_files: unmerged,
            truncated,
        },
    )
}

/// The porcelain v1 unmerged states, per `git status`'s documented table.
fn is_unmerged(index: char, worktree: char) -> bool {
    matches!(
        (index, worktree),
        ('D', 'D' | 'U') | ('A', 'U' | 'A') | ('U', 'D' | 'A' | 'U')
    )
}

/// Returns true when the value was dropped because the projection is full.
fn push_bounded(target: &mut Vec<String>, value: String) -> bool {
    if target.len() >= MAX_PROJECTED_PATHS {
        return true;
    }
    target.push(value);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_and_further_modified_file_appears_in_both_projections() {
        let (index, worktree) = parse_porcelain("MM src/main.rs\n");
        assert_eq!(index.staged_files, vec!["src/main.rs"]);
        assert_eq!(worktree.modified_files, vec!["src/main.rs"]);
    }

    #[test]
    fn untracked_files_are_not_reported_as_staged() {
        let (index, worktree) = parse_porcelain("?? notes.md\n");
        assert!(index.staged_files.is_empty());
        assert_eq!(worktree.untracked_files, vec!["notes.md"]);
        assert!(worktree.modified_files.is_empty());
    }

    #[test]
    fn a_conflict_is_unmerged_and_never_offered_as_a_stageable_change() {
        let (index, worktree) = parse_porcelain("UU src/lib.rs\nAA other.rs\n");
        assert_eq!(worktree.unmerged_files, vec!["src/lib.rs", "other.rs"]);
        assert!(index.staged_files.is_empty());
        assert!(worktree.modified_files.is_empty());
    }

    #[test]
    fn a_rename_reports_the_destination_path() {
        let (index, _) = parse_porcelain("R  old/name.rs -> new/name.rs\n");
        assert_eq!(index.staged_files, vec!["new/name.rs"]);
    }

    #[test]
    fn a_deleted_but_unstaged_file_is_a_worktree_change_only() {
        let (index, worktree) = parse_porcelain(" D gone.rs\n");
        assert!(index.staged_files.is_empty());
        assert_eq!(worktree.modified_files, vec!["gone.rs"]);
    }

    #[test]
    fn malformed_and_empty_lines_are_skipped_rather_than_panicking() {
        let (index, worktree) = parse_porcelain("\nM\n   \nM  ok.rs\n");
        assert_eq!(index.staged_files, vec!["ok.rs"]);
        assert!(worktree.unmerged_files.is_empty());
    }

    #[test]
    fn an_over_limit_status_reports_truncation_instead_of_growing() {
        use std::fmt::Write as _;
        let mut output = String::new();
        for number in 0..(MAX_PROJECTED_PATHS + 5) {
            writeln!(output, "M  file{number}.rs").expect("write to a String");
        }
        let (index, _) = parse_porcelain(&output);
        assert_eq!(index.staged_files.len(), MAX_PROJECTED_PATHS);
        assert!(index.truncated);
    }
}
