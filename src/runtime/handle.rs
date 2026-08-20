//! Public runtime handle and actor construction.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::context::ProjectContext;
use crate::core::command::MjolnrCommand;
use crate::core::error::{MjolnrError, ReasonCode};
use crate::core::mcp::McpServerSummary;
use crate::core::provider::Provider;
use crate::core::recovery::RecoveryState;
use crate::core::routing::RouteTable;
use crate::core::runtime::{MjolnrRuntime, RuntimeSnapshot, RuntimeSubscription, SnapshotStream};
use crate::core::store::EventStore;
use crate::runtime::budget::BudgetLimits;
use crate::runtime::session::SessionState;
use crate::tools::ToolRegistry;

use super::{Actor, EVENT_CAPACITY, MAILBOX_CAPACITY, Mail};

/// Handle to a running runtime.
#[derive(Debug)]
pub struct Runtime {
    mailbox: mpsc::Sender<Mail>,
    events: broadcast::Sender<crate::core::event::MjolnrEvent>,
    snapshot: watch::Receiver<RuntimeSnapshot>,
    shutdown: CancellationToken,
    store: Arc<dyn EventStore>,
}

impl Runtime {
    /// Spawn the actor.
    ///
    /// Providers are injected rather than registered globally:
    /// forbids a global mutable singleton registry, and injection is also what
    /// makes the headless integration tests possible.
    #[must_use]
    pub fn spawn(providers: Vec<Arc<dyn Provider>>, store: Arc<dyn EventStore>) -> Self {
        Self::spawn_configured(
            providers,
            store,
            ToolRegistry::builtins(),
            BudgetLimits::default(),
            ProjectContext::empty(),
            Arc::new(Vec::new()),
            None,
            true,
            Arc::new(Vec::new()),
            Arc::new(RouteTable::default()),
        )
    }

