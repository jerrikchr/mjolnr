//! The scheduler process: reuses the headless host to fire trigger directives
//! .
//!
//! Every firing dispatches through [`crate::headless::run`] on an ordinary
//! [`Runtime`] — the same host `mjolnr exec` drives — so a scheduled run's
//! transcript is, by construction, the transcript a manual headless run would
//! produce: same policy gate, same budgets, same approval-denial rule, same
//! evidence discipline. The only addition is identity: the firing session is
//! linked to its trigger's control session
//! ([`Runtime::spawn_trigger_host`]), and the control session records the
//! lifecycle decisions (skip/queue/replace, disable, re-arm) that have
//! nowhere else to live.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::context::ProjectContext;
use crate::core::command::MjolnrCommand;
use crate::core::error::ReasonCode;
use crate::core::event::{MjolnrEvent, SessionId};
use crate::core::mcp::McpServerSummary;
use crate::core::model::{ModelId, ProviderId};
use crate::core::policy::PolicyMode;
use crate::core::provider::Provider;
use crate::core::routing::RouteTable;
use crate::core::runtime::MjolnrRuntime;
use crate::core::store::{EventStore, StoreError};
use crate::core::trigger::TriggerOutcome;
use crate::headless::HeadlessOutcome;
use crate::runtime::{ChildLink, Runtime};
use crate::tools::ToolRegistry;

use super::control;
use super::definition::{TriggerDefinition, TriggerSource};
use super::overlap::{self, OverlapDecision};

/// How long a disabled trigger waits before checking for a re-arm.
const REARM_POLL: Duration = Duration::from_secs(30);

/// How long a firing session gets to become ready before the scheduler gives
/// up on it. Generous: a trigger process may be racing MCP server startup.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// A schedule trigger with no computable next firing (an unsatisfiable
/// expression, or a clock error) is re-checked on this cadence rather than
/// spinning.
const RECHECK_IDLE: Duration = Duration::from_hours(1);

/// Everything every firing needs, assembled once by the composition root
/// (`main.rs`), exactly as `mcp::connect_project` and `provider_registry` are
/// assembled once for `mjolnr exec`.
#[derive(Clone)]
#[allow(
    missing_debug_implementations,
    reason = "holds Arc<dyn Provider>/Arc<dyn EventStore> trait objects with no Debug bound, matching runtime::Actor's own allowance"
)]
pub struct SchedulerDeps {
    pub providers: Vec<Arc<dyn Provider>>,
    pub store: Arc<dyn EventStore>,
    pub workspace_root: PathBuf,
    pub project_context: ProjectContext,
    pub mcp_servers: Arc<Vec<McpServerSummary>>,
    pub tools: ToolRegistry,
    /// The project's routing config , loaded once like every
    /// other dependency here. Empty when the project has none — a firing then
    /// behaves exactly as it did before this phase.
    pub route_table: Arc<RouteTable>,
}

/// Run every configured trigger until `cancel` fires.
///
/// # Errors
/// A store failure opening the project. Individual trigger misfires never
/// propagate here — a broken trigger disables itself, it does not take down
/// the scheduler.
pub async fn run(deps: SchedulerDeps, cancel: CancellationToken) -> Result<(), StoreError> {
    let root_realpath = control::root_realpath(&deps.workspace_root)
        .unwrap_or_else(|_| deps.workspace_root.to_string_lossy().into_owned());
    let project = deps.store.open_project(deps.workspace_root.clone()).await?;
    let (definitions, _diagnostics) = super::definition::load_dir(&deps.workspace_root);

    let mut handles = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let deps = deps.clone();
        let cancel = cancel.clone();
        let root_realpath = root_realpath.clone();
        handles.push(tokio::spawn(async move {
            run_trigger(deps, project, root_realpath, definition, cancel).await;
        }));
    }

    cancel.cancelled().await;
    for handle in handles {
        let _ = handle.await;
    }
    Ok(())
}

/// One trigger's whole lifetime in this process.
async fn run_trigger(
    deps: SchedulerDeps,
    project: crate::core::store::ProjectId,
    root_realpath: String,
    definition: TriggerDefinition,
    cancel: CancellationToken,
) {
    let control_session = control::control_session_id(&root_realpath, &definition.name);
    if control::ensure(
        deps.store.as_ref(),
        project,
        control_session,
        &definition.name,
    )
    .await
    .is_err()
    {
        return;
    }

    let mut state = TriggerLoop {
        webhook_rx: spawn_webhook_listener_if_needed(&definition, &cancel),
        settle_tx_rx: mpsc::channel(2),
        in_flight: None,
        queued: false,
    };

    loop {
        match trigger_is_disabled(deps.store.as_ref(), control_session, &definition).await {
            Some(true) => {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(REARM_POLL) => {}
                }
                continue;
            }
            Some(false) => {}
            None => return,
        }

        if state
            .tick(&deps, control_session, &definition, &cancel)
            .await
            .is_break()
        {
            return;
        }
    }
}

