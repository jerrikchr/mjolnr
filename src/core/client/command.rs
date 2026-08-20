//! Bounded client command intent.

use serde::{Deserialize, Serialize};

use crate::core::command::ApprovalDecision;
use crate::core::policy::PolicyMode;
use crate::core::recovery::RecoveryDecision;

/// Largest accepted `name` on the child-run commands (Phase D2). The name
/// becomes a worktree and branch identifier once execution lands, so the wire
/// refuses anything that could not survive that use.
pub const MAX_CHILD_RUN_NAME_BYTES: usize = 64;

/// Largest accepted `directive` on `StartChild`. `MAX_DIRECTIVE_TEXT` (500)
/// bounds user directives in the *transcript*; a child-run directive is a full
/// task specification and gets its own, larger ceiling.
pub const MAX_CHILD_RUN_DIRECTIVE_BYTES: usize = 4_096;

/// Largest accepted `base_revision` string (rev-parse output or a ref name).
pub const MAX_BASE_REVISION_BYTES: usize = 256;

/// Largest source identifier accepted by governed clone.
pub const MAX_CLONE_SOURCE_BYTES: usize = 2_048;

/// Largest absolute destination path accepted by governed clone.
pub const MAX_CLONE_DESTINATION_BYTES: usize = 1_024;

/// Largest accepted repository `paths` list on the Phase D5 staging commands.
/// Each element becomes one argv entry of a `git` invocation; a review surface
/// that needs more than this is asking for a whole-tree operation, which is a
/// different, separately-approved intent.
pub const MAX_REPOSITORY_PATHS: usize = 512;

/// Largest accepted single repository path. Comfortably under every supported
/// platform's `PATH_MAX` so a refusal happens on the wire, not inside `git`.
pub const MAX_REPOSITORY_PATH_BYTES: usize = 1_024;

/// Largest accepted branch name on `CreateBranch` / `IntegrateChildBranch`.
pub const MAX_BRANCH_NAME_BYTES: usize = 200;

/// Largest accepted human-supplied commit message. Commit messages are always
/// human-authored or human-edited text (Phase D5 acceptance: suggestions are
/// advisory only), so this bounds a person's typing, not a model's output.
pub const MAX_COMMIT_MESSAGE_BYTES: usize = 8_192;

/// Largest target expression accepted by governed rebase.
pub const MAX_REBASE_TARGET_BYTES: usize = 256;

/// Largest accepted integration id (Phase D6), e.g. `github`.
pub const MAX_INTEGRATION_ID_BYTES: usize = 64;

/// Largest accepted remote task id. Generous enough for a Linear identifier or
/// a GitHub node id, bounded because it comes from outside.
pub const MAX_REMOTE_TASK_ID_BYTES: usize = 256;

/// Maximum number of remote tasks one bounded fetch command may request.
/// Fetches remain sequential so a partial batch has a clear durable prefix and
/// cannot turn a provider rate limit into an unbounded fan-out.
pub const MAX_FETCH_BATCH_SIZE: usize = 32;

/// Largest accepted review-note body (Phase D3). A line note is a remark about
/// one line, not a document; this bounds a person's typing where it enters the
/// durable record and, through it, the directive `sendReviewNotes` builds.
pub const MAX_REVIEW_NOTE_BYTES: usize = 2_048;

/// Largest accepted decision-ticket question (Phase E5). One unknown, stated
/// once — a question that needs more than this is a document, which is what
/// the plan family is for.
pub const MAX_TICKET_QUESTION_BYTES: usize = 2_048;

/// Largest accepted single option on a decision ticket.
pub const MAX_TICKET_OPTION_BYTES: usize = 1_024;

/// Most options one decision ticket may record. A decision is a choice among
/// few; more than this is a decomposition request, not a decision.
pub const MAX_TICKET_OPTIONS: usize = 8;

/// Most blocking edges one ticket may record at open. Beyond this the honest
/// structure is a dependency fan-in across several tickets, not one node's
/// waiting list.
pub const MAX_TICKET_BLOCKERS: usize = 64;

/// Largest accepted resolution note (Phase E5): the reasoning in the human's
/// words, bounded like a review note for the same reason — it enters the
/// durable record.
pub const MAX_TICKET_NOTE_BYTES: usize = 2_048;

/// Largest number of threads one `sendReviewNotes` may carry.
///
/// The two bounds multiply: this times [`MAX_REVIEW_NOTE_BYTES`] is the ceiling
/// on the directive text a review request can put in front of a model, and a
/// ceiling that exists only on the per-note half is not a ceiling.
/// [`MAX_REVIEW_THREADS_PER_ITEM`](crate::core::client::workspace::MAX_REVIEW_THREADS_PER_ITEM)
/// bounds how many threads may *exist*; this bounds how many may be sent at
/// once, which is a different question with a much smaller sensible answer.
pub const MAX_REVIEW_THREADS_PER_REQUEST: usize = 20;

