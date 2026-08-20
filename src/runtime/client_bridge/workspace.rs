//! Bounded workspace projections (Phase D0).
//!
//! These functions define the projection boundary between runtime workspace
//! state and client-safe DTOs. In D0 they establish the contract and limits;
//! actual data sources arrive in D1+.
//!
//! Authority: the runtime owns all workspace truth. These projections
//! produce bounded, trust-labeled DTOs safe for any frontend.

use crate::core::change_capture::{
    ChangeStatus, ChangeView, FileChange, Hunk, LineSide, ReadRecord,
};
use crate::core::changes::{
    ChangeSet, ChangeState, ChangedFile, DiffHunk, DiffLine, FileContent, FileStatus, LineKind,
    ReadBeforeEditEvidence,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::core::client::types::{
    ClientWorkspaceSearchFilter, ClientWorkspaceSearchPage, ClientWorkspaceSearchResult,
};
use crate::core::client::workspace::{
    BoundedProjection, DirectoryEntryView, DirectoryPage, FileContentView, FileModeView,
    FileOpenView, MAX_DIFF_BYTES_PER_HUNK, MAX_DIFF_HUNKS_PER_FILE, MAX_DIRECTORY_ENTRIES_PER_PAGE,
    MAX_FILE_PREVIEW_BYTES, MAX_FILE_TEXT_BYTES, MAX_FILES_IN_CHANGESET,
    MAX_RELATIONSHIPS_PER_ITEM, MAX_REPOSITORY_DETAIL_BYTES, MAX_REVIEW_COMMENT_BYTES,
    MAX_REVIEW_COMMENTS_PER_THREAD, MAX_REVIEW_THREADS_PER_ITEM, MAX_SEARCH_RESULTS_PER_PAGE,
    MAX_SEARCH_SNIPPET_BYTES, MAX_WORK_ITEMS_PER_QUERY, RepositoryFreshness, RepositoryState,
    RepositorySyncState, ReviewAnchorView, ReviewCommentView, ReviewThreadSummary, SymlinkView,
    TrustClass, WorkItem, WorkRelation, WorkspaceCapability, WorkspaceRefusal,
};
use crate::core::error::ReasonCode;
use crate::core::event::SessionId;
use crate::core::repository::{RepositoryView, UpstreamPosition};
use crate::core::review::{ReviewComment, ReviewThread};
use crate::core::store::{ProjectId, SessionStatus, WorkspaceSearchFilter, WorkspaceSearchPage};
use crate::core::workspace_files::{
    ContentFacts, DirectoryEntry, DirectoryListing, EntryKind, FileMode, FileRead, SymlinkTarget,
};

/// Build a bounded projection of work items, enforcing the query limit.
///
/// Items beyond `MAX_WORK_ITEMS_PER_QUERY` are truncated with metadata.
/// The reason code is set when truncation occurs.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by D1 work-hierarchy producer"
)]
pub(crate) fn project_work_items(
    items: Vec<WorkItem>,
    total: Option<u32>,
) -> BoundedProjection<WorkItem> {
    let limit = MAX_WORK_ITEMS_PER_QUERY;
    let count = items.len();
    let truncated = count > limit as usize;
    let bounded: Vec<WorkItem> = if truncated {
        items.into_iter().take(limit as usize).collect()
    } else {
        items
    };
    BoundedProjection {
        items: bounded,
        limit,
        total,
        truncated,
        reason_code: if truncated {
            Some(ReasonCode::OutputTruncated.as_str().to_owned())
        } else {
            None
        },
    }
}

/// Build a bounded projection of work relations.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by D1 work-hierarchy producer"
)]
pub(crate) fn project_work_relations(
    relations: Vec<WorkRelation>,
    total: Option<u32>,
) -> BoundedProjection<WorkRelation> {
    let limit = MAX_RELATIONSHIPS_PER_ITEM;
    let count = relations.len();
    let truncated = count > limit as usize;
    let bounded = if truncated {
        relations.into_iter().take(limit as usize).collect()
    } else {
        relations
    };
    BoundedProjection {
        items: bounded,
        limit,
        total,
        truncated,
        reason_code: if truncated {
            Some(ReasonCode::OutputTruncated.as_str().to_owned())
        } else {
            None
        },
    }
}

/// Build a bounded projection of review thread summaries.
pub(crate) fn project_review_threads(
    threads: Vec<ReviewThreadSummary>,
    total: Option<u32>,
) -> BoundedProjection<ReviewThreadSummary> {
    let limit = MAX_REVIEW_THREADS_PER_ITEM;
    let count = threads.len();
    let truncated = count > limit as usize;
    let bounded = if truncated {
        threads.into_iter().take(limit as usize).collect()
    } else {
        threads
    };
    BoundedProjection {
        items: bounded,
        limit,
        total,
        truncated,
        reason_code: if truncated {
            Some(ReasonCode::OutputTruncated.as_str().to_owned())
        } else {
            None
        },
    }
}

/// Project this session's review threads, marking each against the current
/// capture (Phase D3 producer).
///
/// The staleness decision lives here and only here, and it is a comparison, not
/// a relocation: a thread whose `capture_digest` differs from the capture mjolnr
/// currently holds arrives with `anchor_stale: true` **and its original line,
/// side, and hunk header untouched**. Nothing in this function can rewrite an
/// anchor, which is how §D3's "cannot silently move to a different line" is
/// held at the boundary a client actually reads.
///
/// No capture at all — no project, or a repository git could not read — is also
/// stale. It is not evidence that the note is still current, and rendering it as
/// current would be the exact claim `RepositoryFreshness` exists to refuse.
///
/// The trust class is `OperatorControlled`: a review note is a human's remark
/// about code. It is not a mjolnr-governed observation, and labelling it one
/// would let a surface show a person's opinion in the colour reserved for
/// things mjolnr verified.
pub(crate) fn project_review_thread_summaries(
    threads: &[ReviewThread],
    changes: &ChangeView,
) -> BoundedProjection<ReviewThreadSummary> {
    let current = changes.capture().map(|capture| capture.digest.as_str());
    let total = u32::try_from(threads.len()).unwrap_or(u32::MAX);
    let summaries = threads
        .iter()
        .map(|thread| project_review_thread(thread, current))
        .collect();
    project_review_threads(summaries, Some(total))
}

fn project_review_thread(thread: &ReviewThread, current: Option<&str>) -> ReviewThreadSummary {
    let comment_limit = MAX_REVIEW_COMMENTS_PER_THREAD as usize;
    let comment_count = u32::try_from(thread.comments.len()).unwrap_or(u32::MAX);
    let comments = thread
        .comments
        .iter()
        .take(comment_limit)
        .map(project_review_comment)
        .collect();

    ReviewThreadSummary {
        id: thread.id.to_string(),
        status: thread.status.as_str().to_owned(),
        comment_count,
        comment_count_truncated: thread.comments.len() > comment_limit,
        trust: TrustClass::OperatorControlled,
        anchor: ReviewAnchorView {
            path: thread.anchor.path.clone(),
            side: thread.anchor.side.as_str().to_owned(),
            line: thread.anchor.line,
            hunk_header: thread.anchor.hunk_header.clone(),
            capture_digest: thread.anchor.capture_digest.clone(),
            base_object_id: thread.anchor.base_object_id.clone(),
        },
        // `None` — nothing captured — is stale, not current. See the fn docs.
        anchor_stale: current.is_none_or(|digest| thread.is_stale_against(digest)),
        comments,
        response_message_id: thread.response_message_id.clone(),
    }
}

fn project_review_comment(comment: &ReviewComment) -> ReviewCommentView {
    let (body, body_truncated) = clamp_comment(&comment.body);
    ReviewCommentView {
        body,
        body_truncated,
        created_at: comment
            .created_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| comment.created_at.to_string()),
    }
}

/// Bound a comment on a char boundary, saying when it was cut.
///
/// The bridge bounds it again even though `MAX_REVIEW_NOTE_BYTES` already
/// refused an over-long body on the way in. A durable note outlives the build
/// that wrote it, and a bound enforced only at the entrance stops protecting
/// the exit the moment those two disagree.
fn clamp_comment(body: &str) -> (String, bool) {
    if body.len() <= MAX_REVIEW_COMMENT_BYTES {
        return (body.to_owned(), false);
    }
    let mut end = MAX_REVIEW_COMMENT_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (body.get(..end).unwrap_or_default().to_owned(), true)
}

