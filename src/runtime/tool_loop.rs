//! Tool proposal, policy, approval, execution, and evidence driving.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::command::{ApprovalDecision, ApprovalId};
use crate::core::error::{ReasonCode, ToolError};
use crate::core::event::{FinishReason, RunId, SmedEvent};
use crate::core::message::{CanonicalMessage, ToolCall, ToolEffect, ToolOutcome, ToolResult};
use crate::core::policy::PendingApproval;
use crate::core::tool::{CommandSpec, Tool, ToolContext};
use crate::policy::{PolicyDecision, decide};

use super::{Actor, Mail, PendingTool, RunPhase, ToolPreparation, ToolTaskOutcome};

/// What an armed envelope says about one proposal.
enum EnvelopeDisposition {
    /// The envelope pre-authorised this draw; no prompt is needed.
    Authorised,
    /// The draw did not fit. Carries whether the loop should stop.
    Refused(bool),
    /// No envelope applies — the ordinary approval path, unchanged.
    NotApplicable,
}

impl Actor {
    pub(super) async fn drive_tools(&mut self, run: RunId) {
        loop {
            let call = {
                let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
                    return;
                };
                active.pending_tools.pop_front()
            };
            let Some(mut call) = call else {
                self.begin_provider_turn(run).await;
                return;
            };
            // Normalise before anything reads the arguments: the preview, the
            // `ToolProposed` event, and execution must all describe the same
            // act. Anything that transforms a call after approval is a preview
            // that can disagree with what runs.
            self.normalize_call(&mut call);
            if !self.reserve_tool_call(run).await {
                return;
            }
            match self.prepare_tool(run, &call).await {
                ToolPreparation::Ready { tool, preview } => {
                    if self.dispose_proposal(run, call, tool, preview).await {
                        return;
                    }
                }
                ToolPreparation::Continue => {}
                ToolPreparation::Stop => return,
            }
        }
    }

    /// Rewrite a proposed call into the exact form that will run.
    ///
    /// Only `spawn_subagent` needs this today: a child's policy is clamped to
    /// the parent's ceiling, and that clamp has to be visible in the preview the
    /// human approves rather than applied afterwards.
    fn normalize_call(&self, call: &mut ToolCall) {
        if call.name == crate::tools::subagent::SpawnSubagent::NAME {
            // The envelope's ceiling narrows the same way the parent's policy
            // does, and must be in the preview for the same reason: whatever the
            // human reads is what has to run.
            let ceiling = self
                .state
                .envelope
                .as_ref()
                .map_or(self.state.policy, |active| {
                    crate::core::envelope::clamp_ceiling(self.state.policy, active.envelope.ceiling)
                });
            crate::runtime::subagent::clamp_call_policies(ceiling, &mut call.arguments);
        }
    }

    /// What the gate says about this proposal.
    ///
    /// Three things can stand in for the ordinary policy decision, and their
    /// order is the contract. Untrusted project content always asks, whatever
    /// else is true. An envelope already carrying a human's authorisation for
    /// this shape allows. Otherwise the ordinary gate decides, honouring an
    /// exact-command grant if one covers this call.
    fn gate_decision(
        &self,
        call: &ToolCall,
        tool: &dyn Tool,
        envelope_authorised: bool,
    ) -> PolicyDecision {
        if tool.requires_workspace_trust(&call.arguments) && !self.state.workspace_trusted {
            return PolicyDecision::Ask;
        }
        if envelope_authorised {
            return PolicyDecision::Allow;
        }
        let exact_approved =
            command_spec(call).is_some_and(|command| self.state.exact_commands.contains(&command));
        decide(self.state.policy, tool.tier(), exact_approved)
    }

    /// What an armed envelope says about this proposal.
    ///
    /// A draw that fits needs no prompt — that is what arming one buys. A draw
    /// that does not fit is refused with a typed code rather than falling back
    /// to an approval prompt: the model can re-plan a smaller draw against a
    /// number, and a sixteen-child preview appearing mid-fleet is exactly the
    /// previewability problem the envelope exists to avoid.
    async fn settle_envelope_draw(&mut self, run: RunId, call: &ToolCall) -> EnvelopeDisposition {
        match self.envelope_draw(call) {
            None => EnvelopeDisposition::NotApplicable,
            Some(Ok(())) => EnvelopeDisposition::Authorised,
            Some(Err(refusal)) => {
                let result =
                    ToolResult::refused(ReasonCode::SpawnEnvelopeRefused, refusal.detail());
                EnvelopeDisposition::Refused(!self.record_tool_result(run, call, result).await)
            }
        }
    }

    async fn reserve_tool_call(&mut self, run: RunId) -> bool {
        let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
            return false;
        };
        let exhausted = active.tool_calls >= self.limits.max_tool_calls;
        if !exhausted {
            active.tool_calls += 1;
            self.state.budget.tool_calls = active.tool_calls;
            self.publish_snapshot();
            return true;
        }
        self.exhaust_budget(run).await;
        false
    }

    async fn prepare_tool(&mut self, run: RunId, call: &ToolCall) -> ToolPreparation {
        let Some(tool) = self.tools.get(&call.name) else {
            let result = ToolResult::refused(
                ReasonCode::SchemaInvalid,
                format!("unknown tool `{}`", call.name),
            );
            return self.recorded_preparation(run, call, result).await;
        };
        if let Err(error) = self.tools.validate(tool.as_ref(), &call.arguments) {
            let result = ToolResult::refused(ReasonCode::SchemaInvalid, error.to_string());
            return self.recorded_preparation(run, call, result).await;
        }
        let Some(context) = self.tool_context() else {
            let result = ToolResult::failed(
                ReasonCode::PathOutsideWorkspace,
                "open a workspace before using repository tools",
            );
            return self.recorded_preparation(run, call, result).await;
        };
        match tool.preview(&call.arguments, &context).await {
            Ok(preview) => ToolPreparation::Ready {
                tool,
                preview: bound_text(preview, self.limits.max_tool_output_bytes).0,
            },
            Err(ToolError::Refused { code, detail }) => {
                let result = ToolResult::refused(code, detail);
                self.recorded_preparation(run, call, result).await
            }
            Err(error) => {
                let result = ToolResult::failed(error.reason_code(), error.to_string());
                self.recorded_preparation(run, call, result).await
            }
        }
    }

    async fn recorded_preparation(
        &mut self,
        run: RunId,
        call: &ToolCall,
        result: ToolResult,
    ) -> ToolPreparation {
        if self.record_tool_result(run, call, result).await {
            ToolPreparation::Continue
        } else {
            ToolPreparation::Stop
        }
    }

    async fn dispose_proposal(
        &mut self,
        run: RunId,
        call: ToolCall,
        tool: Arc<dyn Tool>,
        preview: String,
    ) -> bool {
        let envelope_authorised = match self.settle_envelope_draw(run, &call).await {
            EnvelopeDisposition::Refused(halted) => return halted,
            EnvelopeDisposition::Authorised => true,
            EnvelopeDisposition::NotApplicable => false,
        };

        let tier = tool.tier();
        let decision = self.gate_decision(&call, tool.as_ref(), envelope_authorised);
        let auto_approval = self.state.policy.is_full_auto()
            && matches!(decision, PolicyDecision::Allow)
            && tier != crate::core::tool::ToolTier::Read;
        let approval =
            (matches!(decision, PolicyDecision::Ask) || auto_approval).then(ApprovalId::new);
        let Some(session) = self.state.session else {
            return true;
        };
        let event = SmedEvent::ToolProposed {
            session,
            run,
            approval,
            call: call.clone(),
            tier,
            preview: preview.clone(),
        };
        if let Err(error) = self.persist(event).await {
            self.fail_store(run, &error);
            return true;
        }

        match decision {
            PolicyDecision::Deny(code) => {
                let result = ToolResult::refused(code, "current policy denied this tool");
                !self.record_tool_result(run, &call, result).await
            }
            PolicyDecision::Ask => {
                self.hold_for_approval(run, approval, call, tool, tier, preview)
                    .await;
                true
            }
            PolicyDecision::Allow if auto_approval => {
                let Some(approval) = approval else {
                    self.fail_run(
                        run,
                        ReasonCode::ToolExecution,
                        "full-auto resolution had no approval identity".to_owned(),
                    )
                    .await;
                    return true;
                };
                if let Err(error) = self
                    .persist(SmedEvent::ApprovalResolved {
                        session,
                        run,
                        approval,
                        decision: ApprovalDecision::AutoByPolicy,
                    })
                    .await
                {
                    self.fail_store(run, &error);
                    return true;
                }
                self.mark_load_authority(
                    run,
                    &call,
                    crate::core::event::ExtensionLoadAuthority::FullAuto,
                );
                self.start_tool(run, call, tool).await
            }
            PolicyDecision::Allow => self.start_tool(run, call, tool).await,
        }
    }

    async fn hold_for_approval(
        &mut self,
        run: RunId,
        approval: Option<ApprovalId>,
        call: ToolCall,
        tool: Arc<dyn Tool>,
        tier: crate::core::tool::ToolTier,
        preview: String,
    ) {
        let Some(approval) = approval else {
            self.fail_run(
                run,
                ReasonCode::ToolExecution,
                "approval identity was not created".to_owned(),
            )
            .await;
            return;
        };
        let request = PendingApproval {
            id: approval,
            tool_name: call.name.clone(),
            tier,
            preview,
        };
        if let Some(active) = self.run.as_mut().filter(|active| active.id == run) {
            active.phase = RunPhase::Approval;
            active.awaiting_approval = Some(PendingTool {
                approval,
                call,
                tool,
            });
            self.state.pending_approval = Some(request);
            self.publish_snapshot();
        }
    }

    pub(super) async fn resolve_approval(
        &mut self,
        approval: ApprovalId,
        decision: ApprovalDecision,
    ) {
        // This decision is runtime-authored only. A client cannot impersonate
        // the policy in the audit trail or use it as an approval shortcut.
        if decision == ApprovalDecision::AutoByPolicy {
            return;
        }
        let pending = {
            let Some(active) = self.run.as_mut() else {
                return;
            };
            let Some(pending) = active.awaiting_approval.take() else {
                return;
            };
            if pending.approval != approval {
                active.awaiting_approval = Some(pending);
                return;
            }
            pending
        };

        let (run, session) = match self.run.as_ref() {
            Some(active) => (active.id, active.session),
            None => return,
        };
        if let Err(error) = self
            .persist(SmedEvent::ApprovalResolved {
                session,
                run,
                approval,
                decision,
            })
            .await
        {
            self.fail_store(run, &error);
            return;
        }
        self.state.pending_approval = None;

        match decision {
            ApprovalDecision::Deny => {
                let result = ToolResult::refused(
                    ReasonCode::ApprovalDenied,
                    "the user denied this exact proposal",
                );
                if self.record_tool_result(run, &pending.call, result).await {
                    self.drive_tools(run).await;
                }
            }
            ApprovalDecision::ApproveOnce | ApprovalDecision::ApproveExactForSession => {
                if decision == ApprovalDecision::ApproveExactForSession
                    && let Some(command) = command_spec(&pending.call)
                {
                    self.state.exact_commands.insert(command);
                }
                self.mark_load_authority(
                    run,
                    &pending.call,
                    crate::core::event::ExtensionLoadAuthority::Approved,
                );
                if !self.start_tool(run, pending.call, pending.tool).await {
                    self.drive_tools(run).await;
                }
            }
            ApprovalDecision::AutoByPolicy => {
                // Rejected before the pending proposal was taken above.
            }
        }
    }

    fn mark_load_authority(
        &mut self,
        run: RunId,
        call: &ToolCall,
        authority: crate::core::event::ExtensionLoadAuthority,
    ) {
        if call.name != crate::context::LOAD_EXTENSION_TOOL {
            return;
        }
        if let Some(active) = self.run.as_mut().filter(|active| active.id == run) {
            active.pending_load_authority = Some((call.id.clone(), authority));
        }
    }

    fn take_load_authority(
        &mut self,
        run: RunId,
        call: &ToolCall,
    ) -> Option<crate::core::event::ExtensionLoadAuthority> {
        let active = self.run.as_mut().filter(|active| active.id == run)?;
        match active.pending_load_authority.as_ref() {
            Some((call_id, _)) if call_id == &call.id => {
                active.pending_load_authority.take().map(|(_, by)| by)
            }
            _ => None,
        }
    }

    /// Final schema and completion guard immediately before execution.
    /// Returns true when a task was spawned, false when a structured refusal was
    /// recorded synchronously.
    async fn start_tool(&mut self, run: RunId, call: ToolCall, tool: Arc<dyn Tool>) -> bool {
        if let Err(error) = self.tools.validate(tool.as_ref(), &call.arguments) {
            let result = ToolResult::refused(ReasonCode::SchemaInvalid, error.to_string());
            let _ = self.record_tool_result(run, &call, result).await;
            return false;
        }
        if let Some(result) = self.completion_refusal(&call) {
            let _ = self.record_tool_result(run, &call, result).await;
            return false;
        }
        let Some(context) = self.tool_context() else {
            let result = ToolResult::failed(
                ReasonCode::PathOutsideWorkspace,
                "open a workspace before using repository tools",
            );
            let _ = self.record_tool_result(run, &call, result).await;
            return false;
        };
        let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
            return false;
        };
        active.phase = RunPhase::Tool;
        let cancel = active.cancel.clone();
        self.publish_snapshot();
        // The runtime hosts subagent spawns itself: the tool is a marker, and
        // only the actor holds providers, the store, and budget state. The
        // ordinary pipeline (schema, policy, approval) has already run above.
        if call.name == crate::tools::subagent::SpawnSubagent::NAME {
            return self.start_spawn(run, call, context.workspace_root).await;
        }
        // Reading the session's own record is hosted for the same reason and
        // with the opposite risk: only the actor holds the store, and no tool may
        // be given it, because `EventStore` carries `append` and a tool that
        // could append could forge the evidence a completion is gated on.
        if call.name == crate::tools::session_query::QuerySession::NAME {
            return self.answer_session_query(run, call).await;
        }
        if call.name == crate::tools::memory::MemorySearch::NAME {
            return self.answer_memory_search(run, call).await;
        }
        if call.name == crate::tools::memory::MemoryTimeline::NAME {
            return self.answer_memory_timeline(run, call).await;
        }
        if call.name == crate::tools::memory::MemoryExpand::NAME {
            return self.answer_memory_expand(run, call).await;
        }
        spawn_tool(tool, call, context, cancel, run, self.mailbox.clone());
        true
    }

    pub(super) async fn handle_tool_ended(
        &mut self,
        run: RunId,
        call: ToolCall,
        outcome: ToolTaskOutcome,
    ) {
        if self.run.as_ref().is_none_or(|active| active.id != run) {
            return;
        }
        match outcome {
            Ok(Ok(result)) => self.handle_tool_success(run, call, result).await,
            Ok(Err(ToolError::Cancelled)) => {
                self.finish_run(run, FinishReason::Cancelled).await;
            }
            Ok(Err(error)) => {
                if self.record_tool_error(run, &call, &error).await {
                    self.drive_tools(run).await;
                }
            }
            Err(join_error) => {
                let error = ToolError::Execution {
                    detail: format!("tool task did not complete: {join_error}"),
                };
                if self.record_tool_error(run, &call, &error).await {
                    self.drive_tools(run).await;
                }
            }
        }
    }

    /// A tool that ran and produced a result: record it, let any write it made
    /// reach the repository projection, then continue or finish the run.
    ///
    /// Extracted from `handle_tool_ended` when the D5 refresh trigger pushed
    /// that function past the cognitive-complexity lint. The lint was the
    /// signal AGENTS.md §2.3 says it is — the success path had accumulated four
    /// decisions that have nothing to do with the cancelled and failed arms.
    async fn handle_tool_success(&mut self, run: RunId, call: ToolCall, mut result: ToolResult) {
        bound_result(&mut result, self.limits.max_tool_output_bytes);
        let completion = matches!(
            (&result.outcome, &result.effect),
            (ToolOutcome::Ok, ToolEffect::Completion { .. })
        );
        let touched_worktree = tool_touched_worktree(&result.effect);

        if !self.record_tool_result(run, &call, result).await {
            return;
        }
        if touched_worktree {
            self.refresh_repository(crate::core::repository::RefreshTrigger::ToolWrite)
                .await;
        }
        if completion {
            self.finish_run(run, FinishReason::Stop).await;
        } else {
            self.drive_tools(run).await;
        }
    }

    pub(super) async fn record_tool_result(
        &mut self,
        run: RunId,
        call: &ToolCall,
        mut result: ToolResult,
    ) -> bool {
        bound_result(&mut result, self.limits.max_tool_output_bytes);
        let load_authority = self.take_load_authority(run, call);
        let session = match self.run.as_ref().filter(|active| active.id == run) {
            Some(active) => active.session,
            None => return false,
        };
        let stored = match self
            .persist(SmedEvent::ToolCompleted {
                session,
                run,
                call_id: call.id.clone(),
                name: call.name.clone(),
                result: result.clone(),
            })
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                self.fail_store(run, &error);
                return false;
            }
        };
        result.evidence_event_id = Some(stored.id.to_string());
        if result.outcome.is_ok() {
            match &result.effect {
                ToolEffect::Mutation { .. } => {
                    self.state.last_mutation_sequence = Some(stored.sequence);
                }
                ToolEffect::Command { success: true, .. } => {
                    self.state
                        .successful_command_evidence
                        .insert(stored.id.to_string(), stored.sequence);
                }
                ToolEffect::SkillActivated { name, project } => {
                    self.state.activated_skills.insert(name.clone());
                    self.state.workspace_trusted |= *project;
                }
                // The one place the read and the id of the event that carried
                // it are both in hand. The read set is written
                // earlier, inside the tool, where no event exists yet — which
                // is exactly why `ReadBeforeEditEvidence::tool_event_id` was
                // empty until here.
                ToolEffect::Read { path, sha256 } => {
                    self.state.read_evidence.insert(
                        path.clone(),
                        crate::core::change_capture::ReadRecord {
                            path: path.clone(),
                            sha256: sha256.clone(),
                            tool_event_id: stored.id.to_string(),
                        },
                    );
                }
                ToolEffect::None
                | ToolEffect::Command { success: false, .. }
                | ToolEffect::Completion { .. } => {}
            }

            // A completed load_extension is the model proposing to extend itself.
            // The tool validated the name and the gate authorised the call; only
            // the actor holds the registry, so the registration happens here
            // , the same division spawn_subagent uses.
            if call.name == crate::context::LOAD_EXTENSION_TOOL
                && let Some(name) = call
                    .arguments
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                && let Some(authority) = load_authority
            {
                self.agent_load_extension(session, run, name, authority)
                    .await;
            }
        }

        let message = CanonicalMessage::tool_result(&call.id, &call.name, result);
        self.state.push_message(Some(stored.sequence), message);
        self.publish_snapshot();
        true
    }

    async fn record_tool_error(&mut self, run: RunId, call: &ToolCall, error: &ToolError) -> bool {
        let session = match self.run.as_ref().filter(|active| active.id == run) {
            Some(active) => active.session,
            None => return false,
        };
        let stored = match self
            .persist(SmedEvent::ToolFailed {
                session,
                run,
                call_id: call.id.clone(),
                name: call.name.clone(),
                code: error.reason_code(),
                detail: error.to_string(),
            })
            .await
        {
            Ok(stored) => stored,
            Err(store_error) => {
                self.fail_store(run, &store_error);
                return false;
            }
        };
        let result = ToolResult::failed(error.reason_code(), error.to_string());
        let message = CanonicalMessage::tool_result(&call.id, &call.name, result);
        self.state.push_message(Some(stored.sequence), message);
        self.publish_snapshot();
        true
    }

    fn completion_refusal(&self, call: &ToolCall) -> Option<ToolResult> {
        if call.name != "finish_task"
            || call
                .arguments
                .get("outcome")
                .and_then(serde_json::Value::as_str)
                != Some("verified")
        {
            return None;
        }
        let mutation = self.state.last_mutation_sequence?;
        let evidence = call
            .arguments
            .get("evidence_event_ids")
            .and_then(serde_json::Value::as_array);
        let valid = evidence.is_some_and(|ids| {
            ids.iter().filter_map(serde_json::Value::as_str).any(|id| {
                self.state
                    .successful_command_evidence
                    .get(id)
                    .is_some_and(|sequence| *sequence > mutation)
            })
        });
        (!valid).then(|| {
            ToolResult::refused(
                ReasonCode::CompletionEvidenceMissing,
                "verified completion requires a cited successful command after the last mutation",
            )
        })
    }

    fn tool_context(&self) -> Option<ToolContext> {
        Some(ToolContext {
            workspace_root: self.state.workspace_root.clone()?,
            read_set: Arc::clone(&self.state.read_set),
            max_output_bytes: self.limits.max_tool_output_bytes,
            command_timeout: self.limits.command_timeout,
        })
    }
}

