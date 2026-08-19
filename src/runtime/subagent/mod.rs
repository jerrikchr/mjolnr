//! Subagent orchestration.
//!
//! The parent actor intercepts an approved `spawn_subagent` proposal and hands
//! it here. Each child is an ordinary [`Runtime`] — the same actor, providers,
//! policy gate, budgets, and SQLite store the parent uses — driven the way the
//! Phase 12 headless host drives a run: no approval channel, a would-ask
//! proposal is denied through the ordinary command, and the child's transcript
//! is its own durable session linked to the parent.
//!
//! Isolation is structural: every child works in a fresh `git worktree` on its
//! own branch (see [`worktree`]), its path containment rooted at that worktree.
//! Settlement is one future collecting child records, so "exactly one
//! settlement per spawn group" is a property of the control flow rather than a
//! flag to police. A record that arrives after settlement is reported to the
//! parent actor as a late result and persisted as such — never dropped, never
//! reopening the settled result.

mod collision;
mod worktree;

pub use collision::{AgentTouch, Collision, detect};
pub use worktree::cleanup_orphans;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::core::command::{ApprovalDecision, SmedCommand};
use crate::core::error::{ReasonCode, ToolError};
use crate::core::event::{FinishReason, RunId, SessionId, SmedEvent};
use crate::core::message::{ToolCall, ToolOutcome, ToolResult};
use crate::core::model::{ModelId, ProviderId};
use crate::core::policy::PolicyMode;
use crate::core::provider::Provider;
use crate::core::runtime::SmedRuntime;
use crate::core::store::EventStore;
use crate::runtime::budget::BudgetLimits;
use crate::runtime::{Actor, ChildLink, Mail, Runtime, SubagentNotice};
use crate::tools::ToolRegistry;
use crate::tools::subagent::{
    DEFAULT_CHILD_TOOL_CALLS, DEFAULT_CHILD_TURNS, ReportResult, ResultSlot,
};

/// How long a child gets to become ready before dispatch fails.
const CHILD_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Grace period after a cancelled settlement in which straggler child records
/// are still awaited so they can be recorded as late.
const LATE_GRACE: Duration = Duration::from_secs(10);

/// One requested child, parsed from validated `spawn_subagent` arguments.
#[derive(Debug, Clone)]
pub(super) struct ChildRequest {
    pub directive: String,
    pub policy: PolicyMode,
    pub max_provider_turns: u32,
    pub max_tool_calls: u32,
    pub result_schema: serde_json::Value,
    /// A named route for this child. `None` means "use the
    /// configured child default if one exists", not "use no route" — that is
    /// what makes "children default to a configured cheaper route unless the
    /// directive says otherwise" true without every directive having to say
    /// it.
    pub route: Option<String>,
    /// A role for this child. Resolved through the project's
    /// route tags before `route` above, so a project can retarget every
    /// `smol` spawn by moving one tag rather than editing every directive
    /// that names a route literally.
    pub role: Option<String>,
}

/// Parse validated arguments. The registry schema ran first, so failures here
/// are defensive rather than expected.
pub(super) fn parse_children(arguments: &serde_json::Value) -> Result<Vec<ChildRequest>, String> {
    let children = arguments
        .get("children")
        .and_then(serde_json::Value::as_array)
        .ok_or("children must be an array")?;
    children
        .iter()
        .map(|child| {
            let directive = child
                .get("directive")
                .and_then(serde_json::Value::as_str)
                .ok_or("directive must be a string")?
                .to_owned();
            let policy = match child.get("policy").and_then(serde_json::Value::as_str) {
                None | Some("read-only") => PolicyMode::ReadOnly,
                Some("workspace-write") => PolicyMode::WorkspaceWrite,
                Some("full-auto") => PolicyMode::FullAuto,
                Some(other) => return Err(format!("unknown child policy `{other}`")),
            };
            let turns = child
                .get("max_provider_turns")
                .and_then(serde_json::Value::as_u64)
                .map_or(DEFAULT_CHILD_TURNS, |value| {
                    u32::try_from(value).unwrap_or(u32::MAX)
                });
            let calls = child
                .get("max_tool_calls")
                .and_then(serde_json::Value::as_u64)
                .map_or(DEFAULT_CHILD_TOOL_CALLS, |value| {
                    u32::try_from(value).unwrap_or(u32::MAX)
                });
            let result_schema = child
                .get("result_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
            let route = match child.get("route") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(route)) => Some(route.clone()),
                Some(_) => return Err("route must be a string".to_owned()),
            };
            let role = match child.get("role") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(role)) => Some(role.clone()),
                Some(_) => return Err("role must be a string".to_owned()),
            };
            Ok(ChildRequest {
                directive,
                policy,
                max_provider_turns: turns,
                max_tool_calls: calls,
                result_schema,
                route,
                role,
            })
        })
        .collect()
}

