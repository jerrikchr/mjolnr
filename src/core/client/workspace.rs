//! Integrated-workspace authority contract (Phase D0).
//!
//! These types define the client-safe projections for workspace objects:
//! work items, relations, trust classes, repository state, change sets,
//! review threads, search cursors, and capability declarations.
//!
//! Authority rules (ADR 0006):
//! - `TrustClass` is runtime-owned; the frontend cannot promote it.
//! - Every projected work item carries provenance.
//! - All collections are bounded by explicit limits.
//! - Over-limit conditions produce structured refusals, not silent truncation.
//! - No credentials, environment contents, PTY handles, or unbounded strings.
//!
//! Wire-number contract: every count, revision, and total in this module is
//! `u32`. JavaScript `number` (f64) represents every `u32` exactly, so the
//! bridge can never silently lose precision on the wire; `u64` is deliberately
//! absent from this module. A counter that could plausibly exceed `u32::MAX`
//! (≈4.3×10⁹ revisions or diff lines within one session) does not belong to
//! this product's scale — if that ever changes, introduce a decimal-string
//! newtype rather than widening these fields.
//!
//! Conversion ownership: `BoundedProjection::reason_code` is `Option<String>`
//! on the wire, not `Option<ReasonCode>`, so this DTO layer stays free of
//! `core::error`. The bridge (`runtime/client_bridge/workspace.rs`) owns the
//! `ReasonCode → String` conversion via `ReasonCode::as_str()`; no other
//! module may perform it.

use serde::{Deserialize, Serialize};