    #[must_use]
    pub fn spawn_with_project_context(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        context: ProjectContext,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            ToolRegistry::builtins(),
            BudgetLimits::default(),
            context,
            Arc::new(Vec::new()),
            None,
            true,
            Arc::new(Vec::new()),
            Arc::new(RouteTable::default()),
        )
    }

    /// Seam for supplying the integration producer rather than building it from
    /// the environment (Phase D6).
    ///
    /// The normal binary uses [`spawn`](Self::spawn), where `fetchTask`
    /// constructs a `GitHubSource` from `GITHUB_TOKEN`. A test supplies a source
    /// pointed at a local mock instead, so the whole command path can be
    /// exercised without touching the network *and* without mutating the process
    /// environment — a mutation that races every other test in the binary and
    /// that this repository's `unsafe` lint forbids outright.
    ///
    /// The supplied source answers only for the integration id it reports.
    #[must_use]
    pub fn spawn_with_task_source(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        task_source: Arc<dyn crate::integrations::TaskSource>,
    ) -> Self {
        let runtime = Self::spawn(providers, store);
        // A freshly spawned runtime's mailbox is empty, so this cannot fail in
        // practice. If it ever did, `fetchTask` would fall back to building a
        // source from the environment and refuse for a missing credential —
        // loudly, not by silently using the wrong producer.
        let _ = runtime
            .mailbox
            .try_send(super::Mail::SetTaskSource { task_source });
        runtime
    }

    /// Test/configuration seam for bounded tools and budgets. The normal binary
    /// uses [`spawn`](Self::spawn) and the built-in registry.
    #[must_use]
    pub fn spawn_with(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        tools: ToolRegistry,
        limits: BudgetLimits,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            tools,
            limits,
            ProjectContext::empty(),
            Arc::new(Vec::new()),
            None,
            true,
            Arc::new(Vec::new()),
            Arc::new(RouteTable::default()),
        )
    }

    /// Composition seam for explicitly configured external tools.
    #[must_use]
    pub fn spawn_with_tools_and_project_context(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        tools: ToolRegistry,
        context: ProjectContext,
        mcp_servers: Arc<Vec<McpServerSummary>>,
        route_table: Arc<RouteTable>,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            tools,
            BudgetLimits::default(),
            context,
            mcp_servers,
            None,
            true,
            Arc::new(Vec::new()),
            route_table,
        )
    }

    /// Test/configuration seam for a project's routing table ,
    /// without the rest of the interactive composition.
    #[must_use]
    pub fn spawn_with_routes(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        route_table: Arc<RouteTable>,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            ToolRegistry::builtins(),
            BudgetLimits::default(),
            ProjectContext::empty(),
            Arc::new(Vec::new()),
            None,
            true,
            Arc::new(Vec::new()),
            route_table,
        )
    }

    /// Host a subagent session: same runtime, same store, linked identity.
    ///
    /// Used only by the Phase 13 orchestrator. The child's tool registry and
    /// budget slice are the parent's decision, passed in already clamped. A
    /// subagent host never registers `spawn_subagent`: fan-out depth is capped
    /// at one structurally, not by a rule a prompt could argue with.
    #[must_use]
    pub(crate) fn spawn_subagent_host(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        tools: ToolRegistry,
        limits: BudgetLimits,
        link: super::ChildLink,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            tools,
            limits,
            ProjectContext::empty(),
            Arc::new(Vec::new()),
            Some(link),
            false,
            Arc::new(Vec::new()),
            Arc::new(RouteTable::default()),
        )
    }

    /// Host a scheduled trigger firing: an ordinary session — full project
    /// context, full MCP catalogue, `spawn_subagent` available — linked to the
    /// trigger's control session so its parentage is visible.
    ///
    /// Unlike [`spawn_subagent_host`](Self::spawn_subagent_host) this keeps
    /// every capability a manual headless run has: the checklist requires a
    /// scheduled run's transcript to be indistinguishable from one a human
    /// typed. Only the identity is linked; nothing about the run is narrowed
    /// beyond the trigger's own budgets and policy ceiling.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "a trigger-host session is assembled from every dependency a firing needs, exactly like spawn_configured below"
    )]
    pub(crate) fn spawn_trigger_host(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        tools: ToolRegistry,
        limits: BudgetLimits,
        context: ProjectContext,
        mcp_servers: Arc<Vec<McpServerSummary>>,
        link: super::ChildLink,
        route_table: Arc<RouteTable>,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            tools,
            limits,
            context,
            mcp_servers,
            Some(link),
            true,
            Arc::new(Vec::new()),
            route_table,
        )
    }

    /// Composition seam for the interactive TUI: adds the read-only trigger
    /// status list the `/triggers` overlay renders.
    #[must_use]
    pub fn spawn_with_tools_project_context_and_triggers(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        tools: ToolRegistry,
        context: ProjectContext,
        mcp_servers: Arc<Vec<McpServerSummary>>,
        triggers: Arc<Vec<crate::core::trigger::TriggerStatus>>,
        route_table: Arc<RouteTable>,
    ) -> Self {
        Self::spawn_configured(
            providers,
            store,
            tools,
            BudgetLimits::default(),
            context,
            mcp_servers,
            None,
            true,
            triggers,
            route_table,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the constructor is the one place every dependency is injected; grouping them would invent a builder for one call site"
    )]
    fn spawn_configured(
        providers: Vec<Arc<dyn Provider>>,
        store: Arc<dyn EventStore>,
        mut tools: ToolRegistry,
        limits: BudgetLimits,
        context: ProjectContext,
        mcp_servers: Arc<Vec<McpServerSummary>>,
        child_link: Option<super::ChildLink>,
        register_spawn_subagent: bool,
        triggers: Arc<Vec<crate::core::trigger::TriggerStatus>>,
        route_table: Arc<RouteTable>,
    ) -> Self {
        if let Some(tool) = context.activation_tool() {
            tools.add(tool);
        }
        if let Some(tool) = context.extension_loader_tool() {
            tools.add(tool);
        }
        if register_spawn_subagent {
            tools.add(Arc::new(crate::tools::subagent::SpawnSubagent));
        }
        // Every session can read its own record, including a child's: a subagent
        // re-deriving what it already established costs the same as a parent
        // doing it, and the window is scoped to the caller's own session either
        // way.
        tools.add(Arc::new(crate::tools::session_query::QuerySession));
        tools.add(Arc::new(crate::tools::memory::MemorySearch));
        tools.add(Arc::new(crate::tools::memory::MemoryTimeline));
        tools.add(Arc::new(crate::tools::memory::MemoryExpand));
        let (mailbox_tx, mailbox_rx) = mpsc::channel(MAILBOX_CAPACITY);
        let (event_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (snapshot_tx, snapshot_rx) = watch::channel(RuntimeSnapshot::default());
        let shutdown = CancellationToken::new();

        let provider_connections = providers
            .iter()
            .map(|provider| {
                let id = provider.id();
                let state = if provider.credentialed() {
                    crate::core::runtime::ProviderConnectionState::Discovering
                } else {
                    crate::core::runtime::ProviderConnectionState::Disconnected
                };
                (
                    id.clone(),
                    crate::core::runtime::ProviderConnection {
                        provider: id,
                        state,
                        detail: None,
                    },
                )
            })
            .collect();

        let actor = Actor {
            providers,
            tools,
            limits,
            store: store.clone(),
            state: SessionState::default(),
            events: event_tx.clone(),
            snapshot: snapshot_tx,
            mailbox: mailbox_tx.clone(),
            run: None,
            recovery: RecoveryState::Clean,
            lease: None,
            context,
            mcp_servers,
            child_link,
            triggers,
            route_table,
            model_catalogs: std::collections::HashMap::new(),
            provider_connections,
            catalog_generation: 0,
            catalog_cancel: CancellationToken::new(),
            shutdown: shutdown.clone(),
            memory_consolidation_in_flight: std::sync::Arc::new(
                std::sync::atomic::AtomicBool::new(false),
            ),
            pending_catalog_commands: std::collections::VecDeque::new(),
            last_discovery: None,
            task_sources: std::collections::HashMap::new(),
            external_agents: crate::runtime::external_agent::ExternalAgentRegistry::new(),
            external_agent_profiles: std::collections::HashMap::new(),
        };

        tokio::spawn(actor.run(mailbox_rx, shutdown.clone()));

        Self {
            mailbox: mailbox_tx,
            events: event_tx,
            snapshot: snapshot_rx,
            shutdown,
            store,
        }
    }
}