/// Clamp a requested child policy to the parent's ceiling.
///
/// - A read-only parent can only delegate read-only work, and a child that
///   asked for read-only keeps it under any parent.
/// - An `ask` parent's ceiling is workspace-write: the human approved this
///   exact spawn with the child policies in the preview, and that approval is
///   what authorises the children's autonomous writes.
/// - Full-auto is never inherited silently: a child runs full-auto only when
///   the spawn *explicitly requested* it and the parent itself is full-auto.
pub(super) const fn clamp_policy(parent: PolicyMode, requested: PolicyMode) -> PolicyMode {
    match (parent, requested) {
        (PolicyMode::ReadOnly, _) | (_, PolicyMode::ReadOnly) => PolicyMode::ReadOnly,
        (PolicyMode::FullAuto, PolicyMode::FullAuto) => PolicyMode::FullAuto,
        _ => PolicyMode::WorkspaceWrite,
    }
}

/// The wire spelling a child policy takes in `spawn_subagent` arguments.
///
/// `Ask` has none: a child has no human attached, so there is nobody for it to
/// ask. [`clamp_policy`] never yields it either, which is what makes rewriting
/// a clamped policy back into the arguments schema-safe.
const fn policy_wire(policy: PolicyMode) -> Option<&'static str> {
    match policy {
        PolicyMode::ReadOnly => Some("read-only"),
        PolicyMode::WorkspaceWrite => Some("workspace-write"),
        PolicyMode::FullAuto => Some("full-auto"),
        PolicyMode::Ask => None,
    }
}

const fn policy_from_wire(wire: &str) -> Option<PolicyMode> {
    match wire.as_bytes() {
        b"read-only" => Some(PolicyMode::ReadOnly),
        b"workspace-write" => Some(PolicyMode::WorkspaceWrite),
        b"full-auto" => Some(PolicyMode::FullAuto),
        _ => None,
    }
}

/// Rewrite each child's requested policy to the one that will actually run.
///
/// Applied to the proposal *before* it is previewed, persisted, or approved, so
/// that the preview the human reads, the `ToolProposed` event in the ledger, and
/// the policy the child opens on are all the same value. Previously the clamp
/// happened in `start_spawn`, after approval: an `ask` parent whose model asked
/// for `full-auto` showed the human `[full-auto, …]` and then ran the child at
/// `workspace-write`. The error favoured safety, but an approval gate whose
/// preview is approximate teaches that previews are approximate.
///
/// A value that does not parse is left exactly as it is. Rewriting it would
/// launder a schema violation into a valid-looking call; leaving it lets the
/// ordinary registry validation refuse the spawn, which is the correct outcome.
pub(super) fn clamp_call_policies(parent: PolicyMode, arguments: &mut serde_json::Value) {
    let Some(children) = arguments
        .get_mut("children")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for child in children {
        // An absent policy means the schema default, `read-only`, which clamps
        // to itself under every parent — so there is nothing to rewrite and
        // writing one in would only add noise to the preview.
        let Some(requested) = child
            .get("policy")
            .and_then(serde_json::Value::as_str)
            .and_then(policy_from_wire)
        else {
            continue;
        };
        let Some(effective) = policy_wire(clamp_policy(parent, requested)) else {
            continue;
        };
        if let Some(object) = child.as_object_mut() {
            object.insert(
                "policy".to_owned(),
                serde_json::Value::String(effective.to_owned()),
            );
        }
    }
}

/// Everything the orchestration task needs, assembled by the actor.
pub(super) struct SpawnPlan {
    pub run: RunId,
    pub session: SessionId,
    pub call: ToolCall,
    pub workspace: PathBuf,
    pub children: Vec<ChildSpec>,
    pub providers: Vec<Arc<dyn Provider>>,
    pub store: Arc<dyn EventStore>,
    pub events: broadcast::Sender<SmedEvent>,
    pub mailbox: mpsc::Sender<Mail>,
    pub cancel: CancellationToken,
}

#[derive(Clone)]
pub(super) struct ChildSpec {
    pub link: ChildLink,
    pub request: ChildRequest,
    pub limits: BudgetLimits,
    pub branch: String,
    pub worktree: PathBuf,
    /// The provider/model this child opens on. Resolved by the parent before
    /// dispatch: from a named or configured-default route's first hop (plan
    /// §Phase 15), or — when neither resolves, including whenever no routing
    /// config exists — the parent's own provider/model exactly as before
    /// this phase.
    pub provider: ProviderId,
    pub model: ModelId,
}

