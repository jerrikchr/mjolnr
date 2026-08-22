//! The runtime actor: the only owner of session truth.
//!
//! ```text
//! client ──dispatch()──▶ ┐
//!                        ├─ bounded mailbox ──▶ actor task
//! run task ──events()──▶ ┘                       │ owns SessionState
//!                                                │ appends to EventStore
//!        ◀──subscribe()── bounded broadcast ─────┤
//!        ◀──snapshot()─── watch (Arc) ───────────┘
//! ```
//!
//! One task owns the state, so there is no lock to hold across an `.await`
//! (AGENTS.md §4) and no global registry ( anti-pattern).
//!
//! **Commands and provider events share one mailbox.** That is the load-bearing
//! decision here. The obvious alternative — drain the provider stream inline
//! inside the command handler — deadlocks the thing that matters: the actor
//! cannot accept `CancelRun` while it is blocked draining the very stream the
//! user wants cancelled. An earlier draft did exactly that, and the cancel test
//! caught it. A run now forwards its events into the mailbox, so the actor stays
//! responsive and still has exactly one writer.
//!
//! Every channel is bounded, and backpressure composes end-to-end: a slow actor
//! stalls the forwarder, which stalls the adapter's `send`.

pub mod budget;
mod catalog;
pub mod client_bridge;
mod continuation;
mod council;
mod discovery;
mod durability;
mod envelope;
pub mod external_agent;
mod governance;
mod handle;
pub mod images;
mod interview;
mod memory;
mod model_selection;
mod provider_loop;
pub mod recovery;
mod review;
mod routing;
pub mod session;
mod session_query;
pub mod subagent;
pub mod terminal;
mod tool_loop;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::context::ProjectContext;
use crate::core::change_capture::ChangeView;
use crate::core::command::{ApprovalId, MjolnrCommand};
use crate::core::continuation::QuotaReserveStatus;
use crate::core::directive::DirectiveSource;
use crate::core::error::{MjolnrError, ProviderError, ReasonCode, ToolError};
use crate::core::event::{FinishReason, MjolnrEvent, ProviderEvent, RunId, SessionId};
use crate::core::mcp::McpServerSummary;
use crate::core::message::{CanonicalMessage, ToolCall, ToolResult};
use crate::core::model::{ModelId, ProviderId};
use crate::core::provider::{Provider, ProviderCompletion};
use crate::core::recovery::RecoveryState;
use crate::core::repository::{RefreshTrigger, RepositoryView};
use crate::core::routing::RouteTable;
use crate::core::runtime::RuntimeSnapshot;
use crate::core::store::SessionLease;
use crate::core::store::{EventStore, StoreError};
use crate::core::tool::Tool;
use crate::runtime::budget::BudgetLimits;
use crate::runtime::interview::PlanRun;
use crate::runtime::session::{SessionState, StreamAccumulator};
use crate::tools::ToolRegistry;

pub use handle::Runtime;

/// Identity a subagent host session opens with.
///
/// The parent mints the child's `SessionId` *before* the child runtime exists
/// so the durable `SubagentSpawned` event, the worktree name, and the child's
/// own transcript all agree on one identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildLink {
    pub parent: SessionId,
    pub session: SessionId,
}

/// Bounded mailbox. Sized for stream traffic, since provider events share it.
const MAILBOX_CAPACITY: usize = 64;

/// Bounded event broadcast. A subscriber that falls behind is told it lagged and
/// resyncs from a snapshot, rather than being allowed to grow the heap.
const EVENT_CAPACITY: usize = 256;

/// Bounded provider event queue — the backpressure seam. When the actor is slow,
/// the adapter's `send` awaits capacity.
const PROVIDER_EVENT_CAPACITY: usize = 16;

/// What the provider task reports back when its stream ends.
type StreamOutcome = Result<Result<ProviderCompletion, ProviderError>, tokio::task::JoinError>;
type ToolTaskOutcome = Result<Result<ToolResult, ToolError>, tokio::task::JoinError>;

/// Messages the actor processes. Commands and stream traffic are deliberately
/// the same queue so neither can starve the other.
enum Mail {
    Command(MjolnrCommand),
    /// Plan commands are acknowledged after fail-closed validation and durable
    /// append. Other commands retain the runtime contract's accepted-not-
    /// completed dispatch semantics.
    PlanCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// Child-run commands (Phase D2) are acknowledged with the same
    /// fail-closed semantics as plan commands: the reply carries the typed
    /// refusal while execution is not yet implemented.
    ChildRunCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// Repository commands (Phase D5) are acknowledged so the caller learns
    /// the exact outcome of a git side effect — success, typed refusal, or an
    /// uncertain partial effect — rather than discovering it from a later
    /// snapshot poll.
    RepositoryCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// Integration commands (Phase D6) are acknowledged with the same
    /// fail-closed semantics: the reply carries the typed refusal while no
    /// integration performs network I/O.
    IntegrationCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// Supply the integration producer (Phase D6). Sent once, immediately after
    /// spawn, by [`Runtime::spawn_with_task_source`](crate::runtime::Runtime::spawn_with_task_source);
    /// it exists so a test can exercise `fetchTask` against a local mock without
    /// mutating the process environment.
    SetTaskSource {
        task_source: Arc<dyn crate::integrations::TaskSource>,
    },
    /// Review commands (Phase D3) are acknowledged because every one of them
    /// can refuse for a reason the human has to see: the diff moved under the
    /// note, the line is not in the captured diff, or a run is already in flight.
    /// A refusal the client never learns of is a review note that silently was
    /// not taken.
    ReviewCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// Board commands (Phase E5) are acknowledged for the same shape of
    /// reason: an unknown ticket, a dangling blocker, or an unrecorded option
    /// are refusals the human must hear about — silently not recording a
    /// decision is the worst thing a decision record can do.
    BoardCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    ExternalAgentCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// Workspace commands are acknowledged because refusing one is the normal
    /// case: a run in flight, a session already open on the root, or a path
    /// that is not a directory. Unacknowledged, each of those reached the
    /// client as a button that did nothing.
    WorkspaceCommand {
        command: MjolnrCommand,
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    /// File reads (Phase D7) are acknowledged because the answer *is* the
    /// point: a listing or a file, or the typed refusal that containment,
    /// pagination, or the filesystem produced instead. They are a query, not a
    /// command, and nothing they do changes published state.
    WorkspaceFileQuery {
        request: crate::core::workspace_files::WorkspaceFileRequest,
        reply:
            oneshot::Sender<Result<crate::core::workspace_files::WorkspaceFileAnswer, MjolnrError>>,
    },
    /// Discovery writes a new bounded OKF bundle and therefore reports its
    /// completion or refusal to the human who requested it.
    DiscoveryCommand {
        reply: oneshot::Sender<Result<(), MjolnrError>>,
    },
    CatalogDiscovered {
        generation: u64,
        provider: crate::core::model::ProviderId,
        outcome: Result<Vec<crate::core::model::ModelDescriptor>, ProviderError>,
    },
    ProviderEvent {
        run: RunId,
        event: ProviderEvent,
    },
    ProviderTurnEnded {
        run: RunId,
        outcome: StreamOutcome,
    },
    ToolEnded {
        run: RunId,
        call: ToolCall,
        outcome: ToolTaskOutcome,
    },
    BudgetExpired {
        run: RunId,
    },
    /// A subagent orchestration boundary the actor must record.
    ///
    /// The orchestrator task never writes to the store: the actor is the only
    /// writer of the parent transcript, so spawn and late-result boundaries
    /// arrive here as mail like every other out-of-actor fact.
    Subagent {
        notice: SubagentNotice,
    },
    /// A council finished deliberating. The orchestration task
    /// never writes to the store — the actor is the only writer of the parent
    /// transcript — so the review arrives here as mail and is appended as
    /// evidence by the actor.
    CouncilFinished {
        session: SessionId,
        review: Box<crate::core::council::CouncilReview>,
    },
    /// Shut down and answer when the data is durable.
    ///
    /// A message rather than a cancellation token because shutdown must be
    /// *acknowledged*: `close` has to know settled state was checkpointed (or an
    /// interrupted tail deliberately preserved), the store drained, and the
    /// lease released. A token can only ask; it cannot prove completion.
    Shutdown {
        reply: oneshot::Sender<Result<(), StoreError>>,
    },
}

/// Subagent facts reported by the orchestration task.
#[derive(Debug)]
enum SubagentNotice {
    Spawned {
        run: RunId,
        child: SessionId,
        directive: String,
        policy: crate::core::policy::PolicyMode,
        branch: String,
        worktree: String,
    },
    Late {
        child: SessionId,
        detail: String,
    },
    /// A concurrent sibling's mutation invalidated one child's read of a path
    /// (Phase 5 Slice 5.2). Durable: it is the fact that turns an otherwise
    /// verified finish into a re-validation requirement.
    Collision {
        reader: SessionId,
        writer: SessionId,
        path: String,
    },
}

impl std::fmt::Debug for Mail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(command) => write!(formatter, "Command({command:?})"),
            Self::PlanCommand { command, .. } => {
                write!(formatter, "PlanCommand({command:?})")
            }
            Self::ChildRunCommand { command, .. } => {
                write!(formatter, "ChildRunCommand({command:?})")
            }
            Self::RepositoryCommand { command, .. } => {
                write!(formatter, "RepositoryCommand({command:?})")
            }
            Self::IntegrationCommand { command, .. } => {
                write!(formatter, "IntegrationCommand({command:?})")
            }
            Self::SetTaskSource { .. } => formatter.write_str("SetTaskSource"),
            Self::ReviewCommand { command, .. } => {
                write!(formatter, "ReviewCommand({command:?})")
            }
            Self::BoardCommand { command, .. } => {
                write!(formatter, "BoardCommand({command:?})")
            }
            Self::WorkspaceCommand { command, .. } => {
                write!(formatter, "WorkspaceCommand({command:?})")
            }
            Self::WorkspaceFileQuery { request, .. } => {
                write!(formatter, "WorkspaceFileQuery({request:?})")
            }
            Self::DiscoveryCommand { .. } => formatter.write_str("DiscoveryCommand"),
            Self::CatalogDiscovered { provider, .. } => {
                write!(formatter, "CatalogDiscovered({provider})")
            }
            Self::ProviderEvent { run, .. } => write!(formatter, "ProviderEvent({run})"),
            Self::ProviderTurnEnded { run, .. } => {
                write!(formatter, "ProviderTurnEnded({run})")
            }
            Self::ToolEnded { run, call, .. } => {
                write!(formatter, "ToolEnded({run}, {})", call.name)
            }
            Self::BudgetExpired { run } => write!(formatter, "BudgetExpired({run})"),
            Self::Subagent { notice } => write!(formatter, "Subagent({notice:?})"),
            Self::CouncilFinished { session, .. } => {
                write!(formatter, "CouncilFinished({session})")
            }
            Self::ExternalAgentCommand { command, .. } => {
                write!(formatter, "ExternalAgentCommand({command:?})")
            }
            Self::Shutdown { .. } => formatter.write_str("Shutdown"),
        }
    }
}

