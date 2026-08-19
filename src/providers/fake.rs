//! A deterministic fake provider.
//!
//! Exists so the whole vertical slice — runtime, store, TUI — can be built and
//! tested with no network, no credentials, and no flakiness. Phase 2 adds the
//! first real adapter *against the same trait*, which is the point: if the fake
//! needs a trait change to work, the trait is wrong.
//!
//! It deliberately reproduces the awkward parts of real providers rather than an
//! idealised version of them:
//!
//! - text arrives in **fragments**, split mid-word
//! - tool arguments arrive as **partial JSON strings**, split mid-token, and are
//!   only parseable at the completion boundary
//! - an **unknown upstream event** appears, because real providers add them
//!
//! A fake that only emits tidy whole messages would let a decoder bug through.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::{FinishReason, ProviderEvent};
use crate::core::message::{ContentBlock, ToolCall, ToolEffect};
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId, Usage};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};

/// What the fake should do when asked to stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FakeScript {
    /// Stream chunked text and stop.
    #[default]
    Text,
    /// Stream text, then a tool call whose arguments arrive as fragments.
    TextThenToolCall,
    /// Exercise the complete guarded Phase 3 loop in a disposable repository.
    GuardedLoop,
    /// Attempt to claim verified completion after a mutation without command
    /// evidence. The runtime must refuse it and return the refusal to the fake.
    EvidenceMissing,
    /// Propose an edit outside the workspace. Used to prove policy cannot
    /// bypass containment, including in full-auto.
    OutsideWorkspaceWrite,
    /// Fail partway through, after emitting output.
    ///
    /// The interesting case: a stream that produced tokens and *then* failed is
    /// never safe to replay (AGENTS.md §4).
    FailMidStream,
    /// Drive the Phase 13 subagent loop. One script serves parent and children
    /// because they share a provider registry: behaviour dispatches on the
    /// latest user directive (`spawn-two:` proposes a two-child spawn;
    /// `worker:<file>` writes the file and reports a result; `worker-noreport:`
    /// finishes without reporting; `worker-hold:` blocks until cancelled).
    Subagent,
    /// Read this session's own record, then finish.
    SessionQuery,
    /// Return bounded JSON for the interview/PRD/plan synthesis workflow.
    PlanInterview,
}

/// A provider that answers from a script.
#[derive(Debug, Clone)]
pub struct FakeProvider {
    script: FakeScript,
    /// Delay between fragments. Zero in tests for determinism and speed; the
    /// binary uses a human-visible value to prove streaming is incremental.
    fragment_delay: std::time::Duration,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new(FakeScript::Text)
    }
}

impl FakeProvider {
    #[must_use]
    pub fn new(script: FakeScript) -> Self {
        Self {
            script,
            fragment_delay: std::time::Duration::ZERO,
        }
    }

    /// Slow the fake down so a human can see text arrive incrementally.
    #[must_use]
    pub fn with_fragment_delay(mut self, delay: std::time::Duration) -> Self {
        self.fragment_delay = delay;
        self
    }

    pub const MODEL: &'static str = "fake-1";
    pub const ID: &'static str = "fake";

    /// Sends an event, honouring cancellation and backpressure.
    ///
    /// Returns `false` when the consumer is gone or the run was cancelled, so
    /// the caller stops rather than pushing into a closed channel.
    async fn emit(
        &self,
        events: &mpsc::Sender<ProviderEvent>,
        cancel: &CancellationToken,
        event: ProviderEvent,
    ) -> bool {
        if cancel.is_cancelled() {
            return false;
        }

        if !self.fragment_delay.is_zero() {
            tokio::select! {
                () = cancel.cancelled() => return false,
                () = tokio::time::sleep(self.fragment_delay) => {}
            }
        }

        // `send` awaits capacity: a slow consumer slows the producer, which is
        // exactly the backpressure the bounded channel exists to provide.
        tokio::select! {
            () = cancel.cancelled() => false,
            result = events.send(event) => result.is_ok(),
        }
    }

