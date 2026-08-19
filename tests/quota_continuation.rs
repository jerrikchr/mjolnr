//! Phase 10 quota, handoff, compact-resume, and advisor acceptance tests.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use smed::core::command::SmedCommand;
use smed::core::continuation::{QuotaReserveBasis, ResumeChoice, ResumeWarning};
use smed::core::error::{ProviderError, ReasonCode};
use smed::core::event::{FinishReason, ProviderEvent, SmedEvent};
use smed::core::model::{
    ModelCapabilities, ModelDescriptor, ModelId, ProviderId, QuotaSnapshot, QuotaWindow, Usage,
};
use smed::core::policy::PolicyMode;
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::runtime::Runtime;
use smed::runtime::budget::BudgetLimits;
use smed::store::memory::InMemoryEventStore;
use smed::tools::ToolRegistry;
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
struct Reply {
    text: &'static str,
    quota: Option<f32>,
    usage: Option<Usage>,
}

impl Reply {
    const fn text(text: &'static str) -> Self {
        Self {
            text,
            quota: None,
            usage: None,
        }
    }
}

#[derive(Debug)]
struct ScriptedProvider {
    id: &'static str,
    model: &'static str,
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<ProviderRequest>>,
}

impl ScriptedProvider {
    fn new(id: &'static str, model: &'static str, replies: Vec<Reply>) -> Self {
        Self {
            id,
            model,
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }

    fn requests(&self) -> Vec<ProviderRequest> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.id)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(self.model),
            provider: self.id(),
            display_name: self.model.to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(200_000),
            max_output_tokens: Some(16_384),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        self.requests.lock().expect("requests").push(request);
        let reply = self
            .replies
            .lock()
            .expect("replies")
            .pop_front()
            .unwrap_or_else(|| Reply::text("done"));
        events
            .send(ProviderEvent::Started)
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        if let Some(used_fraction) = reply.quota {
            events
                .send(ProviderEvent::Quota {
                    snapshot: QuotaSnapshot {
                        provider: self.id(),
                        windows: vec![QuotaWindow {
                            label: "plan".to_owned(),
                            used_fraction,
                            resets_at: Some(OffsetDateTime::now_utc() + time::Duration::hours(1)),
                        }],
                    },
                })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
        }
        events
            .send(ProviderEvent::TextDelta {
                text: reply.text.to_owned(),
            })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        if let Some(usage) = reply.usage {
            events
                .send(ProviderEvent::Usage { usage })
                .await
                .map_err(|_| ProviderError::Cancelled)?;
        }
        events
            .send(ProviderEvent::Finished {
                reason: FinishReason::Stop,
            })
            .await
            .map_err(|_| ProviderError::Cancelled)?;
        Ok(ProviderCompletion {
            reason: FinishReason::Stop,
            usage: reply.usage,
        })
    }
}

async fn open(runtime: &Runtime, provider: &str, model: &str) {
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("cwd"),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        })
        .await
        .expect("create session");
}

async fn send_and_wait(runtime: &Runtime, text: &str) -> SmedEvent {
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: text.to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    terminal(&mut events).await
}

async fn terminal(events: &mut smed::core::runtime::RuntimeSubscription) -> SmedEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("event");
            if matches!(
                event,
                SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
            ) {
                return event;
            }
        }
    })
    .await
    .expect("terminal event")
}

#[tokio::test]
async fn reported_soft_threshold_drains_once_and_persists_a_handoff() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "quota",
        "q1",
        vec![
            Reply {
                text: "work complete; preparing landing",
                quota: Some(0.82),
                usage: None,
            },
            Reply::text("done: work; remaining: none; next: review; risks: none"),
        ],
    ));
    let runtime = Runtime::spawn(
        vec![provider.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    open(&runtime, "quota", "q1").await;
    let terminal = send_and_wait(&runtime, "complete the task").await;

    assert!(matches!(
        terminal,
        SmedEvent::RunFinished {
            reason: FinishReason::QuotaDrained,
            ..
        }
    ));
    assert_eq!(
        provider.request_count(),
        2,
        "one work turn plus one landing turn"
    );
    let session = runtime.snapshot().session.expect("session");
    let history = store.events(session).await.expect("history");
    assert!(
        history
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::QuotaBoundaryReached { .. }))
    );
    assert!(
        history
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::HandoffCreated { .. }))
    );
    assert!(runtime.snapshot().handoff.is_some());
    let quota = runtime
        .snapshot()
        .quota
        .expect("the full multi-window snapshot survives, not just the worst window");
    assert!(
        quota
            .windows
            .iter()
            .any(|window| window.used_fraction > 0.0),
        "observe_quota must retain the reported window, not drop it after computing quota_reserve"
    );
}