/// What one child's run came to. Serialised into the group settlement.
#[derive(Debug, Clone, serde::Serialize)]
pub(super) struct ChildRecord {
    session: String,
    branch: Option<String>,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserved_commit: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
    /// Workspace-relative paths this child read, from its durable
    /// `read_evidence`. The read side of a collision check.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    read_paths: Vec<String>,
    /// Workspace-relative paths this child's branch changed, from `git diff`
    /// against the spawn base. The write side of a collision check.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    touched_paths: Vec<String>,
    /// Paths a concurrent sibling invalidated. Non-empty means this child's
    /// verified finish was refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    collision_paths: Vec<String>,
}

impl ChildRecord {
    fn dispatch_failure(child: SessionId, code: ReasonCode, note: String) -> Self {
        Self {
            session: child.to_string(),
            branch: None,
            outcome: "failed",
            reason_code: Some(code.as_str().to_owned()),
            result: None,
            preserved_commit: None,
            input_tokens: 0,
            output_tokens: 0,
            notes: vec![note],
            read_paths: Vec::new(),
            touched_paths: Vec::new(),
            collision_paths: Vec::new(),
        }
    }

    /// Mark a verified finish as stale: a sibling mutated `path` after this
    /// child read it, so the child may not report a verified result without
    /// re-reading it. Fail closed — the result is withheld, not left in place.
    fn invalidate(&mut self, path: String) {
        let note = format!(
            "read of {path} was invalidated by a concurrent sibling; re-read before finishing"
        );
        self.collision_paths.push(path);
        self.outcome = "revalidation_required";
        self.reason_code = Some(ReasonCode::ReadSetCollision.as_str().to_owned());
        self.result = None;
        self.notes.push(note);
    }
}

/// Run one spawn group to its single settlement.
pub(super) async fn orchestrate(plan: SpawnPlan) {
    let outcome = settle(&plan).await;
    let _ = plan
        .mailbox
        .send(Mail::ToolEnded {
            run: plan.run,
            call: plan.call.clone(),
            outcome: Ok(outcome),
        })
        .await;
}

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    reason = "one settlement loop keeps cancellation, grace, late-result ordering, and collision invalidation explicit"
)]
async fn settle(plan: &SpawnPlan) -> Result<ToolResult, ToolError> {
    if let Some(refusal) = worktree::preflight(&plan.workspace).await {
        return Ok(refusal);
    }
    let base = match worktree::head(&plan.workspace).await {
        Ok(base) => base,
        Err(result) => return Ok(result),
    };

    let (mut records, mut records_rx) = dispatch_group(plan, &base).await;

    // Exactly one settlement: this loop is the only fold over child records
    // and it exits exactly once — when every child reported, or when the run
    // was cancelled and the grace elapsed.
    let outstanding = |records: &[Option<ChildRecord>]| records.iter().any(Option::is_none);
    let mut cancelled = false;
    while outstanding(&records) {
        tokio::select! {
            received = records_rx.recv() => {
                let Some((index, record)) = received else { break };
                if let Some(slot) = records.get_mut(index) {
                    *slot = Some(record);
                }
            }
            () = plan.cancel.cancelled(), if !cancelled => {
                cancelled = true;
                // Children observe the same token and cancel themselves; give
                // them a bounded grace to land their records, then settle.
                let deadline = tokio::time::sleep(LATE_GRACE);
                tokio::pin!(deadline);
                while outstanding(&records) {
                    tokio::select! {
                        received = records_rx.recv() => {
                            let Some((index, record)) = received else { break };
                            if let Some(slot) = records.get_mut(index) {
                                *slot = Some(record);
                            }
                        }
                        () = &mut deadline => break,
                    }
                }
                break;
            }
        }
    }

    if cancelled && outstanding(&records) {
        // Stragglers past the grace: recorded late as they arrive, never
        // reopening this settlement.
        let mailbox = plan.mailbox.clone();
        tokio::spawn(async move {
            while let Some((_, record)) = records_rx.recv().await {
                let Ok(child) = record.session.parse::<uuid::Uuid>() else {
                    continue;
                };
                let detail = serde_json::to_string(&record)
                    .unwrap_or_else(|_| "unencodable late child record".to_owned());
                let _ = mailbox
                    .send(Mail::Subagent {
                        notice: SubagentNotice::Late {
                            child: SessionId::from_uuid(child),
                            detail,
                        },
                    })
                    .await;
            }
        });
    }

    let mut settled: Vec<ChildRecord> = records
        .into_iter()
        .enumerate()
        .map(|(index, slot)| {
            slot.unwrap_or_else(|| {
                let spec = plan.children.get(index);
                ChildRecord {
                    session: spec.map_or_else(String::new, |spec| spec.link.session.to_string()),
                    branch: spec.map(|spec| spec.branch.clone()),
                    outcome: "cancelled",
                    reason_code: Some(ReasonCode::Cancelled.as_str().to_owned()),
                    result: None,
                    preserved_commit: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    notes: vec!["did not settle before the cancellation grace elapsed".to_owned()],
                    read_paths: Vec::new(),
                    touched_paths: Vec::new(),
                    collision_paths: Vec::new(),
                }
            })
        })
        .collect();

    if cancelled {
        return Err(ToolError::Cancelled);
    }

    invalidate_collisions(plan, &mut settled).await;

    let delivered = settled
        .iter()
        .filter(|record| record.result.is_some())
        .count();
    let content = serde_json::json!({ "children": settled }).to_string();
    if delivered > 0 {
        Ok(ToolResult::ok(content))
    } else {
        let code = settled
            .iter()
            .find_map(|record| record.reason_code.as_deref())
            .and_then(reason_from_wire)
            .unwrap_or(ReasonCode::SubagentResultMissing);
        Ok(ToolResult::failed(code, content))
    }
}