/// Refuse a workspace operation when the client revision is stale.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by D1 work-hierarchy producer"
)]
pub(crate) fn refuse_stale_revision(attempted: u32, current: u32) -> WorkspaceRefusal {
    WorkspaceRefusal {
        code: ReasonCode::WorkspaceStaleRevision.as_str().to_owned(),
        message: format!("Client revision {attempted} is stale; current is {current}"),
        attempted_revision: Some(attempted),
        current_revision: Some(current),
    }
}

/// Refuse a workspace operation when a capability is unavailable.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by the first capability-gated surface"
)]
pub(crate) fn refuse_capability_unavailable(capability: &str, reason: &str) -> WorkspaceRefusal {
    WorkspaceRefusal {
        code: ReasonCode::WorkspaceCapabilityUnavailable
            .as_str()
            .to_owned(),
        message: format!("Capability '{capability}' is unavailable: {reason}"),
        attempted_revision: None,
        current_revision: None,
    }
}

/// Refuse a workspace operation referencing external unverified data
/// where verified data is required.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by D9 external-agent producer"
)]
pub(crate) fn refuse_external_unverified(detail: &str) -> WorkspaceRefusal {
    WorkspaceRefusal {
        code: ReasonCode::WorkspaceExternalUnverified.as_str().to_owned(),
        message: format!("Verified data required but source is external-unverified: {detail}"),
        attempted_revision: None,
        current_revision: None,
    }
}

/// Refuse a workspace operation when a diff is stale.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by D3 diff producer"
)]
pub(crate) fn refuse_stale_diff(attempted: u32, current: u32) -> WorkspaceRefusal {
    WorkspaceRefusal {
        code: ReasonCode::WorkspaceStaleDiff.as_str().to_owned(),
        message: format!("Diff revision {attempted} is stale; current tree is at {current}"),
        attempted_revision: Some(attempted),
        current_revision: Some(current),
    }
}

/// Refuse a workspace operation when integration authentication fails.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by D6 integration producer"
)]
pub(crate) fn refuse_auth(detail: &str) -> WorkspaceRefusal {
    WorkspaceRefusal {
        code: ReasonCode::WorkspaceAuthRefused.as_str().to_owned(),
        message: format!("Integration authentication refused: {detail}"),
        attempted_revision: None,
        current_revision: None,
    }
}

/// Build workspace capability status, honestly reporting availability.
///
/// Capabilities that are not yet implemented report as unavailable with
/// an honest reason, never as available.
///
/// Every phase reference here names the phase in
/// `docs/integrated-workspace-phases.md` that owns the capability. The first
/// version of this list was written against an earlier numbering and pointed
/// each capability at the wrong phase — search at D5, diffs at D4, terminal at
/// D3 — which is a claim about the roadmap, and a wrong one, on a string
/// destined for a user-facing empty state.
#[allow(
    dead_code,
    reason = "D0 contract landed ahead of its producer; consumed by the D12 capability-aware empty states"
)]
pub(crate) fn build_workspace_capabilities() -> Vec<WorkspaceCapability> {
    vec![
        WorkspaceCapability {
            key: "work_items".to_owned(),
            available: false,
            reason: Some("Work-item projection not yet implemented (owner: D1)".to_owned()),
        },
        WorkspaceCapability {
            key: "diff_review".to_owned(),
            available: true,
            // Available, with its limits stated rather than implied — the same
            // correction `scm_status` needed once its producer landed. The
            // diffs are real; what is still missing is named here rather than
            // left for a user to discover by its absence.
            reason: Some(
                "Working-tree diffs are captured on the same explicit triggers as repository \
                 status and carry the moment they were captured; every change set is \
                 currentWorkingTree, because a working-tree read cannot tell a governed tool's \
                 write from a human's. Read-before-edit evidence cites the durable tool event \
                 that recorded each read, for the files this set shows"
                    .to_owned(),
            ),
        },
        WorkspaceCapability {
            key: "review_threads".to_owned(),
            available: true,
            reason: Some(
                "Line notes anchored to file, side, line, hunk context, and the diff revision \
                 they were taken against. A note against a diff that has since moved is refused \
                 rather than relocated, and an existing note whose diff moved stays visible on \
                 the line it was taken against, marked stale. Sending notes to mjolnr is an \
                 ordinary human directive that widens nothing; a thread links to the message \
                 mjolnr answered with. A thread is never marked resolved, applied, or verified — \
                 mjolnr cannot know a note was addressed"
                    .to_owned(),
            ),
        },
        WorkspaceCapability {
            key: "search".to_owned(),
            available: true,
            // Available, with its limits stated rather than implied — the same
            // shape `scm_status` and `diff_review` took when their producers
            // landed. "Available" is a claim about a producer existing, not
            // about the phase being finished.
            reason: Some(
                "Deterministic search over indexed session events: message text, tool proposals \
                 and results, and refusals. Ordering is newest-first and never by relevance, so \
                 a rebuild reproduces it. A query shorter than three characters, a cursor from \
                 another filter, and a page past the enumeration bound are refused rather than \
                 answered with an empty page. Work items, review notes, and available actions \
                 are not indexed: work items have no producer yet (D1), while review notes now \
                 exist and indexing them is D4 surface breadth"
                    .to_owned(),
            ),
        },
        WorkspaceCapability {
            key: "scm_status".to_owned(),
            available: true,
            // Available, with its limits stated rather than implied. The
            // earlier text ("no live status reaches the client snapshot")
            // described the gap the D5 producer closed, and leaving it would
            // have a capability declaration lying in the other direction.
            reason: Some(
                "Repository status is read on explicit triggers and carries the moment it was \
                 captured; nothing watches the filesystem. Remote sync is computed from the \
                 last-seen tracking ref without any network call, so it is exact about that ref \
                 and says nothing about the remote now. Hunk-level staging is refused rather \
                 than implemented"
                    .to_owned(),
            ),
        },
        WorkspaceCapability {
            key: "file_explorer".to_owned(),
            available: true,
            // Available, with what is missing named rather than left for a user
            // to discover by its absence — the shape `scm_status`,
            // `diff_review`, and `search` each took when their producers
            // landed. "Available" is a claim about a producer existing, not
            // about §D7 being finished; the editor surface is now wired to
            // the producer, while terminal/process support remains D8.
            reason: Some(
                "Contained, paginated directory listings and file reads. Containment is \
                 rechecked immediately before every read, and a symlink leaving the workspace is \
                 refused rather than followed — a listing describes it as escaping and reports \
                 nothing about its target. Binary, over-limit, and undecodable files open in a \
                 bounded preview rather than the editor. A file is marked generated only when it \
                 declares itself so; a directory name is never evidence. Ignored comes from git \
                 when there is a repository to ask, and is false — meaning unasked — when there \
                 is not. Human editor saves are operator-controlled, compare the digest returned \
                 by the preceding read, recheck containment immediately before writing, and emit \
                 durable FileSaved truth; model-proposed writes still use the ordinary tool gate"
                    .to_owned(),
            ),
        },
        WorkspaceCapability {
            key: "task_integration".to_owned(),
            available: false,
            reason: Some(
                "GitHub and Linear task sources: the contract landed in D6 but no integration \
                 performs network requests"
                    .to_owned(),
            ),
        },
        WorkspaceCapability {
            key: "terminal".to_owned(),
            available: false,
            reason: Some("Terminal orchestration not yet implemented (owner: D8)".to_owned()),
        },
        WorkspaceCapability {
            key: "external_agent".to_owned(),
            available: false,
            reason: Some(
                "External-agent attention rail not yet implemented (owner: D9)".to_owned(),
            ),
        },
    ]
}

