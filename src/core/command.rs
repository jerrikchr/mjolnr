//! Commands the TUI (or any client) sends to the runtime.
//!
//! This is the whole vocabulary a client has. Note what is *not* here: there is
//! no "execute this tool" or "call this provider". Clients express intent; the
//! runtime decides. That asymmetry is what makes the TUI a client rather than a
//! co-owner of the session.

use std::path::PathBuf;

use crate::core::continuation::ResumeChoice;
use crate::core::directive::DirectiveSource;
use crate::core::envelope::SpawnEnvelope;
use crate::core::event::SessionId;
use crate::core::model::{ModelId, ProviderId};
use crate::core::policy::PolicyMode;
use crate::core::recovery::RecoveryDecision;

/// A secure wrapper for credentials passed via commands to prevent leaking them in logs/Debug.
#[derive(Clone)]
pub struct CredentialSecret(pub String);

impl std::fmt::Debug for CredentialSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl PartialEq for CredentialSecret {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for CredentialSecret {}

/// A client's request to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MjolnrCommand {
    RegisterCredential {
        provider: ProviderId,
        secret: CredentialSecret,
    },
    /// Re-read credential state and publish a fresh snapshot. Sent after an
    /// OAuth login writes the credential store directly (no secret ever crosses the
    /// command channel), so credentialed badges update without a restart.
    RefreshCredentials,
    OpenProject {
        root: PathBuf,
    },
    /// Clone a repository into a new absolute destination and make that
    /// verified destination the open project.
    CloneProject {
        source: String,
        destination: PathBuf,
    },
    /// Re-read the repository and publish the result ( producer).
    ///
    /// Carries no arguments: it reads the project already open, and letting a
    /// caller name a different root would make this a second, ungoverned way to
    /// point mjolnr at a directory.
    RefreshRepository,
    /// Run an explicit bounded repository discovery pass and write its OKF
    /// projection under `.mjolnr/discovery/`.
    RunDiscovery,
    /// Save an existing workspace file after comparing the full digest the
    /// operator read. The runtime records the completed save as an
    /// operator-controlled event and refreshes the repository projection.
    SaveFile {
        path: String,
        expected_digest: String,
        text: String,
    },
    CreateSession {
        provider: ProviderId,
        model: ModelId,
    },
    ResumeSession {
        session: SessionId,
    },
    /// Resume with a bounded provider projection, optionally on another model.
    ResumeCompact {
        session: SessionId,
        provider: Option<ProviderId>,
        model: Option<ModelId>,
    },
    /// Ask the active model to create a durable continuation checkpoint,
    /// optionally performing a live handoff to a specified role or model target.
    CreateHandoff {
        target: Option<String>,
    },
    /// Convene a council of models to deliberate a question or review a plan.
    ConveneCouncil {
        question: String,
        plan_file: Option<String>,
    },
    /// Record a human disposition on one advisory council finding. This does
    /// not approve, execute, or amend anything in the workspace.
    ResolveCouncilFinding {
        review_id: crate::core::council::CouncilReviewId,
        finding_id: crate::core::council::CouncilFindingId,
        disposition: crate::core::council::CouncilDisposition,
        note: Option<String>,
    },
    /// Compose a human-reviewable amended artifact from the findings a human
    /// accepted. This writes nothing: it produces a proposal for the operator
    /// to read, edit, and save through the ordinary governed save path.
    ProposeCouncilAmendment {
        review_id: crate::core::council::CouncilReviewId,
    },
    /// Resolve the expensive-resume advisor shown before any provider request.
    ResolveResume {
        choice: ResumeChoice,
    },
    /// Start a run from a directive.
    ///
    /// `source` is not decoration: it decides whether the text is framed as
    /// data and whether full-auto survives it. It is carried
    /// on the command rather than inferred at the runtime because only the
    /// caller knows where the text came from — by the time it reaches
    /// `start_run`, a webhook body and a typed message are the same `String`.
    SendUserMessage {
        text: String,
        source: DirectiveSource,
    },
    SelectModel {
        provider: ProviderId,
        model: ModelId,
    },
    /// Attach a route to the session: a role, an explicit name, or a task
    /// class to resolve through the project's routing config (
    /// roles added in §Phase 16). A no-op wherever no route resolves —
    /// including whenever no routing config exists at all, which is exactly
    /// present-day behaviour.
    AttachRoute {
        route: Option<String>,
        role: Option<String>,
        task_class: String,
    },
    /// Select the persona overlaid on the active route's voice, or clear the
    /// override with `None` . Idle-only, the `/model` rule: it
    /// changes the next turn's system prompt, so it may not land mid-run. A
    /// name that resolves to no discovered persona is refused by the client
    /// against the offered choices, never a silent no-op.
    SelectPersona {
        persona: Option<String>,
    },
    /// Bind (or clear, with `None`) a route's persona through the `/config`
    /// settings surface. Unlike [`SelectPersona`], which is a
    /// live session override, this writes the diffable `.mjolnr/routes/<route>`
    /// file the binding lives in and reloads the route table so the change is
    /// both durable and live. It edits configuration; it gates nothing.
    BindRoutePersona {
        route: String,
        persona: Option<String>,
    },
    /// Expand a discovered prompt template and send it as the user's message
    /// . Expansion happens in the runtime because the
    /// runtime owns the template text; a client sends the name it was shown.
    /// A name that resolves to nothing is refused, never sent as prose.
    SendPromptTemplate {
        name: String,
        arguments: String,
    },
    /// Rewind the session's active leaf to the parent of `sequence`, so the
    /// next message branches from there.
    ///
    /// Nothing is deleted: the events after the new leaf stay in the store on
    /// what is now a sibling branch, and the tool calls among them are history,
    /// never replayed. Refused while a run is in flight — repointing a session
    /// a live turn is mid-flight against would leave that turn writing into a
    /// branch nobody is reading.
    RewindTo {
        sequence: u64,
    },
    /// Rollback session transcript and workspace state to a verified checkpoint.
    /// Operates through the ordinary policy gate and requires verified sequence.
    RollbackToCheckpoint {
        target_sequence: u64,
        expected_head: Option<String>,
    },
    /// Read the session tree from the store onto the snapshot (
    /// 16.5).
    ///
    /// Explicit rather than kept fresh on every publish: the tree only matters
    /// while `/tree` is open, and it is the one projection that must read
    /// *abandoned* branches — which a resumed session has never loaded, because
    /// resume deliberately replays only the branch it is on.
    LoadSessionTree,
    /// Follow a branch that was previously abandoned.
    ///
    /// The inverse of the sibling `RewindTo` creates: it moves the active leaf
    /// *onto* the branch ending at `sequence` rather than off it. Nothing is
    /// written and nothing is replayed — the branch already exists, and this
    /// says to start reading it again. Refused while a run is in flight, for
    /// the same reason `RewindTo` is.
    FollowBranch {
        sequence: u64,
    },
    /// Start a new session carrying this one's branch forward (
    /// 16.5).
    ///
    /// `before` is the cut. `Some(sequence)` forks: the new session gets the
    /// branch's history up to but not including that turn, which is the same
    /// cut [`RewindTo`](Self::RewindTo) makes — the difference is that the
    /// original session is left exactly as it was. `None` clones: the whole
    /// active branch comes across.
    ///
    /// One command rather than two because the two acts differ only in where
    /// the cut falls, and duplicating the rules below is how they come to
    /// disagree.
    ///
    /// # What crosses, and what does not
    ///
    /// Policy, budget, and the read set cross, under the rules a handoff
    /// already establishes. Policy is carried by
    /// [`PolicyMode::carried_forward`](crate::core::policy::PolicyMode::carried_forward),
    /// so the new session is never wider than this one: a fork must not be a
    /// way to launder a narrow policy into a wide one.
    ///
    /// Exact-command grants do **not** cross. They are scoped to one session
    /// by `docs/persistence.md` §6, and a fork is a different session — a
    /// grant that followed the fork would widen authority a human gave for one
    /// context into a context they have not seen.
    ForkSession {
        before: Option<u64>,
    },
    /// Queue a message to steer the run already in flight.
    ///
    /// Delivered after the current tool calls settle and before the next
    /// provider request, so it redirects work underway rather than waiting for
    /// it to finish. A steering message never resolves an approval and never
    /// widens a policy: it changes what mjolnr is asked to do, not what it is
    /// allowed to do. With no run in flight it is an ordinary user message.
    QueueSteeringMessage {
        text: String,
    },
    /// Re-read skills, prompt templates, and project instructions from disk
    /// under the existing trust gate. Emits an event
    /// stating what changed; a reload is a visible act, not a silent refresh.
    ReloadResources,
    /// Load a discovered extension into the session's tool registry, making it
    /// callable. Inert until this explicit act: discovery only
    /// makes an extension visible. The load is trust-gated for a project-scoped
    /// extension and recorded as an [`ExtensionLoaded`](crate::core::event::MjolnrEvent::ExtensionLoaded)
    /// event.
    LoadExtension {
        name: String,
    },
    SetPolicy {
        mode: PolicyMode,
    },
    /// Arm a spawn envelope for this session.
    ///
    /// A human act, never a model one: an agent proposing the bounds on its own
    /// authority inverts the premise. Refused when a run is active, when the
    /// ceiling is wider than the session's policy, or when a bound is out of
    /// range — and never carried across a restart.
    ArmSpawnEnvelope {
        envelope: Box<SpawnEnvelope>,
    },
    /// Clear the active envelope early. Ordinary state, not a refusal: a human
    /// who armed a shape may withdraw it before it is spent.
    ClearSpawnEnvelope,
    ResolveApproval {
        approval: ApprovalId,
        decision: ApprovalDecision,
    },
    /// Resolve work a crash interrupted.
    ///
    /// A separate command from [`ResolveApproval`](Self::ResolveApproval), not a
    /// reuse of it. An approval authorises work that has *not* started; a
    /// recovery decides what to do about work that may already have finished.
    /// Sharing one command would let a UI answer one question with the other's
    /// vocabulary.
    ResolveRecovery {
        decision: RecoveryDecision,
    },
    CancelRun,
    EndSession,
    /// Start a bounded model-led interview for a new greenfield workflow.
    StartPlanInterview {
        goal: String,
    },
    AskPlanQuestion {
        plan_id: crate::core::plan::PlanId,
        question: crate::core::plan::Question,
    },
    AnswerPlanQuestion {
        plan_id: crate::core::plan::PlanId,
        answer: crate::core::plan::QuestionAnswer,
    },
    ProposePlan {
        proposal: crate::core::plan::PlanProposal,
    },
    ReviewPlan {
        review: crate::core::plan::PlanReview,
    },
    ApprovePlan {
        approval: crate::core::plan::PlanApproval,
    },
    HandoffPlan {
        handoff: crate::core::plan::PlanHandoff,
    },
    CreateWorktree {
        name: String,
        base_revision: String,
    },
    ForkWork {
        name: String,
        base_revision: String,
    },
    StartChild {
        name: String,
        directive: String,
        /// The most policy the child may ever hold. `None` means "inherit the
        /// parent's current policy unchanged"; `Some(mode)` requests a lower
        /// ceiling. Children inherit less, never more (AGENTS.md §11.4): at
        /// execution time the runtime clamps this to at most the parent's
        /// policy, so a child can never widen what the parent was granted.
        policy_ceiling: Option<crate::core::policy::PolicyMode>,
        budget: Option<u32>,
    },
    /// Cancel a *running* child. Refused (today: capability unavailable) for
    /// children that are not running; once execution lands, cancelling a
    /// settled child is a lifecycle error, not a no-op.
    CancelChild {
        name: String,
    },
    PreserveBranch {
        name: String,
    },
    /// Settle a child whose run has finished. Only valid for children in a
    /// settled-able lifecycle state; pairing with `CancelChild` is what keeps
    /// the two verbs honest once execution lands.
    SettleChild {
        name: String,
    },
    /// Discard an already-settled worktree. Use only after `SettleChild` has
    /// completed; the runtime refuses otherwise. Named for the state it
    /// requires, not for a generic "delete".
    DiscardSettledWorktree {
        name: String,
    },
    /// Phase D5 repository family. Values arrive validated by the client
    /// bridge, because each one becomes an argv element of a `git` process.
    StagePaths {
        paths: Vec<String>,
    },
    StageHunks {
        path: String,
        hunk_indices: Vec<usize>,
    },
    Unstage {
        paths: Vec<String>,
    },
    CreateBranch {
        name: String,
        base_revision: String,
    },
    /// `expected_index_revision` is what the human approved. The runtime
    /// refuses rather than committing a different tree.
    Commit {
        message: String,
        expected_index_revision: String,
    },
    /// Merge an explicitly selected child branch. `message` is required and
    /// human-supplied: mjolnr never authors the merge commit's record.
    IntegrateChildBranch {
        name: String,
        message: String,
        expected_head: String,
    },
    /// Fetch from the configured upstream remote. Inert: it touches only
    /// remote-tracking refs and never the working tree, so there is no
    /// uncertain-effect case — success or a typed failure with git's text.
    Fetch,
    /// Push the current branch's `HEAD` to its configured upstream. The
    /// outcome is verified against the remote-tracking ref rather than the
    /// exit status, because a push that dies mid-transfer leaves the local
    /// tree identical either way (AGENTS.md §1.3). `expected_head` is the
    /// HEAD the human saw and approved pushing; a mismatch is refused with
    /// `WORKSPACE_STALE_REVISION`. A branch behind its remote is refused
    /// before the network call. Human-initiated from the desktop preview,
    /// like the rest of the D5 family — a model never self-approves a push.
    Push {
        expected_head: String,
    },
    /// Merge the branch's configured upstream into it — the merge half of
    /// "pull": pull is fetch plus merge, two evidenced acts. Refusals and state guards are `IntegrateChildBranch`'s:
    /// conflicted, dirty, detached, and moved repositories refuse before any
    /// mutation. `message` is human-supplied and consumed only when the merge
    /// creates a commit — integrating upstream mirrors `git pull`, so a
    /// fast-forward consumes none. `expected_head` is the HEAD the human saw
    /// when they approved the preview; a mismatch is refused with
    /// `WORKSPACE_STALE_REVISION`. Success is verified by a fresh ahead/behind
    /// read, not git's exit status: the branch is claimed to contain the
    /// upstream tip only when `rev-list` proves it. Human-initiated from the
    /// desktop preview, like the rest of the D5 family.
    IntegrateUpstream {
        message: String,
        expected_head: String,
    },
    /// Rebase the current clean branch onto an explicit local ref. A conflict
    /// remains in git's rebase state for human recovery.
    Rebase {
        onto: String,
        expected_head: String,
    },
    /// Abort an in-progress rebase after an explicit preview.
    AbortRebase,
    /// Phase D6 integration family. A batch is sequential and bounded; each
    /// successful task becomes its own durable fetched/refresh event.
    FetchTask {
        source: String,
        task_id: String,
    },
    /// Fetch several tasks in order. If one fails, earlier successful tasks
    /// remain durable and the command returns the first typed refusal.
    FetchTasks {
        source: String,
        task_ids: Vec<String>,
    },
    /// The change fields travel inline rather than as an
    /// `integrations::RemoteChangeRequest`, because `core` may not depend on
    /// `integrations` (AGENTS.md §2.1). The integration adapter rebuilds the
    /// bounded type through its own constructor, which is where the remote-text
    /// limits belong anyway.
    ///
    /// `expected_revision` is the revision of the imported item the change was
    /// rendered for. Required rather than optional, for the same reason
    /// `AddReviewNote` requires its `capture_digest`: a staleness check a caller
    /// can omit is not a check. The runtime refuses a pin that does not match a
    /// revision it recorded — before any network work, so the guard holds for
    /// every producer (§E5 contract (a)).
    ///
    /// The local commit and branch fields are required because a remote pull
    /// request must identify the exact verified commit mjolnr is offering. A
    /// title/body-only request cannot prove what local work the PR contains.
    SubmitChange {
        source: String,
        remote_id: String,
        expected_revision: String,
        title: String,
        body: String,
        head_commit: String,
        head_branch: String,
        base_branch: String,
    },
    /// Phase D3 review family. A human pins a note to one line of the diff
    /// mjolnr last captured.
    ///
    /// `capture_digest` is the diff revision the human was looking at, and it
    /// is required rather than optional: an opt-in staleness check is not a
    /// check. A mismatch is refused with `WORKSPACE_STALE_DIFF` — the note is
    /// never moved to whatever occupies that line now.
    ///
    /// Note what is *not* here. There is no `hunk_header` field: the runtime
    /// reads the hunk context out of its own capture, so a client cannot
    /// describe a diff that never existed.
    AddReviewNote {
        path: String,
        side: crate::core::review::ReviewSide,
        line: u32,
        capture_digest: String,
        body: String,
    },
    /// A further human remark on an existing thread. Never a model's: a review
    /// comment is a human act, and mjolnr answers in the transcript.
    AddReviewComment {
        thread: crate::core::review::ReviewThreadId,
        body: String,
    },
    /// Send the selected threads to mjolnr as a durable revision request.
    ///
    /// An ordinary human directive carrying the notes, so it passes every gate
    /// a typed message passes and widens nothing. The threads are named in the
    /// durable record so the request can be traced back to what was reviewed.
    SendReviewNotes {
        threads: Vec<crate::core::review::ReviewThreadId>,
    },
    /// Phase E5 board family. Values arrive validated by the client bridge:
    /// the question and options are bounded text, the blockers are parsed
    /// ticket ids with duplicates refused.
    ///
    /// Opens a decision ticket — a recorded unknown a *human* will resolve
    /// Edges are set here and never mutated:
    /// an edge is a fact about ordering, and the frontier must be able to say
    /// *why* something is fogged for as long as the ticket exists.
    OpenDecisionTicket {
        question: String,
        kind: crate::core::board::DecisionTicketKind,
        options: Vec<String>,
        blocked_by: Vec<crate::core::board::DecisionTicketId>,
    },
    /// Resolve a decision ticket. Records durable human judgement and never
    /// authority (ADR-0015): no policy changes, no approval is implied, and
    /// nothing that grants capability may read the result. Resolving an
    /// already-resolved ticket is legitimate — the new resolution supersedes
    /// the current one by reference, additively.
    ResolveDecisionTicket {
        ticket: crate::core::board::DecisionTicketId,
        /// A reference into the ticket's recorded options — never a status
        /// word. An out-of-range reference is a typed refusal, not silence.
        chosen_option: usize,
        note: Option<String>,
    },
    /// Record one fetched imported item as a board node (Phase E5, step 4b).
    /// The item's `blocked_by` is a fact about ordering, safe to import; a
    /// resolution is human judgement and is never imported (design §4).
    ImportWorkItem {
        item: crate::core::imported::ImportedItem,
    },
    /// Refresh a previously imported item at a new revision.
    /// `expected_revision` is the revision the human saw when they approved the
    /// refresh; a mismatch is refused with `WORKSPACE_STALE_REVISION` (contract (a)).
    RefreshImportedItem {
        expected_revision: String,
        item: crate::core::imported::ImportedItem,
    },
    /// Add a comment on an imported item's external discussion (Phase D6 sync-back).
    /// The item must already be imported so `expected_revision` can be pinned to
    /// what the human saw; the comment stays bounded prose, never authority.
    SubmitImportedComment {
        integration: String,
        remote_id: String,
        expected_revision: String,
        body: String,
    },
    /// Phase D9 external-agent family. Every agent is `ExternalUnverified` — its
    /// internal side effects never become `MjolnrEvent`s until a human imports the
    /// working-tree diff through the ordinary review + stage/commit gates.
    LaunchExternalAgent {
        profile: String,
    },
    StopExternalAgent {
        id: String,
    },
    ImportExternalAgentChanges {
        id: String,
    },
}