pub const MAX_WORK_ITEMS_PER_QUERY: u32 = 200;
pub const MAX_RELATIONSHIPS_PER_ITEM: u32 = 50;
pub const MAX_FILES_IN_CHANGESET: u32 = 500;
pub const MAX_DIFF_HUNKS_PER_FILE: u32 = 100;
pub const MAX_DIFF_BYTES_PER_HUNK: u32 = 8_192;
pub const MAX_REVIEW_THREADS_PER_ITEM: u32 = 100;
pub const MAX_REVIEW_COMMENTS_PER_THREAD: u32 = 200;
pub const MAX_SEARCH_RESULTS_PER_PAGE: u32 = 50;
pub const MAX_SEARCH_CURSOR_DEPTH: u32 = 1_000;
/// Largest acceptable `match_snippet` on a search result. The D4 producer
/// must clamp at the projection boundary; without a ceiling a highlight
/// window could carry an unbounded payload to the client.
pub const MAX_SEARCH_SNIPPET_BYTES: u32 = 512;
pub const MAX_TERMINAL_METADATA_ENTRIES: u32 = 20;
pub const MAX_INTEGRATION_RECORDS_PER_PROVIDER: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum TrustClass {
    MjolnrGoverned,
    OperatorControlled,
    #[serde(other)]
    ExternalUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkItemProvenance {
    pub source: String,
    pub fetched_at: String,
    pub trust: TrustClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkItem {
    pub id: String,
    pub title: String,
    pub title_truncated: bool,
    pub state: String,
    pub provenance: WorkItemProvenance,
    pub revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum WorkRelationKind {
    ParentChild,
    References,
    Blocks,
    Duplicates,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkRelation {
    pub source_id: String,
    pub target_id: String,
    pub kind: WorkRelationKind,
    pub trust: TrustClass,
}

/// Largest number of paths carried in one repository projection, matching the
/// bound the producer applies while parsing porcelain output.
pub const MAX_REPOSITORY_PATHS: u32 = 2_000;

/// Longest refusal detail carried on an unavailable repository.
pub const MAX_REPOSITORY_DETAIL_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct RepositoryState {
    pub branch: Option<String>,
    pub head: Option<String>,
    /// The index revision the projection was read at, for a client to echo into
    /// a `Commit`. Advisory only — the command re-reads and compares, so a
    /// stale value here becomes a refusal, never a wrong commit.
    pub index_revision: Option<String>,
    pub dirty_count: u32,
    pub dirty_count_truncated: bool,
    pub staged_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
    /// Conflicted paths, separate from `modified_files` so a surface never
    /// offers to stage a conflict marker as an ordinary change.
    pub unmerged_files: Vec<String>,
    /// True when `git rebase` has left an explicit recovery state in the
    /// repository. This is not inferred from conflicts.
    pub rebase_in_progress: bool,
    /// True when any path list is bounded rather than complete.
    pub paths_truncated: bool,
    pub remote_sync: RepositorySyncState,
    /// When mjolnr last saw the remote, for qualifying `remote_sync` (ADR 0008).
    ///
    /// `None` when there is no upstream, or when git's reflog could not say.
    /// A surface renders the "as of" qualifier on `remote_sync` **whether or not
    /// this is present** — the qualifier is what makes the counts honest, and
    /// this only sharpens it.
    pub remote_sync_as_of: Option<String>,
    /// When this was read and why — the freshness marker (§D5 producer).
    pub freshness: RepositoryFreshness,
    pub trust: TrustClass,
}

/// One commit in the bounded repository-history query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct RepositoryHistoryEntry {
    pub revision: String,
    pub author: String,
    pub authored_at: String,
    pub subject: String,
}

/// Read-only repository history projected for a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct RepositoryHistory {
    pub entries: Vec<RepositoryHistoryEntry>,
    pub has_more: bool,
    pub limit: u32,
    pub trust: TrustClass,
}

/// Whether the repository was read, and if so at what moment.
///
/// This is the honest half of the D5 producer. mjolnr refreshes on explicit
/// triggers and nothing watches the filesystem, so a client can be told what
/// git said and when it was asked — never that the answer is still true. There
/// is deliberately no `fresh` or `upToDate` variant to render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum RepositoryFreshness {
    /// No project is open, so nothing was read.
    NoProject,
    /// A project is open but git could not answer. Usually "not a git
    /// repository", which is an ordinary state and not a fault.
    #[serde(rename_all = "camelCase")]
    Unavailable { code: String, detail: String },
    /// git answered at this capture. `trigger` is a stable identifier
    /// (`projectOpened`, `repositoryCommand`, `toolWrite`, `requested`);
    /// `sequence` increases on every completed read so a client can tell that a
    /// projection moved.
    #[serde(rename_all = "camelCase")]
    CapturedAt { trigger: String, sequence: u32 },
}

/// Where the branch stands against its remote-tracking ref (ADR 0008).
///
/// Every variant except `Unknown` is a statement about the ref as it stood when
/// mjolnr last saw the remote, **not** about the remote now. Computing these
/// touches no network; learning whether the remote has moved since would, and no
/// read path may.
///
/// # `Synced` is a trap
///
/// It means "identical to the ref last seen", and a surface must never render it
/// as a bare "synced" or in the verified colour. `tauri-design-system.md`
/// forbids a client component claiming a verified state, and being level with a
/// ref fetched an hour ago is not one. Render the qualifier — see
/// `RepositoryState::remote_sync_as_of`. The variant keeps this name for wire
/// compatibility; the *rendering* carries the honesty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum RepositorySyncState {
    /// No upstream is configured, or git would not answer. Genuinely unknown —
    /// this does not mean "not looked at".
    Unknown,
    Ahead {
        count: u32,
    },
    Behind {
        count: u32,
    },
    Diverged {
        ahead: u32,
        behind: u32,
    },
    Synced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ChangeSetSummary {
    pub file_count: u32,
    pub file_count_truncated: bool,
    pub insertions: u32,
    pub deletions: u32,
    pub trust: TrustClass,
    pub revision: u32,
}

/// Longest review-comment body carried on the wire. Matches
/// [`MAX_REVIEW_NOTE_BYTES`](crate::core::client::command::MAX_REVIEW_NOTE_BYTES),
/// which bounds the same text on the way in; enforced again on the way out,
/// because a bound applied only at one end of a durable record stops holding the
/// moment the record outlives the build that wrote it.
pub const MAX_REVIEW_COMMENT_BYTES: usize = 2_048;

/// Where a review thread is pinned, exactly as it was pinned (§D3).
///
/// Every field is the value recorded when the note was taken. Nothing in the
/// projection recomputes one: a stale thread arrives with its original line,
/// side, and hunk header, and `ReviewThreadSummary::anchor_stale` says so
/// beside them. That pairing is §D3's "stale anchors remain visible but cannot
/// silently move to a different line" — visible, and provably not moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ReviewAnchorView {
    pub path: String,
    /// `"old"` or `"new"`.
    pub side: String,
    pub line: u32,
    pub hunk_header: String,
    /// The diff revision the note was taken against.
    pub capture_digest: String,
    pub base_object_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ReviewCommentView {
    pub body: String,
    pub body_truncated: bool,
    pub created_at: String,
}

/// One review thread as a client renders it (§D3 producer).
///
/// The D0 contract shipped the first five fields; the producer added the rest,
/// because a summary that could not say *where* a note was pinned, *whether the
/// diff had moved under it*, or *what mjolnr answered* is not a review thread —
/// it is a count. `MAX_REVIEW_COMMENTS_PER_THREAD` already anticipated the
/// comments travelling here, which is what `comment_count_truncated` is for.
///
/// `status` stays a `String` on the wire, as D0 declared it, and its values come
/// from the closed [`ReviewThreadStatus`](crate::core::review::ReviewThreadStatus)
/// — `"open"` or `"sent"`. There is no `"resolved"`, `"applied"`, or
/// `"verified"`: mjolnr cannot know a note was addressed, so nothing may render
/// as if it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ReviewThreadSummary {
    pub id: String,
    pub status: String,
    pub comment_count: u32,
    pub comment_count_truncated: bool,
    pub trust: TrustClass,
    pub anchor: ReviewAnchorView,
    /// True when the current capture's digest differs from the anchor's — or
    /// when there is no current capture to compare against, which is not the
    /// same as "still current" and must not render as it.
    pub anchor_stale: bool,
    pub comments: Vec<ReviewCommentView>,
    /// The `ClientMessage` id mjolnr answered with, when a sent request produced
    /// one. `None` while a thread is unsent, and also when the run that carried
    /// it ended without an answer.
    pub response_message_id: Option<String>,
}