fn spawn_tool(
    tool: Arc<dyn Tool>,
    call: ToolCall,
    context: ToolContext,
    cancel: CancellationToken,
    run: RunId,
    mailbox: mpsc::Sender<Mail>,
) {
    tokio::spawn(async move {
        let arguments = call.arguments.clone();
        let task = tokio::spawn(async move { tool.execute(arguments, context, cancel).await });
        let outcome = task.await;
        let _ = mailbox.send(Mail::ToolEnded { run, call, outcome }).await;
    });
}

fn command_spec(call: &ToolCall) -> Option<CommandSpec> {
    if call.name != "run_command" {
        return None;
    }
    let program = call.arguments.get("program")?.as_str()?.to_owned();
    let arguments = call
        .arguments
        .get("arguments")?
        .as_array()?
        .iter()
        .map(serde_json::Value::as_str)
        .map(|argument| argument.map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    Some(CommandSpec { program, arguments })
}

fn bound_text(text: String, max_bytes: usize) -> (String, bool) {
    crate::tools::output::truncate(text, max_bytes)
}

/// Whether a tool's effect could have changed the working tree (Phase D5).
///
/// `Command` counts alongside `Mutation`: a governed command can create,
/// delete, or stage files just as an edit can, and a repository surface that
/// updates after `edit_file` but not after `run_command` is wrong in the
/// harder-to-notice direction. `Read`, `Completion`, `SkillActivated`, and
/// `None` cannot touch the tree, so they cost no subprocess — this is what
/// keeps the refresh one `git status` per *write*, never one per token
/// (AGENTS.md §5).
fn tool_touched_worktree(effect: &ToolEffect) -> bool {
    match effect {
        ToolEffect::Mutation { .. } | ToolEffect::Command { .. } => true,
        ToolEffect::None
        | ToolEffect::Read { .. }
        | ToolEffect::Completion { .. }
        | ToolEffect::SkillActivated { .. } => false,
    }
}

fn bound_result(result: &mut ToolResult, max_bytes: usize) {
    let content = std::mem::take(&mut result.content);
    let (content, truncated) = bound_text(content, max_bytes);
    result.content = content;
    result.truncated |= truncated;
}