/// Project one directory listing onto the wire (Phase D7 producer).
///
/// The bound-enforcing half of the producer, in the place every other D-phase
/// puts it: `crate::workspace_files` walks the filesystem and clamps only what
/// it must clamp to bound its own memory, because `tests/architecture.rs`
/// forbids it from naming the module the `MAX_*` wire limits live in. Every
/// wire limit is applied exactly here, once, on the way out.
///
/// The trust class is `OperatorControlled` and this is the only place it may be
/// decided (ADR 0006). A file on disk is not a mjolnr-governed observation —
/// mjolnr read it, it did not produce it — and `MjolnrGoverned` would show a
/// human's own file in the colour reserved for things mjolnr verified.
pub(crate) fn project_directory_page(listing: &DirectoryListing) -> DirectoryPage {
    let limit = MAX_DIRECTORY_ENTRIES_PER_PAGE as usize;
    let truncated = listing.entries.len() > limit;
    let items: Vec<_> = listing
        .entries
        .iter()
        .take(limit)
        .map(project_directory_entry)
        .collect();

    // Computed from the page the client is holding, not from a flag the
    // producer set: a client offered "more" when there is none asks for a page
    // that is refused, which reads as a fault rather than the end of a list.
    let seen = (u64::from(listing.page) + 1).saturating_mul(limit as u64);
    let has_more = listing.total_truncated || u64::from(listing.total_entries) > seen;

    DirectoryPage {
        path: listing.path.clone(),
        page: listing.page,
        entries: BoundedProjection {
            items,
            limit: MAX_DIRECTORY_ENTRIES_PER_PAGE,
            total: Some(listing.total_entries),
            // Two different truncations, one flag, and the reason code
            // distinguishes them: the page hit the wire limit, or the walk hit
            // the enumeration bound so the total is a floor.
            truncated: truncated || listing.total_truncated,
            reason_code: (truncated || listing.total_truncated)
                .then(|| ReasonCode::OutputTruncated.as_str().to_owned()),
        },
        has_more,
        trust: TrustClass::OperatorControlled,
    }
}

fn project_directory_entry(entry: &DirectoryEntry) -> DirectoryEntryView {
    DirectoryEntryView {
        name: entry.name.clone(),
        path: entry.path.clone(),
        kind: entry_kind(entry.kind).to_owned(),
        symlink: entry.symlink.as_ref().map(project_symlink),
        content: project_content(entry.content),
        size_bytes: entry.size_bytes.and_then(|size| u32::try_from(size).ok()),
        ignored: entry.ignored,
        writable: entry.writable,
    }
}

/// Stable identifiers, like a reason code: prose may change, these may not.
const fn entry_kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Directory => "directory",
        EntryKind::File => "file",
        EntryKind::Other => "other",
    }
}

/// An escaping link is projected with no target at all.
///
/// Naming where it points would put a path outside the workspace on the wire —
/// the same path containment just refused to open. `escaping: true` is the
/// entire answer a client needs, and it is the only one it gets.
fn project_symlink(target: &SymlinkTarget) -> SymlinkView {
    match target {
        SymlinkTarget::Contained { path } => SymlinkView {
            target: Some(path.clone()),
            escaping: false,
        },
        SymlinkTarget::Escaping => SymlinkView {
            target: None,
            escaping: true,
        },
    }
}

const fn project_content(content: ContentFacts) -> FileContentView {
    match content {
        ContentFacts::Sniffed { binary, generated } => {
            FileContentView::Sniffed { binary, generated }
        }
        ContentFacts::Oversized => FileContentView::Oversized,
        ContentFacts::Unreadable => FileContentView::Unreadable,
        ContentFacts::NotAFile => FileContentView::NotAFile,
    }
}

/// Project one file read onto the wire (Phase D7 producer).
///
/// Both bodies are clamped again here even though the producer already bounded
/// what it read. The two limits are separate constants that answer separate
/// questions — how much may be read into memory, and how much may cross to a
/// frontend — and a bound enforced at only one of them stops holding the moment
/// they disagree. `the_wire_never_promises_more_than_the_producer_will_read`
/// asserts the relationship rather than leaving it to a comment.
pub(crate) fn project_file_open(read: &FileRead) -> FileOpenView {
    let mode = match &read.mode {
        FileMode::Editable { text } => {
            let (text, text_truncated) = clamp_bytes(text, MAX_FILE_TEXT_BYTES as usize);
            FileModeView::Editable {
                text,
                text_truncated,
            }
        }
        FileMode::Preview {
            reason,
            excerpt,
            excerpt_truncated,
        } => {
            let (excerpt, clamped) = clamp_bytes(excerpt, MAX_FILE_PREVIEW_BYTES as usize);
            FileModeView::Preview {
                reason: reason.as_str().to_owned(),
                excerpt,
                // Either truncation makes the excerpt partial, and a client
                // renders one "…" for both. Losing the producer's flag here
                // would let a clamp at the wire hide a clamp at the read.
                excerpt_truncated: *excerpt_truncated || clamped,
            }
        }
    };

    FileOpenView {
        path: read.path.clone(),
        mode,
        digest: read.digest.clone(),
        // `None` rather than a clamped number for a file past 4 GiB: this wire
        // carries no `u64` (see the DTO module header) and a saturated `u32`
        // would read as the truth.
        size_bytes: u32::try_from(read.size_bytes).ok(),
        writable: read.writable,
        trust: TrustClass::OperatorControlled,
    }
}

/// Bound a string on a char boundary, saying when it was cut.
///
/// Byte-bounded rather than char-bounded because the limits describe payload
/// size, and `floor_char_boundary` is what keeps a cut from splitting a
/// multi-byte character into invalid UTF-8.
fn clamp_bytes(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text.get(..end).unwrap_or_default().to_owned(), true)
}

/// A repository state that reports nothing, for when nothing was read.
///
/// `ExternalUnverified` is deliberate: an empty state is not a governed
/// observation of a clean repository, and labelling it `MjolnrGoverned` would
/// dress up an absence of evidence as evidence.
pub(crate) fn empty_repository_state(freshness: RepositoryFreshness) -> RepositoryState {
    RepositoryState {
        branch: None,
        head: None,
        index_revision: None,
        dirty_count: 0,
        dirty_count_truncated: false,
        staged_files: Vec::new(),
        modified_files: Vec::new(),
        untracked_files: Vec::new(),
        unmerged_files: Vec::new(),
        rebase_in_progress: false,
        paths_truncated: false,
        remote_sync: RepositorySyncState::Unknown,
        remote_sync_as_of: None,
        freshness,
        trust: TrustClass::ExternalUnverified,
    }
}

/// Map a local upstream comparison onto the wire (ADR 0008).
///
/// `None` — no upstream, detached HEAD, unborn branch — becomes `Unknown`, which
/// now means genuinely unknown rather than "not looked at".
///
/// Zero ahead and zero behind becomes `Synced`, whose doc comment carries the
/// warning that matters: it means "level with the ref last seen", and a surface
/// must not render it as a bare "synced" or in the verified colour.
fn project_sync_state(upstream: Option<&UpstreamPosition>) -> RepositorySyncState {
    let Some(position) = upstream else {
        return RepositorySyncState::Unknown;
    };
    match (position.ahead, position.behind) {
        (0, 0) => RepositorySyncState::Synced,
        (ahead, 0) => RepositorySyncState::Ahead { count: ahead },
        (0, behind) => RepositorySyncState::Behind { count: behind },
        (ahead, behind) => RepositorySyncState::Diverged { ahead, behind },
    }
}

/// Project the runtime's repository view onto the wire (Phase D5 producer).
///
/// This is where the trust class is applied, and it is the only place it may
/// be: `crate::repository` runs the git operations and must not also grade how
/// far they are to be trusted (ADR 0006), which is why it returns
/// `core::repository` types carrying no label at all. Two architecture guards
/// keep the arrangement honest — the bridge may not import `crate::repository`,
/// and nothing outside the bridge may touch these DTOs — so the runtime-owned
/// `core::repository::RepositoryView` on the snapshot is the only legal carrier
/// between them.
///
/// `remote_sync` is computed locally (ADR 0008). `rev-list HEAD...@{upstream}`
/// touches no network — it compares two commits that are both already local —
/// so the counts are exact about the ref mjolnr last saw. What no read path may
/// do is learn whether the remote has moved since, which is why the wire also
/// carries `remote_sync_as_of` and why `Synced` must never render unqualified.
///
/// An earlier version of this producer left `remote_sync` permanently `Unknown`
/// on the grounds that a comparison needs a fetch. That is true of the remote's
/// *current* state and false of ahead/behind against an already-fetched ref, so
/// it discarded information mjolnr held. ADR 0008 records the correction.
pub(crate) fn project_repository_state(view: &RepositoryView) -> RepositoryState {
    let projection = match view {
        RepositoryView::NoProject => {
            return empty_repository_state(RepositoryFreshness::NoProject);
        }
        RepositoryView::Unavailable { code, detail } => {
            return empty_repository_state(RepositoryFreshness::Unavailable {
                code: code.as_str().to_owned(),
                detail: truncate_detail(detail),
            });
        }
        RepositoryView::Projected(projection) => projection,
    };

    RepositoryState {
        branch: projection.branch.clone(),
        head: projection.head.clone(),
        index_revision: projection.index_revision.clone(),
        dirty_count: projection.dirty_count,
        dirty_count_truncated: projection.dirty_count_truncated,
        staged_files: projection.staged_files.clone(),
        modified_files: projection.modified_files.clone(),
        untracked_files: projection.untracked_files.clone(),
        unmerged_files: projection.unmerged_files.clone(),
        rebase_in_progress: projection.rebase_in_progress,
        paths_truncated: projection.paths_truncated,
        remote_sync: project_sync_state(projection.upstream.as_ref()),
        remote_sync_as_of: projection
            .upstream
            .as_ref()
            .and_then(|upstream| upstream.ref_updated_at.clone()),
        freshness: RepositoryFreshness::CapturedAt {
            trigger: projection.captured_after.as_str().to_owned(),
            sequence: projection.capture_sequence,
        },
        // mjolnr ran the git invocation itself, through its own argument vector,
        // and re-read the result. That is what this label means here.
        trust: TrustClass::MjolnrGoverned,
    }
}

