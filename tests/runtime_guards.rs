//! Runtime-level negative tests for the Phase 3 proposal pipeline.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mjolnr::core::command::{ApprovalDecision, MjolnrCommand};
use mjolnr::core::error::{ProviderError, ReasonCode, ToolError};
use mjolnr::core::event::{FinishReason, MjolnrEvent, ProviderEvent};
use mjolnr::core::message::{ContentBlock, ToolCall, ToolOutcome, ToolResult};
use mjolnr::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use mjolnr::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::store::EventStore;
use mjolnr::core::tool::{Tool, ToolContext, ToolTier};
use mjolnr::runtime::Runtime;
use mjolnr::runtime::budget::BudgetLimits;
use mjolnr::store::memory::InMemoryEventStore;
use mjolnr::tools::ToolRegistry;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const PROVIDER: &str = "guard-test";
const MODEL: &str = "guard-test-1";

#[derive(Debug)]
struct OneCallProvider {
    call: ToolCall,
    observed: Arc<Mutex<Option<ToolOutcome>>>,
}

#[derive(Debug)]
struct HangingProvider;

#[async_trait]
impl Provider for HangingProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(MODEL),
            provider: self.id(),
            display_name: "Hanging test".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(1024),
            max_output_tokens: Some(1024),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        _events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        cancel.cancelled().await;
        Err(ProviderError::Cancelled)
    }
}

#[async_trait]
impl Provider for OneCallProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(MODEL),
            provider: self.id(),
            display_name: "Guard test".to_owned(),
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
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let result = request
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .find_map(|block| match block {
                ContentBlock::ToolResult { result, .. } => Some(result.outcome.clone()),
                _ => None,
            });
        if let Some(result) = result {
            *self.observed.lock().expect("observation lock") = Some(result);
            events
                .send(ProviderEvent::Finished {
                    reason: FinishReason::Stop,
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
            return Ok(ProviderCompletion {
                reason: FinishReason::Stop,
                usage: None,
            });
        }

        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        events
            .send(ProviderEvent::ToolCallStarted {
                id: self.call.id.clone(),
                name: self.call.name.clone(),
            })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        events
            .send(ProviderEvent::ToolCallCompleted {
                call: self.call.clone(),
            })
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

#[derive(Debug)]
struct CountingTool {
    name: &'static str,
    tier: ToolTier,
    executions: Arc<AtomicUsize>,
    schema_calls: Option<Arc<AtomicUsize>>,
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "test the deterministic runtime guard"
    }

    fn tier(&self) -> ToolTier {
        self.tier
    }

    fn schema(&self) -> serde_json::Value {
        let call = self
            .schema_calls
            .as_ref()
            .map_or(0, |calls| calls.fetch_add(1, Ordering::SeqCst) + 1);
        if call >= 3 {
            serde_json::json!({
                "type": "object",
                "required": ["changed_after_policy"],
                "additionalProperties": true
            })
        } else {
            serde_json::json!({
                "type": "object",
                "properties": { "ok": { "type": "string" } },
                "required": ["ok"],
                "additionalProperties": false
            })
        }
    }

    async fn preview(
        &self,
        _arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok("bounded preview".to_owned())
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::ok("executed"))
    }
}

async fn harness(
    tool: Arc<dyn Tool>,
    arguments: serde_json::Value,
) -> (
    Runtime,
    mjolnr::core::runtime::RuntimeSubscription,
    Arc<Mutex<Option<ToolOutcome>>>,
    TempDir,
) {
    harness_with_limits(tool, arguments, BudgetLimits::default()).await
}

async fn harness_with_limits(
    tool: Arc<dyn Tool>,
    arguments: serde_json::Value,
    limits: BudgetLimits,
) -> (
    Runtime,
    mjolnr::core::runtime::RuntimeSubscription,
    Arc<Mutex<Option<ToolOutcome>>>,
    TempDir,
) {
    let observed = Arc::new(Mutex::new(None));
    let provider: Arc<dyn Provider> = Arc::new(OneCallProvider {
        call: ToolCall {
            id: "call_guard".to_owned(),
            name: tool.name().to_owned(),
            arguments,
            provider_signature: None,
        },
        observed: Arc::clone(&observed),
    });
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn_with(vec![provider], store, ToolRegistry::new(vec![tool]), limits);
    let events = runtime.subscribe();
    let workspace = TempDir::new().expect("workspace");
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open workspace");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "exercise the guard".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");
    (runtime, events, observed, workspace)
}

async fn wait_for(
    events: &mut mjolnr::core::runtime::RuntimeSubscription,
    label: &str,
    mut predicate: impl FnMut(&MjolnrEvent) -> bool,
) -> MjolnrEvent {
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

#[tokio::test]
async fn schema_invalid_arguments_never_reach_the_tool() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn Tool> = Arc::new(CountingTool {
        name: "schema_guard",
        tier: ToolTier::Read,
        executions: Arc::clone(&executions),
        schema_calls: None,
    });
    let (_runtime, mut events, observed, _workspace) =
        harness(tool, serde_json::json!({ "wrong": true })).await;
    wait_for(&mut events, "terminal event", |event| {
        matches!(event, MjolnrEvent::RunFinished { .. })
    })
    .await;

    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        *observed.lock().expect("observation"),
        Some(ToolOutcome::Refused(ReasonCode::SchemaInvalid))
    );
}