#[tokio::test]
async fn hard_threshold_stops_before_another_provider_turn() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "hard",
        "h1",
        vec![Reply {
            text: "response that revealed the window",
            quota: Some(0.97),
            usage: None,
        }],
    ));
    let runtime = Runtime::spawn(
        vec![provider.clone() as Arc<dyn Provider>],
        store as Arc<dyn EventStore>,
    );
    open(&runtime, "hard", "h1").await;
    let terminal = send_and_wait(&runtime, "work").await;
    assert!(matches!(
        terminal,
        SmedEvent::RunFailed {
            code: ReasonCode::ProviderPlanQuota,
            ..
        }
    ));
    assert_eq!(
        provider.request_count(),
        1,
        "no request may start after hard reserve"
    );
}

#[tokio::test]
async fn compact_cross_model_resume_sends_bounded_context_but_keeps_full_history() {
    let store = Arc::new(InMemoryEventStore::new());
    let first = Arc::new(ScriptedProvider::new(
        "first",
        "m1",
        (0..10)
            .map(|_| Reply::text("a deliberately verbose prior answer for durable history"))
            .collect(),
    ));
    let runtime = Runtime::spawn(
        vec![first.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    open(&runtime, "first", "m1").await;
    for index in 0..8 {
        let terminal =
            send_and_wait(&runtime, &format!("prior turn {index} with durable detail")).await;
        assert!(matches!(terminal, SmedEvent::RunFinished { .. }));
    }
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::CreateHandoff { target: None })
        .await
        .expect("handoff");
    let terminal = terminal(&mut events).await;
    assert!(matches!(
        terminal,
        SmedEvent::RunFinished {
            reason: FinishReason::Handoff,
            ..
        }
    ));
    let session = runtime.snapshot().session.expect("session");
    let full_before = runtime.snapshot().messages.len();
    runtime.close().await.expect("close first runtime");

    let second = Arc::new(ScriptedProvider::new(
        "second",
        "m2",
        vec![Reply::text("continued")],
    ));
    let resumed = Runtime::spawn(
        vec![
            Arc::new(ScriptedProvider::new("first", "m1", vec![])) as Arc<dyn Provider>,
            second.clone() as Arc<dyn Provider>,
        ],
        store.clone() as Arc<dyn EventStore>,
    );
    resumed
        .dispatch(SmedCommand::ResumeCompact {
            session,
            provider: Some(ProviderId::new("second")),
            model: Some(ModelId::new("m2")),
        })
        .await
        .expect("compact resume");
    let terminal = send_and_wait(&resumed, "continue on the second model").await;
    assert!(matches!(terminal, SmedEvent::RunFinished { .. }));
    let requests = second.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].messages.len() <= 8,
        "compact request was not bounded"
    );
    assert!(
        requests[0].messages[0]
            .text()
            .contains("SMED COMPACT RESUME")
    );
    assert!(
        resumed.snapshot().messages.len() > full_before,
        "full durable transcript was replaced"
    );
    assert!(store.events(session).await.expect("events").len() > full_before);
}

#[tokio::test]
async fn configured_budget_is_labelled_and_resume_advisor_makes_zero_requests() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "configured",
        "c1",
        vec![
            Reply {
                text: "initial",
                quota: None,
                usage: Some(Usage {
                    input_tokens: 81,
                    output_tokens: 0,
                }),
            },
            Reply::text("manual handoff status"),
            Reply::text("configured reserve landing"),
        ],
    ));
    let limits = BudgetLimits {
        quota_token_budget: Some(100),
        ..BudgetLimits::default()
    };
    let runtime = Runtime::spawn_with(
        vec![provider.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        ToolRegistry::new(vec![]),
        limits,
    );
    open(&runtime, "configured", "c1").await;
    assert!(matches!(
        send_and_wait(&runtime, "first").await,
        SmedEvent::RunFinished { .. }
    ));
    let mut handoff_events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::CreateHandoff { target: None })
        .await
        .expect("handoff");
    assert!(matches!(
        terminal(&mut handoff_events).await,
        SmedEvent::RunFinished { .. }
    ));
    let terminal_event = send_and_wait(&runtime, "second").await;
    assert!(matches!(
        terminal_event,
        SmedEvent::RunFinished {
            reason: FinishReason::QuotaDrained,
            ..
        }
    ));
    assert!(matches!(
        runtime.snapshot().quota_reserve.basis,
        QuotaReserveBasis::ConfiguredTokens { limit: 100 }
    ));

    let session = runtime.snapshot().session.expect("session");
    runtime.close().await.expect("close");
    let hard = Arc::new(ScriptedProvider::new("configured", "c1", vec![]));
    let resumed = Runtime::spawn(
        vec![hard.clone() as Arc<dyn Provider>],
        store as Arc<dyn EventStore>,
    );
    let mut snapshots = resumed.snapshots();
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let advised = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("snapshot");
            if snapshot.resume_advice.is_some() {
                return snapshot;
            }
        }
    })
    .await
    .expect("advisor");
    assert!(matches!(
        advised.resume_advice.expect("advice").warning,
        ResumeWarning::QuotaStopped { resets_at: None }
    ));
    assert_eq!(hard.request_count(), 0);

    resumed
        .dispatch(SmedCommand::ResolveResume {
            choice: ResumeChoice::Full,
        })
        .await
        .expect("stale choice");
    assert_eq!(hard.request_count(), 0);
}