    async fn emit_tool_call(
        &self,
        events: &mpsc::Sender<ProviderEvent>,
        cancel: &CancellationToken,
        call: ToolCall,
        fragments: Vec<String>,
    ) -> Result<(), ProviderError> {
        let started = ProviderEvent::ToolCallStarted {
            id: call.id.clone(),
            name: call.name.clone(),
        };
        if !self.emit(events, cancel, started).await {
            return Err(ProviderError::Cancelled);
        }
        for fragment in fragments {
            let event = ProviderEvent::ToolArgumentsDelta {
                id: call.id.clone(),
                fragment,
            };
            if !self.emit(events, cancel, event).await {
                return Err(ProviderError::Cancelled);
            }
        }
        if !self
            .emit(events, cancel, ProviderEvent::ToolCallCompleted { call })
            .await
        {
            return Err(ProviderError::Cancelled);
        }
        Ok(())
    }

    /// Park a scripted worker until its parent cancels. The timeout is only a
    /// leak guard; deterministic tests always take the cancellation branch.
    async fn hold_subagent_if_requested(
        &self,
        request: &ProviderRequest,
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        if self.script != FakeScript::Subagent
            || !tool_results(request).is_empty()
            || !latest_user_text(request).contains("worker-hold:")
        {
            return Ok(());
        }
        tokio::select! {
            () = cancel.cancelled() => Err(ProviderError::Cancelled),
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => Ok(()),
        }
    }

    async fn emit_text_response(
        &self,
        request: &ProviderRequest,
        events: &mpsc::Sender<ProviderEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        let fragments: Vec<String> = if self.script == FakeScript::PlanInterview {
            vec![plan_response(&latest_user_text(request))]
        } else {
            TEXT_FRAGMENTS
                .iter()
                .map(|fragment| (*fragment).to_owned())
                .collect()
        };
        for fragment in fragments {
            if !self
                .emit(events, cancel, ProviderEvent::TextDelta { text: fragment })
                .await
            {
                return Err(ProviderError::Cancelled);
            }
        }
        if !self
            .emit(
                events,
                cancel,
                ProviderEvent::UnknownUpstream {
                    kind: "fake.future_event".to_owned(),
                },
            )
            .await
        {
            return Err(ProviderError::Cancelled);
        }
        Ok(())
    }

    fn scripted_proposal(
        &self,
        request: &ProviderRequest,
        results: &[&crate::core::message::ToolResult],
    ) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
        match self.script {
            FakeScript::TextThenToolCall if results.is_empty() => {
                let arguments = parse_fake_arguments(&ARGUMENT_FRAGMENTS.concat())?;
                Ok(Some((
                    ToolCall {
                        id: "call_fake_1".to_owned(),
                        name: "read_file".to_owned(),
                        arguments,
                        provider_signature: None,
                    },
                    ARGUMENT_FRAGMENTS
                        .iter()
                        .map(|fragment| (*fragment).to_owned())
                        .collect(),
                )))
            }
            FakeScript::GuardedLoop => guarded_proposal(results),
            FakeScript::EvidenceMissing => Ok(evidence_missing_proposal(results.len())),
            FakeScript::Subagent => subagent_proposal(&latest_user_text(request), results.len()),
            FakeScript::SessionQuery => session_query_proposal(results.len()),
            FakeScript::OutsideWorkspaceWrite if results.is_empty() => Ok(Some((
                ToolCall {
                    id: "call_outside".to_owned(),
                    name: "edit_file".to_owned(),
                    arguments: serde_json::json!({
                        "path": "../outside.txt",
                        "old": "safe",
                        "new": "changed"
                    }),
                    provider_signature: None,
                },
                vec![
                    "{\"path\":\"../outside.txt\",\"old\":\"safe\",\"new\":\"changed\"}".to_owned(),
                ],
            ))),
            FakeScript::Text
            | FakeScript::TextThenToolCall
            | FakeScript::OutsideWorkspaceWrite
            | FakeScript::PlanInterview
            | FakeScript::FailMidStream => Ok(None),
        }
    }
}

/// Text fragments, split mid-word on purpose.
const TEXT_FRAGMENTS: &[&str] = &[
    "smed ", "streams ", "text ", "incre", "mentally", ", so a ", "decoder ", "bug ", "cannot ",
    "hide.",
];