pub(crate) fn project_repository_history(
    history: &crate::core::repository::RepositoryHistory,
    limit: u32,
) -> crate::core::client::workspace::RepositoryHistory {
    crate::core::client::workspace::RepositoryHistory {
        entries: history
            .entries
            .iter()
            .map(
                |entry| crate::core::client::workspace::RepositoryHistoryEntry {
                    revision: entry.revision.clone(),
                    author: entry.author.clone(),
                    authored_at: entry.authored_at.clone(),
                    subject: entry.subject.clone(),
                },
            )
            .collect(),
        has_more: history.has_more,
        limit,
        trust: TrustClass::MjolnrGoverned,
    }
}

/// Project the runtime's change view onto the wire (Phase D3 producer).
///
/// The bound-enforcing half of the producer. `crate::repository` parses what
/// git printed and clamps nothing, because `tests/architecture.rs` forbids it
/// from naming the module the `MAX_DIFF_*` limits live in; every limit is
/// therefore applied exactly here, once, on the way out.
///
/// Returns `None` for "no project" and for a repository that could not be read.
/// That is the same `None` the D3 contract shipped with, and the surface's
/// existing empty state still renders — an empty `ChangeSet` would instead
/// assert that mjolnr looked and nothing had changed.
///
/// The state is always `CurrentWorkingTree`. `Proposed`, `Applied`, and
/// `ExternallyImported` need provenance this producer does not have: it reads
/// the working tree, which cannot tell whether a line arrived from a governed
/// tool, a human's editor, or another process. Labelling a working-tree read
/// `Applied` would be the false promotion §D3 has a negative test against.
pub(crate) fn project_change_set(
    view: &ChangeView,
    read_evidence: &[ReadRecord],
) -> Option<ChangeSet> {
    let capture = view.capture()?;

    let limit = MAX_FILES_IN_CHANGESET as usize;
    let files_truncated = capture.output_truncated || capture.files.len() > limit;
    let files: Vec<ChangedFile> = capture
        .files
        .iter()
        .take(limit)
        .map(project_changed_file)
        .collect();

    Some(ChangeSet {
        base_object_id: capture.base_revision.clone(),
        current_object_id: capture.index_revision.clone(),
        read_evidence: project_read_evidence(&files, read_evidence),
        files,
        state: ChangeState::CurrentWorkingTree,
        capture_digest: capture.digest.clone(),
        capture_sequence: capture.capture_sequence,
        files_truncated,
        undiffed_untracked: capture.undiffed_untracked.clone(),
    })
}

/// Evidence for the files this change set actually shows.
///
/// Scoped to the projected file list on purpose, and it is what bounds this
/// collection: `MAX_FILES_IN_CHANGESET` already caps `files`, and `files_truncated`
/// already says when that cap bit, so the evidence needs no second limit and no
/// second truncation flag. It also happens to be the right meaning — evidence
/// that mjolnr read a file before editing it is only evidence *about* a file
/// under review.
///
/// Matching is exact string equality between the effect's workspace-relative
/// path and git's repository-relative one. Those agree whenever the open
/// project is the repository root, which is the arrangement the D3 producer
/// already assumes when it hands porcelain paths back to `git diff --no-index`.
/// Where they disagree the evidence is simply absent — an under-report, which
/// is the direction that cannot manufacture a citation.
fn project_read_evidence(
    files: &[ChangedFile],
    read_evidence: &[ReadRecord],
) -> Vec<ReadBeforeEditEvidence> {
    read_evidence
        .iter()
        .filter(|record| files.iter().any(|file| file.path == record.path))
        .map(|record| ReadBeforeEditEvidence {
            path: record.path.clone(),
            read_revision: record.sha256.clone(),
            tool_event_id: record.tool_event_id.clone(),
        })
        .collect()
}

fn project_changed_file(file: &FileChange) -> ChangedFile {
    let hunk_limit = MAX_DIFF_HUNKS_PER_FILE as usize;
    let hunks_dropped = file.hunks.len() > hunk_limit;
    let mut any_hunk_clamped = false;

    let hunks = file
        .hunks
        .iter()
        .take(hunk_limit)
        .map(|hunk| {
            let (projected, clamped) = project_hunk(hunk);
            any_hunk_clamped |= clamped;
            projected
        })
        .collect();

    ChangedFile {
        path: file.path.clone(),
        status: match file.status {
            ChangeStatus::Added => FileStatus::Added,
            ChangeStatus::Modified => FileStatus::Modified,
            ChangeStatus::Deleted => FileStatus::Deleted,
            ChangeStatus::Renamed => FileStatus::Renamed,
        },
        hunks,
        // Binary wins when git reported both, which it does not: git either
        // declines to diff a file or hands over bytes. The order encodes that
        // "git said binary" is a stronger statement than "mjolnr could not
        // decode what git said".
        content: if file.binary {
            FileContent::Binary
        } else if file.undecodable {
            FileContent::Undecodable
        } else {
            FileContent::Text
        },
        // "Large" is the reason a file's content did not fit, and at this
        // boundary the only evidence of size is that something had to be cut.
        // Reporting it from any other signal would be a guess about the file
        // rather than a statement about this projection.
        is_large: hunks_dropped,
        is_truncated: file.truncated || hunks_dropped || any_hunk_clamped,
        old_path: file.old_path.clone(),
    }
}

/// Clamp one hunk to `MAX_DIFF_BYTES_PER_HUNK`, reporting whether it was cut.
///
/// Whole lines only. Half a line of code on a review surface is worse than a
/// line that is visibly absent: the first invites a note against text that was
/// never in the file.
fn project_hunk(hunk: &Hunk) -> (DiffHunk, bool) {
    let budget = MAX_DIFF_BYTES_PER_HUNK as usize;
    let mut used = 0_usize;
    let mut clamped = false;
    let mut lines = Vec::new();

    for line in &hunk.lines {
        let cost = line.content.len().saturating_add(1);
        if used.saturating_add(cost) > budget {
            clamped = true;
            break;
        }
        used = used.saturating_add(cost);
        lines.push(DiffLine {
            kind: match line.kind {
                LineSide::Unchanged => LineKind::Unchanged,
                LineSide::Added => LineKind::Added,
                LineSide::Removed => LineKind::Removed,
            },
            content: line.content.clone(),
            old_line_number: line.old_line_number,
            new_line_number: line.new_line_number,
        });
    }

    (
        DiffHunk {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            header: hunk.header.clone(),
            lines,
        },
        clamped,
    )
}

// ---------------------------------------------------------------------------
// Deterministic workspace search (Phase D4 client half)
// ---------------------------------------------------------------------------

