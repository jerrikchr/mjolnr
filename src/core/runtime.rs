//! The runtime boundary.
//!
//! This trait is the entire surface a client gets. The TUI holds one of these
//! and nothing else — no provider, no store, no tool.
//!
//! > "The TUI reduces `MjolnrEvent` values into view state and sends commands
//! > back. It cannot hold the authoritative session transcript." —
//!
//! The snapshot/subscribe split is what makes that true in practice: a client
//! gets a *copy* of state to render and a *feed* of events to reduce, but the
//! runtime owns the truth. `tests/architecture.rs` enforces the import
//! direction; this trait is why that direction is possible at all.

use std::sync::Arc;

use async_trait::async_trait;

use crate::core::command::MjolnrCommand;
use crate::core::context::{
    ContextDiagnostic, ExtensionLoadReport, ExtensionSummary, PersonaSummary, PromptSummary,
    ReloadReport, SkillSummary,
};
use crate::core::continuation::{HandoffCheckpoint, QuotaReserveStatus, ResumeAdvice};
use crate::core::error::MjolnrError;
use crate::core::event::{MjolnrEvent, SessionId};
use crate::core::mcp::McpServerSummary;
use crate::core::message::TranscriptEntry;
use crate::core::model::{ModelDescriptor, ModelId, ProviderId, Usage};
use crate::core::policy::{PendingApproval, PolicyMode};
use crate::core::recovery::RecoveryState;
use crate::core::routing::{BreakerView, RouteRuntime};
use crate::core::trigger::TriggerStatus;