/// Tool arguments as partial JSON, split mid-key and mid-value.
///
/// Concatenated this is `{"path": "src/lib.rs", "limit": 40}`. Parsing any
/// prefix of it is a syntax error — which is the property that matters
/// (`docs/provider-contract.md` §0).
const ARGUMENT_FRAGMENTS: &[&str] = &[
    "{\"pa",
    "th\": \"src/li",
    "b.rs\", ",
    "\"li",
    "mit\": 4",
    "0}",
];

#[async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(Self::ID)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(Self::MODEL),
            provider: self.id(),
            display_name: "Fake 1 (offline)".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(8_192),
            max_output_tokens: Some(4_096),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        validate_model(&request)?;

        if !self.emit(&events, &cancel, ProviderEvent::Started).await {
            return Err(ProviderError::Cancelled);
        }

        self.hold_subagent_if_requested(&request, &cancel).await?;

        self.emit_text_response(&request, &events, &cancel).await?;

        let tool_results = tool_results(&request);
        let proposal = if self.script == FakeScript::FailMidStream {
            let failed = ProviderEvent::Failed {
                detail: "scripted mid-stream failure after output".to_owned(),
            };
            // Emit the terminal event, then report the error. Output was
            // already produced, so this run must never be auto-retried.
            self.emit(&events, &cancel, failed).await;
            return Err(ProviderError::Protocol {
                detail: "scripted mid-stream failure after output".to_owned(),
            });
        } else {
            self.scripted_proposal(&request, &tool_results)?
        };

        if let Some((call, fragments)) = proposal.as_ref() {
            self.emit_tool_call(&events, &cancel, call.clone(), fragments.clone())
                .await?;
        }

        let usage = Usage {
            input_tokens: 12,
            output_tokens: 34,
        };
        if !self
            .emit(&events, &cancel, ProviderEvent::Usage { usage })
            .await
        {
            return Err(ProviderError::Cancelled);
        }

        let reason = if proposal.is_some() {
            FinishReason::ToolCalls
        } else {
            FinishReason::Stop
        };

        // The event narrates; the return value below is the authority. They are
        // built from one `reason` so they cannot disagree.
        if !self
            .emit(&events, &cancel, ProviderEvent::Finished { reason })
            .await
        {
            return Err(ProviderError::Cancelled);
        }

        Ok(ProviderCompletion {
            reason,
            usage: Some(usage),
        })
    }
}

fn validate_model(request: &ProviderRequest) -> Result<(), ProviderError> {
    if request.model.as_str() == FakeProvider::MODEL {
        return Ok(());
    }
    Err(ProviderError::IncompatibleModel {
        model: request.model.to_string(),
        capability: "unknown model for the fake provider".to_owned(),
    })
}

fn plan_response(user_text: &str) -> String {
    if user_text.contains("SYNTHESIZE_PLAN") {
        return r#"{"title":"Deliver the governed planning flow","summary":"Persist the interview PRD, review it through council, and expose the approved plan lifecycle.","steps":[{"title":"Persist the interview","description":"Record bounded questions, answers, and the generated PRD in the session event log."},{"title":"Review the PRD","description":"Run the durable PRD through the advisory council and retain dissent."},{"title":"Hand off the plan","description":"Propose a reviewed implementation plan for human approval and handoff."}]}"#.to_owned();
    }
    if user_text.contains("INTERVIEW_ANSWER") {
        return r#"{"kind":"prd","title":"Governed planning","problem":"Owners need a durable path from an initial idea to a reviewed implementation plan.","users":["repository owner"],"requirements":[{"id":"REQ-1","title":"Record the PRD","description":"Persist the generated product requirements document before review."},{"id":"REQ-2","title":"Retain council dissent","description":"Link the PRD to a durable council review without authorizing execution."}],"acceptance_criteria":["A restart preserves the PRD and its council link","The resulting plan remains human-approvable"],"non_goals":["Automatic code execution"],"constraints":["All side effects remain behind smed policy gates"]}"#.to_owned();
    }
    r#"{"kind":"question","prompt":"What is the smallest useful scope for this plan?","options":["Narrow vertical slice","Broad platform change"],"is_multi_select":false}"#.to_owned()
}