/// Translate a client search filter into the store's own filter.
///
/// Every field a client can set is either typed here or refused here. The store
/// binds parameters and quotes the query as a literal phrase, so this is not the
/// injection boundary — it is the *typing* boundary. `project_id`, `session_id`,
/// `status`, and the two timestamps are strings on the wire and typed values in
/// the store, and a value that will not parse is a `SCHEMA_INVALID` refusal
/// rather than a silently dropped filter.
///
/// That last point is the one worth stating. Dropping an unparseable
/// `project_id` would widen the search past the scope the caller asked for, and
/// §D4's acceptance says a scoped query cannot reach another project. A filter
/// that fails open is the same defect as a guard that fails open.
///
/// `status` deliberately does not use `SessionStatus::parse`, which resolves
/// anything unrecognised to `Ended`. That is the right fail-closed answer when
/// *reading a stored row* and the wrong one for a *query*: it would answer a
/// question the caller did not ask and label the result as if it had.
pub(crate) fn search_filter_from_client(
    filter: &ClientWorkspaceSearchFilter,
) -> Result<WorkspaceSearchFilter, WorkspaceRefusal> {
    fn parse_time(
        field: &str,
        raw: Option<&String>,
    ) -> Result<Option<OffsetDateTime>, WorkspaceRefusal> {
        raw.map(|value| {
            OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
                refuse_search_schema(&format!("{field} must be an RFC 3339 timestamp"))
            })
        })
        .transpose()
    }

    let project_id = filter
        .project_id
        .as_ref()
        .map(|raw| {
            uuid::Uuid::parse_str(raw.trim())
                .map(ProjectId::from_uuid)
                .map_err(|_| refuse_search_schema("project_id must be its UUID text form"))
        })
        .transpose()?;

    let session_id = filter
        .session_id
        .as_ref()
        .map(|raw| {
            uuid::Uuid::parse_str(raw.trim())
                .map(SessionId::from_uuid)
                .map_err(|_| refuse_search_schema("session_id must be its UUID text form"))
        })
        .transpose()?;

    let status = filter
        .status
        .as_ref()
        .map(|raw| match raw.trim() {
            "active" => Ok(SessionStatus::Active),
            "ended" => Ok(SessionStatus::Ended),
            _ => Err(refuse_search_schema(
                "status must be \"active\" or \"ended\"",
            )),
        })
        .transpose()?;

    Ok(WorkspaceSearchFilter {
        query: filter.query.clone(),
        project_id,
        session_id,
        work_kind: filter.work_kind.clone(),
        event_kind: filter.event_kind.clone(),
        status,
        provider_model: filter.provider_model.clone(),
        reason_code: filter.reason_code.clone(),
        file_path: filter.file_path.clone(),
        time_start: parse_time("time_start", filter.time_start.as_ref())?,
        time_end: parse_time("time_end", filter.time_end.as_ref())?,
        cursor: filter.cursor.clone(),
        // Clamped, not refused: a caller asking for more than a page gets a
        // page, which is what pagination is for. The store clamps again — two
        // bounds that agree, neither able to reach the other.
        limit: filter.limit.clamp(1, MAX_SEARCH_RESULTS_PER_PAGE),
    })
}

/// Project a store search page onto the wire.
///
/// The snippet is re-clamped to `MAX_SEARCH_SNIPPET_BYTES` here even though the
/// store already bounds its own output at the same number. The duplication is
/// deliberate and documented at both ends: `tests/architecture.rs` forbids the
/// store from naming the wire contract, so the two constants cannot be shared,
/// and a bound nothing enforces at the boundary it protects is a comment.
pub(crate) fn search_page_to_client(page: WorkspaceSearchPage) -> ClientWorkspaceSearchPage {
    ClientWorkspaceSearchPage {
        items: page
            .items
            .into_iter()
            .map(|item| ClientWorkspaceSearchResult {
                session_id: item.session_id.to_string(),
                event_id: item.event_id.to_string(),
                sequence: item.sequence,
                match_snippet: clamp_snippet(&item.match_snippet),
                occurred_at: item
                    .occurred_at
                    .format(&Rfc3339)
                    .unwrap_or_else(|_| item.occurred_at.to_string()),
            })
            .collect(),
        next_cursor: page.next_cursor,
    }
}

fn refuse_search_schema(detail: &str) -> WorkspaceRefusal {
    WorkspaceRefusal {
        code: ReasonCode::SchemaInvalid.as_str().to_owned(),
        message: format!("Search filter is not valid: {detail}"),
        attempted_revision: None,
        current_revision: None,
    }
}

fn clamp_snippet(snippet: &str) -> String {
    let limit = MAX_SEARCH_SNIPPET_BYTES as usize;
    if snippet.len() <= limit {
        return snippet.to_owned();
    }
    let mut end = limit;
    while end > 0 && !snippet.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    snippet.get(..end).unwrap_or_default().to_owned()
}

