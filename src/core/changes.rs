//! Exact-change contract (Phase D3): change sets, changed files, diff hunks,
//! diff lines, and read-before-edit evidence.
//!
//! As of D3 these types are **contract only**: `snapshot_to_client` sets
//! `ClientSnapshot::changes` to `None`, no runtime code produces a
//! `ChangeSet`, and the desktop's `ChangesSurface` renders its explicit empty
//! state. The producer arrives with the D5 repository projection
//! (`docs/integrated-workspace-phases.md` §D3→§D5). Until then, do not
//! "preview" the shape by fabricating one — an invented diff is worse than an
//! honest empty state (AGENTS.md §3).
//!
//! Bounded representations: the producer (D5) must enforce
//! [`crate::core::client::workspace::MAX_DIFF_HUNKS_PER_FILE`] and
//! [`crate::core::client::workspace::MAX_DIFF_BYTES_PER_HUNK`] at the
//! projection boundary and set `ChangedFile::is_truncated` when it clamps;
//! nothing here enforces them because no data flows yet. Binary, renamed,
//! deleted, large, non-UTF-8, and truncated files each carry an explicit flag
//! — never collapse them into one status string.
//!
//! Never apply a rendered diff as a patch. The contract exists for review,
//! not for round-tripping edits through the frontend.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ChangeSet {
    pub base_object_id: Option<String>,
    pub current_object_id: Option<String>,
    pub files: Vec<ChangedFile>,
    pub state: ChangeState,
    pub read_evidence: Vec<ReadBeforeEditEvidence>,
    /// SHA-256 over the exact diff bytes this set was built from.
    ///
    /// The review anchor's identity. `base_object_id` moves only when HEAD
    /// does, but a working tree changes underneath a review without HEAD
    /// moving at all, so a commit id alone cannot detect the staleness that
    /// actually matters. This is content identity, never a git object id, and
    /// must never be handed back to git.
    pub capture_digest: String,
    /// Matches `RepositoryState.freshness.sequence` for the same refresh, so a
    /// client can prove a change set and a repository status came from one
    /// capture instead of assuming it.
    pub capture_sequence: u32,
    /// True when files were dropped at the projection bound, or when git's own
    /// output was cut. Added by the D3 producer: the original contract had no
    /// place to say "this set is part of the working tree", and a bounded list
    /// with no way to admit it was bounded reads as a complete one.
    pub files_truncated: bool,
    /// Untracked paths that exist but were not diffed — over the producer's
    /// per-refresh process bound, or a wholly untracked directory. Named rather
    /// than dropped, so the surface can say what it is not showing.
    pub undiffed_untracked: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ChangeState {
    Proposed,
    Applied,
    ExternallyImported,
    CurrentWorkingTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    /// Why this file does or does not carry reviewable text.
    ///
    /// An enum rather than the pair of booleans this started as: binary and
    /// undecodable are mutually exclusive, and two flags can represent a file
    /// that is somehow both — an illegal state the type now cannot express
    /// (AGENTS.md §2.4). The distinction still matters and is still explicit;
    /// it is the *combination* that was never meaningful.
    pub content: FileContent,
    pub is_large: bool,
    pub is_truncated: bool,
    pub old_path: Option<String>,
}