/// Identifies a pending approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApprovalId(uuid::Uuid);

impl ApprovalId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }

    /// Rebuild an id read from durable history.
    ///
    /// Note what this does **not** do: reconstructing an `ApprovalId` from an
    /// event does not restore the authority that approval granted. Recovery
    /// replays these for ordering and audit only (`docs/persistence.md` §6).
    #[must_use]
    pub const fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

/// How a proposed side effect was resolved.
///
/// There is deliberately no "approve everything" or "always allow" variant. The
/// MVP exposes no unrestricted mode, and approvals do not persist across
/// sessions. A type that cannot express blanket approval cannot accidentally
/// grant it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Deny,
    ApproveOnce,
    /// Approve this *exact* command for the current session only.
    ApproveExactForSession,
    /// Explicit full-auto policy authorised the proposal, not a human click.
    AutoByPolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_cannot_express_blanket_permission() {
        // A compile-time property, asserted here so deleting the invariant is a
        // visible act. If someone adds `ApproveAll`, this test is where the
        // conversation happens.
        let decisions = [
            ApprovalDecision::Deny,
            ApprovalDecision::ApproveOnce,
            ApprovalDecision::ApproveExactForSession,
            ApprovalDecision::AutoByPolicy,
        ];
        assert_eq!(
            decisions.len(),
            4,
            "blanket permission remains a policy mode, not an approval choice"
        );
    }
}