/// A point-in-time view of the runtime, for rendering.
///
/// `messages` is `Arc`-shared, not deep-copied. A frame render that clones the
/// whole transcript is O(n) per frame and therefore O(n²) over a session — the
/// classic TUI death (AGENTS.md §5). Cloning a `RuntimeSnapshot` bumps a
/// refcount and nothing more.
#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub session: Option<SessionId>,
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
    /// The active branch's transcript, each entry anchored to the durable
    /// event that introduced it. A client reads the message
    /// through the entry's `Deref`; it reads `sequence` only when it needs to
    /// name a point in history, which today means `/tree` choosing a rewind
    /// target.
    pub messages: Arc<Vec<TranscriptEntry>>,
    /// The session tree, abandoned branches included, as of the last
    /// `LoadSessionTree` .
    ///
    /// Empty means "not loaded", not "no branches" — a reader that treats the
    /// two as the same will tell the user their history is linear when it may
    /// not be.
    pub tree: Arc<Vec<crate::core::store::SessionTreeNode>>,
    /// What happened on the branch most recently switched away from (plan
    /// §Phase 16.5).
    ///
    /// A projection of recorded events, never a model's summary — see
    /// [`BranchSummary`](crate::core::store::BranchSummary). Set by a rewind or
    /// a follow, so the work being left behind is stated rather than silently
    /// dropped off the screen.
    pub left_branch: Option<crate::core::store::BranchSummary>,
    pub run_active: bool,
    pub usage: Usage,
    pub workspace_root: Option<std::path::PathBuf>,
    pub policy: PolicyMode,
    /// The spawn envelope in force, if one is armed.
    ///
    /// Carries what remains rather than a running tally, so a client shows the
    /// authorisation's current state without reconstructing it from events.
    pub envelope: Option<crate::core::envelope::ActiveEnvelope>,
    /// Why the last arming attempt was refused.
    pub envelope_refusal: Option<String>,
    pub pending_approval: Option<PendingApproval>,
    pub budget: BudgetStatus,
    /// Whether a crash left work whose outcome mjolnr cannot establish.
    ///
    /// The TUI renders this and blocks the composer on it. It travels in the
    /// snapshot rather than being inferred from the event feed because a client
    /// that joins late — or resyncs after lagging — must still learn that the
    /// session is blocked. A guard a subscriber can miss is not a guard.
    pub recovery: RecoveryState,
    /// Set when a durable write failed.
    ///
    /// Separate from [`recovery`](Self::recovery) because they are different
    /// facts with different exits: interrupted work is resolved by a human
    /// decision, while a store that rejected a write needs the store fixed.
    /// Merging them into one "blocked" flag would offer the wrong remedy for
    /// one of them.
    pub store_failure: Option<String>,
    pub skills: Arc<Vec<SkillSummary>>,
    /// Prompt templates discovered for this project.
    pub prompts: Arc<Vec<PromptSummary>>,
    /// Extensions discovered but not yet loaded. Visible so a
    /// client can offer them to load; each is inert until an explicit load act.
    pub extensions: Arc<Vec<ExtensionSummary>>,
    /// The most recent `/reload` and what it found, if one has run.
    pub last_reload: Option<ReloadReport>,
    /// The most recent load-extension act and its outcome.
    pub last_extension_load: Option<ExtensionLoadReport>,
    /// The most recent bounded repository discovery projection, if one ran.
    /// The durable source is the OKF bundle named by the report.
    pub last_discovery: Option<crate::core::discovery::DiscoveryReport>,
    /// The most recent completed advisory council review, if one ran.
    /// This is a client projection; it never grants authority.
    pub last_council: Option<crate::core::council::CouncilReview>,
    /// The latest human-reviewable amendment composed from accepted findings.
    /// A proposal, never a write: the editor save path re-checks the digest.
    pub last_council_amendment: Option<crate::core::council::CouncilAmendment>,
    pub activated_skills: Arc<Vec<String>>,
    pub context_diagnostics: Arc<Vec<ContextDiagnostic>>,
    pub workspace_trusted: bool,
    pub handoff: Option<HandoffCheckpoint>,
    pub quota_reserve: QuotaReserveStatus,
    /// The full multi-window quota snapshot behind `quota_reserve`'s single
    /// worst window. `None` until a provider has reported quota at
    /// least once this session.
    pub quota: Option<crate::core::model::QuotaSnapshot>,
    pub resume_advice: Option<ResumeAdvice>,
    pub mcp_servers: Arc<Vec<McpServerSummary>>,
    /// Configured triggers, their overlap policy, and last-known outcome (plan
    /// §Phase 14). Computed once at startup from `.mjolnr/triggers/`, the same
    /// way [`mcp_servers`](Self::mcp_servers) is computed once from
    /// `.mjolnr/mcp.yaml` — a client renders it, it does not poll for it.
    pub triggers: Arc<Vec<TriggerStatus>>,
    /// This session's live position on an attached route, if any (plan
    /// §Phase 15). `None` when no route is attached — including whenever no
    /// routing config exists, which is exactly present-day behaviour.
    pub route: Option<RouteRuntime>,
    /// Live breaker state for every provider the attached route has touched,
    /// for the `/usage` overlay.
    pub breakers: Arc<Vec<BreakerView>>,
    /// Every provider/model pair whose live catalog discovery succeeded.
    ///
    /// Refreshed by the runtime from the provider registry. The TUI may not
    /// import `providers` (see `src/tui/mod.rs`), so a picker has to be handed
    /// its choices rather than discovering them. Disconnected providers remain
    /// visible through [`providers`](Self::providers), while every row here is
    /// actionable.
    pub models: Arc<Vec<ModelChoice>>,
    /// Connection and catalog-discovery status for every registered provider.
    ///
    /// `/auth` renders this complete list; `/model` renders only
    /// [`models`](Self::models).
    pub providers: Arc<Vec<ProviderConnection>>,
    /// Every route the project's `.mjolnr/routes/` declares, with the role tags
    /// it answers to and the provider/model of its first hop (
    /// roles §Phase 16). Computed once from the loaded `RouteTable`, exactly
    /// like [`triggers`](Self::triggers) and [`models`](Self::models): the TUI
    /// may not import `routing`, so a `/route`/`/role` picker has to be handed
    /// its choices rather than discover them. Empty whenever no routing config
    /// exists, which is exactly present-day behaviour.
    pub routes: Arc<Vec<RouteChoice>>,
    /// Personas discovered under `.mjolnr/personas/` and the user config dir
    /// , for a `/persona` picker. Computed once from the
    /// project context, exactly like [`prompts`](Self::prompts). Empty when no
    /// persona files exist.
    pub personas: Arc<Vec<PersonaSummary>>,
    /// The persona name the session has explicitly selected, overriding
    /// whatever the active route would wear. `None` means no
    /// override — the active route's own persona (if any) applies.
    pub active_persona: Option<String>,
    /// The Soul/profile files in effect, labelled for `/soul` to display. A
    /// view of the record; selecting nothing here changes no behaviour.
    pub souls: Arc<Vec<String>>,
    /// Bounded recent sessions for the current project root ( / A0).
    pub sessions: Arc<Vec<crate::core::store::SessionSummary>>,
    /// Authoritative plan workflow state.
    pub plan: Option<crate::core::plan::PlanWorkflow>,
    /// What git last said about the open project ( producer).
    ///
    /// Re-read on the explicit triggers in
    /// [`RefreshTrigger`](crate::core::repository::RefreshTrigger) and never on
    /// a timer, so it carries the moment it was captured rather than a claim to
    /// be current. `git status` is a subprocess; running one per snapshot
    /// publish would put a process spawn on the hot path of every token.
    pub repository: crate::core::repository::RepositoryView,
    /// The exact diffs behind `repository` ( producer), captured
    /// in the same refresh and carrying the same `capture_sequence`. A client
    /// that renders one without the other is showing half a moment.
    pub changes: crate::core::change_capture::ChangeView,
    /// Which durable tool event recorded each file this session read (plan
    /// §Phase D3). Accumulated from `ToolCompleted` events, never from the read
    /// set, because only the event carries the id
    /// [`ReadBeforeEditEvidence`](crate::core::changes::ReadBeforeEditEvidence)
    /// asks for. Ordered by path — it is projected onto a change set, and a
    /// list that reordered itself between publishes would move under a reader.
    pub read_evidence: Arc<Vec<crate::core::change_capture::ReadRecord>>,
    /// Line notes pinned to a diff, oldest first.
    ///
    /// On the snapshot rather than reduced from the event feed, for the reason
    /// `recovery` is: a client that joined late or lagged must still see every
    /// note, and a review surface built from a feed a subscriber can miss would
    /// silently drop one. Each thread carries the anchor it was taken against;
    /// whether that anchor is stale is decided at the projection boundary by
    /// comparing it with `changes`, never by moving it.
    pub review_threads: Arc<Vec<crate::core::review::ReviewThread>>,
    /// Summary of the workspace memory state.
    pub memory: Arc<crate::core::memory::MemorySummary>,
    /// Summary of discovered third-party plugins.
    pub plugins: Arc<Vec<crate::core::plugin::PluginSummary>>,
    /// Live multi-agent fleet roster summary.
    pub fleet: Arc<crate::core::fleet::FleetSummary>,
    /// Live Studio Preview canvas projection.
    pub preview: Arc<crate::core::preview::PreviewState>,
    /// External-agent worktrees (Phase D9) — always `ExternalUnverified`.
    pub external_agents: Vec<crate::core::client::external_agent::ExternalAgentView>,
    pub external_agent_capability: crate::core::client::external_agent::ExternalAgentCapability,
}