#[tokio::test]
async fn a_fresh_recent_session_bypasses_the_resume_advisor() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "fresh",
        "f1",
        vec![Reply::text("recent answer")],
    ));
    let runtime = Runtime::spawn(
        vec![provider as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    open(&runtime, "fresh", "f1").await;
    assert!(matches!(
        send_and_wait(&runtime, "recent work").await,
        SmedEvent::RunFinished { .. }
    ));
    let session = runtime.snapshot().session.expect("session");
    runtime.close().await.expect("close");

    let resumed_provider = Arc::new(ScriptedProvider::new("fresh", "f1", vec![]));
    let resumed = Runtime::spawn(
        vec![resumed_provider.clone() as Arc<dyn Provider>],
        store as Arc<dyn EventStore>,
    );
    let mut snapshots = resumed.snapshots();
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let snapshot = tokio::time::timeout(Duration::from_secs(5), snapshots.changed())
        .await
        .expect("resume snapshot")
        .expect("snapshot");
    assert!(snapshot.resume_advice.is_none());
    assert_eq!(resumed_provider.request_count(), 0);
}

async fn seed_quota_stopped_session(
    store: &Arc<InMemoryEventStore>,
) -> smed::core::event::SessionId {
    let provider = Arc::new(ScriptedProvider::new(
        "advised",
        "a1",
        vec![
            Reply::text("done: baseline; remaining: work; next: continue; risks: quota"),
            Reply {
                text: "the response that exposed hard quota",
                quota: Some(0.96),
                usage: None,
            },
        ],
    ));
    let runtime = Runtime::spawn(
        vec![provider as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    open(&runtime, "advised", "a1").await;
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::CreateHandoff { target: None })
        .await
        .expect("handoff");
    assert!(matches!(
        terminal(&mut events).await,
        SmedEvent::RunFinished {
            reason: FinishReason::Handoff,
            ..
        }
    ));
    assert!(matches!(
        send_and_wait(&runtime, "continue").await,
        SmedEvent::RunFailed {
            code: ReasonCode::ProviderPlanQuota,
            ..
        }
    ));
    let session = runtime.snapshot().session.expect("session");
    runtime.close().await.expect("close");
    session
}

#[tokio::test]
async fn quota_stopped_resume_is_advised_before_any_request_and_compact_is_explicit() {
    let store = Arc::new(InMemoryEventStore::new());
    let session = seed_quota_stopped_session(&store).await;

    let resumed_provider = Arc::new(ScriptedProvider::new(
        "advised",
        "a1",
        vec![Reply::text("compact continuation")],
    ));
    let resumed = Runtime::spawn(
        vec![resumed_provider.clone() as Arc<dyn Provider>],
        store as Arc<dyn EventStore>,
    );
    let mut snapshots = resumed.snapshots();
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let advised = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("snapshot");
            if snapshot.resume_advice.is_some() {
                return snapshot;
            }
        }
    })
    .await
    .expect("advisor snapshot");
    let advice = advised.resume_advice.expect("quota advice");
    assert!(matches!(
        advice.warning,
        ResumeWarning::QuotaStopped { resets_at: Some(_) }
    ));
    assert!(advice.handoff.is_some());
    assert_eq!(
        resumed_provider.request_count(),
        0,
        "advisor must be store-only"
    );

    let mut snapshots = resumed.snapshots();
    resumed
        .dispatch(SmedCommand::ResolveResume {
            choice: ResumeChoice::Compact,
        })
        .await
        .expect("choose compact");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if snapshots
                .changed()
                .await
                .expect("snapshot")
                .resume_advice
                .is_none()
            {
                break;
            }
        }
    })
    .await
    .expect("compact choice snapshot");
    assert!(matches!(
        send_and_wait(&resumed, "continue compactly").await,
        SmedEvent::RunFinished { .. }
    ));
    assert_eq!(resumed_provider.request_count(), 1);
    assert!(
        resumed_provider.requests()[0].messages[0]
            .text()
            .contains("SMED COMPACT RESUME")
    );
}