fn parse_fake_arguments(raw: &str) -> Result<serde_json::Value, ProviderError> {
    serde_json::from_str(raw).map_err(|error| ProviderError::Protocol {
        detail: format!("fake produced unparseable tool arguments: {error}"),
    })
}

fn tool_results(request: &ProviderRequest) -> Vec<&crate::core::message::ToolResult> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::ToolResult { result, .. } => Some(result),
            _ => None,
        })
        .collect()
}

fn guarded_proposal(
    results: &[&crate::core::message::ToolResult],
) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
    let (name, arguments) = match results.len() {
        0 => ("read_file", serde_json::json!({ "path": "fixture.txt" })),
        1 => (
            "edit_file",
            serde_json::json!({ "path": "fixture.txt", "old": "before\n", "new": "after\n" }),
        ),
        2 => (
            "run_command",
            serde_json::json!({ "program": "git", "arguments": ["diff", "--check"] }),
        ),
        3 => {
            let evidence = results
                .iter()
                .find_map(|result| match (&result.effect, &result.evidence_event_id) {
                    (ToolEffect::Command { success: true, .. }, Some(event_id)) => {
                        Some(event_id.clone())
                    }
                    _ => None,
                })
                .ok_or_else(|| ProviderError::Protocol {
                    detail: "guarded fake did not receive successful command evidence".to_owned(),
                })?;
            (
                "finish_task",
                serde_json::json!({
                    "outcome": "verified",
                    "summary": "fixture updated and git diff check passed",
                    "evidence_event_ids": [evidence],
                    "remaining_risks": []
                }),
            )
        }
        _ => return Ok(None),
    };
    let raw = serde_json::to_string(&arguments).map_err(|error| ProviderError::Protocol {
        detail: format!("fake cannot encode tool arguments: {error}"),
    })?;
    let split = raw.len() / 2;
    let (first, second) = raw.split_at(split);
    Ok(Some((
        ToolCall {
            id: format!("call_guarded_{}", results.len() + 1),
            name: name.to_owned(),
            arguments,
            provider_signature: None,
        },
        vec![first.to_owned(), second.to_owned()],
    )))
}

/// The most recent user message's text, which for the subagent script carries
/// the directive that selects behaviour.
fn latest_user_text(request: &ProviderRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == crate::core::message::Role::User)
        .map(crate::core::message::CanonicalMessage::text)
        .unwrap_or_default()
}

/// The directive-driven Phase 13 script.
fn subagent_proposal(
    directive: &str,
    results: usize,
) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
    if let Some(proposal) = worker_proposal(directive, results)? {
        return Ok(Some(proposal));
    }
    spawn_proposal(directive, results)
}

/// Read the session's own record, then finish.
///
/// `finish_task` carries no evidence IDs because reading is not a mutation, so
/// nothing needs proving — which is also why the run is allowed to end.
fn session_query_proposal(
    results: usize,
) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
    let (name, arguments) = match results {
        0 => ("query_session", serde_json::json!({ "limit": 10 })),
        1 => finish("read the record"),
        _ => return Ok(None),
    };
    split_call(name, arguments, results)
}