/// A run in flight, and everything needed to finish it.
#[derive(Debug)]
struct ActiveRun {
    id: RunId,
    session: SessionId,
    provider: ProviderId,
    model: ModelId,
    cancel: CancellationToken,
    accumulator: StreamAccumulator,
    pending_tools: VecDeque<ToolCall>,
    awaiting_approval: Option<PendingTool>,
    /// Authority resolved for an in-flight model-proposed extension load.
    ///
    /// The call id prevents a failed load from being attributed to a later
    /// proposal. Tools execute serially within a run, so one slot is enough.
    pending_load_authority: Option<(String, crate::core::event::ExtensionLoadAuthority)>,
    phase: RunPhase,
    provider_turns: u32,
    tool_calls: u32,
    intent: RunIntent,
    pending_drain: Option<QuotaReserveStatus>,
    hard_stop: Option<QuotaReserveStatus>,
    /// The provider/model a live handoff will swap to once this run lands
    /// . Applied at `finish_run`, never mid-turn.
    handoff_target: Option<(ProviderId, ModelId)>,
    /// Review threads this run was started to answer.
    ///
    /// Held on the run, not on session state, so it cannot outlive the run it
    /// belongs to: when the run ends the marker is dropped with it, and a
    /// later, unrelated run can never report its answer as this request's.
    /// Empty for every run that was not started by `SendReviewNotes`.
    pending_review_threads: Vec<crate::core::review::ReviewThreadId>,
    /// A bounded structured planning exchange. Such a run receives no tools
    /// and its final text is parsed into durable workflow events.
    plan_run: Option<PlanRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunIntent {
    Normal,
    ManualHandoff,
    QuotaDrain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunPhase {
    Provider,
    Approval,
    Tool,
}

#[derive(Debug)]
struct PendingTool {
    approval: ApprovalId,
    call: ToolCall,
    tool: Arc<dyn Tool>,
}

enum ToolPreparation {
    Ready {
        tool: Arc<dyn Tool>,
        preview: String,
    },
    Continue,
    Stop,
}

struct Actor {
    providers: Vec<Arc<dyn Provider>>,
    tools: ToolRegistry,
    limits: BudgetLimits,
    store: Arc<dyn EventStore>,
    state: SessionState,
    events: broadcast::Sender<MjolnrEvent>,
    snapshot: watch::Sender<RuntimeSnapshot>,
    mailbox: mpsc::Sender<Mail>,
    run: Option<ActiveRun>,
    /// Work a crash interrupted. Blocks autonomous progress until resolved.
    recovery: RecoveryState,
    /// Proof this process owns the session's writes. Released on clean
    /// shutdown; deliberately left behind by a crash (`docs/persistence.md` §5).
    lease: Option<SessionLease>,
    context: ProjectContext,
    mcp_servers: Arc<Vec<McpServerSummary>>,
    /// Set when this actor hosts a subagent or trigger-firing session for a
    /// parent.
    child_link: Option<ChildLink>,
    /// Configured triggers, for [`RuntimeSnapshot::triggers`](crate::core::runtime::RuntimeSnapshot::triggers).
    triggers: Arc<Vec<crate::core::trigger::TriggerStatus>>,
    model_catalogs: std::collections::HashMap<
        crate::core::model::ProviderId,
        Vec<crate::core::model::ModelDescriptor>,
    >,
    provider_connections: std::collections::HashMap<
        crate::core::model::ProviderId,
        crate::core::runtime::ProviderConnection,
    >,
    catalog_generation: u64,
    catalog_cancel: CancellationToken,
    /// The handle's shutdown token, cloned so the background consolidation
    /// task cancels exactly when the actor stops (AGENTS.md §4: cancellation
    /// is plumbed everywhere, and a task nobody can cancel is decorative).
    shutdown: CancellationToken,
    /// Single-flight guard for background consolidation: a second run end
    /// while one pass is still open skips rather than overlapping (two
    /// overlapping passes would both read progress N and append duplicate
    /// episodes for N+1..M).
    memory_consolidation_in_flight: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Commands accepted while their provider catalogue is still discovering.
    ///
    /// Once one command is deferred, later commands queue behind it so a
    /// `SendUserMessage` cannot overtake a compact resume or model switch.
    pending_catalog_commands: VecDeque<MjolnrCommand>,
    /// Most recent discovery result. The bundle itself is durable; this is a
    /// bounded client projection only.
    last_discovery: Option<crate::core::discovery::DiscoveryReport>,
    /// Task sources supplied by the caller instead of built from the
    /// environment.
    ///
    /// The seam a runtime-level test uses to point `fetchTask` at a local mock,
    /// and the reason no test has to mutate the process environment — which
    /// this repository refuses to do (`core::process`), and which the `unsafe`
    /// lint would forbid anyway. Each injected source stands in only for the
    /// integration whose id it reports, so an injected GitHub source cannot
    /// silently answer for Linear. Held as a map so Vercel/Supabase add one
    /// registration line instead of a new `match` arm per integration.
    task_sources: HashMap<String, Arc<dyn crate::integrations::TaskSource>>,
    /// The project's routing config. Empty by default —
    /// every constructor that does not explicitly load `.mjolnr/routes/`
    /// leaves this at [`RouteTable::default`], which is exactly what makes
    /// "no routing config" restore present-day behaviour: nothing in
    /// [`routing`](super::routing) does anything when this table is empty.
    route_table: Arc<RouteTable>,
    /// External-agent worktrees (D9) — always `ExternalUnverified`.
    external_agents: crate::runtime::external_agent::ExternalAgentRegistry,
    /// Injected external-agent profiles (test seam + future host discovery).
    #[allow(dead_code)]
    external_agent_profiles: HashMap<String, crate::context::external_agent::ExternalAgentProfile>,
}

impl std::fmt::Debug for Actor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Actor")
            .field("providers", &self.providers.len())
            .field("tools", &self.tools.definitions().len())
            .field("run_active", &self.run.is_some())
            .finish_non_exhaustive()
    }
}

impl Actor {
    async fn run(mut self, mut mailbox: mpsc::Receiver<Mail>, shutdown: CancellationToken) {
        self.refresh_provider_catalogs();
        self.refresh_session_list().await;
        loop {
            tokio::select! {
                // The abrupt path: no checkpoint, no flush, no lease release —
                // which is exactly what a crash looks like, and what the
                // recovery tests need to reproduce. `Mail::Shutdown` is the
                // clean path.
                () = shutdown.cancelled() => break,
                maybe_mail = mailbox.recv() => {
                    let Some(mail) = maybe_mail else { break };
                    if !self.handle(mail).await {
                        break;
                    }
                }
            }
        }

        // Cancel any run still in flight rather than detaching it (AGENTS.md §4).
        if let Some(run) = self.run.take() {
            run.cancel.cancel();
        }
        self.catalog_cancel.cancel();
    }

    /// Returns false when the actor must stop.
    async fn handle(&mut self, mail: Mail) -> bool {
        // The acknowledged command families first, as one group. They are the
        // same shape — run a handler, answer the caller — and folding them in
        // beside the stream traffic is what pushed this function past the
        // cognitive-complexity lint when the D3 family arrived. The split the
        // lint asked for is a real one: "a command someone is waiting on"
        // and "something the actor observed" are two different jobs.
        let mail = match self.handle_acknowledged(mail).await {
            Ok(()) => return true,
            Err(mail) => mail,
        };
        match mail {
            Mail::Command(command) => {
                if !self.pending_catalog_commands.is_empty()
                    || self.command_waits_for_catalog(&command)
                {
                    self.pending_catalog_commands.push_back(command);
                } else {
                    self.handle_command(command).await;
                }
            }
            Mail::CatalogDiscovered {
                generation,
                provider,
                outcome,
            } => {
                self.handle_catalog_discovered(generation, provider, outcome)
                    .await;
            }
            Mail::ProviderEvent { run, event } => self.handle_provider_event(run, event).await,
            Mail::ProviderTurnEnded { run, outcome } => {
                self.handle_provider_turn_ended(run, outcome).await;
            }
            Mail::ToolEnded { run, call, outcome } => {
                self.handle_tool_ended(run, call, outcome).await;
            }
            Mail::SetTaskSource { task_source } => {
                self.task_sources
                    .insert(task_source.id().as_str().to_owned(), task_source);
            }
            Mail::BudgetExpired { run } => self.exhaust_budget(run).await,
            Mail::Subagent { notice } => self.handle_subagent_notice(notice).await,
            Mail::CouncilFinished { session, review } => {
                self.finish_council(session, *review).await;
            }
            Mail::Shutdown { reply } => {
                let result = self.shutdown().await;
                let _ = reply.send(result);
                return false;
            }
            // Handled by `handle_acknowledged` above, which only returns the
            // mail it did not claim.
            Mail::PlanCommand { .. }
            | Mail::ChildRunCommand { .. }
            | Mail::RepositoryCommand { .. }
            | Mail::IntegrationCommand { .. }
            | Mail::ReviewCommand { .. }
            | Mail::BoardCommand { .. }
            | Mail::ExternalAgentCommand { .. }
            | Mail::WorkspaceCommand { .. }
            | Mail::WorkspaceFileQuery { .. }
            | Mail::DiscoveryCommand { .. } => {}
        }
        true
    }