#[tokio::test]
async fn live_handoff_swaps_to_target_at_landing_records_model_changed_and_bounds_next_turn() {
    let store = Arc::new(InMemoryEventStore::new());
    let first = Arc::new(ScriptedProvider::new(
        "first",
        "m1",
        (0..12)
            .map(|_| Reply::text("a verbose prior answer kept in durable history"))
            .collect(),
    ));
    let second = Arc::new(ScriptedProvider::new(
        "second",
        "m2",
        vec![Reply::text("continued on second")],
    ));
    let runtime = Runtime::spawn(
        vec![
            first.clone() as Arc<dyn Provider>,
            second.clone() as Arc<dyn Provider>,
        ],
        store.clone() as Arc<dyn EventStore>,
    );
    open(&runtime, "first", "m1").await;
    for index in 0..6 {
        assert!(matches!(
            send_and_wait(&runtime, &format!("prior turn {index} with detail")).await,
            SmedEvent::RunFinished { .. }
        ));
    }

    // Live handoff to a different model. The landing turn runs on the *current*
    // model; the swap is applied only when that run lands — never mid-turn.
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::CreateHandoff {
            target: Some("second/m2".to_owned()),
        })
        .await
        .expect("handoff");
    assert!(matches!(
        terminal(&mut events).await,
        SmedEvent::RunFinished {
            reason: FinishReason::Handoff,
            ..
        }
    ));

    // The landing ran on the original model; the target was not touched until
    // after the landing — the proof that no swap happened mid-turn.
    assert_eq!(
        first.request_count(),
        7,
        "six work turns plus one landing turn, all on the original model"
    );
    assert_eq!(
        second.request_count(),
        0,
        "the target model is not called during the landing"
    );

    let snap = runtime.snapshot();
    assert_eq!(snap.provider.expect("provider").as_str(), "second");
    assert_eq!(snap.model.expect("model").as_str(), "m2");

    // The swap is evidenced, so recovery replay and `/model` can see it.
    let session = snap.session.expect("session");
    let history = store.events(session).await.expect("history");
    assert!(
        history.iter().any(|stored| matches!(&stored.event,
            SmedEvent::ModelChanged { provider, model, .. }
            if provider.as_str() == "second" && model.as_str() == "m2")),
        "a live handoff swap must persist a ModelChanged event"
    );

    // The target model's first turn sends the bounded, compacted context.
    assert!(matches!(
        send_and_wait(&runtime, "continue").await,
        SmedEvent::RunFinished { .. }
    ));
    let requests = second.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].messages.len() <= 8,
        "the handoff did not bound the target's first turn"
    );
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| message.text().contains("SMED COMPACT RESUME"))
    );
}

#[tokio::test]
async fn live_handoff_to_same_model_resets_full_auto_and_shrinks_context() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "solo",
        "s1",
        (0..10)
            .map(|_| Reply::text("a verbose durable answer that bloats the window"))
            .collect(),
    ));
    let runtime = Runtime::spawn(
        vec![provider.clone() as Arc<dyn Provider>],
        store as Arc<dyn EventStore>,
    );
    open(&runtime, "solo", "s1").await;
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("set policy");
    for index in 0..5 {
        assert!(matches!(
            send_and_wait(&runtime, &format!("turn {index} with durable detail")).await,
            SmedEvent::RunFinished { .. }
        ));
        assert_eq!(
            runtime.snapshot().policy,
            PolicyMode::FullAuto,
            "full-auto persists across ordinary turns"
        );
    }

    // Handing to the current model is a deliberate window reset, not a no-op.
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::CreateHandoff {
            target: Some("s1".to_owned()),
        })
        .await
        .expect("handoff");
    assert!(matches!(
        terminal(&mut events).await,
        SmedEvent::RunFinished {
            reason: FinishReason::Handoff,
            ..
        }
    ));

    // Full-auto never survives a handoff, exactly as it does not survive resume.
    assert_eq!(runtime.snapshot().policy, PolicyMode::Ask);
    assert_eq!(runtime.snapshot().model.expect("model").as_str(), "s1");

    // The next turn on the same model is shrunk to the checkpoint.
    assert!(matches!(
        send_and_wait(&runtime, "continue on the same model").await,
        SmedEvent::RunFinished { .. }
    ));
    let requests = provider.requests();
    assert!(
        requests.last().expect("a request").messages.len() <= 8,
        "same-model handoff did not shrink the sent context"
    );
}