/// Bound a refusal detail on a char boundary, marking that it was cut.
fn truncate_detail(detail: &str) -> String {
    if detail.len() <= MAX_REPOSITORY_DETAIL_BYTES {
        return detail.to_owned();
    }
    let mut end = MAX_REPOSITORY_DETAIL_BYTES;
    while end > 0 && !detail.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", detail.get(..end).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use crate::core::client::workspace::WorkItemProvenance;
    use crate::core::workspace_files::PreviewReason;

    fn make_work_item(id: &str) -> WorkItem {
        WorkItem {
            id: id.into(),
            title: format!("Item {id}"),
            title_truncated: false,
            state: "open".into(),
            provenance: WorkItemProvenance {
                source: "test".into(),
                fetched_at: "2025-01-01T00:00:00Z".into(),
                trust: TrustClass::ExternalUnverified,
            },
            revision: 1,
        }
    }

    #[test]
    fn project_work_items_within_limit_not_truncated() {
        let items: Vec<WorkItem> = (0..5).map(|i| make_work_item(&i.to_string())).collect();
        let proj = project_work_items(items.clone(), Some(5));
        assert!(!proj.truncated);
        assert_eq!(proj.items.len(), 5);
        assert!(proj.reason_code.is_none());
        assert_eq!(proj.limit, MAX_WORK_ITEMS_PER_QUERY);
    }

    #[test]
    fn project_work_items_at_exact_limit_not_truncated() {
        let items: Vec<WorkItem> = (0..MAX_WORK_ITEMS_PER_QUERY)
            .map(|i| make_work_item(&i.to_string()))
            .collect();
        let proj = project_work_items(items, Some(MAX_WORK_ITEMS_PER_QUERY));
        assert!(!proj.truncated);
        assert_eq!(proj.items.len(), MAX_WORK_ITEMS_PER_QUERY as usize);
        assert!(proj.reason_code.is_none());
    }

    /// The key negative test: items beyond the limit are truncated with
    /// explicit metadata, never silently dropped.
    #[test]
    fn project_work_items_over_limit_truncated_with_reason_code() {
        let over = MAX_WORK_ITEMS_PER_QUERY + 10;
        let items: Vec<WorkItem> = (0..over).map(|i| make_work_item(&i.to_string())).collect();
        let proj = project_work_items(items, Some(over));
        assert!(proj.truncated);
        assert_eq!(proj.items.len(), MAX_WORK_ITEMS_PER_QUERY as usize);
        assert_eq!(proj.reason_code.as_deref(), Some("OUTPUT_TRUNCATED"));
    }

    #[test]
    fn refuse_stale_revision_carries_both_revisions() {
        let refusal = refuse_stale_revision(3, 7);
        assert_eq!(refusal.code, "WORKSPACE_STALE_REVISION");
        assert_eq!(refusal.attempted_revision, Some(3));
        assert_eq!(refusal.current_revision, Some(7));
        assert!(refusal.message.contains('3'));
        assert!(refusal.message.contains('7'));
    }

    #[test]
    fn refuse_capability_unavailable_sets_code_and_detail() {
        let refusal = refuse_capability_unavailable("terminal", "not yet built");
        assert_eq!(refusal.code, "WORKSPACE_CAPABILITY_UNAVAILABLE");
        assert!(refusal.message.contains("terminal"));
        assert!(refusal.message.contains("not yet built"));
        assert!(refusal.attempted_revision.is_none());
    }

    #[test]
    fn refuse_external_unverified_sets_code() {
        let refusal = refuse_external_unverified("github webhook");
        assert_eq!(refusal.code, "WORKSPACE_EXTERNAL_UNVERIFIED");
        assert!(refusal.message.contains("github webhook"));
    }

    #[test]
    fn refuse_stale_diff_carries_revisions() {
        let refusal = refuse_stale_diff(1, 5);
        assert_eq!(refusal.code, "WORKSPACE_STALE_DIFF");
        assert_eq!(refusal.attempted_revision, Some(1));
        assert_eq!(refusal.current_revision, Some(5));
    }

    #[test]
    fn refuse_auth_sets_code() {
        let refusal = refuse_auth("token expired");
        assert_eq!(refusal.code, "WORKSPACE_AUTH_REFUSED");
        assert!(refusal.message.contains("token expired"));
    }

    /// Exactly the capabilities with a live producer report available.
    ///
    /// This was `all_capabilities_unavailable_in_d0` and asserted the blanket.
    /// The blanket stopped being true when the D5 producer landed, and a test
    /// that has to be edited every time a phase ships teaches nothing anyway.
    /// What matters is that the list and the code agree, in both directions: a
    /// capability claimed available whose producer does not exist is the lie
    /// this guards, and so is one left unavailable after its producer landed.
    #[test]
    fn only_capabilities_with_a_live_producer_report_available() {
        // Update this set in the same commit that lands a producer, never
        // afterwards. `diff_review` joined when the D3 change-set producer
        // landed; its `reason` states what is still missing behind it, because
        // "available" is a claim about a producer existing, not about a phase
        // being finished. `review_threads` joined when the D3 review producer
        // landed, and it is a separate key rather than more text under
        // `diff_review` because a client can want a diff without a review
        // surface — and because D0's `MAX_REVIEW_THREADS_PER_ITEM` had been
        // sitting there naming a capability nothing declared.
        // `file_explorer` joined when the D7 read producer landed, and its
        // `reason` says in as many words that saving is not implemented —
        // because the guard proves a producer exists, and only the text can
        // say which half of the phase it is.
        const AVAILABLE: [&str; 5] = [
            "scm_status",
            "diff_review",
            "search",
            "review_threads",
            "file_explorer",
        ];

        let caps = build_workspace_capabilities();
        assert!(!caps.is_empty());
        for cap in &caps {
            assert_eq!(
                cap.available,
                AVAILABLE.contains(&cap.key.as_str()),
                "capability '{}' reports available = {}, which disagrees with the producers that \
                 actually exist",
                cap.key,
                cap.available
            );
            // Available or not, every entry states its limits: an available
            // capability with no reason invites a surface to assume it is
            // complete.
            assert!(
                cap.reason.is_some(),
                "capability '{}' must state what it does and does not do",
                cap.key
            );
        }
    }

    // -----------------------------------------------------------------------
    // D7 file projections
    // -----------------------------------------------------------------------

    fn listing(
        entries: Vec<DirectoryEntry>,
        total: u32,
        total_truncated: bool,
    ) -> DirectoryListing {
        DirectoryListing {
            path: "src".to_owned(),
            entries,
            page: 0,
            total_entries: total,
            total_truncated,
        }
    }

    fn file_entry(name: &str) -> DirectoryEntry {
        DirectoryEntry {
            name: name.to_owned(),
            path: format!("src/{name}"),
            kind: EntryKind::File,
            symlink: None,
            content: ContentFacts::Sniffed {
                binary: false,
                generated: false,
            },
            size_bytes: Some(10),
            ignored: false,
            writable: true,
        }
    }

    /// The two limits answer different questions — what may be read into memory
    /// and what may cross to a frontend — and they are separate constants in
    /// separate modules. This is the guard that keeps them from drifting into
    /// disagreement, which is the gap the D4 search report recorded as untested
    /// between the store's snippet bound and the wire's.
    #[test]
    fn the_wire_never_promises_more_than_the_producer_will_read() {
        assert!(
            u64::from(MAX_FILE_TEXT_BYTES) <= crate::core::workspace_files::MAX_EDITABLE_FILE_BYTES,
            "the wire would promise text the producer refuses to read"
        );
        assert!(
            MAX_FILE_PREVIEW_BYTES as usize <= crate::core::workspace_files::PREVIEW_BYTES,
            "the wire would promise a preview longer than the producer builds"
        );
        assert!(
            MAX_DIRECTORY_ENTRIES_PER_PAGE as usize
                <= crate::core::workspace_files::MAX_ENUMERATED_ENTRIES,
            "a page could not be filled from a directory walk this shallow"
        );
    }

    /// An escaping link is projected with no target at all. The assertion is
    /// the absence: naming where it points would ship a path outside the
    /// workspace to a client — the same path containment just refused to open.
    #[test]
    fn an_escaping_symlink_is_projected_without_its_target() {
        let entry = DirectoryEntry {
            name: "escape".to_owned(),
            path: "src/escape".to_owned(),
            kind: EntryKind::Other,
            symlink: Some(SymlinkTarget::Escaping),
            content: ContentFacts::NotAFile,
            size_bytes: None,
            ignored: false,
            writable: false,
        };
        let page = project_directory_page(&listing(vec![entry], 1, false));
        let view = &page.entries.items[0];
        let symlink = view.symlink.as_ref().expect("a link is projected");
        assert!(symlink.escaping);
        assert_eq!(symlink.target, None);
        assert_eq!(view.kind, "other");
        assert_eq!(view.content, FileContentView::NotAFile);
    }

    #[test]
    fn a_contained_symlink_keeps_its_project_relative_target() {
        let mut entry = file_entry("link.rs");
        entry.symlink = Some(SymlinkTarget::Contained {
            path: "src/real.rs".to_owned(),
        });
        let page = project_directory_page(&listing(vec![entry], 1, false));
        let symlink = page.entries.items[0]
            .symlink
            .as_ref()
            .expect("a link is projected");
        assert!(!symlink.escaping);
        assert_eq!(symlink.target.as_deref(), Some("src/real.rs"));
    }

    /// A page over the wire limit is clamped and says so. Without the reason
    /// code a truncated page is indistinguishable from a short directory.
    #[test]
    fn a_page_beyond_the_wire_limit_is_clamped_with_its_reason() {
        let entries: Vec<_> = (0..MAX_DIRECTORY_ENTRIES_PER_PAGE + 5)
            .map(|index| file_entry(&format!("f{index}.rs")))
            .collect();
        let total = u32::try_from(entries.len()).expect("fits");
        let page = project_directory_page(&listing(entries, total, false));

        assert_eq!(
            page.entries.items.len(),
            MAX_DIRECTORY_ENTRIES_PER_PAGE as usize
        );
        assert!(page.entries.truncated);
        assert_eq!(
            page.entries.reason_code.as_deref(),
            Some(ReasonCode::OutputTruncated.as_str())
        );
        assert_eq!(page.trust, TrustClass::OperatorControlled);
    }

    /// A directory bigger than the producer will walk reports a total that is a
    /// floor, and says so, rather than a count that quietly stopped.
    #[test]
    fn a_walk_that_stopped_short_reports_a_floor_not_a_count() {
        let page = project_directory_page(&listing(vec![file_entry("a.rs")], 10_000, true));
        assert!(page.entries.truncated);
        assert!(page.has_more);
        assert_eq!(page.entries.total, Some(10_000));
    }

    /// `has_more` is computed from the page the client holds, so the last page
    /// of an exactly-full directory does not offer a page that would be refused.
    #[test]
    fn the_last_page_does_not_offer_another() {
        let mut exact = listing(
            vec![file_entry("a.rs")],
            MAX_DIRECTORY_ENTRIES_PER_PAGE,
            false,
        );
        exact.page = 0;
        assert!(!project_directory_page(&exact).has_more);

        let mut more = listing(
            vec![file_entry("a.rs")],
            MAX_DIRECTORY_ENTRIES_PER_PAGE + 1,
            false,
        );
        more.page = 0;
        assert!(project_directory_page(&more).has_more);
    }

    /// A file past what the wire's `u32` can state says nothing rather than a
    /// clamped number that reads as the truth. `u64` is deliberately absent
    /// from the DTO module; see its header.
    #[test]
    fn a_size_too_large_for_the_wire_is_absent_rather_than_clamped() {
        let read = FileRead {
            path: "big.bin".to_owned(),
            mode: FileMode::Preview {
                reason: PreviewReason::TooLarge,
                excerpt: "x".to_owned(),
                excerpt_truncated: true,
            },
            digest: "abc".to_owned(),
            size_bytes: u64::from(u32::MAX) + 1,
            writable: true,
        };
        let view = project_file_open(&read);
        assert_eq!(view.size_bytes, None);

        let mut small = read;
        small.size_bytes = 512;
        assert_eq!(project_file_open(&small).size_bytes, Some(512));
    }

    /// The producer's truncation flag survives the wire clamp. Losing it would
    /// let a cut made at the read be hidden by a cut made at the wire, and a
    /// client would render a partial file as a whole one.
    #[test]
    fn a_preview_truncated_at_the_read_stays_truncated_on_the_wire() {
        let read = FileRead {
            path: "big.bin".to_owned(),
            mode: FileMode::Preview {
                reason: PreviewReason::Binary,
                excerpt: "short".to_owned(),
                excerpt_truncated: true,
            },
            digest: "abc".to_owned(),
            size_bytes: 4_096,
            writable: false,
        };
        let FileModeView::Preview {
            reason,
            excerpt_truncated,
            ..
        } = project_file_open(&read).mode
        else {
            panic!("a preview must project as a preview");
        };
        assert_eq!(reason, "binary");
        assert!(excerpt_truncated);
    }

    /// Clamping cuts on a char boundary. A naive byte slice here produces
    /// invalid UTF-8 and panics, which in a projection is a crash where a
    /// bounded answer belongs.
    #[test]
    fn clamping_never_splits_a_multi_byte_character() {
        let text = "é".repeat(10);
        let (clamped, truncated) = clamp_bytes(&text, 5);
        assert!(truncated);
        assert_eq!(clamped, "éé");
        assert!(clamped.len() <= 5);

        let (whole, truncated) = clamp_bytes("abc", 5);
        assert_eq!(whole, "abc");
        assert!(!truncated);
    }

    #[test]
    fn empty_repository_state_is_honest() {
        let state = empty_repository_state(RepositoryFreshness::NoProject);
        assert!(state.branch.is_none());
        assert!(state.head.is_none());
        assert_eq!(state.dirty_count, 0);
        assert!(!state.dirty_count_truncated);
        assert!(state.staged_files.is_empty());
        assert_eq!(state.remote_sync, RepositorySyncState::Unknown);
        assert_eq!(state.trust, TrustClass::ExternalUnverified);
        assert_eq!(state.freshness, RepositoryFreshness::NoProject);
    }

    fn sample_projection() -> crate::core::repository::RepositoryProjection {
        crate::core::repository::RepositoryProjection {
            branch: Some("main".to_owned()),
            head: Some("abc123".to_owned()),
            index_revision: Some("tree789".to_owned()),
            dirty_count: 2,
            dirty_count_truncated: false,
            staged_files: vec!["src/lib.rs".to_owned()],
            modified_files: vec!["README.md".to_owned()],
            untracked_files: Vec::new(),
            unmerged_files: Vec::new(),
            rebase_in_progress: false,
            paths_truncated: false,
            upstream: None,
            captured_after: crate::core::repository::RefreshTrigger::ToolWrite,
            capture_sequence: 5,
        }
    }

    #[test]
    fn a_projected_repository_carries_its_capture_moment_and_the_governed_label() {
        let state =
            project_repository_state(&RepositoryView::Projected(Box::new(sample_projection())));

        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!(state.index_revision.as_deref(), Some("tree789"));
        assert_eq!(state.staged_files, vec!["src/lib.rs".to_owned()]);
        // mjolnr ran the git invocation itself and re-read the result.
        assert_eq!(state.trust, TrustClass::MjolnrGoverned);
        assert_eq!(
            state.freshness,
            RepositoryFreshness::CapturedAt {
                trigger: "toolWrite".to_owned(),
                sequence: 5,
            }
        );
    }

    #[test]
    fn remote_sync_is_unknown_when_there_is_no_upstream_to_compare_against() {
        // `Unknown` now means exactly this and only this: nothing to compare
        // against. It stopped being the value for "we did not look" when
        // ADR 0008 landed the local computation.
        let state =
            project_repository_state(&RepositoryView::Projected(Box::new(sample_projection())));
        assert_eq!(state.remote_sync, RepositorySyncState::Unknown);
        assert_eq!(state.remote_sync_as_of, None);
    }

    fn positioned(ahead: u32, behind: u32) -> RepositorySyncState {
        project_sync_state(Some(&UpstreamPosition {
            ahead,
            behind,
            ref_updated_at: None,
        }))
    }

    /// Ahead and behind must not be transposed. Getting this backwards tells a
    /// user to pull when they need to push, which is the kind of wrong that
    /// destroys work rather than merely confusing someone.
    #[test]
    fn ahead_and_behind_are_not_transposed() {
        assert_eq!(positioned(3, 0), RepositorySyncState::Ahead { count: 3 });
        assert_eq!(positioned(0, 2), RepositorySyncState::Behind { count: 2 });
        assert_eq!(
            positioned(3, 2),
            RepositorySyncState::Diverged {
                ahead: 3,
                behind: 2
            }
        );
        assert_eq!(positioned(0, 0), RepositorySyncState::Synced);
    }

    /// The qualifier travels with the counts. A surface that renders `Synced`
    /// without it is claiming currency mjolnr never had — which is why the
    /// timestamp reaches the wire on the same projection.
    #[test]
    fn the_as_of_marker_reaches_the_wire_beside_the_counts() {
        let mut projection = sample_projection();
        projection.upstream = Some(UpstreamPosition {
            ahead: 0,
            behind: 0,
            ref_updated_at: Some("2026-07-30T18:34:50+07:00".to_owned()),
        });
        let state = project_repository_state(&RepositoryView::Projected(Box::new(projection)));
        assert_eq!(state.remote_sync, RepositorySyncState::Synced);
        assert_eq!(
            state.remote_sync_as_of.as_deref(),
            Some("2026-07-30T18:34:50+07:00")
        );
    }

    /// A reflog that cannot answer must not suppress the counts: the counts are
    /// the useful part and the qualifier is rendered from the variant's meaning
    /// whether or not a timestamp exists.
    #[test]
    fn counts_survive_a_missing_reflog_timestamp() {
        let mut projection = sample_projection();
        projection.upstream = Some(UpstreamPosition {
            ahead: 4,
            behind: 0,
            ref_updated_at: None,
        });
        let state = project_repository_state(&RepositoryView::Projected(Box::new(projection)));
        assert_eq!(state.remote_sync, RepositorySyncState::Ahead { count: 4 });
        assert_eq!(state.remote_sync_as_of, None);
    }

    #[test]
    fn an_unreadable_repository_never_renders_as_a_clean_one() {
        // The failure this guards: `Unavailable` collapsing into an empty
        // projection would show branch `None`, zero dirty files, and no
        // conflicts — a description of a clean repository, for one mjolnr could
        // not read at all.
        let state = project_repository_state(&RepositoryView::Unavailable {
            code: ReasonCode::WorkspaceCapabilityUnavailable,
            detail: "not a git repository".to_owned(),
        });

        assert_eq!(state.trust, TrustClass::ExternalUnverified);
        match state.freshness {
            RepositoryFreshness::Unavailable { code, detail } => {
                assert_eq!(code, "WORKSPACE_CAPABILITY_UNAVAILABLE");
                assert!(detail.contains("not a git repository"));
            }
            other => panic!("expected an unavailable freshness, got {other:?}"),
        }
    }

    #[test]
    fn an_over_long_refusal_detail_is_bounded_on_a_char_boundary() {
        let state = project_repository_state(&RepositoryView::Unavailable {
            code: ReasonCode::WorkspaceCapabilityUnavailable,
            // Multi-byte, so a naive byte slice would panic.
            detail: "é".repeat(MAX_REPOSITORY_DETAIL_BYTES),
        });
        match state.freshness {
            RepositoryFreshness::Unavailable { detail, .. } => {
                assert!(detail.len() <= MAX_REPOSITORY_DETAIL_BYTES.saturating_add(4));
                assert!(detail.ends_with('…'));
            }
            other => panic!("expected an unavailable freshness, got {other:?}"),
        }
    }

    #[test]
    fn no_project_and_unreadable_are_distinguishable_on_the_wire() {
        // They send a reader to different remedies: open a project, versus this
        // directory is not a repository. An `Option` would have collapsed them.
        let none = project_repository_state(&RepositoryView::NoProject);
        let broken = project_repository_state(&RepositoryView::Unavailable {
            code: ReasonCode::WorkspaceCapabilityUnavailable,
            detail: "not a git repository".to_owned(),
        });
        assert_ne!(none.freshness, broken.freshness);
    }

    // -----------------------------------------------------------------------
    // Read-before-edit evidence (Phase D3)
    // -----------------------------------------------------------------------

    fn captured_view(paths: &[&str]) -> ChangeView {
        ChangeView::Captured(Box::new(crate::core::change_capture::ChangeCapture {
            base_revision: Some("abc123".to_owned()),
            index_revision: Some("tree789".to_owned()),
            digest: "digest".to_owned(),
            files: paths
                .iter()
                .map(|path| FileChange {
                    path: (*path).to_owned(),
                    old_path: None,
                    status: ChangeStatus::Modified,
                    hunks: Vec::new(),
                    binary: false,
                    undecodable: false,
                    truncated: false,
                })
                .collect(),
            output_truncated: false,
            undiffed_untracked: Vec::new(),
            capture_sequence: 5,
        }))
    }

    fn read(path: &str, event: &str) -> ReadRecord {
        ReadRecord {
            path: path.to_owned(),
            sha256: "0f1e2d".to_owned(),
            tool_event_id: event.to_owned(),
        }
    }

    #[test]
    fn evidence_cites_the_event_id_the_store_assigned() {
        let set = project_change_set(
            &captured_view(&["src/lib.rs"]),
            &[read("src/lib.rs", "evt-7")],
        )
        .expect("a change set");

        let evidence = set.read_evidence.first().expect("one evidence entry");
        assert_eq!(evidence.path, "src/lib.rs");
        assert_eq!(evidence.tool_event_id, "evt-7");
        assert_eq!(evidence.read_revision, "0f1e2d");
    }

    /// The guard the whole field exists for. Until this producer landed the
    /// list was empty *because* there was no id to cite, and the one failure
    /// worth refusing is a citation to an event that never happened — so a file
    /// nothing recorded a read for gets no entry, not a blank or invented one.
    #[test]
    fn a_file_with_no_recorded_read_is_never_given_a_citation() {
        let set = project_change_set(
            &captured_view(&["src/lib.rs", "README.md"]),
            &[read("src/lib.rs", "evt-7")],
        )
        .expect("a change set");

        assert_eq!(set.read_evidence.len(), 1);
        assert!(
            set.read_evidence
                .iter()
                .all(|item| item.path == "src/lib.rs"),
            "evidence must name only files a read was actually recorded for"
        );
        assert!(
            set.read_evidence
                .iter()
                .all(|item| !item.tool_event_id.is_empty()),
            "an entry with an empty event id is a citation to nothing"
        );
    }

    /// Evidence is scoped to the files this set shows, which is also what
    /// bounds it: `MAX_FILES_IN_CHANGESET` caps `files`, so nothing here can
    /// grow past that cap and no second truncation flag is owed.
    #[test]
    fn evidence_for_a_file_outside_the_change_set_is_not_projected() {
        let set = project_change_set(
            &captured_view(&["src/lib.rs"]),
            &[
                read("src/lib.rs", "evt-7"),
                read("docs/unrelated.md", "evt-8"),
            ],
        )
        .expect("a change set");

        assert_eq!(set.read_evidence.len(), 1);
        assert_eq!(
            set.read_evidence
                .first()
                .map(|item| item.path.as_str())
                .unwrap_or_default(),
            "src/lib.rs"
        );
    }

    #[test]
    fn evidence_never_promotes_a_working_tree_read_to_applied() {
        // The state and the evidence are independent claims, and neither may
        // borrow authority from the other: citing a read event says a read
        // happened, never that a change was applied or verified.
        let set = project_change_set(
            &captured_view(&["src/lib.rs"]),
            &[read("src/lib.rs", "evt-7")],
        )
        .expect("a change set");
        assert_eq!(set.state, ChangeState::CurrentWorkingTree);
    }

    // -----------------------------------------------------------------------
    // Deterministic workspace search (Phase D4 client half)
    // -----------------------------------------------------------------------

    fn client_filter() -> ClientWorkspaceSearchFilter {
        ClientWorkspaceSearchFilter {
            query: Some("refused".to_owned()),
            project_id: None,
            session_id: None,
            work_kind: None,
            event_kind: None,
            status: None,
            provider_model: None,
            reason_code: None,
            file_path: None,
            time_start: None,
            time_end: None,
            cursor: None,
            limit: 10,
        }
    }

    /// The scope-widening failure. Dropping a `project_id` that will not parse
    /// leaves a filter that searches *everything*, which is precisely the leak
    /// §D4's "a scoped query cannot reach another project" bullet forbids. A
    /// filter that fails open is the same defect as a guard that fails open.
    #[test]
    fn an_unparseable_scope_is_refused_rather_than_dropped() {
        let mut filter = client_filter();
        filter.project_id = Some("not-a-uuid".to_owned());

        let refusal = search_filter_from_client(&filter).expect_err("must refuse");
        assert_eq!(refusal.code, "SCHEMA_INVALID");
        assert!(refusal.message.contains("project_id"));

        filter.project_id = None;
        filter.session_id = Some("also-not-a-uuid".to_owned());
        let refusal = search_filter_from_client(&filter).expect_err("must refuse");
        assert_eq!(refusal.code, "SCHEMA_INVALID");
        assert!(refusal.message.contains("session_id"));
    }

    /// `SessionStatus::parse` resolves anything unrecognised to `Ended`, which
    /// is the right answer for reading a stored row and the wrong one for a
    /// query: it would silently answer a question nobody asked.
    #[test]
    fn an_unrecognised_status_filter_is_refused_not_resolved_to_ended() {
        let mut filter = client_filter();
        filter.status = Some("archived".to_owned());

        let refusal = search_filter_from_client(&filter).expect_err("must refuse");
        assert_eq!(refusal.code, "SCHEMA_INVALID");

        filter.status = Some("ended".to_owned());
        let typed = search_filter_from_client(&filter).expect("ended is a real status");
        assert_eq!(typed.status, Some(SessionStatus::Ended));
    }

    #[test]
    fn a_timestamp_that_is_not_rfc3339_is_refused() {
        let mut filter = client_filter();
        filter.time_start = Some("yesterday".to_owned());
        let refusal = search_filter_from_client(&filter).expect_err("must refuse");
        assert_eq!(refusal.code, "SCHEMA_INVALID");
        assert!(refusal.message.contains("time_start"));

        filter.time_start = Some("2026-07-30T00:00:00Z".to_owned());
        let typed = search_filter_from_client(&filter).expect("rfc3339 parses");
        assert!(typed.time_start.is_some());
    }

    /// Clamped, not refused: asking for more than a page is what pagination
    /// answers. Zero is clamped up, because a zero-size page is a question with
    /// no possible useful answer.
    #[test]
    fn an_over_limit_page_size_is_clamped_to_the_wire_bound() {
        let mut filter = client_filter();
        filter.limit = 10_000;
        assert_eq!(
            search_filter_from_client(&filter).expect("clamped").limit,
            MAX_SEARCH_RESULTS_PER_PAGE
        );

        filter.limit = 0;
        assert_eq!(
            search_filter_from_client(&filter).expect("clamped").limit,
            1
        );
    }

    /// The store bounds its own snippets at 512 bytes and the wire bound is the
    /// same number, held in a constant the store may not name
    /// (`tests/architecture.rs`). Nothing proved the two agreed until here.
    #[test]
    fn a_snippet_is_re_clamped_at_the_wire_boundary() {
        let long = "x".repeat(MAX_SEARCH_SNIPPET_BYTES as usize * 2);
        let page = WorkspaceSearchPage {
            items: vec![crate::core::store::WorkspaceSearchResult {
                session_id: SessionId::new(),
                event_id: crate::core::event::EventId::new(),
                sequence: 7,
                match_snippet: long,
                occurred_at: OffsetDateTime::UNIX_EPOCH,
            }],
            next_cursor: None,
        };

        let projected = search_page_to_client(page);
        let item = projected.items.first().expect("one result");
        assert_eq!(item.match_snippet.len(), MAX_SEARCH_SNIPPET_BYTES as usize);
    }

    /// Clamping must not split a UTF-8 sequence. A snippet is transcript text,
    /// so a multi-byte character landing on the bound is ordinary, not exotic.
    #[test]
    fn clamping_a_snippet_never_produces_invalid_utf8() {
        // 'é' is two bytes, so 512 bytes lands mid-character.
        let snippet = "é".repeat(MAX_SEARCH_SNIPPET_BYTES as usize);
        let clamped = clamp_snippet(&snippet);
        assert!(clamped.len() <= MAX_SEARCH_SNIPPET_BYTES as usize);
        assert!(snippet.starts_with(&clamped));
    }

    #[test]
    fn a_projected_result_carries_rfc3339_and_the_cursor() {
        let page = WorkspaceSearchPage {
            items: vec![crate::core::store::WorkspaceSearchResult {
                session_id: SessionId::new(),
                event_id: crate::core::event::EventId::new(),
                sequence: 3,
                match_snippet: "a refusal".to_owned(),
                occurred_at: OffsetDateTime::UNIX_EPOCH,
            }],
            next_cursor: Some("opaque".to_owned()),
        };

        let projected = search_page_to_client(page);
        assert_eq!(projected.next_cursor.as_deref(), Some("opaque"));
        let item = projected.items.first().expect("one result");
        assert_eq!(item.occurred_at, "1970-01-01T00:00:00Z");
        assert_eq!(item.sequence, 3);
    }
}