/// Fold cross-child read-set collisions into settlement: invalidate each stale
/// reader's verified finish and record the boundary durably.
///
/// Detection happens here — at settlement — because it is the only point where
/// every sibling's reads and writes are both known (Phase 5 Slice 5.2). The
/// actor is still the only writer of the parent transcript: the collision
/// arrives as a [`SubagentNotice::Collision`] and is persisted by
/// [`Actor::handle_subagent_notice`], exactly like a spawn or late-result
/// boundary.
async fn invalidate_collisions(plan: &SpawnPlan, settled: &mut [ChildRecord]) {
    let agents: Vec<AgentTouch> = plan
        .children
        .iter()
        .zip(settled.iter())
        .map(|(spec, record)| AgentTouch {
            id: spec.link.session,
            read: record.read_paths.clone(),
            wrote: record.touched_paths.clone(),
        })
        .collect();
    let index_of: std::collections::HashMap<SessionId, usize> = plan
        .children
        .iter()
        .enumerate()
        .map(|(index, spec)| (spec.link.session, index))
        .collect();

    for collision in detect(&agents) {
        if let Some(index) = index_of.get(&collision.reader).copied()
            && let Some(reader) = settled.get_mut(index)
        {
            reader.invalidate(collision.path.clone());
        }
        let _ = plan
            .mailbox
            .send(Mail::Subagent {
                notice: SubagentNotice::Collision {
                    reader: collision.reader,
                    writer: collision.writer,
                    path: collision.path,
                },
            })
            .await;
    }
}

async fn dispatch_group(
    plan: &SpawnPlan,
    base: &str,
) -> (
    Vec<Option<ChildRecord>>,
    mpsc::Receiver<(usize, ChildRecord)>,
) {
    let (records_tx, records_rx) = mpsc::channel(plan.children.len());
    let mut records: Vec<Option<ChildRecord>> = Vec::new();
    records.resize_with(plan.children.len(), || None);
    for (index, child) in plan.children.iter().enumerate() {
        match dispatch_child(plan, child, base).await {
            Ok(task) => {
                let sender = records_tx.clone();
                tokio::spawn(async move {
                    let record = run_child(task).await;
                    let _ = sender.send((index, record)).await;
                });
            }
            Err(record) => {
                if let Some(slot) = records.get_mut(index) {
                    *slot = Some(record);
                }
            }
        }
    }
    drop(records_tx);
    (records, records_rx)
}

fn reason_from_wire(wire: &str) -> Option<ReasonCode> {
    [
        ReasonCode::SchemaInvalid,
        ReasonCode::PolicyReadOnly,
        ReasonCode::BudgetExhausted,
        ReasonCode::Cancelled,
        ReasonCode::ProviderPlanQuota,
        ReasonCode::WorktreeUnavailable,
        ReasonCode::SubagentResultMissing,
        ReasonCode::ReadSetCollision,
        ReasonCode::ToolExecution,
    ]
    .into_iter()
    .find(|code| code.as_str() == wire)
}

/// Everything a single child task owns.
struct ChildTask {
    spec: ChildSpec,
    providers: Vec<Arc<dyn Provider>>,
    store: Arc<dyn EventStore>,
    provider: ProviderId,
    model: ModelId,
    parent_session: SessionId,
    parent_run: RunId,
    events: broadcast::Sender<SmedEvent>,
    cancel: CancellationToken,
    workspace: PathBuf,
}

