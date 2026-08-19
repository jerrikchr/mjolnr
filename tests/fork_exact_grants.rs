//! Exact-command grants are session-scoped and do not cross a fork
//! (; `docs/persistence.md` §6).
//!
//! The claim is structural — `SessionState::reset_keeping_project` drops
//! `exact_commands` and the fork path never restores them — but until now it was
//! guaranteed by absence, not by a test, because exercising it needs an
//! approval-flow fixture. This is that fixture: a provider that proposes the
//! same command on demand, driven across a grant and then a fork.
//!
//! The signal is the `ToolProposed` event's `approval` field. Under
//! `workspace-write` an `Execute` command asks unless it is exact-approved, so
//! `Some(approval)` means "gated" and `None` means "auto-ran on the grant". The
//! grant is therefore the only variable: it flips the field within a session,
//! and a fork must flip it back.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use smed::core::command::{ApprovalDecision, SmedCommand};
use smed::core::error::ProviderError;
use smed::core::event::{FinishReason, ProviderEvent, SessionId, SmedEvent};
use smed::core::message::{ContentBlock, ToolCall};
use smed::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use smed::core::policy::PolicyMode;
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::runtime::{RuntimeSnapshot, RuntimeSubscription, SmedRuntime};
use smed::core::store::EventStore;
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PROVIDER: &str = "cmd";
const MODEL: &str = "cmd-1";

/// Proposes one `run_command` per user turn, then finishes.
///
/// The decision is on the *last* message so history does not confuse it: a turn
/// whose last message is a tool result is the turn after a command ran, and it
/// finishes; any other turn proposes the command. The argv is fixed, so every
/// proposal is the same `CommandSpec` and an exact grant matches it.
#[derive(Debug)]
struct CommandProvider;

#[async_trait]
impl Provider for CommandProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(MODEL),
            provider: self.id(),
            display_name: MODEL.to_owned(),
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
        let after_command = request.messages.last().is_some_and(|message| {
            message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        });
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        let reason = if after_command {
            events
                .send(ProviderEvent::TextDelta {
                    text: "done".to_owned(),
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            FinishReason::Stop
        } else {
            let call = ToolCall {
                id: "cmd-call".to_owned(),
                name: "run_command".to_owned(),
                arguments: serde_json::json!({ "program": "echo", "arguments": ["hi"] }),
                provider_signature: None,
            };
            events
                .send(ProviderEvent::ToolCallStarted {
                    id: call.id.clone(),
                    name: call.name.clone(),
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            events
                .send(ProviderEvent::ToolCallCompleted { call })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            FinishReason::ToolCalls
        };
        events
            .send(ProviderEvent::Finished { reason })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason,
            usage: None,
        })
    }
}

async fn wait_for(
    events: &mut RuntimeSubscription,
    label: &str,
    mut predicate: impl FnMut(&SmedEvent) -> bool,
) -> SmedEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => {}
                Err(error) => panic!("event feed ended while waiting for {label}: {error}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"))
}

async fn settle(runtime: &Runtime, ready: impl Fn(&RuntimeSnapshot) -> bool) -> RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

fn is_command_proposal(event: &SmedEvent) -> bool {
    matches!(event, SmedEvent::ToolProposed { call, .. } if call.name == "run_command")
}

fn approval_of(event: &SmedEvent) -> Option<smed::core::command::ApprovalId> {
    match event {
        SmedEvent::ToolProposed { approval, .. } => *approval,
        other => panic!("expected a tool proposal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_exact_command_grant_does_not_cross_a_fork() {
    let repo = tempfile::tempdir().expect("temp repository");
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(CommandProvider);
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: repo.path().to_owned(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::WorkspaceWrite,
        })
        .await
        .expect("set policy");
    let opened = settle(&runtime, |snapshot| snapshot.session.is_some()).await;
    let original = opened.session.expect("session");

    // Turn 1: no grant yet, so the command is gated. Approve it for the session.
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "run it".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let first = wait_for(&mut events, "first proposal", is_command_proposal).await;
    let approval = approval_of(&first).expect("the first command must be gated");
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::ApproveExactForSession,
        })
        .await
        .expect("grant the command for the session");
    wait_for(&mut events, "first run finished", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    // Turn 2, same session: the grant now auto-runs the identical command, so
    // the proposal carries no approval. This proves the grant is real.
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "run it again".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let second = wait_for(&mut events, "second proposal", is_command_proposal).await;
    assert!(
        approval_of(&second).is_none(),
        "the granted command must auto-run within the session, not re-ask"
    );
    wait_for(&mut events, "second run finished", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    // Fork the branch into a new session. Policy carries; the grant must not.
    runtime
        .dispatch(SmedCommand::ForkSession { before: None })
        .await
        .expect("fork");
    let forked: SessionId = settle(&runtime, |snapshot| {
        snapshot.session.is_some_and(|session| session != original) && !snapshot.run_active
    })
    .await
    .session
    .expect("forked session");
    assert_ne!(forked, original);
    assert_eq!(
        runtime.snapshot().policy,
        PolicyMode::WorkspaceWrite,
        "the fork must carry policy forward"
    );

    // Turn 3, in the fork: the same command is gated again — the grant did not
    // cross. If `reset_keeping_project` had kept it, this would auto-run and the
    // proposal would carry no approval.
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "run it in the fork".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let third = wait_for(&mut events, "forked proposal", is_command_proposal).await;
    assert!(
        approval_of(&third).is_some(),
        "the exact-command grant must not cross the fork; the command must re-ask"
    );

    let _ = runtime.close().await;
}