/// The mutable state one iteration of [`run_trigger`]'s loop needs, moved out
/// of the function itself to keep its cognitive complexity within
/// `clippy.toml`'s backstop.
struct TriggerLoop {
    webhook_rx: Option<mpsc::Receiver<serde_json::Value>>,
    settle_tx_rx: (mpsc::Sender<Settlement>, mpsc::Receiver<Settlement>),
    in_flight: Option<CancellationToken>,
    queued: bool,
}

impl TriggerLoop {
    /// One iteration: wait for either a new occurrence or an in-flight
    /// firing's settlement, and act on it. `Break` means the caller must stop.
    async fn tick(
        &mut self,
        deps: &SchedulerDeps,
        control_session: SessionId,
        definition: &TriggerDefinition,
        cancel: &CancellationToken,
    ) -> std::ops::ControlFlow<()> {
        let (settle_tx, settle_rx) = &mut self.settle_tx_rx;
        tokio::select! {
            () = cancel.cancelled() => {
                if let Some(token) = self.in_flight.take() {
                    token.cancel();
                }
                return std::ops::ControlFlow::Break(());
            }
            payload = wait_for_occurrence(definition, &mut self.webhook_rx) => {
                handle_occurrence(
                    deps,
                    control_session,
                    definition,
                    payload,
                    &mut self.in_flight,
                    &mut self.queued,
                    settle_tx.clone(),
                )
                .await;
            }
            Some(settlement) = settle_rx.recv() => {
                self.in_flight = None;
                record_settlement(deps, control_session, definition, settlement).await;
                if self.queued {
                    self.queued = false;
                    handle_occurrence(
                        deps,
                        control_session,
                        definition,
                        None,
                        &mut self.in_flight,
                        &mut self.queued,
                        settle_tx.clone(),
                    )
                    .await;
                }
            }
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// Bind the local listener for a webhook-sourced trigger, or do nothing for a
/// schedule-sourced one.
fn spawn_webhook_listener_if_needed(
    definition: &TriggerDefinition,
    cancel: &CancellationToken,
) -> Option<mpsc::Receiver<serde_json::Value>> {
    let TriggerSource::Webhook { port, path } = &definition.source else {
        return None;
    };
    let (tx, rx) = mpsc::channel(1);
    let listen_cancel = cancel.clone();
    let port = *port;
    let path = path.clone();
    tokio::spawn(async move {
        let _ = super::webhook::listen(port, path, tx, listen_cancel).await;
    });
    Some(rx)
}

/// `Some(true)` if disabled, `Some(false)` if armed, `None` if the control
/// session's history could not be read (a store failure — the trigger stops
/// rather than firing on unknown state).
async fn trigger_is_disabled(
    store: &dyn EventStore,
    control_session: SessionId,
    definition: &TriggerDefinition,
) -> Option<bool> {
    let events = control::history(store, control_session).await.ok()?;
    let state = control::replay(&events, &definition.name);
    Some(state.disabled_reason.is_some())
}

/// Wait until the trigger's source says "fire now", returning a webhook
/// payload when that is why.
async fn wait_for_occurrence(
    definition: &TriggerDefinition,
    webhook_rx: &mut Option<mpsc::Receiver<serde_json::Value>>,
) -> Option<serde_json::Value> {
    match &definition.source {
        TriggerSource::Schedule { cron } => {
            let Ok(schedule) = super::schedule::CronSchedule::parse(cron) else {
                tokio::time::sleep(RECHECK_IDLE).await;
                return None;
            };
            let Some(next) = schedule.next_after(time::OffsetDateTime::now_utc()) else {
                tokio::time::sleep(RECHECK_IDLE).await;
                return None;
            };
            let remaining = (next - time::OffsetDateTime::now_utc())
                .whole_seconds()
                .max(0);
            #[allow(
                clippy::cast_sign_loss,
                reason = "remaining is clamped non-negative above"
            )]
            tokio::time::sleep(Duration::from_secs(remaining as u64)).await;
            None
        }
        TriggerSource::Webhook { .. } => {
            if let Some(rx) = webhook_rx {
                rx.recv().await
            } else {
                tokio::time::sleep(RECHECK_IDLE).await;
                None
            }
        }
    }
}

struct Settlement {
    child: SessionId,
    outcome: TriggerOutcome,
    reason_code: Option<ReasonCode>,
}

/// Decide what a new occurrence means under the trigger's overlap policy, and
/// act on it.
async fn handle_occurrence(
    deps: &SchedulerDeps,
    control_session: SessionId,
    definition: &TriggerDefinition,
    payload: Option<serde_json::Value>,
    in_flight: &mut Option<CancellationToken>,
    queued: &mut bool,
    settle_tx: mpsc::Sender<Settlement>,
) {
    match overlap::decide(definition.overlap, in_flight.is_some(), *queued) {
        OverlapDecision::Start => {
            let token = start_firing(deps, control_session, definition, payload, settle_tx).await;
            *in_flight = Some(token);
        }
        OverlapDecision::Skip => {
            let _ = deps
                .store
                .append(MjolnrEvent::TriggerSkipped {
                    session: control_session,
                    trigger: definition.name.clone(),
                    overlap: definition.overlap,
                    detail: "a firing was already in flight".to_owned(),
                })
                .await;
        }
        OverlapDecision::Queue => {
            *queued = true;
            let _ = deps
                .store
                .append(MjolnrEvent::TriggerQueued {
                    session: control_session,
                    trigger: definition.name.clone(),
                })
                .await;
        }
        OverlapDecision::Replace => {
            if let Some(token) = in_flight.take() {
                token.cancel();
            }
            let token = start_firing(deps, control_session, definition, payload, settle_tx).await;
            *in_flight = Some(token);
        }
    }
}

/// Start one firing: an ordinary session, linked to the control session,
/// driven exactly as `mjolnr exec` drives one.
async fn start_firing(
    deps: &SchedulerDeps,
    control_session: SessionId,
    definition: &TriggerDefinition,
    payload: Option<serde_json::Value>,
    settle_tx: mpsc::Sender<Settlement>,
) -> CancellationToken {
    let child = SessionId::new();
    let cancel = CancellationToken::new();
    let directive = directive_for(definition, payload.as_ref());

    let _ = deps
        .store
        .append(MjolnrEvent::TriggerFired {
            session: control_session,
            trigger: definition.name.clone(),
            child,
            source: definition.source.kind(),
        })
        .await;

    let deps = deps.clone();
    let definition = definition.clone();
    let firing_cancel = cancel.clone();
    tokio::spawn(async move {
        let (outcome, reason_code) = fire(
            &deps,
            control_session,
            &definition,
            child,
            directive,
            firing_cancel,
        )
        .await;
        let _ = settle_tx
            .send(Settlement {
                child,
                outcome,
                reason_code,
            })
            .await;
    });
    cancel
}

/// A webhook payload travels as canonical input: appended verbatim, never
/// interpreted as configuration (the anti-pattern this phase forbids is a
/// workflow DSL, and letting a payload field silently widen policy would be
/// exactly that).
fn directive_for(definition: &TriggerDefinition, payload: Option<&serde_json::Value>) -> String {
    match payload {
        Some(value) if !value.is_null() => format!(
            "{}\n\n<webhook_payload>\n{}\n</webhook_payload>",
            definition.directive,
            serde_json::to_string(value).unwrap_or_default()
        ),
        _ => definition.directive.clone(),
    }
}

/// Drive one firing to its terminal outcome: an ordinary `OpenProject` /
/// `CreateSession` / `SetPolicy` setup, then [`crate::headless::run`] — the
/// same sequence `mjolnr exec` runs, so the transcript this produces is a
/// manual headless run's transcript in every respect but its parentage.
async fn fire(
    deps: &SchedulerDeps,
    control_session: SessionId,
    definition: &TriggerDefinition,
    child: SessionId,
    directive: String,
    cancel: CancellationToken,
) -> (TriggerOutcome, Option<ReasonCode>) {
    let Some((provider, model)) = resolve_model(deps, definition) else {
        return (
            TriggerOutcome::Failed,
            Some(ReasonCode::ProviderIncompatibleModel),
        );
    };

    let runtime = Runtime::spawn_trigger_host(
        deps.providers.clone(),
        Arc::clone(&deps.store),
        deps.tools.clone(),
        definition.budgets,
        deps.project_context.clone(),
        Arc::clone(&deps.mcp_servers),
        ChildLink {
            parent: control_session,
            session: child,
        },
        Arc::clone(&deps.route_table),
    );

    let setup = async {
        runtime
            .dispatch(MjolnrCommand::OpenProject {
                root: deps.workspace_root.clone(),
            })
            .await?;
        runtime
            .dispatch(MjolnrCommand::CreateSession {
                provider: provider.clone(),
                model: model.clone(),
            })
            .await?;
        // A firing may open on a named route instead of the fixed
        // provider/model above. A no-op when the trigger
        // names none, or when it names one that does not resolve.
        // A trigger may name a route, a role, or both. Attaching on either
        // keeps a role-only trigger working; attaching on neither leaves the
        // firing on the trigger's fixed provider/model exactly as before.
        if definition.route.is_some() || definition.role.is_some() {
            runtime
                .dispatch(MjolnrCommand::AttachRoute {
                    route: definition.route.clone(),
                    role: definition.role.clone(),
                    task_class: "default".to_owned(),
                })
                .await?;
        }
        runtime
            .dispatch(MjolnrCommand::SetPolicy {
                mode: definition.policy_ceiling,
            })
            .await
    };
    if setup.await.is_err() {
        let _ = runtime.close().await;
        return (TriggerOutcome::Failed, Some(ReasonCode::ToolExecution));
    }
    if !wait_ready(&runtime, definition.policy_ceiling).await {
        let _ = runtime.close().await;
        return (
            TriggerOutcome::Failed,
            Some(ReasonCode::ProviderIncompatibleModel),
        );
    }

    let run = tokio::select! {
        report = crate::headless::run(&runtime, directive) => report,
        () = cancel.cancelled() => {
            let _ = runtime.dispatch(MjolnrCommand::CancelRun).await;
            let _ = runtime.close().await;
            return (TriggerOutcome::Refused, Some(ReasonCode::Cancelled));
        }
    };

    let _ = runtime.close().await;

    match run {
        Ok(report) => {
            let outcome = map_outcome(report.outcome);
            let reason_code = report.reason_code.as_deref().and_then(ReasonCode::parse);
            (outcome, reason_code)
        }
        Err(_) => (TriggerOutcome::Failed, Some(ReasonCode::ToolExecution)),
    }
}

async fn wait_ready(runtime: &Runtime, expected_policy: PolicyMode) -> bool {
    if runtime.snapshot().session.is_some() && runtime.snapshot().policy == expected_policy {
        return true;
    }
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let Ok(snapshot) = snapshots.changed().await else {
                return false;
            };
            if snapshot.session.is_some() && snapshot.policy == expected_policy {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false)
}

fn resolve_model(
    deps: &SchedulerDeps,
    definition: &TriggerDefinition,
) -> Option<(ProviderId, ModelId)> {
    let provider = ProviderId::new(definition.provider.clone());
    if deps
        .providers
        .iter()
        .any(|candidate| candidate.id() == provider)
    {
        Some((provider, ModelId::new(definition.model.clone())))
    } else {
        None
    }
}

async fn record_settlement(
    deps: &SchedulerDeps,
    control_session: SessionId,
    definition: &TriggerDefinition,
    settlement: Settlement,
) {
    let _ = deps
        .store
        .append(MjolnrEvent::TriggerSettled {
            session: control_session,
            trigger: definition.name.clone(),
            child: settlement.child,
            outcome: settlement.outcome,
            reason_code: settlement.reason_code,
        })
        .await;

    if !settlement.outcome.counts_as_failure() {
        return;
    }
    let Ok(events) = control::history(deps.store.as_ref(), control_session).await else {
        return;
    };
    let state = control::replay(&events, &definition.name);
    if state.consecutive_failures >= definition.max_consecutive_failures {
        let _ = deps
            .store
            .append(MjolnrEvent::TriggerDisabled {
                session: control_session,
                trigger: definition.name.clone(),
                code: ReasonCode::TriggerDisabled,
                consecutive_failures: state.consecutive_failures,
            })
            .await;
    }
}

fn map_outcome(outcome: HeadlessOutcome) -> TriggerOutcome {
    match outcome {
        HeadlessOutcome::Verified => TriggerOutcome::Verified,
        HeadlessOutcome::Refused => TriggerOutcome::Refused,
        HeadlessOutcome::BudgetOrQuotaStopped => TriggerOutcome::BudgetOrQuotaStopped,
        HeadlessOutcome::Failed => TriggerOutcome::Failed,
    }
}