/// Create the worktree and announce the spawn, or fail this child.
async fn dispatch_child(
    plan: &SpawnPlan,
    child: &ChildSpec,
    base: &str,
) -> Result<ChildTask, ChildRecord> {
    worktree::create(&plan.workspace, child, base).await?;

    let _ = plan
        .mailbox
        .send(Mail::Subagent {
            notice: SubagentNotice::Spawned {
                run: plan.run,
                child: child.link.session,
                directive: child.request.directive.clone(),
                policy: child.request.policy,
                branch: child.branch.clone(),
                worktree: child.worktree.to_string_lossy().into_owned(),
            },
        })
        .await;

    Ok(ChildTask {
        spec: child.clone(),
        providers: plan.providers.clone(),
        store: Arc::clone(&plan.store),
        provider: child.provider.clone(),
        model: child.model.clone(),
        parent_session: plan.session,
        parent_run: plan.run,
        events: plan.events.clone(),
        cancel: plan.cancel.clone(),
        workspace: plan.workspace.clone(),
    })
}

/// Drive one child session to its record. Approvals are denied — a subagent
/// has no human; the Phase 12 rule applies unchanged.
async fn run_child(task: ChildTask) -> ChildRecord {
    let slot: ResultSlot = Arc::default();
    let mut registry = ToolRegistry::builtins();
    registry.add(Arc::new(ReportResult::new(
        task.spec.request.result_schema.clone(),
        Arc::clone(&slot),
    )));

    let runtime = Runtime::spawn_subagent_host(
        task.providers.clone(),
        Arc::clone(&task.store),
        registry,
        task.spec.limits,
        task.spec.link,
    );

    let mut record = drive_child(&task, &runtime, &slot).await;
    let _ = runtime.close().await;

    worktree::finish(&task.workspace, &task.spec, &mut record).await;
    record
}

#[allow(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one linear observation loop; splitting it would scatter the classification"
)]
async fn drive_child(task: &ChildTask, runtime: &Runtime, slot: &ResultSlot) -> ChildRecord {
    let child = task.spec.link.session;
    let mut record = ChildRecord {
        session: child.to_string(),
        branch: Some(task.spec.branch.clone()),
        outcome: "failed",
        reason_code: None,
        result: None,
        preserved_commit: None,
        input_tokens: 0,
        output_tokens: 0,
        notes: Vec::new(),
        read_paths: Vec::new(),
        touched_paths: Vec::new(),
        collision_paths: Vec::new(),
    };

    let setup = async {
        runtime
            .dispatch(SmedCommand::OpenProject {
                root: task.spec.worktree.clone(),
            })
            .await?;
        runtime
            .dispatch(SmedCommand::CreateSession {
                provider: task.provider.clone(),
                model: task.model.clone(),
            })
            .await?;
        runtime
            .dispatch(SmedCommand::SetPolicy {
                mode: task.spec.request.policy,
            })
            .await
    };
    if setup.await.is_err() {
        record.reason_code = Some(ReasonCode::ToolExecution.as_str().to_owned());
        record.notes.push("child runtime setup failed".to_owned());
        return record;
    }

    if !wait_ready(runtime, task.spec.request.policy).await {
        record.reason_code = Some(ReasonCode::ToolExecution.as_str().to_owned());
        record
            .notes
            .push("child session did not become ready".to_owned());
        return record;
    }

    let directive = format!(
        "You are a smed subagent in an isolated git worktree; your branch is `{}`. \
         Complete only this directive. Commit your changes if you can; then call \
         report_result exactly once with a result matching its schema, then \
         finish_task with an honest outcome.\n\nDirective: {}",
        task.spec.branch, task.spec.request.directive
    );

    let mut events = runtime.subscribe();
    if runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: directive,
            // Composed by the parent, inside a spawn the human already approved.
            source: crate::core::directive::DirectiveSource::Internal,
        })
        .await
        .is_err()
    {
        record.reason_code = Some(ReasonCode::ToolExecution.as_str().to_owned());
        record
            .notes
            .push("child directive was not accepted".to_owned());
        return record;
    }

    let mut refusal: Option<ReasonCode> = None;
    let mut failure: Option<ReasonCode> = None;
    let mut stopped: Option<ReasonCode> = None;
    let mut cancelled_child = false;
    let mut cancel_requested = false;

    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            () = task.cancel.cancelled(), if !cancel_requested => {
                cancel_requested = true;
                let _ = runtime.dispatch(SmedCommand::CancelRun).await;
                continue;
            }
        };
        let event = match event {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };
        forward_activity(task, &event);
        match &event {
            SmedEvent::ToolProposed {
                approval: Some(approval),
                ..
            } => {
                let _ = runtime
                    .dispatch(SmedCommand::ResolveApproval {
                        approval: *approval,
                        decision: ApprovalDecision::Deny,
                    })
                    .await;
            }
            SmedEvent::ToolCompleted { result, .. } => match result.outcome {
                ToolOutcome::Refused(code) => refusal = Some(code),
                ToolOutcome::Failed(code) => failure = Some(code),
                ToolOutcome::Ok => {}
            },
            SmedEvent::ToolFailed { code, .. } | SmedEvent::RunFailed { code, .. } => {
                failure = Some(*code);
            }
            SmedEvent::BudgetExhausted { .. } => {
                stopped = Some(ReasonCode::BudgetExhausted);
            }
            SmedEvent::QuotaBoundaryReached { reserve, .. }
                if reserve.phase == crate::core::continuation::QuotaReservePhase::Stopped =>
            {
                stopped = Some(ReasonCode::ProviderPlanQuota);
            }
            SmedEvent::RunFinished {
                reason: FinishReason::Cancelled,
                ..
            } => cancelled_child = true,
            _ => {}
        }
        if matches!(
            &event,
            SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
        ) {
            break;
        }
    }

    let snapshot = runtime.snapshot();
    record.input_tokens = snapshot.usage.input_tokens;
    record.output_tokens = snapshot.usage.output_tokens;
    record.read_paths = snapshot
        .read_evidence
        .iter()
        .map(|record| record.path.clone())
        .collect();

    let reported = slot.lock().ok().and_then(|mut reported| reported.take());
    let validated = reported.and_then(|value| {
        match jsonschema::draft202012::options().build(&task.spec.request.result_schema) {
            Ok(validator) if validator.validate(&value).is_ok() => Some(value),
            Ok(_) | Err(_) => {
                record.reason_code = Some(ReasonCode::SchemaInvalid.as_str().to_owned());
                record
                    .notes
                    .push("reported result failed schema validation at settlement".to_owned());
                None
            }
        }
    });

    if cancelled_child {
        record.outcome = "cancelled";
        record.reason_code = Some(ReasonCode::Cancelled.as_str().to_owned());
        record.result = validated;
        return record;
    }
    if let Some(code) = stopped {
        record.outcome = "budget_stopped";
        record.reason_code = Some(code.as_str().to_owned());
        record.result = validated;
        return record;
    }
    if let Some(value) = validated {
        record.outcome = "completed";
        record.result = Some(value);
        if let Some(code) = refusal.or(failure) {
            record
                .notes
                .push(format!("child recorded a non-fatal {}", code.as_str()));
        }
    } else {
        record.outcome = "failed";
        if record.reason_code.is_none() {
            let code = failure
                .or(refusal)
                .unwrap_or(ReasonCode::SubagentResultMissing);
            record.reason_code = Some(code.as_str().to_owned());
        }
        if record.notes.is_empty() {
            record
                .notes
                .push("the child finished without reporting a result".to_owned());
        }
    }
    record
}