#[async_trait]
impl MjolnrRuntime for Runtime {
    fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.borrow().clone()
    }

    fn snapshots(&self) -> SnapshotStream {
        SnapshotStream::new(self.snapshot.clone())
    }

    fn subscribe(&self) -> RuntimeSubscription {
        RuntimeSubscription::new(self.events.subscribe())
    }

    // A routing match that grows with every new command family; splitting it
    // obscures the dispatch shape.
    #[allow(clippy::too_many_lines)]
    async fn dispatch(&self, command: MjolnrCommand) -> Result<(), MjolnrError> {
        if crate::runtime::interview::is_plan_command(&command) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::PlanCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        // Child-run commands are acknowledged like plan commands: the caller
        // learns the typed refusal (today: capability unavailable) instead of
        // the command disappearing into the mailbox (Phase D2).
        if matches!(
            &command,
            MjolnrCommand::CreateWorktree { .. }
                | MjolnrCommand::ForkWork { .. }
                | MjolnrCommand::StartChild { .. }
                | MjolnrCommand::CancelChild { .. }
                | MjolnrCommand::PreserveBranch { .. }
                | MjolnrCommand::SettleChild { .. }
                | MjolnrCommand::DiscardSettledWorktree { .. }
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::ChildRunCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        // Repository commands are acknowledged because they are the first
        // family that actually performs a side effect on the caller's behalf.
        // A git write whose outcome the caller never learns is exactly the
        // uncertain effect AGENTS.md §1.4 refuses to paper over (Phase D5).
        if matches!(
            &command,
            MjolnrCommand::StagePaths { .. }
                | MjolnrCommand::StageHunks { .. }
                | MjolnrCommand::Unstage { .. }
                | MjolnrCommand::CreateBranch { .. }
                | MjolnrCommand::Commit { .. }
                | MjolnrCommand::IntegrateChildBranch { .. }
                | MjolnrCommand::Fetch
                | MjolnrCommand::Push { .. }
                | MjolnrCommand::IntegrateUpstream { .. }
                | MjolnrCommand::CloneProject { .. }
                | MjolnrCommand::Rebase { .. }
                | MjolnrCommand::AbortRebase
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::RepositoryCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        // Integration commands are acknowledged so a client learns the typed
        // refusal instead of watching a "fetching…" state that never resolves
        // (Phase D6).
        if matches!(
            &command,
            MjolnrCommand::FetchTask { .. }
                | MjolnrCommand::FetchTasks { .. }
                | MjolnrCommand::SubmitChange { .. }
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::IntegrationCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        // Review commands are acknowledged because every one of them can refuse
        // for a reason the human has to act on: the diff moved under the note,
        // the line is not in the captured diff, or a run is already in flight.
        // A refusal the client never learns of is a note that silently was not
        // taken (Phase D3).
        if matches!(
            &command,
            MjolnrCommand::AddReviewNote { .. }
                | MjolnrCommand::AddReviewComment { .. }
                | MjolnrCommand::SendReviewNotes { .. }
                | MjolnrCommand::ResolveCouncilFinding { .. }
                | MjolnrCommand::ProposeCouncilAmendment { .. }
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::ReviewCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        // Board commands are acknowledged because silently *not* recording a
        // decision — an unknown ticket, a dangling blocker, an unrecorded
        // option — is the worst failure mode a decision record has (Phase E5).
        // `SubmitImportedComment` is board-acknowledged so the client learns the
        // typed refusal, even though its durability is `ImportedCommentRecorded`
        // after the network effect.
        if matches!(
            &command,
            MjolnrCommand::OpenDecisionTicket { .. }
                | MjolnrCommand::ResolveDecisionTicket { .. }
                | MjolnrCommand::ImportWorkItem { .. }
                | MjolnrCommand::RefreshImportedItem { .. }
                | MjolnrCommand::SubmitImportedComment { .. }
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::BoardCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        if matches!(
            &command,
            MjolnrCommand::LaunchExternalAgent { .. }
                | MjolnrCommand::StopExternalAgent { .. }
                | MjolnrCommand::ImportExternalAgentChanges { .. }
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::ExternalAgentCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        // Opening a project is acknowledged because refusing it is routine —
        // a run in flight, a session already anchored to the current root, or
        // a path that is not a directory. Accepted-not-completed semantics
        // turned every one of those into a control that did nothing.
        if matches!(
            &command,
            MjolnrCommand::OpenProject { .. }
                | MjolnrCommand::RefreshRepository
                | MjolnrCommand::SaveFile { .. }
        ) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::WorkspaceCommand { command, reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        if matches!(&command, MjolnrCommand::RunDiscovery) {
            let (reply, acknowledged) = oneshot::channel();
            self.mailbox
                .send(Mail::DiscoveryCommand { reply })
                .await
                .map_err(|_| MjolnrError::RuntimeClosed)?;
            return acknowledged.await.map_err(|_| MjolnrError::RuntimeClosed)?;
        }
        self.mailbox
            .send(Mail::Command(command))
            .await
            .map_err(|_| MjolnrError::RuntimeClosed)
    }

    /// Ask the actor for one contained directory page or file (Phase D7).
    ///
    /// Through the mailbox rather than straight to the filesystem, unlike
    /// `search_workspace`, which can reach the store directly. The difference
    /// is containment: the workspace root lives in actor state, and a handle
    /// that kept its own copy would read against a root the actor had already
    /// moved on from — a stale containment boundary, which is the one thing
    /// containment may never be.
    async fn read_workspace_files(
        &self,
        request: crate::core::workspace_files::WorkspaceFileRequest,
    ) -> Result<crate::core::workspace_files::WorkspaceFileAnswer, MjolnrError> {
        let (reply, answered) = oneshot::channel();
        self.mailbox
            .send(Mail::WorkspaceFileQuery { request, reply })
            .await
            .map_err(|_| MjolnrError::RuntimeClosed)?;
        answered.await.map_err(|_| MjolnrError::RuntimeClosed)?
    }

    async fn search_workspace(
        &self,
        filter: crate::core::store::WorkspaceSearchFilter,
    ) -> Result<crate::core::store::WorkspaceSearchPage, MjolnrError> {
        self.store.search_workspace(filter).await.map_err(|error| {
            // The store distinguishes "that question cannot be answered" from
            // "the store is broken" — that is the entire point of
            // `StoreError::Refused`, added with the D4 producer. Collapsing
            // both into `MjolnrError::Store` threw the distinction away one hop
            // later: `Store` carries no reason code, so a query too short for
            // the trigram index reached the client as an untyped failure
            // indistinguishable from a corrupt database, and a surface could
            // only print it. A refusal is a normal result (AGENTS.md §6).
            match error {
                crate::core::store::StoreError::Refused { detail } => {
                    MjolnrError::workspace_refused(ReasonCode::WorkspaceSearchRefused, detail)
                }
                other => MjolnrError::Store {
                    detail: other.to_string(),
                },
            }
        })
    }

    /// Read the board: what is decidable right now (Phase E5, step 3).
    ///
    /// The board is a cross-session projection: every decision ticket and plan
    /// whose project root is the open workspace, folded into the frontier.
    /// Sessions are read through the store's branch events so a board query
    /// and a live session cannot disagree about what a record says; the
    /// per-session durable records are rebuilt from the same events that
    /// recovery would replay (the board shares no checkpoint authority).
    async fn query_board(&self) -> Result<crate::core::frontier::BoardOverview, MjolnrError> {
        let workspace_root = self.snapshot().workspace_root.ok_or_else(|| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "the board requires an open workspace",
            )
        })?;

        let sessions = self
            .store
            .sessions()
            .await
            .map_err(store_refusal_to_mjolnr)?;

        let mut labels: std::collections::BTreeMap<crate::core::frontier::NodeId, String> =
            std::collections::BTreeMap::new();
        let mut tickets: std::collections::BTreeMap<
            crate::core::board::DecisionTicketId,
            crate::core::board::DecisionTicketRecord,
        > = std::collections::BTreeMap::new();
        let mut plans: Vec<crate::core::plan::PlanWorkflow> = Vec::new();
        let mut imported: std::collections::BTreeMap<
            crate::core::imported::ImportedItemId,
            crate::core::imported::ImportedItem,
        > = std::collections::BTreeMap::new();
        let mut imported_acts: std::collections::BTreeMap<
            crate::core::imported::ImportedActId,
            crate::core::imported::ImportedAct,
        > = std::collections::BTreeMap::new();

        for summary in sessions.iter().filter(|s| s.project_root == workspace_root) {
            let events = self
                .store
                .branch_events(summary.id)
                .await
                .map_err(store_refusal_to_mjolnr)?;
            let mut state = crate::runtime::session::SessionState::default();
            state
                .rebuild_durable_records_from(&events)
                .map_err(store_refusal_to_mjolnr)?;
            for (id, record) in state.decision_tickets {
                let node = crate::core::frontier::NodeId::Decision(id);
                tickets.insert(id, record.clone());
                labels.insert(node, record.ticket.question.clone());
            }
            for (id, item) in state.imported_items {
                let node = crate::core::frontier::NodeId::Imported(id);
                imported.insert(id, item.clone());
                labels.insert(node, item.title.clone());
            }
            for (id, act) in state.imported_acts {
                imported_acts.insert(id, act);
            }
            if let Some(plan) = state.plan {
                let node = crate::core::frontier::NodeId::Plan(plan.plan_id);
                let label = plan
                    .active_revision
                    .and_then(|revision| {
                        plan.proposals
                            .iter()
                            .find(|proposal| proposal.revision_id == revision)
                    })
                    .or(plan.proposals.last())
                    .map(|proposal| proposal.title.clone());
                if let Some(label) = label {
                    labels.insert(node, label);
                }
                plans.push(plan);
            }
        }
        let board = crate::core::frontier::compute_frontier(&tickets, &plans, &imported);
        let mut overview = crate::core::frontier::BoardOverview::from_frontier(&board, &labels);
        overview.imported_tasks = imported;
        overview.imported_acts = imported_acts;
        Ok(overview)
    }

    async fn query_repository_history(
        &self,
        limit: u32,
    ) -> Result<crate::core::repository::RepositoryHistory, MjolnrError> {
        let root = self.snapshot().workspace_root.ok_or_else(|| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "repository history requires an open workspace",
            )
        })?;
        let history = tokio::task::spawn_blocking(move || {
            crate::repository::Repository::open(root)?.history(limit)
        })
        .await
        .map_err(|error| {
            MjolnrError::workspace_refused(
                ReasonCode::RepositoryUncertainEffect,
                format!("repository history worker failed: {error}"),
            )
        })?
        .map_err(|error| MjolnrError::workspace_refused(error.reason_code(), error.to_string()))?;
        Ok(history)
    }

    /// Shut down only once every accepted durable write is flushed.
    async fn close(&self) -> Result<(), MjolnrError> {
        let (reply, acknowledged) = oneshot::channel();

        if self.mailbox.send(Mail::Shutdown { reply }).await.is_err() {
            self.shutdown.cancel();
            return Ok(());
        }

        let result = match acknowledged.await {
            Ok(result) => result.map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            }),
            Err(_) => Err(MjolnrError::RuntimeClosed),
        };

        self.shutdown.cancel();
        result
    }
}

/// Fold a store failure into a runtime error, preserving the refusal/decay
/// split that `search_workspace` relies on: a query a store cannot answer is a
/// normal result carrying `WorkspaceSearchRefused`, while a corrupt or
/// unreadable store is a `Store` error indistinguishable from a broken DB.
fn store_refusal_to_mjolnr(error: crate::core::store::StoreError) -> MjolnrError {
    match error {
        crate::core::store::StoreError::Refused { detail } => {
            MjolnrError::workspace_refused(ReasonCode::WorkspaceSearchRefused, detail)
        }
        other => MjolnrError::Store {
            detail: other.to_string(),
        },
    }
}