/// Largest number of directory entries carried in one page (§D7).
///
/// A page, not a directory: `node_modules` has more children than any surface
/// can render, and the producer walks the whole directory to build the pages
/// from. This is what a client receives at once.
pub const MAX_DIRECTORY_ENTRIES_PER_PAGE: u32 = 200;

/// Largest accepted directory or file path on the D7 read commands. Comfortably
/// under every supported platform's `PATH_MAX`, so a refusal happens on the wire
/// rather than inside a syscall.
pub const MAX_WORKSPACE_FILE_PATH_BYTES: usize = 1_024;

/// Largest file text carried to an editor, in bytes.
///
/// The wire's own ceiling, deliberately separate from the producer's
/// `MAX_EDITABLE_FILE_BYTES`: one bounds what may be read into memory at all
/// and the other bounds what may cross to a frontend. They are equal today, and
/// `the_wire_never_promises_more_than_the_producer_will_read` in the bridge is
/// what keeps them from drifting apart silently — the guard D4's search bounds
/// were noted as lacking.
pub const MAX_FILE_TEXT_BYTES: u32 = 1_048_576;

/// Largest preview excerpt carried for a file the editor may not have.
pub const MAX_FILE_PREVIEW_BYTES: u32 = 4_096;

/// What a client may ask mjolnr to page through. `page` is zero-based; a page
/// past the end is refused rather than answered empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct DirectoryRequest {
    /// Project-relative, `/`-separated. The empty string is the project root.
    pub path: String,
    pub page: u32,
}

/// One page of one directory (§D7 producer).
///
/// `entries` is a `BoundedProjection` rather than a bare `Vec` for the reason
/// `review_threads` is: a list that hit its ceiling with no way to say so reads
/// as a complete one. `total` on that projection is the directory's total as
/// far as the producer walked it, and `truncated` says the walk stopped short.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct DirectoryPage {
    pub path: String,
    pub page: u32,
    pub entries: BoundedProjection<DirectoryEntryView>,
    /// Whether another page exists after this one, so a surface can offer
    /// "more" without asking for a page that would be refused.
    pub has_more: bool,
    pub trust: TrustClass,
}