/// One selectable route: its name, the roles it answers to, and where its
/// first hop points. A client renders and offers it; resolving the actual
/// attachment stays in the runtime (`AttachRoute`), which owns the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteChoice {
    pub name: String,
    pub roles: Vec<String>,
    pub provider: ProviderId,
    pub model: ModelId,
    /// The persona this route currently binds, if any (/27). The
    /// `/config` surface renders and edits it; `None` means the route runs the
    /// Soul alone.
    pub persona: Option<String>,
}

/// One selectable provider/model pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub descriptor: ModelDescriptor,
}

/// What mjolnr currently knows about one provider's usability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderConnectionState {
    Disconnected,
    Discovering,
    Connected,
    NeedsReauth,
    Unavailable,
}

impl ProviderConnectionState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "DISCONNECTED",
            Self::Discovering => "DISCOVERING",
            Self::Connected => "CONNECTED",
            Self::NeedsReauth => "NEEDS REAUTH",
            Self::Unavailable => "UNAVAILABLE",
        }
    }
}

/// Provider status rendered by `/auth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConnection {
    pub provider: ProviderId,
    pub state: ProviderConnectionState,
    /// Sanitized remedy or failure summary. Never credential material.
    pub detail: Option<String>,
}

/// Live budget counters shown to clients without exposing runtime internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetStatus {
    pub provider_turns: u32,
    pub max_provider_turns: u32,
    pub tool_calls: u32,
    pub max_tool_calls: u32,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            session: None,
            provider: None,
            model: None,
            messages: Arc::new(Vec::new()),
            tree: Arc::new(Vec::new()),
            left_branch: None,
            run_active: false,
            usage: Usage::default(),
            workspace_root: None,
            envelope: None,
            envelope_refusal: None,
            policy: PolicyMode::default(),
            pending_approval: None,
            budget: BudgetStatus::default(),
            recovery: RecoveryState::default(),
            store_failure: None,
            skills: Arc::new(Vec::new()),
            prompts: Arc::new(Vec::new()),
            extensions: Arc::new(Vec::new()),
            last_reload: None,
            last_extension_load: None,
            last_discovery: None,
            last_council: None,
            last_council_amendment: None,
            activated_skills: Arc::new(Vec::new()),
            context_diagnostics: Arc::new(Vec::new()),
            workspace_trusted: false,
            handoff: None,
            quota_reserve: QuotaReserveStatus::default(),
            quota: None,
            resume_advice: None,
            mcp_servers: Arc::new(Vec::new()),
            triggers: Arc::new(Vec::new()),
            route: None,
            breakers: Arc::new(Vec::new()),
            models: Arc::new(Vec::new()),
            providers: Arc::new(Vec::new()),
            routes: Arc::new(Vec::new()),
            personas: Arc::new(Vec::new()),
            active_persona: None,
            souls: Arc::new(Vec::new()),
            sessions: Arc::new(Vec::new()),
            plan: None,
            repository: crate::core::repository::RepositoryView::NoProject,
            changes: crate::core::change_capture::ChangeView::NoProject,
            read_evidence: Arc::new(Vec::new()),
            review_threads: Arc::new(Vec::new()),
            memory: Arc::default(),
            plugins: Arc::new(Vec::new()),
            fleet: Arc::default(),
            preview: Arc::default(),
            external_agents: Vec::new(),
            external_agent_capability:
                crate::core::client::external_agent::ExternalAgentCapability {
                    available: false,
                    reason: Some("external-agent profiles not yet loaded".to_owned()),
                },
        }
    }
}