#[tokio::test]
async fn arguments_are_revalidated_immediately_before_execute() {
    let executions = Arc::new(AtomicUsize::new(0));
    let schema_calls = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn Tool> = Arc::new(CountingTool {
        name: "changing_schema",
        tier: ToolTier::Read,
        executions: Arc::clone(&executions),
        schema_calls: Some(Arc::clone(&schema_calls)),
    });
    let (_runtime, mut events, observed, _workspace) =
        harness(tool, serde_json::json!({ "ok": "initially valid" })).await;
    wait_for(&mut events, "terminal event", |event| {
        matches!(event, MjolnrEvent::RunFinished { .. })
    })
    .await;

    assert!(schema_calls.load(Ordering::SeqCst) >= 3);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        *observed.lock().expect("observation"),
        Some(ToolOutcome::Refused(ReasonCode::SchemaInvalid))
    );
}

#[tokio::test]
async fn unknown_tier_fails_closed_to_execute_approval() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn Tool> = Arc::new(CountingTool {
        name: "unclassified",
        tier: ToolTier::default(),
        executions: Arc::clone(&executions),
        schema_calls: None,
    });
    let (runtime, mut events, _observed, _workspace) =
        harness(tool, serde_json::json!({ "ok": "value" })).await;
    let proposal = wait_for(&mut events, "execute approval", |event| {
        matches!(
            event,
            MjolnrEvent::ToolProposed {
                approval: Some(_),
                tier: ToolTier::Execute,
                ..
            }
        )
    })
    .await;
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    let MjolnrEvent::ToolProposed {
        approval: Some(approval),
        ..
    } = proposal
    else {
        panic!("expected proposal")
    };
    runtime
        .dispatch(MjolnrCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::Deny,
        })
        .await
        .expect("deny");
}

#[tokio::test]
async fn denied_tool_returns_a_structured_result_to_the_model() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn Tool> = Arc::new(CountingTool {
        name: "write_guard",
        tier: ToolTier::Write,
        executions: Arc::clone(&executions),
        schema_calls: None,
    });
    let (runtime, mut events, observed, _workspace) =
        harness(tool, serde_json::json!({ "ok": "value" })).await;
    let proposal = wait_for(&mut events, "write approval", |event| {
        matches!(
            event,
            MjolnrEvent::ToolProposed {
                approval: Some(_),
                ..
            }
        )
    })
    .await;
    let MjolnrEvent::ToolProposed {
        approval: Some(approval),
        ..
    } = proposal
    else {
        panic!("expected proposal")
    };
    runtime
        .dispatch(MjolnrCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::Deny,
        })
        .await
        .expect("deny");
    wait_for(&mut events, "terminal event", |event| {
        matches!(event, MjolnrEvent::RunFinished { .. })
    })
    .await;

    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        *observed.lock().expect("observation"),
        Some(ToolOutcome::Refused(ReasonCode::ApprovalDenied))
    );
}

#[tokio::test]
async fn tool_budget_exhaustion_fails_before_execute() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn Tool> = Arc::new(CountingTool {
        name: "budgeted",
        tier: ToolTier::Read,
        executions: Arc::clone(&executions),
        schema_calls: None,
    });
    let limits = BudgetLimits {
        max_tool_calls: 0,
        ..BudgetLimits::default()
    };
    let (_runtime, mut events, observed, _workspace) =
        harness_with_limits(tool, serde_json::json!({ "ok": "value" }), limits).await;
    let terminal = wait_for(&mut events, "budget exhaustion", |event| {
        matches!(event, MjolnrEvent::RunFailed { .. })
    })
    .await;

    assert!(matches!(
        terminal,
        MjolnrEvent::RunFailed {
            code: ReasonCode::BudgetExhausted,
            ..
        }
    ));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(*observed.lock().expect("observation"), None);
}

#[tokio::test]
async fn provider_turn_budget_stops_before_a_second_request() {
    let executions = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn Tool> = Arc::new(CountingTool {
        name: "turn_budgeted",
        tier: ToolTier::Read,
        executions: Arc::clone(&executions),
        schema_calls: None,
    });
    let limits = BudgetLimits {
        max_provider_turns: 1,
        ..BudgetLimits::default()
    };
    let (_runtime, mut events, observed, _workspace) =
        harness_with_limits(tool, serde_json::json!({ "ok": "value" }), limits).await;
    let terminal = wait_for(&mut events, "turn budget exhaustion", |event| {
        matches!(event, MjolnrEvent::RunFailed { .. })
    })
    .await;

    assert!(matches!(
        terminal,
        MjolnrEvent::RunFailed {
            code: ReasonCode::BudgetExhausted,
            ..
        }
    ));
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(*observed.lock().expect("observation"), None);
}

#[tokio::test]
async fn wall_time_budget_cancels_a_hanging_provider() {
    let workspace = TempDir::new().expect("workspace");
    let provider: Arc<dyn Provider> = Arc::new(HangingProvider);
    let store: Arc<dyn EventStore> = Arc::new(InMemoryEventStore::new());
    let limits = BudgetLimits {
        max_wall_time: Duration::from_millis(20),
        ..BudgetLimits::default()
    };
    let runtime = Runtime::spawn_with(vec![provider], store, ToolRegistry::new(vec![]), limits);
    let mut events = runtime.subscribe();
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("open workspace");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(PROVIDER),
            model: ModelId::new(MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(MjolnrCommand::SendUserMessage {
            text: "hang until the budget fires".to_owned(),
            source: mjolnr::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("start run");

    let terminal = wait_for(&mut events, "wall budget exhaustion", |event| {
        matches!(event, MjolnrEvent::RunFailed { .. })
    })
    .await;
    assert!(matches!(
        terminal,
        MjolnrEvent::RunFailed {
            code: ReasonCode::BudgetExhausted,
            ..
        }
    ));
}