/// One entry as a client renders it.
///
/// The six metadata answers §D7 names by name — symlink, binary, generated,
/// ignored, large-file, permission — are here, and three of them are `Option`
/// or a tagged enum rather than a bare `bool`, because "no" and "mjolnr did not
/// look" are different statements and one `false` cannot carry both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct DirectoryEntryView {
    pub name: String,
    pub path: String,
    /// `"directory"`, `"file"`, or `"other"`. Stable identifiers, like a reason
    /// code: prose may change, these may not.
    pub kind: String,
    /// Present only when the entry is a symbolic link. `target` is `None` for a
    /// link that escapes the workspace or could not be resolved — mjolnr refuses
    /// to open either, and reporting where an escaping link points would be
    /// reporting a path outside the workspace.
    pub symlink: Option<SymlinkView>,
    pub content: FileContentView,
    /// `None` for directories, and for entries whose metadata could not be read
    /// or whose size exceeds what this wire can state. A `u32` of bytes stops
    /// at 4 GiB and this wire carries no `u64` (see the module header), so a
    /// larger file says nothing rather than saying a clamped number that reads
    /// as the truth.
    pub size_bytes: Option<u32>,
    /// True when git reported the path ignored. False also means "there was no
    /// repository to ask" — an unasked question cannot answer "ignored".
    pub ignored: bool,
    /// Permission metadata from the filesystem's read-only bit. A hint, not a
    /// promise: ownership, ACLs, and mount options all outrank it, so mjolnr
    /// still attempts a write and reports what happened.
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct SymlinkView {
    /// Project-relative target, or `None` when the link leaves the workspace.
    pub target: Option<String>,
    /// True when the link resolves outside the workspace or not at all. mjolnr
    /// refuses to open it either way, which is why one flag covers both.
    pub escaping: bool,
}

/// What mjolnr could tell about a file's bytes, or that it could not tell.
///
/// Tagged rather than two booleans: `unreadable` and `notAFile` are neither
/// "text" nor "binary", and a surface that rendered them as either would be
/// making a claim about bytes nobody read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum FileContentView {
    /// The prefix was read and classified.
    #[serde(rename_all = "camelCase")]
    Sniffed { binary: bool, generated: bool },
    /// Over the editable ceiling, so no prefix was read. Not the same as
    /// binary — an oversized file may be perfectly good text.
    Oversized,
    /// The prefix could not be read. Not evidence of anything about content.
    Unreadable,
    /// Not a regular file, so there is nothing to classify.
    NotAFile,
}

/// One file read, and whether an editor may have it (§D7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct FileOpenView {
    pub path: String,
    pub mode: FileModeView,
    /// SHA-256 of the exact bytes on disk when mjolnr read them, computed over
    /// the whole file in both modes. A client echoes it back when it saves, and
    /// the runtime compares rather than trusts.
    pub digest: String,
    /// `None` when the file is larger than this wire can state; see
    /// [`DirectoryEntryView::size_bytes`].
    pub size_bytes: Option<u32>,
    pub writable: bool,
    /// `OperatorControlled`. A file on disk is not a mjolnr-governed
    /// observation: mjolnr read it, it did not produce it, and labelling it
    /// `MjolnrGoverned` would show a human's file in the colour reserved for
    /// things mjolnr verified.
    pub trust: TrustClass,
}

