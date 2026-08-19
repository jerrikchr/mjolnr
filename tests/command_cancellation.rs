//! Cancellation must terminate the approved process group and the agent loop.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use smed::core::command::{ApprovalDecision, SmedCommand};
use smed::core::error::ProviderError;
use smed::core::event::{FinishReason, ProviderEvent, SmedEvent};
use smed::core::message::{ContentBlock, ToolCall};
use smed::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PROVIDER: &str = "cancel-test";
const MODEL: &str = "cancel-test-1";

#[derive(Debug)]
struct CommandProvider {
    requests: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for CommandProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(MODEL),
            provider: self.id(),
            display_name: "Cancellation test".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(1024),
            max_output_tokens: Some(1024),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let has_result = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
        if has_result {
            return Err(ProviderError::Protocol {
                detail: "a cancelled command must not trigger a later model call".to_owned(),
            });
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let call = ToolCall {
            id: "call_long_command".to_owned(),
            name: "run_command".to_owned(),
            arguments: serde_json::json!({
                "program": "/bin/sh",
                "arguments": ["-c", "touch started; sleep 1; touch later"]
            }),
            provider_signature: None,
        };
        events
            .send(ProviderEvent::ToolCallCompleted { call })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        events
            .send(ProviderEvent::Finished {
                reason: FinishReason::ToolCalls,
            })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason: FinishReason::ToolCalls,
            usage: None,
        })
    }
}

async fn wait_for(
    events: &mut smed::core::runtime::RuntimeSubscription,
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

#[cfg(unix)]
#[tokio::test]
async fn cancel_kills_descendants_and_prevents_later_provider_calls() {
    let workspace = TempDir::new().expect("workspace");
    let requests = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CommandProvider {
        requests: Arc::clone(&requests),
    });
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(vec![provider], store);
    let mut events = runtime.subscribe();

    runtime
        .dispatch(SmedCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open workspace");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "start the cancellable command".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");

    let proposal = wait_for(&mut events, "command approval", |event| {
        matches!(event, SmedEvent::ToolProposed { approval: Some(_), call, .. } if call.name == "run_command")
    })
    .await;
    let SmedEvent::ToolProposed {
        approval: Some(approval),
        preview,
        ..
    } = proposal
    else {
        panic!("expected command proposal")
    };
    assert!(preview.contains("touch started; sleep 1; touch later"));
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve command");

    tokio::time::timeout(Duration::from_secs(5), async {
        while !workspace.path().join("started").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("child command starts");
    runtime
        .dispatch(SmedCommand::CancelRun)
        .await
        .expect("cancel run");

    let terminal = wait_for(&mut events, "cancelled terminal event", |event| {
        matches!(
            event,
            SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
        )
    })
    .await;
    match terminal {
        SmedEvent::RunFinished { reason, .. } => assert_eq!(reason, FinishReason::Cancelled),
        other => panic!("cancel must finish cleanly: {other:?}"),
    }

    // The child would create `later` after one second if either the shell or
    // its sleep descendant survived. Waiting past that deadline is the
    // observable proof that the whole process group was terminated.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert!(!workspace.path().join("later").exists());
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}