/// Whether a changed file's content is reviewable, and if not, why not.
///
/// Each variant sends a reader to a different remedy, which is why this is not
/// collapsed into "no diff available".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum FileContent {
    /// A text diff. Possibly zero hunks — an empty added file really has no
    /// lines, which is a fact rather than a failure.
    Text,
    /// git recognized the content as binary and declined to diff it.
    Binary,
    /// git produced bytes that are not valid UTF-8. A text file in an encoding
    /// mjolnr will not guess at, which is a different problem from binary and
    /// has a different answer.
    Undecodable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub old_line_number: Option<u32>,
    pub new_line_number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum LineKind {
    Unchanged,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ReadBeforeEditEvidence {
    pub path: String,
    pub read_revision: String,
    pub tool_event_id: String,
}

#[cfg(test)]
mod tests {
    // serde_json's `Value` indexing returns `Value::Null` rather than
    // panicking, and the assertions below are the point of the test.
    #![allow(clippy::indexing_slicing)]

    use super::*;

    fn sample_change_set() -> ChangeSet {
        ChangeSet {
            base_object_id: Some("abc123".to_owned()),
            current_object_id: Some("def456".to_owned()),
            files: vec![ChangedFile {
                path: "src/main.rs".to_owned(),
                status: FileStatus::Renamed,
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_lines: 5,
                    new_start: 1,
                    new_lines: 6,
                    header: "@@ -1,5 +1,6 @@".to_owned(),
                    // Content carries no ` `/`+`/`-` marker: `kind` is the
                    // marker, as a type. A renderer draws its own gutter, and
                    // content that repeated it would show it twice and indent
                    // every line of code by one column.
                    lines: vec![
                        DiffLine {
                            kind: LineKind::Unchanged,
                            content: "line 1".to_owned(),
                            old_line_number: Some(1),
                            new_line_number: Some(1),
                        },
                        DiffLine {
                            kind: LineKind::Added,
                            content: "line 2".to_owned(),
                            old_line_number: None,
                            new_line_number: Some(2),
                        },
                        DiffLine {
                            kind: LineKind::Removed,
                            content: "old line 2".to_owned(),
                            old_line_number: Some(2),
                            new_line_number: None,
                        },
                    ],
                }],
                content: FileContent::Text,
                is_large: false,
                is_truncated: false,
                old_path: Some("src/old_main.rs".to_owned()),
            }],
            state: ChangeState::Proposed,
            read_evidence: vec![ReadBeforeEditEvidence {
                path: "src/main.rs".to_owned(),
                read_revision: "abc123".to_owned(),
                tool_event_id: "evt-1".to_owned(),
            }],
            capture_digest: "9f8e7d".to_owned(),
            capture_sequence: 3,
            files_truncated: false,
            undiffed_untracked: Vec::new(),
        }
    }

    #[test]
    fn change_set_serde_round_trip() {
        let change_set = sample_change_set();
        let json = serde_json::to_string(&change_set).unwrap();
        let parsed: ChangeSet = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, change_set);
    }

    /// Rename provenance must survive the wire: the renderer shows where a
    /// file came from, and a serializer change must not silently drop it.
    #[test]
    fn renamed_file_keeps_its_old_path_on_the_wire() {
        let json = serde_json::to_value(sample_change_set()).unwrap();
        assert_eq!(
            json["files"][0]["oldPath"],
            serde_json::Value::String("src/old_main.rs".to_owned())
        );
    }

    #[test]
    fn change_state_wire_forms_round_trip() {
        for (variant, wire) in [
            (ChangeState::Proposed, "\"proposed\""),
            (ChangeState::Applied, "\"applied\""),
            (ChangeState::ExternallyImported, "\"externallyImported\""),
            (ChangeState::CurrentWorkingTree, "\"currentWorkingTree\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, wire);
            let parsed: ChangeState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    #[test]
    fn file_status_and_line_kind_wire_forms_round_trip() {
        for (variant, wire) in [
            (FileStatus::Added, "\"added\""),
            (FileStatus::Modified, "\"modified\""),
            (FileStatus::Deleted, "\"deleted\""),
            (FileStatus::Renamed, "\"renamed\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, wire);
            assert_eq!(serde_json::from_str::<FileStatus>(&json).unwrap(), variant);
        }
        for (variant, wire) in [
            (LineKind::Unchanged, "\"unchanged\""),
            (LineKind::Added, "\"added\""),
            (LineKind::Removed, "\"removed\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, wire);
            assert_eq!(serde_json::from_str::<LineKind>(&json).unwrap(), variant);
        }
    }

    /// The closed enums refuse unknown variants: a frontend cannot invent a
    /// state (e.g. "verified") and have it accepted — false promotion fails
    /// at the wire, at compile time in Rust, and at deserialization in JSON.
    #[test]
    fn closed_enums_refuse_invented_variants() {
        assert!(serde_json::from_str::<ChangeState>("\"verified\"").is_err());
        assert!(serde_json::from_str::<FileStatus>("\"copied\"").is_err());
        assert!(serde_json::from_str::<LineKind>("\"context\"").is_err());
        assert!(serde_json::from_str::<FileContent>("\"available\"").is_err());
    }

    /// The state the pair of booleans could express and this cannot: a file
    /// that is binary *and* undecodable. There is no such file, and now no
    /// way to say there is (AGENTS.md §2.4).
    #[test]
    fn file_content_wire_forms_round_trip_and_are_mutually_exclusive() {
        for (variant, wire) in [
            (FileContent::Text, "\"text\""),
            (FileContent::Binary, "\"binary\""),
            (FileContent::Undecodable, "\"undecodable\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, wire);
            assert_eq!(serde_json::from_str::<FileContent>(&json).unwrap(), variant);
        }
    }

    #[test]
    fn structs_reject_unknown_fields() {
        let complete = r#""baseObjectId":null,"currentObjectId":null,"files":[],"state":"proposed","readEvidence":[],"captureDigest":"d","captureSequence":1,"filesTruncated":false,"undiffedUntracked":[]"#;
        // The control: the complete object parses. Without it this test would
        // still pass if the extra field were ignored and a *required* one were
        // missing, which proves nothing about unknown-field rejection.
        assert!(serde_json::from_str::<ChangeSet>(&format!("{{{complete}}}")).is_ok());
        assert!(serde_json::from_str::<ChangeSet>(&format!("{{{complete},\"x\":1}}")).is_err());

        let file = r#""path":"a","status":"modified","hunks":[],"content":"text","isLarge":false,"isTruncated":false,"oldPath":null"#;
        assert!(serde_json::from_str::<ChangedFile>(&format!("{{{file}}}")).is_ok());
        assert!(serde_json::from_str::<ChangedFile>(&format!("{{{file},\"x\":1}}")).is_err());

        let hunk_with_extra = r#"{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"header":"h","lines":[],"x":1}"#;
        assert!(serde_json::from_str::<DiffHunk>(hunk_with_extra).is_err());

        let line_with_extra =
            r#"{"kind":"added","content":"+x","oldLineNumber":null,"newLineNumber":1,"x":1}"#;
        assert!(serde_json::from_str::<DiffLine>(line_with_extra).is_err());

        let evidence_with_extra = r#"{"path":"a","readRevision":"r","toolEventId":"e","x":1}"#;
        assert!(serde_json::from_str::<ReadBeforeEditEvidence>(evidence_with_extra).is_err());
    }
}