    /// Run one acknowledged command family and answer its caller.
    ///
    /// `Err(mail)` hands back mail this function does not claim, so the caller
    /// continues its own match. Every family here is acknowledged for the same
    /// reason, stated once: refusing is the normal case, and a refusal the
    /// caller never learns of is indistinguishable from a control that did
    /// nothing.
    async fn handle_acknowledged(&mut self, mail: Mail) -> Result<(), Mail> {
        match mail {
            Mail::PlanCommand { command, reply } => {
                let result = self.handle_plan_command(command).await;
                let _ = reply.send(result);
            }
            Mail::ChildRunCommand { command, reply } => {
                let result = Self::handle_child_run_command(&command);
                let _ = reply.send(result);
            }
            Mail::RepositoryCommand { command, reply } => {
                let result = self.handle_repository_command(command).await;
                let _ = reply.send(result);
            }
            Mail::IntegrationCommand { command, reply } => {
                let result = self.handle_integration_command(&command).await;
                let _ = reply.send(result);
            }
            Mail::ReviewCommand { command, reply } => {
                let result = self.handle_review_command(command).await;
                let _ = reply.send(result);
            }
            Mail::BoardCommand { command, reply } => {
                let result = self.handle_board_command(command).await;
                let _ = reply.send(result);
            }
            Mail::ExternalAgentCommand { command, reply } => {
                let result = self.handle_external_agent_command(command).await;
                let _ = reply.send(result);
            }
            Mail::WorkspaceCommand { command, reply } => {
                let result = self.handle_workspace_command(command).await;
                let _ = reply.send(result);
            }
            Mail::WorkspaceFileQuery { request, reply } => {
                let result = self.read_workspace_files(request).await;
                let _ = reply.send(result);
            }
            Mail::DiscoveryCommand { reply } => {
                let result = self.run_discovery().await;
                let _ = reply.send(result);
            }
            other => return Err(other),
        }
        Ok(())
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "one flat command dispatch;  added the AttachRoute arm alongside the existing ones"
    )]
    async fn handle_command(&mut self, command: MjolnrCommand) {
        match command {
            MjolnrCommand::RegisterCredential { provider, secret } => {
                use crate::core::secrets::{Credential, Secret, SecretStore};
                use crate::store::secrets::OsSecretStore;
                let secrets = OsSecretStore::new();
                let _ = secrets.store(&provider, Credential::ApiKey(Secret::new(secret.0)));
                self.refresh_provider_catalogs();
            }
            MjolnrCommand::RefreshCredentials => self.refresh_provider_catalogs(),
            MjolnrCommand::CreateSession { provider, model } => {
                self.create_session(provider, model).await;
            }
            MjolnrCommand::SelectModel { provider, model } => {
                let _ = self.select_model(provider, model).await;
            }
            MjolnrCommand::AttachRoute {
                route,
                role,
                task_class,
            } => {
                self.attach_route(route, role, task_class).await;
            }
            MjolnrCommand::BindRoutePersona { route, persona } => {
                self.bind_route_persona(&route, persona.as_deref());
            }
            MjolnrCommand::SelectPersona { persona } => {
                // Voice only: it changes the next turn's system prompt and
                // nothing else. The client has already validated the name
                // against the offered personas, so an unknown one never reaches
                // here as a silent no-op.
                self.state.persona_override = persona;
                self.publish_snapshot();
            }
            MjolnrCommand::SetPolicy { mode } => {
                self.set_policy(mode).await;
                // An envelope the new policy no longer justifies must not
                // survive the narrowing that invalidated it.
                self.reconcile_envelope_with_policy(mode).await;
            }
            MjolnrCommand::ArmSpawnEnvelope { envelope } => {
                self.arm_spawn_envelope(*envelope).await;
            }
            MjolnrCommand::ClearSpawnEnvelope => {
                self.clear_spawn_envelope(crate::core::event::EnvelopeEnd::Withdrawn)
                    .await;
            }
            MjolnrCommand::SendUserMessage { text, source } => {
                self.start_run(source.frame(&text), &source).await;
            }
            MjolnrCommand::SendPromptTemplate { name, arguments } => {
                self.send_prompt_template(name, arguments).await;
            }
            MjolnrCommand::ReloadResources => self.reload_resources(),
            MjolnrCommand::LoadExtension { name } => self.load_extension(name).await,
            MjolnrCommand::RewindTo { sequence } => self.rewind_to(sequence).await,
            MjolnrCommand::RollbackToCheckpoint {
                target_sequence,
                expected_head,
            } => {
                self.rollback_to_checkpoint(target_sequence, expected_head)
                    .await;
            }
            MjolnrCommand::LoadSessionTree => self.load_session_tree().await,
            MjolnrCommand::ForkSession { before } => self.fork_session(before).await,
            MjolnrCommand::FollowBranch { sequence } => self.follow_branch(sequence).await,
            MjolnrCommand::QueueSteeringMessage { text } => {
                // With nothing in flight there is nothing to steer, so the
                // message is simply the next thing said. That keeps one key
                // meaning one thing whether or not a run happens to be active.
                if self.run.is_some() {
                    self.state.steering.push_back(text);
                    self.publish_snapshot();
                } else {
                    self.start_run(text, &DirectiveSource::Human).await;
                }
            }
            MjolnrCommand::CreateHandoff { target } => self.start_handoff(target).await,
            MjolnrCommand::StartPlanInterview { goal } => {
                let _ = self.start_plan_interview(goal).await;
            }
            MjolnrCommand::ConveneCouncil {
                question,
                plan_file,
            } => {
                self.convene_council(question, plan_file).await;
            }
            MjolnrCommand::ResolveResume { choice } => self.resolve_resume(choice).await,
            MjolnrCommand::ResolveApproval { approval, decision } => {
                self.resolve_approval(approval, decision).await;
            }
            MjolnrCommand::ResolveRecovery { decision } => self.resolve_recovery(decision).await,
            MjolnrCommand::CancelRun => self.cancel_run().await,
            MjolnrCommand::EndSession => self.end_session().await,
            MjolnrCommand::ReleaseSession => self.release_session().await,
            MjolnrCommand::ReclaimSession { session } => self.reclaim_session(session).await,
            MjolnrCommand::ResumeSession { session } => self.resume_session(session).await,
            MjolnrCommand::ResumeCompact {
                session,
                provider,
                model,
            } => {
                self.resume_compact(session, provider, model).await;
            }
            MjolnrCommand::RunDiscovery
            | MjolnrCommand::AskPlanQuestion { .. }
            | MjolnrCommand::AnswerPlanQuestion { .. }
            | MjolnrCommand::ProposePlan { .. }
            | MjolnrCommand::ReviewPlan { .. }
            | MjolnrCommand::ApprovePlan { .. }
            | MjolnrCommand::HandoffPlan { .. }
            | MjolnrCommand::CreateWorktree { .. }
            | MjolnrCommand::ForkWork { .. }
            | MjolnrCommand::StartChild { .. }
            | MjolnrCommand::CancelChild { .. }
            | MjolnrCommand::PreserveBranch { .. }
            | MjolnrCommand::SettleChild { .. }
            | MjolnrCommand::DiscardSettledWorktree { .. }
            | MjolnrCommand::StagePaths { .. }
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
            | MjolnrCommand::FetchTask { .. }
            | MjolnrCommand::FetchTasks { .. }
            | MjolnrCommand::SubmitChange { .. }
            | MjolnrCommand::SubmitImportedComment { .. }
            | MjolnrCommand::AddReviewNote { .. }
            | MjolnrCommand::AddReviewComment { .. }
            | MjolnrCommand::SendReviewNotes { .. }
            | MjolnrCommand::ResolveCouncilFinding { .. }
            | MjolnrCommand::ProposeCouncilAmendment { .. }
            | MjolnrCommand::OpenDecisionTicket { .. }
            | MjolnrCommand::ResolveDecisionTicket { .. }
            | MjolnrCommand::ImportWorkItem { .. }
            | MjolnrCommand::RefreshImportedItem { .. }
            | MjolnrCommand::LaunchExternalAgent { .. }
            | MjolnrCommand::StopExternalAgent { .. }
            | MjolnrCommand::ImportExternalAgentChanges { .. }
            | MjolnrCommand::OpenProject { .. }
            | MjolnrCommand::RefreshRepository
            | MjolnrCommand::SaveFile { .. } => {
                // All six families are routed through acknowledged mail:
                // `Runtime::dispatch` sends plan commands via
                // `Mail::PlanCommand`, child-run commands via
                // `Mail::ChildRunCommand`, repository commands via
                // `Mail::RepositoryCommand`, integration commands via
                // `Mail::IntegrationCommand`, review commands via
                // `Mail::ReviewCommand`, and workspace commands via
                // `Mail::WorkspaceCommand`, so they never arrive here. The
                // arm exists for exhaustiveness — `Mail` is private to this
                // module — and it refuses to act rather than making the
                // generic command path a second authority surface.
            }
        }
    }

    /// The Phase D2 contract is on the wire but child-run execution is not
    /// implemented. Every child-run command is refused with
    /// [`ReasonCode::WorkspaceCapabilityUnavailable`] — a typed refusal the
    /// bridge renders, not a panic and not a fabricated success (AGENTS.md §2,
    /// §3). Execution lands in the phase that wires `subagent::worktree` to
    /// this handler; until then this is deliberately the only behaviour.
    fn handle_child_run_command(command: &MjolnrCommand) -> Result<(), MjolnrError> {
        Err(Self::child_run_unavailable(command))
    }

    fn child_run_unavailable(command: &MjolnrCommand) -> MjolnrError {
        let capability = match command {
            MjolnrCommand::CreateWorktree { .. } => "createWorktree",
            MjolnrCommand::ForkWork { .. } => "forkWork",
            MjolnrCommand::StartChild { .. } => "startChild",
            MjolnrCommand::CancelChild { .. } => "cancelChild",
            MjolnrCommand::PreserveBranch { .. } => "preserveBranch",
            MjolnrCommand::SettleChild { .. } => "settleChild",
            MjolnrCommand::DiscardSettledWorktree { .. } => "discardSettledWorktree",
            _ => "childRun",
        };
        MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            format!(
                "Capability '{capability}' is unavailable: child-run execution is not yet \
                 implemented; the command was refused and nothing ran"
            ),
        )
    }

    /// Execute one Phase D5 repository command against the open project.
    ///
    /// Three properties this function exists to hold:
    ///
    /// - **No project, no effect.** Without an open workspace root there is no
    ///   repository to name, so the command is refused rather than run against
    ///   the process's current directory.
    /// - **Blocking git runs on a blocking thread.** `Repository` is
    ///   deliberately synchronous, so it goes through `spawn_blocking` and
    ///   never stalls the actor's mailbox (AGENTS.md §4).
    /// - **The caller learns the outcome.** The reply carries success, a typed
    ///   refusal, or an uncertain effect — the last of which is neither.
    async fn handle_repository_command(
        &mut self,
        command: MjolnrCommand,
    ) -> Result<(), MjolnrError> {
        if let MjolnrCommand::CloneProject {
            source,
            destination,
        } = command
        {
            return self.handle_clone_project(source, destination).await;
        }

        let Some(root) = self.state.workspace_root.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "No project is open, so there is no repository to act on; the command was \
                 refused and nothing ran",
            ));
        };

        let outcome = tokio::task::spawn_blocking(move || {
            let repository = crate::repository::Repository::open(root)?;
            match command {
                MjolnrCommand::StagePaths { paths } => repository.stage_paths(&paths),
                MjolnrCommand::Unstage { paths } => repository.unstage_paths(&paths),
                MjolnrCommand::StageHunks { path, hunk_indices } => {
                    repository.stage_hunks(&path, &hunk_indices)
                }
                MjolnrCommand::CreateBranch {
                    name,
                    base_revision,
                } => repository.create_branch(&name, &base_revision),
                MjolnrCommand::Commit {
                    message,
                    expected_index_revision,
                } => repository
                    .commit(&message, &expected_index_revision)
                    .map(drop),
                MjolnrCommand::IntegrateChildBranch {
                    name,
                    message,
                    expected_head,
                } => repository
                    .integrate_child_branch(&name, &message, &expected_head)
                    .map(drop),
                MjolnrCommand::Fetch => repository.fetch(),
                MjolnrCommand::Push { expected_head } => repository.push(&expected_head),
                MjolnrCommand::IntegrateUpstream {
                    message,
                    expected_head,
                } => repository
                    .integrate_upstream(&message, &expected_head)
                    .map(drop),
                MjolnrCommand::Rebase {
                    onto,
                    expected_head,
                } => repository.rebase(&onto, &expected_head).map(drop),
                MjolnrCommand::AbortRebase => repository.abort_rebase(),
                // `Runtime::dispatch` routes only the repository variants
                // here. A routing bug becomes a typed refusal, not a panic.
                _ => Err(crate::repository::RepositoryError::CapabilityUnavailable {
                    capability: "repositoryCommand",
                }),
            }
        })
        .await;

        // Re-read on every outcome, not only success. A refusal usually leaves
        // the repository untouched, but "usually" is not evidence, and
        // `UncertainEffect` exists precisely because a command can fail after
        // changing something. Refreshing only on success would leave the
        // surface most wrong in the case that matters most.
        self.refresh_repository(RefreshTrigger::RepositoryCommand)
            .await;

        match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(MjolnrError::workspace_refused(
                error.reason_code(),
                error.to_string(),
            )),
            // The blocking task died mid-git. mjolnr cannot prove the operation
            // did not happen, so this is an uncertain effect requiring a human
            // decision — never a retry (AGENTS.md §1.4).
            Err(join_error) => Err(MjolnrError::workspace_refused(
                ReasonCode::RepositoryUncertainEffect,
                format!(
                    "The repository task ended before reporting an outcome ({join_error}); \
                     mjolnr cannot prove whether the operation took effect"
                ),
            )),
        }
    }

    async fn handle_clone_project(
        &mut self,
        source: String,
        destination: std::path::PathBuf,
    ) -> Result<(), MjolnrError> {
        if self.run.is_some() {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::RunActive,
                "the project cannot change while a run is active; cancel the run first",
            ));
        }
        if self.state.session.is_some() {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceRootLocked,
                "the project cannot change while a session is open; end the session first",
            ));
        }
        let outcome = tokio::task::spawn_blocking(move || {
            crate::repository::Repository::clone_project(&source, destination)
        })
        .await;
        let destination = match outcome {
            Ok(Ok(destination)) => destination,
            Ok(Err(error)) => {
                return Err(MjolnrError::workspace_refused(
                    error.reason_code(),
                    error.to_string(),
                ));
            }
            Err(join_error) => {
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::RepositoryUncertainEffect,
                    format!(
                        "the clone task ended before reporting an outcome ({join_error}); mjolnr cannot prove whether the clone took effect"
                    ),
                ));
            }
        };
        self.open_project(destination).await
    }

    /// Re-read the repository and replace the published view (Phase D5
    /// producer).
    ///
    /// Called only from the four [`RefreshTrigger`] sites. There is no timer
    /// and no filesystem watcher, so this is the complete set of moments at
    /// which the projection can change — and why the projection records *when*
    /// it was captured instead of claiming to be current (AGENTS.md §1.3).
    ///
    /// Never returns a refusal to a caller: a refresh is a read the runtime
    /// performs for itself, and a repository it could not read becomes
    /// [`RepositoryView::Unavailable`] on the snapshot rather than an error
    /// thrown at whatever happened to trigger it. A failed refresh must not
    /// turn a successful commit into a reported failure.
    ///
    /// The capture counter advances only on a completed read, so a client
    /// cannot mistake a failed refresh for new data.
    async fn refresh_repository(&mut self, trigger: RefreshTrigger) {
        let Some(root) = self.state.workspace_root.clone() else {
            self.state.repository = RepositoryView::NoProject;
            self.state.changes = ChangeView::NoProject;
            return;
        };
        let sequence = self.state.repository_captures.saturating_add(1);

        // One blocking task for both reads, not two: a change set captured from
        // a different task could straddle a write and describe a HEAD the
        // status beside it never saw. They still cost two `git` invocations —
        // no single command answers both — which is why neither claims to be
        // current, only to have been captured together.
        let outcome = tokio::task::spawn_blocking(move || {
            let repository = crate::repository::Repository::open(root)?;
            let projection = repository.project(trigger, sequence)?;
            let capture = repository.capture_changes(&projection)?;
            Ok::<_, crate::repository::RepositoryError>((projection, capture))
        })
        .await;

        let outcome = match outcome {
            Ok(Ok((projection, capture))) => {
                self.state.changes = ChangeView::Captured(Box::new(capture));
                Ok(Ok(projection))
            }
            // A failed read leaves no capture to pair with the refusal, and the
            // change view carries the same refusal rather than an empty list —
            // an empty change set renders as "nothing has changed", which is a
            // claim mjolnr did not earn.
            Ok(Err(error)) => {
                self.state.changes = ChangeView::Unavailable {
                    code: error.reason_code(),
                    detail: error.to_string(),
                };
                Ok(Err(error))
            }
            Err(join_error) => {
                self.state.changes = ChangeView::Unavailable {
                    code: ReasonCode::ToolExecution,
                    detail: format!("the repository read did not complete: {join_error}"),
                };
                Err(join_error)
            }
        };

        self.state.repository = match outcome {
            Ok(Ok(projection)) => {
                self.state.repository_captures = sequence;
                RepositoryView::Projected(Box::new(projection))
            }
            // Most often "not a git repository", which is an ordinary state for
            // an open project, not a fault. It is reported rather than left as
            // a stale earlier projection, because a projection from a previous
            // root is worse than none.
            Ok(Err(error)) => RepositoryView::Unavailable {
                code: error.reason_code(),
                detail: error.to_string(),
            },
            // A read that did not finish wrote nothing — `project` only reads —
            // so this is a failed read, not an uncertain effect.
            Err(join_error) => RepositoryView::Unavailable {
                code: ReasonCode::ToolExecution,
                detail: format!("the repository read did not complete: {join_error}"),
            },
        };

        // Publishing is part of the refresh, not the caller's job. Without it
        // the new view sits in actor state where no client can see it — which
        // is how the first draft of this producer passed its open-a-project
        // test (`refresh_session_list` happens to publish) and failed every
        // other trigger. Safe to publish here because this method touches only
        // the repository fields: unlike `create_session`, there is no
        // half-applied state for a subscriber to observe.
        self.publish_snapshot();
    }

    /// Read one contained directory page or file from the open project
    /// (Phase D7 producer).
    ///
    /// Three properties this function exists to hold:
    ///
    /// - **No project, no read.** Without an open workspace root there is no
    ///   containment boundary to check against, so the query is refused rather
    ///   than run against the process's current directory. This is the same
    ///   first line `handle_repository_command` opens with, and for the same
    ///   reason.
    /// - **Blocking I/O runs on a blocking thread.** `workspace_files` is
    ///   deliberately synchronous, so it goes through `spawn_blocking` and never
    ///   stalls the actor's mailbox (AGENTS.md §4).
    /// - **Nothing published changes.** A listing is one client's current
    ///   directory. Publishing it would put one client's navigation into every
    ///   other client's snapshot, and would make a read look like a state
    ///   change on the wire.
    ///
    /// The composition here is the reason `workspace_files` does not run git:
    /// the ignore answer is the repository producer's, the walk is the file
    /// producer's, and this is the one place that holds both (AGENTS.md §2.3).
    /// A project that is not a repository is not an error — `ignored_under`
    /// simply has nobody to ask, and every entry reports `ignored: false`,
    /// which `DirectoryEntry::ignored` documents as also meaning "unasked".
    async fn read_workspace_files(
        &mut self,
        request: crate::core::workspace_files::WorkspaceFileRequest,
    ) -> Result<crate::core::workspace_files::WorkspaceFileAnswer, MjolnrError> {
        use crate::core::workspace_files::{WorkspaceFileAnswer, WorkspaceFileRequest};

        let Some(root) = self.state.workspace_root.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "No project is open, so there is no workspace to read files from; the request \
                 was refused and nothing was read",
            ));
        };

        let outcome = tokio::task::spawn_blocking(move || match request {
            WorkspaceFileRequest::Directory {
                path,
                page,
                page_size,
            } => {
                // Best-effort by construction: a project with no repository, or
                // a git that would not answer, yields an empty set rather than
                // failing the listing. An explorer that refused to draw a
                // directory because git was unavailable would be useless on
                // every non-repository project, and "ignored" is metadata on a
                // listing rather than the listing itself.
                let ignored = crate::repository::Repository::open(root.clone())
                    .and_then(|repository| repository.ignored_under(&path))
                    .unwrap_or_default();
                crate::workspace_files::list_directory(&root, &path, page, page_size, &ignored)
                    .map(|listing| WorkspaceFileAnswer::Directory(Box::new(listing)))
            }
            WorkspaceFileRequest::File { path } => crate::workspace_files::read_file(&root, &path)
                .map(|read| WorkspaceFileAnswer::File(Box::new(read))),
        })
        .await;

        match outcome {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(error)) => Err(MjolnrError::workspace_refused(
                error.reason_code(),
                error.to_string(),
            )),
            // A read that did not finish wrote nothing, so this is a failed
            // read and never an uncertain effect. The distinction matters:
            // `RepositoryUncertainEffect` asks a human to decide, and there is
            // nothing here for them to decide about.
            Err(join_error) => Err(MjolnrError::workspace_refused(
                ReasonCode::ToolExecution,
                format!("the file read did not complete: {join_error}"),
            )),
        }
    }

    async fn submit_imported_comment(
        &mut self,
        integration: &str,
        remote_id: &str,
        expected_revision: &str,
        body: &str,
    ) -> Result<(), MjolnrError> {
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;
        let producer = self.resolve_comment_producer(integration)?;
        let item_id = *self
            .state
            .imported_items
            .values()
            .find(|item| item.integration == integration && item.remote_id == remote_id)
            .map(|item| &item.id)
            .ok_or_else(|| {
                MjolnrError::workspace_refused(
                    ReasonCode::SchemaInvalid,
                    format!("no imported item {integration}/{remote_id} in this session"),
                )
            })?;
        let comment_id = producer
            .submit_comment(remote_id, expected_revision, body)
            .await
            .map_err(|error| {
                let code = error.reason_code();
                MjolnrError::workspace_refused(code, error.to_string())
            })?;
        let event = MjolnrEvent::ImportedCommentRecorded {
            session,
            item_id,
            comment_id,
            body: body.to_owned(),
        };
        self.state.validate_event(&event)?;
        self.persist(event)
            .await
            .map(|_| ())
            .map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            })
    }

    fn resolve_comment_producer(
        &self,
        source: &str,
    ) -> Result<Arc<dyn crate::integrations::TaskSource>, MjolnrError> {
        if !matches!(source, "github" | "linear") {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("integration '{source}' has no comment producer"),
            ));
        }
        if let Some(producer) = self.task_sources.get(source).cloned() {
            return Ok(producer);
        }
        match source {
            "github" => crate::integrations::github::GitHubSource::from_environment()
                .map(|producer| Arc::new(producer) as Arc<dyn crate::integrations::TaskSource>)
                .map_err(|error| {
                    MjolnrError::workspace_refused(error.reason_code(), error.to_string())
                }),
            "linear" => crate::integrations::linear::LinearSource::from_environment()
                .map(|producer| Arc::new(producer) as Arc<dyn crate::integrations::TaskSource>)
                .map_err(|error| {
                    MjolnrError::workspace_refused(error.reason_code(), error.to_string())
                }),
            _ => Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("integration '{source}' has no comment producer"),
            )),
        }
    }

    async fn handle_integration_command(
        &mut self,
        command: &MjolnrCommand,
    ) -> Result<(), MjolnrError> {
        if let MjolnrCommand::FetchTask { source, task_id } = command {
            return self.fetch_and_record_task(source, task_id).await;
        }
        if let MjolnrCommand::FetchTasks { source, task_ids } = command {
            return self.fetch_and_record_tasks(source, task_ids).await;
        }
        if let MjolnrCommand::SubmitChange {
            source,
            remote_id,
            expected_revision,
            title,
            body,
            head_commit,
            head_branch,
            base_branch,
        } = command
        {
            crate::core::imported::check_act_pin(
                self.state.imported_items.values(),
                source,
                remote_id,
                expected_revision,
            )
            .map_err(|refusal| {
                let code = match refusal {
                    crate::core::imported::ActRefusal::StaleRevision { .. } => {
                        ReasonCode::WorkspaceStaleRevision
                    }
                    crate::core::imported::ActRefusal::NeverImported { .. } => {
                        ReasonCode::SchemaInvalid
                    }
                };
                MjolnrError::workspace_refused(code, refusal.to_string())
            })?;
            let request = crate::integrations::RemoteChangeRequest::new(
                remote_id,
                expected_revision,
                title,
                body,
                head_commit,
                head_branch,
                base_branch,
            )
            .map_err(|error| {
                MjolnrError::workspace_refused(ReasonCode::SchemaInvalid, error.to_string())
            })?;
            return self.submit_change(source, request).await;
        }
        if let MjolnrCommand::SubmitImportedComment {
            integration,
            remote_id,
            expected_revision,
            body,
        } = command
        {
            crate::core::imported::check_act_pin(
                self.state.imported_items.values(),
                integration,
                remote_id,
                expected_revision,
            )
            .map_err(|refusal| {
                let code = match refusal {
                    crate::core::imported::ActRefusal::StaleRevision { .. } => {
                        ReasonCode::WorkspaceStaleRevision
                    }
                    crate::core::imported::ActRefusal::NeverImported { .. } => {
                        ReasonCode::SchemaInvalid
                    }
                };
                MjolnrError::workspace_refused(code, refusal.to_string())
            })?;
            if body.len() > crate::integrations::MAX_REMOTE_BODY_BYTES {
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::SchemaInvalid,
                    format!(
                        "comment body may not exceed {} bytes",
                        crate::integrations::MAX_REMOTE_BODY_BYTES
                    ),
                ));
            }
            if body
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            {
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::SchemaInvalid,
                    "comment body may not contain control characters other than newline and tab",
                ));
            }
            return self
                .submit_imported_comment(integration, remote_id, expected_revision, body)
                .await;
        }
        Err(MjolnrError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "integration command is unavailable",
        ))
    }

    async fn submit_change(
        &mut self,
        source: &str,
        request: crate::integrations::RemoteChangeRequest,
    ) -> Result<(), MjolnrError> {
        if source != "github" {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("integration '{source}' has no submit producer"),
            ));
        }
        let producer = self.resolve_submission_producer(source)?;
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;
        let run = RunId::new();
        let call = ToolCall {
            id: format!("submit_change_{run}"),
            name: "submit_change".to_owned(),
            arguments: serde_json::to_value(&request).unwrap_or_else(|_| serde_json::json!({})),
            provider_signature: None,
        };
        let preview = format!(
            "create GitHub pull request for {}: {} ({}) -> {}",
            request.remote_id, request.head_branch, request.head_commit, request.base_branch
        );
        self.persist(MjolnrEvent::RunStarted { session, run })
            .await
            .map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            })?;
        self.persist(MjolnrEvent::ToolProposed {
            session,
            run,
            approval: None,
            call: call.clone(),
            tier: crate::core::tool::ToolTier::Execute,
            preview,
        })
        .await
        .map_err(|error| MjolnrError::Store {
            detail: error.to_string(),
        })?;

        if let Err(error) = self.verify_submission_head(&request).await {
            return self
                .finish_submission_failure(
                    session,
                    run,
                    &call,
                    error.reason_code().unwrap_or(ReasonCode::ToolExecution),
                    error.to_string(),
                )
                .await;
        }
        self.execute_submission(producer, request, session, run, call)
            .await
    }

    fn resolve_submission_producer(
        &self,
        source: &str,
    ) -> Result<Arc<dyn crate::integrations::TaskSource>, MjolnrError> {
        if source != "github" {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("integration '{source}' has no submit producer"),
            ));
        }
        if let Some(producer) = self.task_sources.get(source).cloned() {
            return Ok(producer);
        }
        crate::integrations::github::GitHubSource::from_environment()
            .map(|producer| Arc::new(producer) as Arc<dyn crate::integrations::TaskSource>)
            .map_err(|error| MjolnrError::workspace_refused(error.reason_code(), error.to_string()))
    }

    async fn verify_submission_head(
        &self,
        request: &crate::integrations::RemoteChangeRequest,
    ) -> Result<(), MjolnrError> {
        let root = self
            .state
            .workspace_root
            .clone()
            .ok_or(MjolnrError::NoSession)?;
        let expected_head = request.head_commit.clone();
        let expected_branch = request.head_branch.clone();
        tokio::task::spawn_blocking(move || {
            crate::repository::Repository::open(root).and_then(|repository| {
                repository.verify_head_and_branch(&expected_head, &expected_branch)
            })
        })
        .await
        .map_err(|error| {
            MjolnrError::workspace_refused(ReasonCode::ToolExecution, error.to_string())
        })?
        .map_err(|error| MjolnrError::workspace_refused(error.reason_code(), error.to_string()))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one submission mapping that now records both submitted and uncertain imported acts"
    )]
    async fn execute_submission(
        &mut self,
        producer: Arc<dyn crate::integrations::TaskSource>,
        request: crate::integrations::RemoteChangeRequest,
        session: SessionId,
        run: RunId,
        call: ToolCall,
    ) -> Result<(), MjolnrError> {
        match producer.submit_change(&request).await {
            Ok(remote_url) => {
                let item_id = self
                    .state
                    .imported_items
                    .values()
                    .find(|item| {
                        item.integration == "github" && item.remote_id == request.remote_id
                    })
                    .map_or_else(crate::core::imported::ImportedItemId::new, |item| item.id);
                let act = crate::core::imported::ImportedAct {
                    act_id: crate::core::imported::ImportedActId::new(),
                    item_id,
                    kind: crate::core::imported::ImportedActKind::PullRequest,
                    expected_revision: request.expected_revision.clone(),
                    head_branch: request.head_branch.clone(),
                    base_branch: request.base_branch.clone(),
                    outcome: crate::core::imported::ImportedActOutcome::Submitted {
                        remote_url: remote_url.clone(),
                    },
                };
                let act_event = MjolnrEvent::ImportedActRecorded { session, act };
                if self.state.validate_event(&act_event).is_ok() {
                    let _ = self
                        .persist(act_event)
                        .await
                        .map_err(|error| MjolnrError::Store {
                            detail: error.to_string(),
                        });
                }
                let result = crate::core::message::ToolResult::ok(remote_url);
                self.persist(MjolnrEvent::ToolCompleted {
                    session,
                    run,
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    result,
                })
                .await
                .map_err(|error| MjolnrError::Store {
                    detail: error.to_string(),
                })?;
                self.persist(MjolnrEvent::RunFinished {
                    session,
                    run,
                    reason: FinishReason::Stop,
                })
                .await
                .map_err(|error| MjolnrError::Store {
                    detail: error.to_string(),
                })?;
                Ok(())
            }
            Err(error) if error.requires_recovery() => {
                let item_id = self
                    .state
                    .imported_items
                    .values()
                    .find(|item| {
                        item.integration == "github" && item.remote_id == request.remote_id
                    })
                    .map(|item| item.id);
                if let Some(item_id) = item_id {
                    let act = crate::core::imported::ImportedAct {
                        act_id: crate::core::imported::ImportedActId::new(),
                        item_id,
                        kind: crate::core::imported::ImportedActKind::PullRequest,
                        expected_revision: request.expected_revision.clone(),
                        head_branch: request.head_branch.clone(),
                        base_branch: request.base_branch.clone(),
                        outcome: crate::core::imported::ImportedActOutcome::Uncertain,
                    };
                    let act_event = MjolnrEvent::ImportedActRecorded { session, act };
                    if self.state.validate_event(&act_event).is_ok() {
                        let _ = self
                            .persist(act_event)
                            .await
                            .map_err(|error| MjolnrError::Store {
                                detail: error.to_string(),
                            });
                    }
                }
                self.recovery = crate::core::recovery::RecoveryState::Required(
                    crate::core::recovery::InterruptedWork {
                        run,
                        kind: crate::core::recovery::InterruptedKind::EffectUncertain {
                            authority: crate::core::recovery::Authority::Policy,
                            call,
                            tier: crate::core::tool::ToolTier::Execute,
                            preview: "GitHub pull-request submission".to_owned(),
                        },
                    },
                );
                self.publish_snapshot();
                Err(MjolnrError::workspace_refused(
                    ReasonCode::RecoveryRequiresDecision,
                    error.to_string(),
                ))
            }
            Err(error) => {
                self.finish_submission_failure(
                    session,
                    run,
                    &call,
                    error.reason_code(),
                    error.to_string(),
                )
                .await
            }
        }
    }

    async fn finish_submission_failure(
        &mut self,
        session: SessionId,
        run: RunId,
        call: &ToolCall,
        code: ReasonCode,
        detail: String,
    ) -> Result<(), MjolnrError> {
        self.persist(MjolnrEvent::ToolFailed {
            session,
            run,
            call_id: call.id.clone(),
            name: call.name.clone(),
            code,
            detail: detail.clone(),
        })
        .await
        .map_err(|error| MjolnrError::Store {
            detail: error.to_string(),
        })?;
        self.persist(MjolnrEvent::RunFailed {
            session,
            run,
            code,
            detail: detail.clone(),
        })
        .await
        .map_err(|error| MjolnrError::Store {
            detail: error.to_string(),
        })?;
        Err(MjolnrError::workspace_refused(code, detail))
    }

    /// Read one item from an integration and record it — as a fetch the first
    /// time a remote is seen, and as a *pinned refresh* every time after.
    ///
    /// The order is the contract. Every refusal that can be decided from
    /// recorded state — no session, or an integration with no producer —
    /// happens *before* the credential is read and before anything leaves the
    /// machine. Only then does the fetch run, and only a fetch that returned
    /// something becomes an event.
    ///
    /// A remote this session already holds is not refused and not merged into a
    /// second row: it becomes a refresh. The fetched content is attached to the
    /// *existing* board id and blockers, and the event is pinned to the revision
    /// the record holds — so [`ImportedItemRecord::apply_refresh`] (reached
    /// through `validate_event`) is the one staleness guard both the live fetch
    /// and the replayed fold apply, rather than the fetch path re-deriving one.
    /// A refresh that finds the remote unchanged is refused `SameRevision`
    /// rather than re-recording the same state, because re-recording would hide
    /// that the remote moved.
    ///
    /// The fetch is awaited inside the actor, like the repository commands that
    /// shell out to `git fetch`. It is bounded by the producer's own request
    /// timeout, and it is human-initiated: the same shape §D5 already accepted.
    async fn fetch_and_record_task(
        &mut self,
        source: &str,
        task_id: &str,
    ) -> Result<(), MjolnrError> {
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;

        let injected = self.task_sources.get(source).cloned();

        // A remote this session already holds routes to a refresh, not a second
        // row: the same item, pinned to the recorded revision, advanced to what
        // the remote says now. `apply_refresh` is the guard that decides whether
        // that is allowed — reached through `validate_event`, so the live path
        // and the replay path apply it — and the recorded `fetched_revision` is
        // the pin. `blocked_by` is mjolnr's own ordering and is preserved: a
        // refresh is about the remote's content, and must not let a remote wipe
        // the blocking graph a human recorded.
        let existing = self
            .state
            .imported_items
            .values()
            .find(|item| item.integration == source && item.remote_id == task_id)
            .cloned();

        let producer: Arc<dyn crate::integrations::TaskSource> = match injected {
            Some(source) => source,
            None => match source {
                "github" => Arc::new(
                    crate::integrations::github::GitHubSource::from_environment()
                        .map_err(|error| integration_refusal(&error))?,
                ),
                "linear" => Arc::new(
                    crate::integrations::linear::LinearSource::from_environment()
                        .map_err(|error| integration_refusal(&error))?,
                ),
                "vercel" => Arc::new(
                    crate::integrations::vercel::VercelSource::from_environment()
                        .map_err(|error| integration_refusal(&error))?,
                ),
                "supabase" => Arc::new(
                    crate::integrations::supabase::SupabaseSource::from_environment()
                        .map_err(|error| integration_refusal(&error))?,
                ),
                _ => {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::WorkspaceCapabilityUnavailable,
                        format!(
                            "Capability 'fetchTask' is unavailable for integration '{source}': only \
                              'github', 'linear', 'vercel', and 'supabase' have producers; nothing was fetched and no \
                              credential was read"
                        ),
                    ));
                }
            },
        };
        let task = producer
            .fetch_task(task_id)
            .await
            .map_err(|error| integration_refusal(&error))?;

        let event = if let Some(record) = existing {
            let expected_revision = record.fetched_revision.clone();
            let item = task.into_imported_item(record.id, record.blocked_by);
            MjolnrEvent::ImportedItemRefreshed {
                session,
                expected_revision,
                item,
            }
        } else {
            let item =
                task.into_imported_item(crate::core::imported::ImportedItemId::new(), Vec::new());
            MjolnrEvent::ImportedItemFetched { session, item }
        };

        self.state.validate_event(&event)?;
        self.persist(event)
            .await
            .map(|_| ())
            .map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            })
    }

    /// Fetch a bounded list sequentially. The durable log is intentionally
    /// updated after each item rather than once at the end: if a later remote
    /// refuses, the earlier successful prefix remains truthful and the caller
    /// receives the first refusal without an automatic retry.
    async fn fetch_and_record_tasks(
        &mut self,
        source: &str,
        task_ids: &[String],
    ) -> Result<(), MjolnrError> {
        if task_ids.is_empty() || task_ids.len() > crate::core::client::MAX_FETCH_BATCH_SIZE {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::SchemaInvalid,
                format!(
                    "a task batch must contain 1-{} ids",
                    crate::core::client::MAX_FETCH_BATCH_SIZE
                ),
            ));
        }
        for (index, task_id) in task_ids.iter().enumerate() {
            if let Err(error) = self.fetch_and_record_task(source, task_id).await {
                if index == 0 {
                    return Err(error);
                }
                return Err(MjolnrError::workspace_refused(
                    error
                        .reason_code()
                        .unwrap_or(ReasonCode::WorkspaceCapabilityUnavailable),
                    format!(
                        "task batch stopped after {index} successful item(s) at {task_id}: {error}; earlier items remain recorded and were not retried"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// One Phase D3 review command: pin a note, add a remark, or send the
    /// selection to mjolnr.
    ///
    /// Every arm persists before it changes state, and the state change is the
    /// single reducer in `runtime::review` — the same one recovery replays
    /// through, so a note taken live and a note replayed after a restart are
    /// built by the same code.
    async fn handle_review_command(&mut self, command: MjolnrCommand) -> Result<(), MjolnrError> {
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;
        match command {
            MjolnrCommand::AddReviewNote {
                path,
                side,
                line,
                capture_digest,
                body,
            } => {
                let anchor =
                    review::anchor_note(&self.state.changes, &path, side, line, &capture_digest)?;
                self.record_review_event(MjolnrEvent::ReviewNoteRecorded {
                    session,
                    thread: crate::core::review::ReviewThreadId::new(),
                    anchor,
                    comment: review::comment(body),
                })
                .await
            }
            MjolnrCommand::AddReviewComment { thread, body } => {
                if !self.state.review_threads.contains_key(&thread) {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "There is no review thread {thread} in this session, so there is \
                             nothing to comment on; nothing was recorded"
                        ),
                    ));
                }
                self.record_review_event(MjolnrEvent::ReviewCommentAdded {
                    session,
                    thread,
                    comment: review::comment(body),
                })
                .await
            }
            MjolnrCommand::SendReviewNotes { threads } => {
                self.send_review_notes(session, threads).await
            }
            MjolnrCommand::ResolveCouncilFinding {
                review_id,
                finding_id,
                disposition,
                note,
            } => {
                if note
                    .as_ref()
                    .is_some_and(|note| note.len() > crate::core::client::MAX_COUNCIL_NOTE_BYTES)
                {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        "a council disposition note may not exceed 2048 bytes",
                    ));
                }
                let Some(review) = self.state.last_council.as_ref() else {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        "there is no completed council review in this session",
                    ));
                };
                if review.review_id != review_id {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        "the council review changed; refresh before choosing a finding",
                    ));
                }
                if !review
                    .findings
                    .iter()
                    .any(|finding| finding.id == finding_id)
                {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        "the council finding does not exist in the current review",
                    ));
                }
                self.record_review_event(MjolnrEvent::CouncilFindingDispositionRecorded {
                    session,
                    disposition: crate::core::council::CouncilFindingDisposition {
                        review_id,
                        finding_id,
                        disposition,
                        note,
                        decided_at: time::OffsetDateTime::now_utc(),
                    },
                })
                .await
            }
            MjolnrCommand::ProposeCouncilAmendment { review_id } => {
                self.propose_council_amendment(session, review_id).await
            }
            // `Runtime::dispatch` routes the review and council variants here.
            // A routing bug becomes a typed refusal, not a panic.
            _ => Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "a command was routed as a review command but no review handler claims it; \
                 nothing was recorded",
            )),
        }
    }

    /// Record one Phase E5 decision-ticket command.
    ///
    /// The shape is `handle_plan_command`'s: build the event, validate it
    /// against current state, then append — so an event that refuses
    /// validation can never enter durable history, and a live fold and a
    /// replay fold are the same reduction.
    async fn handle_board_command(&mut self, command: MjolnrCommand) -> Result<(), MjolnrError> {
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;
        let event = match command {
            MjolnrCommand::OpenDecisionTicket {
                question,
                kind,
                options,
                blocked_by,
            } => MjolnrEvent::DecisionTicketOpened {
                session,
                ticket: crate::core::board::DecisionTicket {
                    id: crate::core::board::DecisionTicketId::new(),
                    question,
                    kind,
                    options,
                    blocked_by,
                },
            },
            MjolnrCommand::ResolveDecisionTicket {
                ticket,
                chosen_option,
                note,
            } => {
                let Some(record) = self.state.decision_tickets.get(&ticket) else {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "There is no decision ticket {ticket} in this session; resolving \
                             would record a decision against nothing — nothing was recorded"
                        ),
                    ));
                };
                // The same check the reducer makes — done eagerly here so the
                // caller receives the typed refusal before any event exists,
                // after `validate_event` would otherwise make it only internal.
                if chosen_option >= record.ticket.options.len() {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::SchemaInvalid,
                        format!(
                            "Option {chosen_option} is not one of the {} recorded options on \
                             this ticket; a resolution must reference an option considered — \
                             nothing was recorded",
                            record.ticket.options.len()
                        ),
                    ));
                }
                MjolnrEvent::DecisionTicketResolved {
                    session,
                    resolution: crate::core::board::DecisionResolution {
                        id: crate::core::board::DecisionResolutionId::new(),
                        ticket,
                        // Verbatim copies, per ADR-0015: the resolution read
                        // without its question is not evidence of anything.
                        question: record.ticket.question.clone(),
                        options: record.ticket.options.clone(),
                        chosen_option,
                        // Stamped by the runtime, never taken from the client —
                        // the field's whole point is that a model cannot
                        // appear in it.
                        decided_by: crate::core::board::DecisionAuthor::Human,
                        decided_at: time::OffsetDateTime::now_utc(),
                        note,
                        // Changing your mind records a new resolution that
                        // supersedes by reference; never a mutation (ADR-0015).
                        supersedes: record.resolution.as_ref().map(|current| current.id),
                    },
                }
            }
            MjolnrCommand::ImportWorkItem { item } => {
                MjolnrEvent::ImportedItemFetched { session, item }
            }
            MjolnrCommand::RefreshImportedItem {
                expected_revision,
                item,
            } => MjolnrEvent::ImportedItemRefreshed {
                session,
                expected_revision,
                item,
            },
            MjolnrCommand::SubmitImportedComment { .. } => {
                // Board-acknowledged, not persisted here: comment durability is
                // `ImportedCommentRecorded` after the network effect, via the
                // integration path. Handle the board command as no-op so it
                // does not refuse as unknown while the runtime owns the type.
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::WorkspaceCapabilityUnavailable,
                    "use the integration path for imported comments; this board path records only imported items",
                ));
            }
            // `Runtime::dispatch` routes only the board variants here. A
            // routing bug becomes a typed refusal, not a panic.
            _ => {
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::WorkspaceCapabilityUnavailable,
                    "a command was routed as a board command but no board handler claims it; \
                     nothing was recorded",
                ));
            }
        };
        self.state.validate_event(&event)?;
        self.persist(event)
            .await
            .map(|_| ())
            .map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            })
    }

    /// Compose the amended artifact a human asked for, and record that it was
    /// composed. Nothing is written to the workspace here: the proposal goes
    /// back to the operator, who edits it and saves it through the ordinary
    /// governed save path, which re-checks the digest independently.
    async fn propose_council_amendment(
        &mut self,
        session: SessionId,
        review_id: crate::core::council::CouncilReviewId,
    ) -> Result<(), MjolnrError> {
        let Some(review) = self.state.last_council.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::SchemaInvalid,
                "there is no completed council review in this session",
            ));
        };
        if review.review_id != review_id {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::SchemaInvalid,
                "the council review changed; refresh before composing an amendment",
            ));
        }
        let Some(artifact) = review.artifact.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::SchemaInvalid,
                "this council reviewed a question rather than an artifact, so there is nothing to amend",
            ));
        };
        let Some(root) = self.state.workspace_root.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "No project is open, so the amendment was not composed",
            ));
        };

        // Re-read rather than trusting the council's captured text: the point
        // of the digest is to notice that the file moved underneath the review.
        let path = artifact.path.clone();
        let read =
            tokio::task::spawn_blocking(move || crate::workspace_files::read_file(&root, &path))
                .await;
        let read = match read {
            Ok(Ok(read)) => read,
            Ok(Err(error)) => {
                return Err(MjolnrError::workspace_refused(
                    error.reason_code(),
                    error.to_string(),
                ));
            }
            Err(error) => {
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::WorkspaceCapabilityUnavailable,
                    format!("The artifact read ended before reporting an outcome ({error})"),
                ));
            }
        };
        let crate::core::workspace_files::FileMode::Editable { text } = read.mode else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::SchemaInvalid,
                format!(
                    "`{}` is preview-only, so no amendment was composed",
                    artifact.path
                ),
            ));
        };

        let amendment = review
            .propose_amendment(&text, &read.digest)
            .map_err(|error| MjolnrError::workspace_refused(ReasonCode::SchemaInvalid, error))?;

        self.record_review_event(MjolnrEvent::CouncilAmendmentProposed {
            session,
            amendment: Box::new(amendment),
        })
        .await
    }

    /// Append a review event, then fold it in. Persist-then-project, so a
    /// thread a client can see is a thread the store already accepted.
    async fn record_review_event(&mut self, event: MjolnrEvent) -> Result<(), MjolnrError> {
        self.persist(event.clone())
            .await
            .map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            })?;
        review::apply_event(&mut self.state.review_threads, &event);
        self.publish_snapshot();
        Ok(())
    }

    /// Send the selected notes into the session as a human revision request.
    ///
    /// The ordering here is the whole design. `ReviewRequestSent` is appended
    /// **after** `start_run` has actually started a run, not before: the human's
    /// directive is already durable by then — `start_run` appends the message
    /// and the run marker before any provider request — so nothing is at risk,
    /// and recording the request first would let a run that failed to start
    /// leave threads marked `sent` when nothing was sent. The event asserts a
    /// request that demonstrably went out.
    ///
    /// The request is an ordinary [`DirectiveSource::Human`] directive. It
    /// grants nothing: no approval, no policy change, no budget. §D3 asks for a
    /// durable revision request, not a new authority.
    async fn send_review_notes(
        &mut self,
        session: SessionId,
        threads: Vec<crate::core::review::ReviewThreadId>,
    ) -> Result<(), MjolnrError> {
        let mut selected = Vec::with_capacity(threads.len());
        for id in &threads {
            let thread = self.state.review_threads.get(id).ok_or_else(|| {
                MjolnrError::workspace_refused(
                    ReasonCode::SchemaInvalid,
                    format!(
                        "There is no review thread {id} in this session; nothing was sent and \
                         no thread was marked sent"
                    ),
                )
            })?;
            selected.push(thread);
        }
        if self.run.is_some() {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "A run is already in flight, so the review request was not sent; wait for it to \
                 settle or cancel it first",
            ));
        }

        let text = review::request_text(&selected);
        self.start_run(text, &DirectiveSource::Human).await;

        let Some(run) = self.run.as_ref().map(|active| active.id) else {
            // `start_run` declined — no session model, a blocked session, or an
            // unavailable provider. Nothing was sent, so nothing is recorded:
            // a `sent` thread with no request behind it is exactly the false
            // claim §D3's negative tests exist for.
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "The session could not start a run, so the review request was not sent and no \
                 thread was marked sent",
            ));
        };

        self.record_review_event(MjolnrEvent::ReviewRequestSent {
            session,
            threads: threads.clone(),
            run,
        })
        .await?;

        // Held on the run rather than on session state so it cannot outlive the
        // run it belongs to: when the run ends, `self.run` is cleared and the
        // marker goes with it. A later, unrelated run can never inherit it and
        // report its answer as this request's.
        if let Some(active) = self.run.as_mut() {
            active.pending_review_threads = threads;
        }
        Ok(())
    }

    async fn handle_external_agent_command(
        &mut self,
        command: MjolnrCommand,
    ) -> Result<(), MjolnrError> {
        match command {
            MjolnrCommand::LaunchExternalAgent { profile } => {
                self.launch_external_agent(profile).await
            }
            MjolnrCommand::StopExternalAgent { id } => self.stop_external_agent(&id).await,
            MjolnrCommand::ImportExternalAgentChanges { id } => {
                self.import_external_agent_changes(&id).await
            }
            _ => Err(MjolnrError::workspace_refused(
                ReasonCode::SchemaInvalid,
                "not an external-agent command",
            )),
        }
    }

    async fn launch_external_agent(&mut self, profile: String) -> Result<(), MjolnrError> {
        let workspace_root = self.state.workspace_root.clone().ok_or_else(|| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "no project is open — open a project before launching an external agent",
            )
        })?;
        let project_context = self.context.clone();
        let catalog = project_context.external_agents();
        let discovered = catalog.get(&profile).cloned().ok_or_else(|| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("unknown external-agent profile `{profile}` — add .mjolnr/external-agent/{profile}.yaml"),
            )
        })?;
        let resolved = crate::runtime::external_agent::profile::resolve_executable(
            &discovered.profile.executable,
            &workspace_root,
        )?;
        let ext_id = uuid::Uuid::now_v7().to_string();
        let (worktree_path, branch) =
            crate::runtime::external_agent::worktree::create(&workspace_root, &ext_id).await?;
        let record = crate::runtime::external_agent::runner::spawn_external_agent(
            &discovered.profile,
            &resolved.display().to_string(),
            &worktree_path.display().to_string(),
            ext_id.clone(),
            branch,
        )
        .map_err(|detail| {
            MjolnrError::workspace_refused(ReasonCode::WorkspaceCapabilityUnavailable, detail)
        })?;
        self.external_agents.insert(record);
        self.publish_snapshot();
        Ok(())
    }

    async fn stop_external_agent(&mut self, id: &str) -> Result<(), MjolnrError> {
        let record = self.external_agents.get_mut(id).ok_or_else(|| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("no external agent `{id}`"),
            )
        })?;
        crate::runtime::external_agent::runner::stop_agent(record)
            .await
            .map_err(|detail| {
                MjolnrError::workspace_refused(ReasonCode::WorkspaceCapabilityUnavailable, detail)
            })?;
        self.publish_snapshot();
        Ok(())
    }

    async fn import_external_agent_changes(&mut self, id: &str) -> Result<(), MjolnrError> {
        let record = self.external_agents.get(id).ok_or_else(|| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("no external agent `{id}`"),
            )
        })?;
        let worktree = std::path::PathBuf::from(record.worktree.clone());
        let result = tokio::task::spawn_blocking(
            move || -> Result<_, crate::repository::RepositoryError> {
                let repo = crate::repository::Repository::open(worktree)?;
                let proj = repo.project(crate::core::repository::RefreshTrigger::Requested, 0)?;
                let view = repo.capture_changes(&proj)?;
                Ok(view)
            },
        )
        .await
        .map_err(|e| {
            MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                e.to_string(),
            )
        })?;
        match result {
            Ok(_) => {}
            Err(e) => {
                return Err(MjolnrError::workspace_refused(
                    e.reason_code(),
                    e.to_string(),
                ));
            }
        }
        self.publish_snapshot();
        Ok(())
    }

    /// Workspace-scoped commands, acknowledged so the caller learns the typed
    /// refusal. The arm is a match rather than a direct call so the next
    /// workspace command joins the acknowledged path by construction instead of
    /// being dropped into the unacknowledged one.
    async fn handle_workspace_command(
        &mut self,
        command: MjolnrCommand,
    ) -> Result<(), MjolnrError> {
        match command {
            MjolnrCommand::OpenProject { root } => self.open_project(root).await,
            // A human asking what git says now. Refused when no project is
            // open, because "nothing to read" and "read and found nothing" are
            // different answers and the caller asked a question.
            MjolnrCommand::RefreshRepository => {
                if self.state.workspace_root.is_none() {
                    return Err(MjolnrError::workspace_refused(
                        ReasonCode::WorkspaceCapabilityUnavailable,
                        "No project is open, so there is no repository to read; nothing was run",
                    ));
                }
                self.refresh_repository(RefreshTrigger::Requested).await;
                Ok(())
            }
            MjolnrCommand::SaveFile {
                path,
                expected_digest,
                text,
            } => self.save_workspace_file(path, expected_digest, text).await,
            // Deliberately does not print the command: `MjolnrCommand` carries
            // credential-bearing variants, and a refusal message is exactly
            // the kind of string that ends up in a log (AGENTS.md §3).
            _ => Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "a command was routed as a workspace command but no workspace handler claims \
                 it; nothing was changed",
            )),
        }
    }

    /// Save an operator-edited file, record the completed effect, and refresh
    /// the repository/change projection. The filesystem write happens before
    /// the durable event; if the event append fails, the effect is uncertain
    /// and the session is blocked rather than reporting success or inviting a
    /// retry that could overwrite newer bytes.
    async fn save_workspace_file(
        &mut self,
        path: String,
        expected_digest: String,
        text: String,
    ) -> Result<(), MjolnrError> {
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;
        let Some(root) = self.state.workspace_root.clone() else {
            return Err(MjolnrError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "No project is open, so the file was not saved",
            ));
        };
        let request = crate::core::workspace_files::WorkspaceFileSaveRequest::new(
            path,
            expected_digest,
            text,
        );
        let outcome =
            tokio::task::spawn_blocking(move || crate::workspace_files::save_file(&root, request))
                .await;
        let saved = match outcome {
            Ok(Ok(saved)) => saved,
            Ok(Err(error)) => {
                return Err(MjolnrError::workspace_refused(
                    error.reason_code(),
                    error.to_string(),
                ));
            }
            Err(error) => {
                return Err(MjolnrError::workspace_refused(
                    ReasonCode::RepositoryUncertainEffect,
                    format!(
                        "The file-save task ended before reporting an outcome ({error}); mjolnr cannot prove whether the write took effect"
                    ),
                ));
            }
        };

        let event = MjolnrEvent::FileSaved {
            session,
            path: saved.path,
            observed_digest: saved.observed_digest,
            new_digest: saved.new_digest,
            size_bytes: saved.size_bytes,
        };
        if let Err(error) = self.persist(event).await {
            self.note_store_failure(&error);
            return Err(MjolnrError::workspace_refused(
                ReasonCode::RepositoryUncertainEffect,
                format!(
                    "The file was written but its operator-controlled record was not durable ({error}); do not retry until the file and event log are reviewed"
                ),
            ));
        }

        self.refresh_repository(RefreshTrigger::FileSave).await;
        Ok(())
    }

    async fn send_prompt_template(&mut self, name: String, arguments: String) {
        if let Some(text) = self.context.expand_prompt(&name, &arguments) {
            // A template is text the human could have typed: they invoked the
            // slash command, and the body came from a trust-gated location.
            self.start_run(text, &DirectiveSource::Human).await;
            return;
        }
        self.state.last_reload = Some(crate::core::context::ReloadReport {
            skills: self.context.skills().len(),
            prompts: self.context.prompts().templates().len(),
            changes: Vec::new(),
            failure: Some(format!("no prompt template named `{name}`")),
        });
        self.publish_snapshot();
    }

    async fn handle_plan_command(&mut self, command: MjolnrCommand) -> Result<(), MjolnrError> {
        if let MjolnrCommand::StartPlanInterview { goal } = command {
            return self.start_plan_interview(goal).await;
        }
        let session = self.state.session.ok_or(MjolnrError::NoSession)?;
        let answer = match &command {
            MjolnrCommand::AnswerPlanQuestion { answer, .. } => Some(answer.clone()),
            _ => None,
        };
        let event = match command {
            MjolnrCommand::AskPlanQuestion { plan_id, question } => {
                MjolnrEvent::PlanQuestionAsked {
                    session,
                    plan_id,
                    question,
                }
            }
            MjolnrCommand::AnswerPlanQuestion { plan_id, answer } => {
                MjolnrEvent::PlanQuestionAnswered {
                    session,
                    plan_id,
                    answer,
                }
            }
            MjolnrCommand::ProposePlan { proposal } => {
                MjolnrEvent::PlanProposed { session, proposal }
            }
            MjolnrCommand::ReviewPlan { review } => MjolnrEvent::PlanReviewed { session, review },
            MjolnrCommand::ApprovePlan { approval } => {
                MjolnrEvent::PlanApproved { session, approval }
            }
            MjolnrCommand::HandoffPlan { handoff } => {
                MjolnrEvent::PlanHandoffCreated { session, handoff }
            }
            _ => {
                return Err(MjolnrError::plan_invalid_transition(
                    "not a plan command",
                    "dispatch plan command",
                    "only plan workflow commands use the acknowledged path",
                ));
            }
        };
        self.state.validate_event(&event)?;
        self.persist(event)
            .await
            .map_err(|error| MjolnrError::Store {
                detail: error.to_string(),
            })?;
        if let Some(answer) = answer
            && let Some(plan) = self.state.plan.as_ref()
            && plan.interview_goal.is_some()
            && plan.prd.is_none()
        {
            self.start_run_with_plan(
                crate::runtime::interview::answer_prompt(&answer),
                &DirectiveSource::Internal,
                Some(PlanRun::Interview {
                    plan_id: plan.plan_id,
                }),
            )
            .await;
        }
        Ok(())
    }

    async fn start_run(&mut self, text: String, source: &DirectiveSource) {
        self.start_run_with_plan(text, source, None).await;
    }

    async fn start_run_with_plan(
        &mut self,
        text: String,
        source: &DirectiveSource,
        plan_run: Option<PlanRun>,
    ) {
        let (Some(session), Some(provider_id), Some(model)) = (
            self.state.session,
            self.state.provider.clone(),
            self.state.model.clone(),
        ) else {
            return;
        };

        if self.run.is_some() {
            return;
        }

        // A directive mjolnr did not get from its owner cannot run unattended
        // . Applied here, at the one door into autonomous work,
        // and *recorded* rather than applied quietly: a session that silently
        // stopped being full-auto would be a lie about its own state
        // (`AGENTS.md` §1.3), and the human needs to see why it changed.
        let capped = source.policy_ceiling(self.state.policy);
        if capped != self.state.policy {
            self.set_policy(capped).await;
        }

        // The declared per-model floor, applied at the same door and for the
        // same reason. Two independent ceilings, both
        // narrowing: who asked for this work, and which model is about to do
        // it. Order does not matter — each is a `min` — but both must run,
        // because a directive from the owner says nothing about the model and
        // a trusted model says nothing about the directive.
        self.apply_governance_floor().await;

        // : a session with unresolved interrupted work does not
        // continue autonomously. Refused here, at the one door into autonomous
        // work, rather than trusted to the UI — a client that forgot to check
        // would otherwise drive a session mjolnr has said it cannot reason about.
        if self.blocked().is_some() {
            self.publish_snapshot();
            return;
        }

        if self.provider(&provider_id).is_none() {
            return;
        }

        let run = RunId::new();
        let cancel = CancellationToken::new();

        // Derived before `text` moves into the durable message: the title is
        // display metadata cut from the same bytes the record keeps.
        let derived_title = durability::title_from_directive(&text);

        let message = CanonicalMessage::user(text);
        // The first message after a rewind is the branch point; every message
        // after it continues that branch normally.
        let stored = match self
            .persist_branching(MjolnrEvent::MessageAppended {
                session,
                message: Box::new(message.clone()),
            })
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                self.note_store_failure(&error);
                return;
            }
        };
        self.state.push_message(Some(stored.sequence), message);
        // The first directive the owner types names the session, so a list of
        // sessions reads as work rather than as seven copies of one folder
        // name. A failed rename leaves the folder-name title, which was true
        // before and stays true; naming never blocks the run that follows.
        if self.state.messages().len() == 1
            && durability::directive_names_session(source)
            && !derived_title.is_empty()
        {
            let _ = self.store.rename_session(session, derived_title).await;
        }

        // The run marker is durable before the provider request. The user
        // message comes first so a failed marker leaves an honest unanswered
        // message, not a phantom interrupted provider turn.
        if let Err(error) = self.persist(MjolnrEvent::RunStarted { session, run }).await {
            self.note_store_failure(&error);
            return;
        }

        self.run = Some(ActiveRun {
            id: run,
            session,
            provider: provider_id,
            model: model.clone(),
            cancel: cancel.clone(),
            accumulator: StreamAccumulator::default(),
            pending_tools: VecDeque::new(),
            awaiting_approval: None,
            pending_load_authority: None,
            phase: RunPhase::Provider,
            provider_turns: 0,
            tool_calls: 0,
            intent: RunIntent::Normal,
            pending_drain: None,
            hard_stop: None,
            handoff_target: None,
            pending_review_threads: Vec::new(),
            plan_run,
        });

        self.state.pending_approval = None;
        self.state.budget.provider_turns = 0;
        self.state.budget.tool_calls = 0;

        self.publish_snapshot();

        let mailbox = self.mailbox.clone();
        let wall_time = self.limits.max_wall_time;
        let budget_cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = budget_cancel.cancelled() => {}
                () = tokio::time::sleep(wall_time) => {
                    let _ = mailbox.send(Mail::BudgetExpired { run }).await;
                }
            }
        });

        self.begin_provider_turn(run).await;
    }

    async fn cancel_run(&mut self) {
        let Some(active) = self.run.as_ref() else {
            return;
        };
        let run = active.id;
        let waiting = active.phase == RunPhase::Approval;
        active.cancel.cancel();
        if waiting {
            self.finish_run(run, FinishReason::Cancelled).await;
        }
    }

    async fn exhaust_budget(&mut self, run: RunId) {
        let session = match self.run.as_ref().filter(|active| active.id == run) {
            Some(active) => {
                active.cancel.cancel();
                active.session
            }
            None => return,
        };
        if let Err(error) = self
            .persist(MjolnrEvent::BudgetExhausted { session, run })
            .await
        {
            self.halt_for_store(run, &error);
            return;
        }
        self.fail_run(
            run,
            ReasonCode::BudgetExhausted,
            "run budget exhausted".to_owned(),
        )
        .await;
    }

    async fn finish_run(&mut self, run: RunId, reason: FinishReason) {
        let reason = match self
            .run
            .as_ref()
            .filter(|active| active.id == run)
            .map(|active| active.intent)
        {
            Some(RunIntent::ManualHandoff) => FinishReason::Handoff,
            Some(RunIntent::QuotaDrain) => FinishReason::QuotaDrained,
            Some(RunIntent::Normal) | None => reason,
        };
        if matches!(reason, FinishReason::Handoff | FinishReason::QuotaDrained)
            && let Err(error) = self.create_handoff_artifact(run).await
        {
            self.halt_for_store(run, &error);
            return;
        }
        if reason == FinishReason::QuotaDrained
            && let Err(error) = self.complete_quota_drain(run).await
        {
            self.halt_for_store(run, &error);
            return;
        }
        // A live handoff swaps provider/model only now, after the landing
        // checkpoint exists — never mid-turn. The swap is an
        // evidenced `ModelChanged`, so recovery replay and `/model` see it.
        if reason == FinishReason::Handoff {
            self.apply_handoff_swap(run).await;
        }
        let Some(session) = self
            .run
            .as_ref()
            .filter(|active| active.id == run)
            .map(|active| active.session)
        else {
            return;
        };
        if let Err(error) = self
            .persist(MjolnrEvent::RunFinished {
                session,
                run,
                reason,
            })
            .await
        {
            self.halt_for_store(run, &error);
            return;
        }
        let Some(active) = self.run.take_if(|active| active.id == run) else {
            return;
        };
        active.cancel.cancel();
        self.state.pending_approval = None;
        // One completed run is one turn against an envelope's clock (plan
        // §Phase 31). Counted here rather than per provider turn because the
        // human armed it against a stretch of *their* work, and a single
        // directive that took nine provider turns is still one thing they asked
        // for.
        self.tick_envelope().await;
        // : a checkpoint after each terminal run. It must follow the
        // terminal event, or it would summarise a run the history still shows as
        // open, and recovery would report interrupted work that finished.
        if let Err(error) = self
            .checkpoint(crate::core::store::SessionStatus::Active)
            .await
        {
            self.note_store_failure(&error);
        }
        self.publish_snapshot();
        self.trigger_background_consolidation();
    }

    async fn fail_run(&mut self, run: RunId, code: ReasonCode, detail: String) {
        let Some(session) = self
            .run
            .as_ref()
            .filter(|active| active.id == run)
            .map(|active| active.session)
        else {
            return;
        };
        if let Err(error) = self
            .persist(MjolnrEvent::RunFailed {
                session,
                run,
                code,
                detail,
            })
            .await
        {
            self.halt_for_store(run, &error);
            return;
        }
        let Some(active) = self.run.take_if(|active| active.id == run) else {
            return;
        };
        active.cancel.cancel();
        self.state.pending_approval = None;
        if let Err(error) = self
            .checkpoint(crate::core::store::SessionStatus::Active)
            .await
        {
            self.note_store_failure(&error);
        }
        self.publish_snapshot();
    }

    fn fail_store(&mut self, run: RunId, error: &StoreError) {
        self.halt_for_store(run, error);
    }

    fn provider(&self, id: &ProviderId) -> Option<Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|provider| &provider.id() == id)
            .map(Arc::clone)
    }
}

/// Carry an integration's typed outcome to the client without flattening it.
///
/// `IntegrationError` already knows which `ReasonCode` it is — a rejected
/// credential, a rate limit, a missing item, and a transport failure are four
/// things a human does four different things about — so this maps rather than
/// decides. The one thing it adds is the sentence that says nothing was
/// recorded, because a failed fetch that left a partial item on the board would
/// be the worst outcome available.
fn integration_refusal(error: &crate::integrations::IntegrationError) -> MjolnrError {
    let code = error.reason_code();
    MjolnrError::workspace_refused(code, format!("{error}; nothing was recorded"))
}