/// Preferences for the human-controlled desktop editor.
///
/// This is deliberately small and diffable. It is not runtime authority, and
/// it never changes the policy gate or the model's write path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ClientEditorPreferences {
    pub autosave: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum FileModeView {
    /// Decoded UTF-8 text within the ceiling, safe for an editor.
    #[serde(rename_all = "camelCase")]
    Editable { text: String, text_truncated: bool },
    /// A bounded excerpt and why the editor may not have the file. `reason` is
    /// a stable identifier: `binary`, `tooLarge`, or `notUtf8`.
    #[serde(rename_all = "camelCase")]
    Preview {
        reason: String,
        excerpt: String,
        excerpt_truncated: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct SearchCursor {
    pub opaque_token: String,
    pub page_size: u32,
    pub total_known: Option<u32>,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceCapability {
    pub key: String,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct BoundedProjection<T> {
    pub items: Vec<T>,
    pub limit: u32,
    pub total: Option<u32>,
    pub truncated: bool,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkspaceRefusal {
    pub code: String,
    pub message: String,
    pub attempted_revision: Option<u32>,
    pub current_revision: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // TrustClass: the security-critical deserialization property
    // -----------------------------------------------------------------------

    #[test]
    fn trust_class_known_variants_round_trip() {
        let governed = serde_json::to_string(&TrustClass::MjolnrGoverned).unwrap();
        assert_eq!(governed, "\"mjolnrGoverned\"");
        let parsed: TrustClass = serde_json::from_str(&governed).unwrap();
        assert_eq!(parsed, TrustClass::MjolnrGoverned);

        let operator = serde_json::to_string(&TrustClass::OperatorControlled).unwrap();
        assert_eq!(operator, "\"operatorControlled\"");
        let parsed: TrustClass = serde_json::from_str(&operator).unwrap();
        assert_eq!(parsed, TrustClass::OperatorControlled);

        let external = serde_json::to_string(&TrustClass::ExternalUnverified).unwrap();
        assert_eq!(external, "\"externalUnverified\"");
        let parsed: TrustClass = serde_json::from_str(&external).unwrap();
        assert_eq!(parsed, TrustClass::ExternalUnverified);
    }

    /// The key authority rule: unknown `TrustClass` variants MUST deserialize
    /// as `ExternalUnverified`, never panic. This prevents the frontend from
    /// inventing a trust promotion.
    #[test]
    fn trust_class_unknown_variant_becomes_external_unverified() {
        let unknown: TrustClass = serde_json::from_str("\"adminOverride\"").unwrap();
        assert_eq!(unknown, TrustClass::ExternalUnverified);

        let garbage: TrustClass = serde_json::from_str("\"\"").unwrap();
        assert_eq!(garbage, TrustClass::ExternalUnverified);

        let numeric: TrustClass = serde_json::from_str("\"42\"").unwrap();
        assert_eq!(numeric, TrustClass::ExternalUnverified);
    }

    /// The inverse guard to the unknown-variant test above: a *known-good*
    /// trust value sent by a legitimate client must parse as that class, not
    /// silently demote to `ExternalUnverified`. Catches a future maintainer
    /// tightening the deserializer and breaking legitimate calls.
    #[test]
    fn provenance_accepts_known_good_trust_value() {
        let json =
            r#"{"source":"mjolnr","fetchedAt":"2026-07-28T12:00:00Z","trust":"mjolnrGoverned"}"#;
        let parsed: WorkItemProvenance = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.trust, TrustClass::MjolnrGoverned);

        let json = r#"{"source":"operator","fetchedAt":"2026-07-28T12:00:00Z","trust":"operatorControlled"}"#;
        let parsed: WorkItemProvenance = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.trust, TrustClass::OperatorControlled);
    }

    // -----------------------------------------------------------------------
    // WorkRelationKind: same catch-all pattern
    // -----------------------------------------------------------------------

    #[test]
    fn work_relation_kind_unknown_variant_becomes_unknown() {
        let unknown: WorkRelationKind = serde_json::from_str("\"relatesTo\"").unwrap();
        assert_eq!(unknown, WorkRelationKind::Unknown);
    }

    #[test]
    fn work_relation_kind_round_trip() {
        for (variant, expected_str) in [
            (WorkRelationKind::ParentChild, "\"parentChild\""),
            (WorkRelationKind::References, "\"references\""),
            (WorkRelationKind::Blocks, "\"blocks\""),
            (WorkRelationKind::Duplicates, "\"duplicates\""),
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_str);
            let parsed: WorkRelationKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }

    // -----------------------------------------------------------------------
    // Struct round-trips
    // -----------------------------------------------------------------------

    #[test]
    fn work_item_serde_round_trip() {
        let item = WorkItem {
            id: "WI-001".into(),
            title: "Fix bug".into(),
            title_truncated: false,
            state: "open".into(),
            provenance: WorkItemProvenance {
                source: "github".into(),
                fetched_at: "2025-01-01T00:00:00Z".into(),
                trust: TrustClass::ExternalUnverified,
            },
            revision: 1,
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: WorkItem = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, item);
    }

    #[test]
    fn repository_state_serde_round_trip() {
        let state = RepositoryState {
            branch: Some("main".into()),
            head: Some("abc123".into()),
            index_revision: Some("def456".into()),
            dirty_count: 3,
            dirty_count_truncated: false,
            staged_files: vec!["src/lib.rs".into()],
            modified_files: vec!["README.md".into()],
            untracked_files: vec!["notes.txt".into()],
            unmerged_files: vec!["src/conflict.rs".into()],
            rebase_in_progress: false,
            paths_truncated: false,
            remote_sync: RepositorySyncState::Diverged {
                ahead: 2,
                behind: 1,
            },
            remote_sync_as_of: Some("2026-07-30T18:34:50+07:00".to_owned()),
            freshness: RepositoryFreshness::CapturedAt {
                trigger: "toolWrite".into(),
                sequence: 7,
            },
            trust: TrustClass::MjolnrGoverned,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: RepositoryState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn every_freshness_variant_round_trips_and_none_of_them_claims_currency() {
        for freshness in [
            RepositoryFreshness::NoProject,
            RepositoryFreshness::Unavailable {
                code: "WORKSPACE_CAPABILITY_UNAVAILABLE".into(),
                detail: "not a git repository".into(),
            },
            RepositoryFreshness::CapturedAt {
                trigger: "projectOpened".into(),
                sequence: 1,
            },
        ] {
            let json = serde_json::to_string(&freshness).unwrap();
            let parsed: RepositoryFreshness = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, freshness);
            // The guard this phase turns on: no variant may serialize a word a
            // client could render as "this is current". A future variant that
            // does has to fail here first.
            for forbidden in ["fresh", "current", "upToDate", "synced"] {
                assert!(
                    !json.contains(forbidden),
                    "freshness must not claim currency, found {forbidden} in {json}"
                );
            }
        }
    }

    #[test]
    fn bounded_projection_round_trip() {
        let proj = BoundedProjection {
            items: vec!["a".to_owned(), "b".to_owned()],
            limit: 10,
            total: Some(2),
            truncated: false,
            reason_code: None,
        };
        let json = serde_json::to_string(&proj).unwrap();
        let parsed: BoundedProjection<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, proj);
    }

    #[test]
    fn workspace_refusal_round_trip() {
        let refusal = WorkspaceRefusal {
            code: "WORKSPACE_STALE_REVISION".into(),
            message: "Client revision 1 is stale; current is 3".into(),
            attempted_revision: Some(1),
            current_revision: Some(3),
        };
        let json = serde_json::to_string(&refusal).unwrap();
        let parsed: WorkspaceRefusal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, refusal);
    }

    // -----------------------------------------------------------------------
    // deny_unknown_fields enforcement
    // -----------------------------------------------------------------------

    #[test]
    fn work_item_rejects_unknown_fields() {
        let json = r#"{"id":"1","title":"T","titleTruncated":false,"state":"open","provenance":{"source":"s","fetchedAt":"t","trust":"externalUnverified"},"revision":1,"extra":"bad"}"#;
        let result = serde_json::from_str::<WorkItem>(json);
        assert!(result.is_err());
    }

    #[test]
    fn workspace_refusal_rejects_unknown_fields() {
        let json = r#"{"code":"X","message":"M","attemptedRevision":null,"currentRevision":null,"extra":true}"#;
        let result = serde_json::from_str::<WorkspaceRefusal>(json);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // RepositorySyncState variants
    // -----------------------------------------------------------------------

    #[test]
    fn repository_sync_state_variants_round_trip() {
        for state in [
            RepositorySyncState::Unknown,
            RepositorySyncState::Ahead { count: 5 },
            RepositorySyncState::Behind { count: 3 },
            RepositorySyncState::Diverged {
                ahead: 2,
                behind: 1,
            },
            RepositorySyncState::Synced,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: RepositorySyncState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    // -----------------------------------------------------------------------
    // Limits are non-zero and reasonable
    // -----------------------------------------------------------------------

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn all_limits_are_positive() {
        assert!(MAX_WORK_ITEMS_PER_QUERY > 0);
        assert!(MAX_RELATIONSHIPS_PER_ITEM > 0);
        assert!(MAX_FILES_IN_CHANGESET > 0);
        assert!(MAX_DIFF_HUNKS_PER_FILE > 0);
        assert!(MAX_DIFF_BYTES_PER_HUNK > 0);
        assert!(MAX_REVIEW_THREADS_PER_ITEM > 0);
        assert!(MAX_REVIEW_COMMENTS_PER_THREAD > 0);
        assert!(MAX_SEARCH_RESULTS_PER_PAGE > 0);
        assert!(MAX_SEARCH_CURSOR_DEPTH > 0);
        assert!(MAX_SEARCH_SNIPPET_BYTES > 0);
        assert!(MAX_TERMINAL_METADATA_ENTRIES > 0);
        assert!(MAX_INTEGRATION_RECORDS_PER_PROVIDER > 0);
    }

    /// D7's limits, in their own test rather than appended to the one above:
    /// the combined function crossed the cognitive-complexity lint, and the
    /// split it asked for is a real one — these bound a filesystem read, and
    /// the others bound projections of durable records.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn all_file_limits_are_positive() {
        assert!(MAX_DIRECTORY_ENTRIES_PER_PAGE > 0);
        assert!(MAX_FILE_TEXT_BYTES > 0);
        assert!(MAX_FILE_PREVIEW_BYTES > 0);
        assert!(MAX_WORKSPACE_FILE_PATH_BYTES > 0);
    }

    // -----------------------------------------------------------------------
    // D7 file projections
    // -----------------------------------------------------------------------

    #[test]
    fn directory_page_serde_round_trip() {
        let page = DirectoryPage {
            path: "src".to_owned(),
            page: 0,
            entries: BoundedProjection {
                items: vec![DirectoryEntryView {
                    name: "lib.rs".to_owned(),
                    path: "src/lib.rs".to_owned(),
                    kind: "file".to_owned(),
                    symlink: None,
                    content: FileContentView::Sniffed {
                        binary: false,
                        generated: false,
                    },
                    size_bytes: Some(42),
                    ignored: false,
                    writable: true,
                }],
                limit: MAX_DIRECTORY_ENTRIES_PER_PAGE,
                total: Some(1),
                truncated: false,
                reason_code: None,
            },
            has_more: false,
            trust: TrustClass::OperatorControlled,
        };
        let json = serde_json::to_string(&page).unwrap();
        let parsed: DirectoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, page);
    }

    /// An escaping symlink carries no target. The assertion is the absence: a
    /// projection that named where an escaping link points would be shipping a
    /// path outside the workspace to a client, which is the thing containment
    /// refuses to open in the first place.
    #[test]
    fn an_escaping_symlink_names_no_target() {
        let view = SymlinkView {
            target: None,
            escaping: true,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(json, r#"{"target":null,"escaping":true}"#);
        let parsed: SymlinkView = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, view);
    }

    /// `unreadable` and `notAFile` must survive the wire as themselves. A
    /// future maintainer collapsing them into `sniffed { binary: false }` makes
    /// this fail, which is the point: that collapse asserts a file is text on
    /// the strength of never having read it.
    #[test]
    fn every_content_class_round_trips_and_none_of_them_guesses() {
        for content in [
            FileContentView::Sniffed {
                binary: false,
                generated: false,
            },
            FileContentView::Sniffed {
                binary: true,
                generated: false,
            },
            FileContentView::Oversized,
            FileContentView::Unreadable,
            FileContentView::NotAFile,
        ] {
            let json = serde_json::to_string(&content).unwrap();
            let parsed: FileContentView = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, content);
        }

        let unreadable = serde_json::to_string(&FileContentView::Unreadable).unwrap();
        assert!(!unreadable.contains("binary"));
        assert!(!unreadable.contains("generated"));
    }

    #[test]
    fn file_open_view_round_trips_in_both_modes() {
        for mode in [
            FileModeView::Editable {
                text: "fn main() {}".to_owned(),
                text_truncated: false,
            },
            FileModeView::Preview {
                reason: "binary".to_owned(),
                excerpt: "\u{fffd}PNG".to_owned(),
                excerpt_truncated: true,
            },
        ] {
            let view = FileOpenView {
                path: "src/main.rs".to_owned(),
                mode,
                digest: "abc".to_owned(),
                size_bytes: Some(12),
                writable: true,
                trust: TrustClass::OperatorControlled,
            };
            let json = serde_json::to_string(&view).unwrap();
            let parsed: FileOpenView = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, view);
        }
    }

    #[test]
    fn directory_request_rejects_unknown_fields() {
        let json = r#"{"path":"src","page":0,"recursive":true}"#;
        assert!(serde_json::from_str::<DirectoryRequest>(json).is_err());
    }
}