fn worker_proposal(
    directive: &str,
    results: usize,
) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
    // Worker roles first: a child's wrapped directive also contains the word
    // "spawn" when the parent's text is echoed, so match worker prefixes
    // before spawn triggers.
    if let Some(file) = directive_argument(directive, "worker:") {
        let (name, arguments) = match results {
            0 => (
                "write_file",
                serde_json::json!({
                    "path": file,
                    "content": format!("made by the {file} worker\n")
                }),
            ),
            1 => (
                "report_result",
                serde_json::json!({ "summary": format!("wrote {file}") }),
            ),
            2 => finish("worker done"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if let Some(file) = directive_argument(directive, "worker-noreport:") {
        let (name, arguments) = match results {
            0 => (
                "write_file",
                serde_json::json!({
                    "path": file,
                    "content": "unreported work\n"
                }),
            ),
            1 => finish("finished without reporting"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    // A child that reads its own record and reports what it saw. The point of
    // the round trip is what is *absent*: the parent's directive is not in the
    // child's session, so a window that contained it would prove the scope
    // boundary had been crossed.
    if directive.contains("worker-query:") {
        let (name, arguments) = match results {
            0 => ("query_session", serde_json::json!({ "limit": 50 })),
            1 => (
                "report_result",
                serde_json::json!({ "summary": "read my own record" }),
            ),
            2 => finish("worker done"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("worker-invalid:") {
        let (name, arguments) = match results {
            0 => ("report_result", serde_json::json!({ "unexpected": true })),
            1 => finish("finished after invalid report"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if let Some(file) = directive_argument(directive, "worker-read:") {
        let (name, arguments) = match results {
            0 => ("read_file", serde_json::json!({ "path": file })),
            1 => (
                "report_result",
                serde_json::json!({ "summary": format!("read {file}") }),
            ),
            2 => finish("worker done"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if let Some(file) = directive_argument(directive, "worker-edit:") {
        let (name, arguments) = match results {
            // Read then write: the write gate refuses an unobserved file
            // (`FileNotObserved`), so the edit worker must observe it first.
            0 => ("read_file", serde_json::json!({ "path": file })),
            1 => (
                "write_file",
                serde_json::json!({
                    "path": file,
                    "content": format!("edited by the {file} worker\n")
                }),
            ),
            2 => (
                "report_result",
                serde_json::json!({ "summary": format!("edited {file}") }),
            ),
            3 => finish("worker done"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    Ok(None)
}

#[allow(
    clippy::too_many_lines,
    reason = "one flat directive-to-spawn mapping; each arm is an independent scripted scenario and splitting it would scatter one vocabulary"
)]
fn spawn_proposal(
    directive: &str,
    results: usize,
) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
    if directive.contains("spawn-two:") {
        let (name, arguments) = match results {
            0 => (
                "spawn_subagent",
                serde_json::json!({
                    "children": [
                        {
                            "directive": "worker:alpha.txt",
                            "policy": "workspace-write",
                            "max_provider_turns": 4,
                            "max_tool_calls": 6,
                            "result_schema": result_schema()
                        },
                        {
                            "directive": "worker:beta.txt",
                            "policy": "workspace-write",
                            "max_provider_turns": 4,
                            "max_tool_calls": 6,
                            "result_schema": result_schema()
                        }
                    ]
                }),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("spawn-noreport:") {
        let (name, arguments) = match results {
            0 => (
                "spawn_subagent",
                child_spawn("worker-noreport:gamma.txt", "workspace-write", 4, 6),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("spawn-invalid:") {
        let (name, arguments) = match results {
            0 => (
                "spawn_subagent",
                child_spawn("worker-invalid: report the wrong shape", "read-only", 3, 4),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("spawn-hold:") {
        let (name, arguments) = match results {
            0 => (
                "spawn_subagent",
                child_spawn("worker-hold: park until cancelled", "workspace-write", 4, 6),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("spawn-query:") {
        let (name, arguments) = match results {
            0 => (
                "spawn_subagent",
                child_spawn("worker-query: read your own record", "read-only", 4, 6),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("spawn-clamp:") {
        let (name, arguments) = match results {
            0 => (
                "spawn_subagent",
                child_spawn("worker:clamped.txt", "full-auto", 4, 6),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    if directive.contains("spawn-collide:") {
        let (name, arguments) = match results {
            // One child reads `shared.txt`; its sibling edits the same path.
            // The reader's finish must be invalidated by the sibling's write.
            0 => (
                "spawn_subagent",
                serde_json::json!({
                    "children": [
                        {
                            "directive": "worker-read:shared.txt",
                            "policy": "read-only",
                            "max_provider_turns": 4,
                            "max_tool_calls": 6,
                            "result_schema": result_schema()
                        },
                        {
                            "directive": "worker-edit:shared.txt",
                            "policy": "workspace-write",
                            "max_provider_turns": 4,
                            "max_tool_calls": 6,
                            "result_schema": result_schema()
                        }
                    ]
                }),
            ),
            1 => finish("delegation settled"),
            _ => return Ok(None),
        };
        return split_call(name, arguments, results);
    }
    Ok(None)
}

fn child_spawn(
    directive: &str,
    policy: &str,
    max_provider_turns: u32,
    max_tool_calls: u32,
) -> serde_json::Value {
    serde_json::json!({
        "children": [{
            "directive": directive,
            "policy": policy,
            "max_provider_turns": max_provider_turns,
            "max_tool_calls": max_tool_calls,
            "result_schema": result_schema()
        }]
    })
}

fn result_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "summary": { "type": "string" } },
        "required": ["summary"],
        "additionalProperties": false
    })
}

fn finish(summary: &str) -> (&'static str, serde_json::Value) {
    (
        "finish_task",
        serde_json::json!({
            "outcome": "unverified",
            "summary": summary,
            "evidence_event_ids": [],
            "remaining_risks": []
        }),
    )
}

fn directive_argument(directive: &str, prefix: &str) -> Option<String> {
    let start = directive.find(prefix)? + prefix.len();
    let rest = directive.get(start..)?;
    let end = rest
        .find(|character: char| character.is_whitespace() || character == '`')
        .unwrap_or(rest.len());
    let value = rest.get(..end)?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn split_call(
    name: &str,
    arguments: serde_json::Value,
    results: usize,
) -> Result<Option<(ToolCall, Vec<String>)>, ProviderError> {
    let raw = serde_json::to_string(&arguments).map_err(|error| ProviderError::Protocol {
        detail: format!("fake cannot encode tool arguments: {error}"),
    })?;
    let split = raw.len() / 2;
    let (first, second) = raw.split_at(split);
    Ok(Some((
        ToolCall {
            id: format!("call_subagent_{}", results + 1),
            name: name.to_owned(),
            arguments,
            provider_signature: None,
        },
        vec![first.to_owned(), second.to_owned()],
    )))
}

fn evidence_missing_proposal(count: usize) -> Option<(ToolCall, Vec<String>)> {
    let (name, arguments) = match count {
        0 => ("read_file", serde_json::json!({ "path": "fixture.txt" })),
        1 => (
            "edit_file",
            serde_json::json!({ "path": "fixture.txt", "old": "before\n", "new": "after\n" }),
        ),
        2 => (
            "finish_task",
            serde_json::json!({
                "outcome": "verified",
                "summary": "claimed success without verification",
                "evidence_event_ids": [],
                "remaining_risks": []
            }),
        ),
        _ => return None,
    };
    let raw = arguments.to_string();
    Some((
        ToolCall {
            id: format!("call_missing_evidence_{}", count + 1),
            name: name.to_owned(),
            arguments,
            provider_signature: None,
        },
        vec![raw],
    ))
}

// AGENTS.md §7: tests may panic freely — a panicking assertion is a failing
// test, not a corrupted terminal. `clippy.toml` covers unwrap/expect/panic in
// tests but has no equivalent for indexing, so it is stated per module.
#[allow(clippy::indexing_slicing)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_fragments_are_only_valid_when_whole() {
        // The property the fake exists to enforce: any prefix is a syntax
        // error, so a decoder that parses early cannot pass.
        for count in 1..ARGUMENT_FRAGMENTS.len() {
            let prefix = ARGUMENT_FRAGMENTS[..count].concat();
            assert!(
                serde_json::from_str::<serde_json::Value>(&prefix).is_err(),
                "prefix of {count} fragment(s) parsed, but should not have: {prefix}"
            );
        }

        let whole = ARGUMENT_FRAGMENTS.concat();
        let value: serde_json::Value = serde_json::from_str(&whole).expect("whole is valid JSON");
        assert_eq!(value["path"], "src/lib.rs");
        assert_eq!(value["limit"], 40);
    }

    #[test]
    fn text_fragments_split_words() {
        // A fake that emitted whole words would not exercise reassembly.
        let joined = TEXT_FRAGMENTS.concat();
        assert!(joined.contains("incrementally"));
        assert!(
            TEXT_FRAGMENTS.iter().any(|f| !f.ends_with(' ')),
            "at least one fragment must split mid-word"
        );
    }
}