/// A live feed of runtime events.
///
/// Bounded by construction. A subscriber that falls behind is told so via
/// [`RecvError::Lagged`](tokio::sync::broadcast::error::RecvError::Lagged)
/// rather than being allowed to consume unbounded memory — losing render deltas
/// is acceptable, losing bounded memory is not.
#[derive(Debug)]
pub struct RuntimeSubscription {
    receiver: tokio::sync::broadcast::Receiver<MjolnrEvent>,
}

impl RuntimeSubscription {
    #[must_use]
    pub fn new(receiver: tokio::sync::broadcast::Receiver<MjolnrEvent>) -> Self {
        Self { receiver }
    }

    /// Await the next event.
    ///
    /// Returns `Err(Lagged(n))` when the subscriber missed `n` events, and
    /// `Err(Closed)` when the runtime shut down. Callers must handle `Lagged`
    /// by resyncing from [`MjolnrRuntime::snapshot`] rather than pretending the
    /// gap did not happen.
    pub async fn recv(&mut self) -> Result<MjolnrEvent, tokio::sync::broadcast::error::RecvError> {
        self.receiver.recv().await
    }
}

/// A feed of state changes.
///
/// # Why this exists alongside [`RuntimeSubscription`]
///
/// They answer different questions. The subscription says *what happened*; this
/// says *what is now true*. A client needs both, and must not infer either from
/// the other.
///
/// Phase 4 found out why the hard way. The TUI used to re-read
/// [`MjolnrRuntime::snapshot`] whenever a durable event arrived, which works
/// right up until state changes with no event to announce it — a resumed session
/// restores an entire transcript and, if nothing was interrupted, emits nothing
/// at all. The screen stayed empty and the runtime was fine: the view was
/// waiting for a message that was never coming. Watching the state directly
/// removes the guess.
#[derive(Debug)]
pub struct SnapshotStream {
    receiver: tokio::sync::watch::Receiver<RuntimeSnapshot>,
}