/// Largest human note attached to a council finding disposition.
pub const MAX_COUNCIL_NOTE_BYTES: usize = 2_048;

/// Largest accepted `captureDigest`. A SHA-256 in hex is 64 characters; the
/// slack is for a future digest, not for arbitrary text.
pub const MAX_CAPTURE_DIGEST_BYTES: usize = 128;

/// Largest editor buffer accepted by the D7 save command. It matches the
/// producer's editable-file ceiling, so a client cannot send a buffer the
/// reader would never allow into an editor.
/// Maximum UTF-8 payload accepted by the human-editor save command.
///
/// This mirrors `MAX_EDITABLE_FILE_BYTES`, whose public type is `u64` because
/// it is also used for filesystem metadata. Keep the two values aligned when
/// changing the editor size policy.
pub const MAX_SAVE_TEXT_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub enum ClientCommand {
    #[serde(rename_all = "camelCase")]
    OpenProject {
        root: String,
    },
    /// Clone into a new absolute destination and open it only after the
    /// resulting repository has been verified.
    CloneProject {
        source: String,
        destination: String,
    },
    /// Ask what git says about the open project right now (§D5 producer).
    ///
    /// No fields, deliberately. The runtime reads the project it already has
    /// open; accepting a root here would be a second way to point mjolnr at a
    /// directory, bypassing every refusal `OpenProject` applies.
    RefreshRepository,
    #[serde(rename_all = "camelCase")]
    SaveFile {
        /// Project-relative path returned by `openFile`.
        path: String,
        /// Full SHA-256 digest returned by `openFile`.
        expected_digest: String,
        /// UTF-8 editor contents, bounded by `MAX_SAVE_TEXT_BYTES`.
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    CreateSession {
        provider: String,
        model: String,
    },
    #[serde(rename_all = "camelCase")]
    ResumeSession {
        session: String,
    },
    #[serde(rename_all = "camelCase")]
    ResolveResume {
        choice: ClientResumeChoice,
    },
    #[serde(rename_all = "camelCase")]
    SendMessage {
        text: String,
    },
    CancelRun,
    #[serde(rename_all = "camelCase")]
    ResolveApproval {
        approval: String,
        decision: ClientApprovalDecision,
    },
    #[serde(rename_all = "camelCase")]
    ResolveRecovery {
        decision: ClientRecoveryDecision,
    },
    #[serde(rename_all = "camelCase")]
    SetPolicy {
        policy: ClientPolicy,
    },
    #[serde(rename_all = "camelCase")]
    StartPlanInterview {
        goal: String,
    },
    #[serde(rename_all = "camelCase")]
    AskPlanQuestion {
        plan_id: String,
        prompt: String,
        options: Vec<String>,
        is_multi_select: bool,
    },
    #[serde(rename_all = "camelCase")]
    AnswerPlanQuestion {
        plan_id: String,
        question_id: String,
        selected_options: Vec<String>,
        freeform_text: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ProposePlan {
        plan_id: String,
        revision: u32,
        title: String,
        summary: String,
        steps: Vec<ClientPlanStep>,
    },
    #[serde(rename_all = "camelCase")]
    ReviewPlan {
        plan_id: String,
        revision: u32,
        reviewer: String,
        verdict: ClientReviewVerdict,
        feedback: String,
    },
    #[serde(rename_all = "camelCase")]
    ApprovePlan {
        plan_id: String,
        revision: u32,
        decision: ClientReviewVerdict,
        note: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    HandoffPlan {
        plan_id: String,
        revision: u32,
        note: String,
    },
    EndSession,
    RequestSnapshot,
    #[serde(rename_all = "camelCase")]
    CreateWorktree {
        name: String,
        base_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    ForkWork {
        name: String,
        base_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    StartChild {
        name: String,
        directive: String,
        /// At most the parent's policy. Omitted means "inherit the parent's
        /// policy unchanged"; a value may only lower the ceiling — children
        /// inherit less, never more (AGENTS.md §11.4), and the runtime clamps
        /// at execution time.
        policy_ceiling: Option<ClientPolicy>,
        budget: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    CancelChild {
        /// Cancel a *running* child; refused for children not running.
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    PreserveBranch {
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    SettleChild {
        /// Settle a finished child; valid only in a settle-able lifecycle
        /// state, refused otherwise.
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    DiscardSettledWorktree {
        /// Discard an already-settled worktree. Use only after `settleChild`
        /// has completed; the runtime refuses otherwise.
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    StagePaths {
        /// Repository-relative paths. Bounded by `MAX_REPOSITORY_PATHS` and
        /// `MAX_REPOSITORY_PATH_BYTES`; each becomes one `git` argv element.
        paths: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    StageHunks {
        path: String,
        hunk_indices: Vec<usize>,
    },
    #[serde(rename_all = "camelCase")]
    Unstage {
        paths: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    CreateBranch {
        name: String,
        base_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    Commit {
        /// The human's chosen message. mjolnr may *suggest* text, but the
        /// suggestion is advisory: the value that reaches `git commit` is
        /// whatever the person selected or edited (Phase D5 acceptance).
        message: String,
        /// The index revision the human saw when they approved this commit.
        /// Required, not optional — an opt-in staleness guard is not a guard.
        /// A mismatch is refused with `WORKSPACE_STALE_REVISION`.
        expected_index_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    IntegrateChildBranch {
        name: String,
        /// Required, not generated. An integration merge is a commit, and a
        /// commit's message is a human act — the earlier auto-generated
        /// "Integrate child branch <name>" made mjolnr the author of a record
        /// the human never wrote.
        message: String,
        /// The HEAD the human saw when they selected this integration.
        expected_head: String,
    },
    /// Fetch from the configured upstream remote (Phase D5 git surface).
    /// Inert and human-initiated; carries no arguments.
    #[serde(rename_all = "camelCase")]
    Fetch,
    /// Push the current branch's HEAD to its configured upstream (Phase D5 git
    /// surface). Human-initiated from the desktop preview; the model never
    /// self-approves a push.
    #[serde(rename_all = "camelCase")]
    Push {
        /// The HEAD the human saw when they approved pushing. A mismatch is
        /// refused with `WORKSPACE_STALE_REVISION`.
        expected_head: String,
    },
    /// Merge the branch's configured upstream into it — the merge half of
    /// "pull" (Phase D5 git surface). Human-
    /// initiated from the desktop preview; the model never self-approves.
    #[serde(rename_all = "camelCase")]
    IntegrateUpstream {
        /// Required and human-supplied: when the merge creates a commit, mjolnr
        /// never authors its record. Consumed only in that case — a
        /// fast-forward creates no commit, exactly as `git pull` does.
        message: String,
        /// The HEAD the human saw when they approved the merge. A mismatch is
        /// refused with `WORKSPACE_STALE_REVISION`.
        expected_head: String,
    },
    /// Rebase the current clean branch onto an explicitly named local ref.
    Rebase {
        onto: String,
        expected_head: String,
    },
    /// Abort an in-progress rebase after an explicit human preview.
    AbortRebase,
    /// Read one task from an integration (Phase D6).
    #[serde(rename_all = "camelCase")]
    FetchTask {
        /// The integration id, e.g. `github`. Not a `ProviderId`: a task source
        /// and an LLM provider are different trust classes.
        source: String,
        task_id: String,
    },
    /// Fetch a bounded list sequentially. Earlier successful items remain
    /// durable if a later item refuses.
    #[serde(rename_all = "camelCase")]
    FetchTasks {
        source: String,
        task_ids: Vec<String>,
    },
    /// Offer a change to an integration (Phase D6). GitHub maps this to a pull
    /// request; Linear refuses because it has no equivalent destination in the
    /// provider-neutral contract.
    #[serde(rename_all = "camelCase")]
    SubmitChange {
        source: String,
        request: ClientRemoteChangeRequest,
    },
    /// Pin a review note to one line of the diff mjolnr last captured (§D3).
    ///
    /// `captureDigest` is the diff revision the human was looking at and is
    /// required, not optional: an opt-in staleness guard is not a guard. A
    /// mismatch is refused with `WORKSPACE_STALE_DIFF`; the note is never moved
    /// to whatever occupies that line now.
    ///
    /// There is deliberately no `hunkHeader` field. The runtime reads the hunk
    /// context out of its own capture, so a client cannot record a note against
    /// a diff that never existed.
    #[serde(rename_all = "camelCase")]
    AddReviewNote {
        path: String,
        side: ClientReviewSide,
        line: u32,
        capture_digest: String,
        body: String,
    },
    /// Add a further human remark to an existing thread. Never a model's:
    /// mjolnr answers a review in the transcript, not in the thread.
    #[serde(rename_all = "camelCase")]
    AddReviewComment {
        thread_id: String,
        body: String,
    },
    /// Send the selected threads to mjolnr as a durable revision request (§D3).
    #[serde(rename_all = "camelCase")]
    SendReviewNotes {
        thread_ids: Vec<String>,
    },
    /// Record an advisory human disposition. It is not an approval and does
    /// not authorize an amended artifact or any tool execution.
    #[serde(rename_all = "camelCase")]
    ResolveCouncilFinding {
        review_id: String,
        finding_id: String,
        disposition: ClientCouncilDisposition,
        note: Option<String>,
    },
    /// Ask the runtime to compose an amended artifact from accepted findings.
    /// This writes nothing; it produces a draft for a human to save.
    #[serde(rename_all = "camelCase")]
    ProposeCouncilAmendment {
        review_id: String,
    },
    /// Open a decision ticket (Phase E5). Human-initiated: the model may
    /// draft options in conversation, but a ticket is recorded by the human
    /// (ADR-0015's "A model may draft the options; a human resolves").
    #[serde(rename_all = "camelCase")]
    OpenDecisionTicket {
        /// The question, verbatim. Bounded by `MAX_TICKET_QUESTION_BYTES`.
        question: String,
        kind: ClientDecisionTicketKind,
        /// The options considered, in stable order. Two or more — a decision
        /// with fewer is not a decision.
        options: Vec<String>,
        /// Ids of tickets that must resolve first, each its UUID text form.
        /// Duplicates and count over `MAX_TICKET_BLOCKERS` are refused.
        blocked_by: Vec<String>,
    },
    /// Resolve a decision ticket. Records durable human judgement and never
    /// authority (ADR-0015): the chosen option is a reference into the
    /// ticket's recorded options — never a status word.
    #[serde(rename_all = "camelCase")]
    ResolveDecisionTicket {
        /// The ticket's `DecisionTicketId` in its UUID text form.
        ticket: String,
        /// Index into the ticket's recorded options.
        chosen_option: u32,
        /// The reasoning, in the human's words. Bounded by
        /// `MAX_TICKET_NOTE_BYTES`.
        note: Option<String>,
    },
    /// Record one fetched imported item as a board node (Phase E5, step 4b).
    /// The item's `blockedBy` is a fact about ordering; a resolution is human
    /// judgement and is never imported (design §4).
    #[serde(rename_all = "camelCase")]
    ImportWorkItem {
        item: crate::core::imported::ImportedItem,
    },
    /// Refresh a previously imported item at a new revision. `expectedRevision`
    /// is the revision the human saw when they approved the refresh; a mismatch
    /// is refused with `WORKSPACE_STALE_REVISION` (contract (a)).
    #[serde(rename_all = "camelCase")]
    RefreshImportedItem {
        expected_revision: String,
        item: crate::core::imported::ImportedItem,
    },
    /// Add a comment on an imported item's external discussion (Phase D6 sync-back).
    #[serde(rename_all = "camelCase")]
    SubmitImportedComment {
        integration: String,
        remote_id: String,
        expected_revision: String,
        body: String,
    },
    /// Rollback session transcript and workspace state to a verified checkpoint.
    /// Operates through the ordinary policy gate and requires verified sequence.
    #[serde(rename_all = "camelCase")]
    RollbackToCheckpoint {
        target_sequence: u64,
        expected_head: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ExternalAgentList,
    #[serde(rename_all = "camelCase")]
    ExternalAgentLaunch {
        profile: String,
    },
    #[serde(rename_all = "camelCase")]
    ExternalAgentStop {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    ExternalAgentImport {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientCouncilDisposition {
    Accept,
    Reject,
    Defer,
}

/// What kind of unknown a decision ticket settles (Phase E5), the client
/// mirror of `core::board::DecisionTicketKind`. It shapes what evidence a
/// resolution is expected to carry — it grants nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientDecisionTicketKind {
    Research,
    Prototype,
    Grilling,
    Task,
}

impl From<ClientDecisionTicketKind> for crate::core::board::DecisionTicketKind {
    fn from(kind: ClientDecisionTicketKind) -> Self {
        match kind {
            ClientDecisionTicketKind::Research => Self::Research,
            ClientDecisionTicketKind::Prototype => Self::Prototype,
            ClientDecisionTicketKind::Grilling => Self::Grilling,
            ClientDecisionTicketKind::Task => Self::Task,
        }
    }
}

/// Which side of the diff a client pinned a note to.
///
/// A closed enum with no catch-all: an unrecognised side is a wire bug between
/// two components that ship together, and guessing one would anchor the note to
/// the other twelve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientReviewSide {
    Old,
    New,
}

impl From<ClientReviewSide> for crate::core::review::ReviewSide {
    fn from(side: ClientReviewSide) -> Self {
        match side {
            ClientReviewSide::Old => Self::Old,
            ClientReviewSide::New => Self::New,
        }
    }
}

/// A change offered to a remote system, as it crosses the wire.
///
/// `deny_unknown_fields` matters here more than on most DTOs: `title` and
/// `body` are externally supplied text, so an extra key — accidental or an
/// injection attempt — must be refused rather than quietly accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ClientRemoteChangeRequest {
    pub remote_id: String,
    /// The revision of the imported item the human was looking at when they
    /// approved this change. Required, not optional: an opt-in staleness check
    /// is not a check (§E5 contract (a), `AddReviewNote::capture_digest`'s rule
    /// applied to a remote). A pin that does not match what mjolnr recorded is
    /// refused with `WORKSPACE_STALE_REVISION` and nothing is posted.
    pub expected_revision: String,
    pub title: String,
    pub body: String,
    /// The exact local commit the remote pull request will point at.
    pub head_commit: String,
    pub head_branch: String,
    pub base_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanStep {
    pub index: usize,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientReviewVerdict {
    Approve,
    Iterate,
    Reject,
}

impl From<ClientReviewVerdict> for crate::core::plan::ReviewVerdict {
    fn from(verdict: ClientReviewVerdict) -> Self {
        match verdict {
            ClientReviewVerdict::Approve => Self::Approve,
            ClientReviewVerdict::Iterate => Self::Iterate,
            ClientReviewVerdict::Reject => Self::Reject,
        }
    }
}

impl From<crate::core::plan::ReviewVerdict> for ClientReviewVerdict {
    fn from(verdict: crate::core::plan::ReviewVerdict) -> Self {
        match verdict {
            crate::core::plan::ReviewVerdict::Approve => Self::Approve,
            crate::core::plan::ReviewVerdict::Iterate => Self::Iterate,
            crate::core::plan::ReviewVerdict::Reject => Self::Reject,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientPolicy {
    ReadOnly,
    #[default]
    Ask,
    WorkspaceWrite,
    FullAuto,
}

impl ClientPolicy {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Ask => "ask",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAuto => "full-auto",
        }
    }
}

impl From<PolicyMode> for ClientPolicy {
    fn from(mode: PolicyMode) -> Self {
        match mode {
            PolicyMode::ReadOnly => Self::ReadOnly,
            PolicyMode::Ask => Self::Ask,
            PolicyMode::WorkspaceWrite => Self::WorkspaceWrite,
            PolicyMode::FullAuto => Self::FullAuto,
        }
    }
}

impl From<ClientPolicy> for PolicyMode {
    fn from(policy: ClientPolicy) -> Self {
        match policy {
            ClientPolicy::ReadOnly => Self::ReadOnly,
            ClientPolicy::Ask => Self::Ask,
            ClientPolicy::WorkspaceWrite => Self::WorkspaceWrite,
            ClientPolicy::FullAuto => Self::FullAuto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientApprovalDecision {
    Deny,
    ApproveOnce,
    ApproveExactForSession,
}

impl From<ClientApprovalDecision> for ApprovalDecision {
    fn from(decision: ClientApprovalDecision) -> Self {
        match decision {
            ClientApprovalDecision::Deny => Self::Deny,
            ClientApprovalDecision::ApproveOnce => Self::ApproveOnce,
            ClientApprovalDecision::ApproveExactForSession => Self::ApproveExactForSession,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientResumeChoice {
    Compact,
    NewFromHandoff,
    Full,
}

impl From<ClientResumeChoice> for crate::core::continuation::ResumeChoice {
    fn from(choice: ClientResumeChoice) -> Self {
        match choice {
            ClientResumeChoice::Compact => Self::Compact,
            ClientResumeChoice::NewFromHandoff => Self::NewFromHandoff,
            ClientResumeChoice::Full => Self::Full,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientRecoveryDecision {
    AbandonAndContinue,
    EndSession,
}

impl From<ClientRecoveryDecision> for RecoveryDecision {
    fn from(decision: ClientRecoveryDecision) -> Self {
        match decision {
            ClientRecoveryDecision::AbandonAndContinue => Self::AbandonAndContinue,
            ClientRecoveryDecision::EndSession => Self::EndSession,
        }
    }
}

impl From<RecoveryDecision> for ClientRecoveryDecision {
    fn from(decision: RecoveryDecision) -> Self {
        match decision {
            RecoveryDecision::AbandonAndContinue => Self::AbandonAndContinue,
            RecoveryDecision::EndSession => Self::EndSession,
        }
    }
}