async fn wait_ready(runtime: &Runtime, policy: PolicyMode) -> bool {
    let snapshot = runtime.snapshot();
    if snapshot.session.is_some() && snapshot.policy == policy {
        return true;
    }
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(CHILD_READY_TIMEOUT, async {
        loop {
            let Ok(snapshot) = snapshots.changed().await else {
                return false;
            };
            if snapshot.session.is_some() && snapshot.policy == policy {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Map a child event onto one ephemeral activity label for the parent.
fn forward_activity(task: &ChildTask, event: &SmedEvent) {
    let label = match event {
        SmedEvent::RunStarted { .. } => Some("started".to_owned()),
        SmedEvent::ToolAssembling { name, .. } => Some(format!("assembling {name}")),
        SmedEvent::ToolProposed { call, .. } => Some(format!("tool {}", call.name)),
        SmedEvent::ToolCompleted { name, result, .. } => Some(match &result.outcome {
            ToolOutcome::Ok => format!("{name} ok"),
            ToolOutcome::Refused(code) => format!("{name} refused {}", code.as_str()),
            ToolOutcome::Failed(code) => format!("{name} failed {}", code.as_str()),
        }),
        SmedEvent::RunFinished { reason, .. } => Some(format!("finished {reason:?}")),
        SmedEvent::RunFailed { code, .. } => Some(format!("failed {}", code.as_str())),
        _ => None,
    };
    if let Some(label) = label {
        let _ = task.events.send(SmedEvent::SubagentActivity {
            session: task.parent_session,
            run: task.parent_run,
            child: task.spec.link.session,
            label,
        });
    }
}

impl Actor {
    /// Host-side execution of an approved `spawn_subagent` proposal.
    ///
    /// The tool with that name is a marker: only the runtime holds providers,
    /// the store, and budget state, so only the runtime may mint children —
    /// and children, whose registry never contains the tool, may not.
    /// Charge the armed envelope for this spawn and return the policy ceiling
    /// its children are bounded by.
    ///
    /// Charged here rather than at approval time because the call is only known
    /// to parse by now: an envelope that ran down on spawns which never
    /// dispatched would be an authorisation spent on nothing.
    ///
    /// The ceiling narrows *in addition* to the parent clamp, never instead of
    /// it — two bounds, both applied, neither able to widen. With no envelope
    /// armed this is the session's own policy, exactly as before the phase.
    async fn charge_envelope(&mut self, run: RunId, call: &ToolCall) -> PolicyMode {
        let Some(ceiling) = self.state.envelope.as_ref().map(|active| {
            crate::core::envelope::clamp_ceiling(self.state.policy, active.envelope.ceiling)
        }) else {
            return self.state.policy;
        };
        let (children, turns, _) = super::envelope::draw_shape(&call.arguments);
        self.record_envelope_draw(run, children, turns).await;
        ceiling
    }

    fn resolve_child_route(
        &self,
        request: &ChildRequest,
    ) -> Option<(
        String,
        crate::core::routing::RouteSelectionReason,
        ProviderId,
        ModelId,
    )> {
        self.route_table
            .resolve_child(request.route.as_deref(), request.role.as_deref())
            .and_then(|(definition, reason)| {
                definition.hop(0).map(|hop| {
                    (
                        definition.name.clone(),
                        reason,
                        hop.provider.clone(),
                        hop.model.clone(),
                    )
                })
            })
    }

    pub(super) async fn start_spawn(
        &mut self,
        run: RunId,
        call: ToolCall,
        workspace: PathBuf,
    ) -> bool {
        let (Some(session), Some(provider), Some(model), Some(cancel)) = (
            self.state.session,
            self.state.provider.clone(),
            self.state.model.clone(),
            self.run
                .as_ref()
                .filter(|active| active.id == run)
                .map(|active| active.cancel.clone()),
        ) else {
            let result = ToolResult::failed(
                ReasonCode::ToolExecution,
                "spawn_subagent needs an open session on an active run",
            );
            return self.record_tool_result(run, &call, result).await;
        };

        let requests = match parse_children(&call.arguments) {
            Ok(requests) => requests,
            Err(detail) => {
                let result = ToolResult::refused(ReasonCode::SchemaInvalid, detail);
                return self.record_tool_result(run, &call, result).await;
            }
        };

        let ceiling = self.charge_envelope(run, &call).await;

        let mut children = Vec::with_capacity(requests.len());
        for mut request in requests {
            request.policy = clamp_policy(ceiling, request.policy);
            let child = SessionId::new();

            // : a child may name a route; naming none falls
            // back to the configured child default. Neither resolving —
            // including whenever no routing config exists at all — leaves
            // the child on the parent's own provider/model exactly as before
            // this phase, which is what makes "no routing config restores
            // present-day behaviour" true for spawns too.
            let route_selection = self.resolve_child_route(&request);

            let (child_provider, child_model) = match route_selection {
                Some((route_name, reason, hop_provider, hop_model)) => {
                    if let Err(error) = self
                        .persist(SmedEvent::RouteSelected {
                            session,
                            child: Some(child),
                            route: route_name,
                            position: 0,
                            provider: hop_provider.clone(),
                            model: hop_model.clone(),
                            reason,
                        })
                        .await
                    {
                        self.note_store_failure(&error);
                        return self
                            .record_tool_result(
                                run,
                                &call,
                                ToolResult::failed(
                                    ReasonCode::ToolExecution,
                                    "could not record the child's route selection",
                                ),
                            )
                            .await;
                    }
                    (hop_provider, hop_model)
                }
                None => (provider.clone(), model.clone()),
            };

            children.push(ChildSpec {
                link: ChildLink {
                    parent: session,
                    session: child,
                },
                limits: BudgetLimits {
                    max_provider_turns: request.max_provider_turns,
                    max_tool_calls: request.max_tool_calls,
                    ..self.limits
                },
                branch: worktree::branch_name(child),
                worktree: worktree::worktree_path(child),
                provider: child_provider,
                model: child_model,
                request,
            });
        }

        let plan = SpawnPlan {
            run,
            session,
            call: call.clone(),
            workspace,
            children,
            providers: self.providers.clone(),
            store: Arc::clone(&self.store),
            events: self.events.clone(),
            mailbox: self.mailbox.clone(),
            cancel,
        };
        tokio::spawn(orchestrate(plan));
        true
    }

    /// Record a subagent boundary reported by the orchestration task. The
    /// actor is the only writer of the parent transcript.
    pub(super) async fn handle_subagent_notice(&mut self, notice: SubagentNotice) {
        let Some(session) = self.state.session else {
            return;
        };
        let event = match notice {
            SubagentNotice::Spawned {
                run,
                child,
                directive,
                policy,
                branch,
                worktree,
            } => SmedEvent::SubagentSpawned {
                session,
                run,
                child,
                directive,
                policy,
                branch,
                worktree,
            },
            SubagentNotice::Late { child, detail } => SmedEvent::SubagentResultLate {
                session,
                child,
                detail,
            },
            SubagentNotice::Collision {
                reader,
                writer,
                path,
            } => SmedEvent::ReadSetCollision {
                session,
                reader,
                writer,
                path,
            },
        };
        if let Err(error) = self.persist(event).await {
            self.note_store_failure(&error);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    #[test]
    fn policy_clamp_never_exceeds_the_parent() {
        use PolicyMode::{Ask, FullAuto, ReadOnly, WorkspaceWrite};
        // A read-only parent delegates nothing wider.
        for requested in [ReadOnly, WorkspaceWrite, FullAuto] {
            assert_eq!(clamp_policy(ReadOnly, requested), ReadOnly);
        }
        // Full-auto requires the parent to be full-auto AND the request to say so.
        assert_eq!(clamp_policy(WorkspaceWrite, FullAuto), WorkspaceWrite);
        assert_eq!(clamp_policy(Ask, FullAuto), WorkspaceWrite);
        assert_eq!(clamp_policy(FullAuto, FullAuto), FullAuto);
        // Full-auto is never inherited silently.
        assert_eq!(clamp_policy(FullAuto, WorkspaceWrite), WorkspaceWrite);
        // Read-only children stay read-only under any parent.
        assert_eq!(clamp_policy(FullAuto, ReadOnly), ReadOnly);
    }

    fn preview_context() -> crate::core::tool::ToolContext {
        crate::core::tool::ToolContext {
            workspace_root: PathBuf::from("/tmp"),
            read_set: Arc::default(),
            max_output_bytes: 4096,
            command_timeout: std::time::Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn the_preview_shows_the_policy_that_will_actually_run() {
        // The defect this closes: an `ask` parent whose model asked for
        // full-auto previewed `[full-auto, …]` and then ran the child at
        // workspace-write, because the clamp happened after approval.
        let mut arguments = serde_json::json!({
            "children": [
                { "directive": "audit", "policy": "full-auto" },
                { "directive": "read", "policy": "read-only" }
            ]
        });
        clamp_call_policies(PolicyMode::Ask, &mut arguments);

        let preview = crate::core::tool::Tool::preview(
            &crate::tools::subagent::SpawnSubagent,
            &arguments,
            &preview_context(),
        )
        .await
        .expect("preview");
        assert!(
            preview.contains("[workspace-write,"),
            "preview must state the clamped policy:\n{preview}"
        );
        assert!(
            !preview.contains("[full-auto,"),
            "preview must not promise authority the child will not get:\n{preview}"
        );

        // And the parsed request agrees with what the human just read.
        let children = parse_children(&arguments).expect("parse");
        assert_eq!(children[0].policy, PolicyMode::WorkspaceWrite);
        assert_eq!(children[1].policy, PolicyMode::ReadOnly);
    }

    #[test]
    fn a_full_auto_parent_still_previews_full_auto() {
        let mut arguments = serde_json::json!({
            "children": [{ "directive": "go", "policy": "full-auto" }]
        });
        clamp_call_policies(PolicyMode::FullAuto, &mut arguments);
        assert_eq!(
            arguments["children"][0]["policy"],
            serde_json::json!("full-auto"),
            "clamping must not narrow a spawn the parent is entitled to make"
        );
    }

    #[test]
    fn an_unparseable_policy_is_left_for_schema_validation_to_refuse() {
        // Rewriting it would launder a schema violation into a valid-looking
        // call; the registry must get the chance to refuse the spawn.
        let mut arguments = serde_json::json!({
            "children": [{ "directive": "go", "policy": "ask" }]
        });
        clamp_call_policies(PolicyMode::FullAuto, &mut arguments);
        assert_eq!(arguments["children"][0]["policy"], serde_json::json!("ask"));

        let registry =
            crate::tools::ToolRegistry::new(vec![Arc::new(crate::tools::subagent::SpawnSubagent)]);
        let tool = registry
            .get(crate::tools::subagent::SpawnSubagent::NAME)
            .expect("registered");
        assert!(
            registry.validate(tool.as_ref(), &arguments).is_err(),
            "`ask` is not a child policy: a child has nobody to ask"
        );
    }

    #[test]
    fn child_requests_parse_with_defaults() {
        let arguments = serde_json::json!({
            "children": [
                { "directive": "do a thing" },
                {
                    "directive": "another",
                    "policy": "workspace-write",
                    "max_provider_turns": 3,
                    "max_tool_calls": 5,
                    "result_schema": { "type": "object", "required": ["summary"] }
                }
            ]
        });
        let children = parse_children(&arguments).expect("parse");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].policy, PolicyMode::ReadOnly);
        assert_eq!(children[0].max_provider_turns, DEFAULT_CHILD_TURNS);
        assert_eq!(children[1].policy, PolicyMode::WorkspaceWrite);
        assert_eq!(children[1].max_provider_turns, 3);
        assert_eq!(children[1].max_tool_calls, 5);
    }
}