impl SnapshotStream {
    #[must_use]
    pub fn new(receiver: tokio::sync::watch::Receiver<RuntimeSnapshot>) -> Self {
        Self { receiver }
    }

    /// Await the next state, or `Err` once the runtime is gone.
    ///
    /// Lossless in the way that matters: `watch` keeps only the newest value, so
    /// a slow client skips intermediate states but never misses the current one.
    /// Intermediate *states* are not history — that is what the event feed is
    /// for — so coalescing them is correct rather than merely cheap.
    pub async fn changed(
        &mut self,
    ) -> Result<RuntimeSnapshot, tokio::sync::watch::error::RecvError> {
        self.receiver.changed().await?;
        Ok(self.receiver.borrow_and_update().clone())
    }
}

/// The runtime a client drives.
#[async_trait]
pub trait MjolnrRuntime: Send + Sync + std::fmt::Debug {
    /// Current state, for an initial render or a resync after lag.
    fn snapshot(&self) -> RuntimeSnapshot;

    /// Watch state as it changes.
    fn snapshots(&self) -> SnapshotStream;

    /// Subscribe to the event feed.
    fn subscribe(&self) -> RuntimeSubscription;

    /// Submit an intent. Returns once the command is accepted, **not** once its
    /// effects complete — a `SendUserMessage` returns before the model answers.
    /// Progress arrives on the subscription.
    async fn dispatch(&self, command: MjolnrCommand) -> Result<(), MjolnrError>;

    /// Search across the workspace index.
    async fn search_workspace(
        &self,
        filter: crate::core::store::WorkspaceSearchFilter,
    ) -> Result<crate::core::store::WorkspaceSearchPage, MjolnrError>;

    /// Read one contained directory page or file from the open project
    /// .
    ///
    /// A query rather than a command, and shaped like `search_workspace` for
    /// the same reason: the caller wants an answer, not a state change, and a
    /// projection carried on the snapshot would publish one client's current
    /// directory to every other client watching the same session.
    async fn read_workspace_files(
        &self,
        request: crate::core::workspace_files::WorkspaceFileRequest,
    ) -> Result<crate::core::workspace_files::WorkspaceFileAnswer, MjolnrError>;

    /// Read the board: what is decidable right now (Phase E5, step 3).
    ///
    /// A cross-session read projection: decision tickets and plans from every
    /// session whose project root is the open workspace, reduced into the
    /// frontier, fog, and settled sets — each fogged node carrying the
    /// unresolved blockers that answer "why is this not decidable". A query,
    /// not a command: it never mutates state, and it refuses with
    /// `WorkspaceCapabilityUnavailable` when no workspace is open rather than
    /// claiming an empty board.
    async fn query_board(&self) -> Result<crate::core::frontier::BoardOverview, MjolnrError>;

    /// Read a bounded newest-first history for the open repository.
    async fn query_repository_history(
        &self,
        limit: u32,
    ) -> Result<crate::core::repository::RepositoryHistory, MjolnrError>;

    /// Shut down, flushing any pending durable writes.
    async fn close(&self) -> Result<(), MjolnrError>;
}
