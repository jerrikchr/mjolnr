//! Phase 15 deterministic model/provider routing acceptance tests.
//!
//! Mirrors `tests/quota_continuation.rs`'s scripted-provider pattern: no
//! network, no real credentials, deterministic timing.

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
use smed::core::continuation::QuotaReserveStatus;
use smed::core::error::{ProviderError, ReasonCode};
use smed::core::event::{FinishReason, ProviderEvent, SmedEvent};
use smed::core::model::{
    ModelCapabilities, ModelDescriptor, ModelId, ProviderId, QuotaSnapshot, QuotaWindow,
};
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::routing::{
    BreakerConfig, BreakerState, RouteAdvanceCondition, RouteDefinition, RouteHop,
    RouteSelectionReason, RouteTable,
};
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
enum Step {
    Text {
        text: &'static str,
        quota: Option<f32>,
    },
    Fail,
}

#[derive(Debug)]
struct ScriptedProvider {
    id: &'static str,
    model: &'static str,
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<ProviderRequest>>,
}

impl ScriptedProvider {
    fn new(id: &'static str, model: &'static str, steps: Vec<Step>) -> Self {
        Self {
            id,
            model,
            steps: Mutex::new(steps.into()),
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
        let step = self
            .steps
            .lock()
            .expect("steps")
            .pop_front()
            .unwrap_or(Step::Text {
                text: "done",
                quota: None,
            });
        match step {
            Step::Fail => Err(ProviderError::Protocol {
                detail: "scripted failure".to_owned(),
            }),
            Step::Text { text, quota } => {
                events
                    .send(ProviderEvent::Started)
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
                if let Some(used_fraction) = quota {
                    events
                        .send(ProviderEvent::Quota {
                            snapshot: QuotaSnapshot {
                                provider: self.id(),
                                windows: vec![QuotaWindow {
                                    label: "plan".to_owned(),
                                    used_fraction,
                                    resets_at: Some(
                                        time::OffsetDateTime::now_utc() + time::Duration::hours(1),
                                    ),
                                }],
                            },
                        })
                        .await
                        .map_err(|_| ProviderError::Cancelled)?;
                }
                events
                    .send(ProviderEvent::TextDelta {
                        text: text.to_owned(),
                    })
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
                events
                    .send(ProviderEvent::Finished {
                        reason: FinishReason::Stop,
                    })
                    .await
                    .map_err(|_| ProviderError::Cancelled)?;
                Ok(ProviderCompletion {
                    reason: FinishReason::Stop,
                    usage: None,
                })
            }
        }
    }
}

fn route_table(hops: &[(&str, &str)]) -> RouteTable {
    let mut table = RouteTable::default();
    table.routes.insert(
        "main".to_owned(),
        RouteDefinition {
            name: "main".to_owned(),
            roles: Vec::new(),
            persona: None,
            hops: hops
                .iter()
                .map(|(provider, model)| RouteHop {
                    provider: ProviderId::new(*provider),
                    model: ModelId::new(*model),
                })
                .collect(),
        },
    );
    table
        .task_classes
        .insert("default".to_owned(), "main".to_owned());
    table
}

async fn open_and_attach(runtime: &Runtime, provider: &str, model: &str) {
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
    wait_for(runtime, |snapshot| snapshot.session.is_some()).await;
    runtime
        .dispatch(SmedCommand::AttachRoute {
            route: None,
            role: None,
            task_class: "default".to_owned(),
        })
        .await
        .expect("attach route");
    wait_for(runtime, |snapshot| snapshot.route.is_some()).await;
}

async fn wait_for(
    runtime: &Runtime,
    mut predicate: impl FnMut(&smed::core::runtime::RuntimeSnapshot) -> bool,
) {
    if predicate(&runtime.snapshot()) {
        return;
    }
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("snapshot");
            if predicate(&snapshot) {
                return;
            }
        }
    })
    .await
    .expect("condition reached");
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

/// Checklist: "A mocked provider whose window exhausts mid-session advances
/// the route at the reserve threshold; the event log names the rule; the
/// continuation obeys Phase 10 compaction rules."
#[tokio::test]
async fn quota_reserve_breach_advances_the_route_and_bounds_the_next_request() {
    let store = Arc::new(InMemoryEventStore::new());
    let first = Arc::new(ScriptedProvider::new(
        "primary",
        "p1",
        vec![Step::Text {
            text: "the response that reveals the hard reserve",
            quota: Some(0.97),
        }],
    ));
    let second = Arc::new(ScriptedProvider::new(
        "secondary",
        "s1",
        vec![Step::Text {
            text: "continued on the fallback hop",
            quota: None,
        }],
    ));
    let runtime = Runtime::spawn_with_routes(
        vec![
            first.clone() as Arc<dyn Provider>,
            second.clone() as Arc<dyn Provider>,
        ],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(route_table(&[("primary", "p1"), ("secondary", "s1")])),
    );
    open_and_attach(&runtime, "primary", "p1").await;

    let terminal = send_and_wait(&runtime, "do the work").await;
    assert!(
        matches!(terminal, SmedEvent::RunFinished { .. }),
        "the route advanced and the run finished on the fallback hop, it did not stop: {terminal:?}"
    );

    assert_eq!(
        first.request_count(),
        1,
        "the exhausted hop is asked exactly once"
    );
    assert_eq!(
        second.request_count(),
        1,
        "the fallback hop is asked exactly once"
    );

    let session = runtime.snapshot().session.expect("session");
    let history = store.events(session).await.expect("history");
    let advanced = history.iter().find_map(|stored| match &stored.event {
        SmedEvent::RouteAdvanced {
            route,
            from_position,
            to_position,
            condition,
            ..
        } => Some((route.clone(), *from_position, *to_position, *condition)),
        _ => None,
    });
    let (route, from_position, to_position, condition) =
        advanced.expect("a RouteAdvanced event names the rule that fired");
    assert_eq!(route, "main");
    assert_eq!(from_position, 0);
    assert_eq!(to_position, 1);
    assert_eq!(condition, RouteAdvanceCondition::QuotaReserveBreached);
    assert!(
        history
            .iter()
            .any(|stored| matches!(stored.event, SmedEvent::HandoffCreated { .. })),
        "the Phase 10 handoff artifact still lands before crossing providers"
    );

    // Phase 10 compaction rule: the fallback hop's request is bounded, not the
    // full transcript replayed onto the new provider.
    let request = &second.requests()[0];
    assert!(
        request.messages.len() <= 2,
        "the request onto the new hop was not bounded"
    );
    assert!(request.messages[0].text().contains("SMED COMPACT RESUME"));

    assert_eq!(
        runtime.snapshot().route.map(|route| route.position),
        Some(1),
        "the session's route position reflects the advance"
    );
}

/// Checklist: "A provider failing repeatedly opens its breaker; `HalfOpen`
/// probes on the recovery timeout; all three states are visible and
/// evidenced." Also covers: "A route with no viable position left yields a
/// typed stop, not a silent retry loop."
#[tokio::test]
async fn a_failing_provider_opens_its_breaker_then_half_opens_and_closes() {
    let store = Arc::new(InMemoryEventStore::new());
    let flaky = Arc::new(ScriptedProvider::new(
        "flaky",
        "f1",
        vec![
            Step::Fail,
            Step::Text {
                text: "recovered",
                quota: None,
            },
        ],
    ));
    let mut table = RouteTable::default();
    table.routes.insert(
        "solo".to_owned(),
        RouteDefinition {
            name: "solo".to_owned(),
            roles: Vec::new(),
            persona: None,
            hops: vec![RouteHop {
                provider: ProviderId::new("flaky"),
                model: ModelId::new("f1"),
            }],
        },
    );
    table
        .task_classes
        .insert("default".to_owned(), "solo".to_owned());
    table.breakers.insert(
        "flaky".to_owned(),
        BreakerConfig {
            failure_threshold: 1,
            recovery_timeout: Duration::ZERO,
        },
    );

    let runtime = Runtime::spawn_with_routes(
        vec![flaky.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(table),
    );
    open_and_attach(&runtime, "flaky", "f1").await;

    // First turn: the provider fails, the breaker trips open on the first
    // failure (threshold 1), and the single-hop route has nowhere to advance
    // to — a typed stop, never a silent retry.
    let first_terminal = send_and_wait(&runtime, "trip it").await;
    assert!(matches!(
        first_terminal,
        SmedEvent::RunFailed {
            code: ReasonCode::RouteExhausted,
            ..
        }
    ));

    // Second turn: `recovery_timeout` is zero, so the breaker gate half-opens
    // on the very next turn's poll and permits exactly one probe, which
    // succeeds and closes the breaker.
    let second_terminal = send_and_wait(&runtime, "probe it").await;
    assert!(matches!(second_terminal, SmedEvent::RunFinished { .. }));
    assert_eq!(flaky.request_count(), 2);

    let session = runtime.snapshot().session.expect("session");
    let history = store.events(session).await.expect("history");
    let transitions: Vec<(BreakerState, BreakerState)> = history
        .iter()
        .filter_map(|stored| match &stored.event {
            SmedEvent::BreakerStateChanged { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert!(
        transitions.contains(&(BreakerState::Closed, BreakerState::Open)),
        "opening transition missing: {transitions:?}"
    );
    assert!(
        transitions.contains(&(BreakerState::Open, BreakerState::HalfOpen)),
        "half-open transition missing: {transitions:?}"
    );
    assert!(
        transitions.contains(&(BreakerState::HalfOpen, BreakerState::Closed)),
        "closing transition missing: {transitions:?}"
    );

    let exhausted = history.iter().any(|stored| {
        matches!(
            &stored.event,
            SmedEvent::RouteExhausted {
                condition: RouteAdvanceCondition::ProviderFailure(_),
                ..
            }
        )
    });
    assert!(
        exhausted,
        "the typed stop names the condition that exhausted the route"
    );
}

/// Checklist: "A child spawn with no named route uses the configured child
/// default; the decision event says so."
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one linear spawn/approve/settle sequence, matching tests/integration_subagents.rs's own shape"
)]
async fn a_child_spawn_with_no_named_route_uses_the_configured_default() {
    let repository = git_repository();
    let store = Arc::new(InMemoryEventStore::new());
    let fake = Arc::new(FakeProvider::new(FakeScript::Subagent));
    let mut table = RouteTable::default();
    table.routes.insert(
        "cheap".to_owned(),
        RouteDefinition {
            name: "cheap".to_owned(),
            roles: Vec::new(),
            persona: None,
            hops: vec![RouteHop {
                provider: ProviderId::new(FakeProvider::ID),
                model: ModelId::new(FakeProvider::MODEL),
            }],
        },
    );
    table.child_default = Some("cheap".to_owned());

    let runtime = Runtime::spawn_with_routes(
        vec![fake.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(table),
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: repository.path().to_path_buf(),
        })
        .await
        .expect("open");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: smed::core::policy::PolicyMode::WorkspaceWrite,
        })
        .await
        .expect("set policy");
    wait_for(&runtime, |snapshot| {
        snapshot.session.is_some()
            && snapshot.policy == smed::core::policy::PolicyMode::WorkspaceWrite
    })
    .await;

    // No route attach here on purpose: the spec's default applies to the
    // *child*, independent of whether the parent itself attached a route.
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "spawn-two: delegate the work".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send directive");
    let approval = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            if let SmedEvent::ToolProposed {
                approval: Some(approval),
                call,
                ..
            } = &event
                && call.name == "spawn_subagent"
            {
                return *approval;
            }
        }
    })
    .await
    .expect("spawn proposal arrives");
    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval,
            decision: smed::core::command::ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve spawn");

    let terminal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            if matches!(
                event,
                SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
            ) {
                return event;
            }
        }
    })
    .await
    .expect("parent run settles");
    assert!(matches!(terminal, SmedEvent::RunFinished { .. }));

    let session = runtime.snapshot().session.expect("session");
    let history = store.events(session).await.expect("history");
    let selections: Vec<_> = history
        .iter()
        .filter_map(|stored| match &stored.event {
            SmedEvent::RouteSelected {
                child: Some(child),
                route,
                reason,
                ..
            } => Some((*child, route.clone(), reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        selections.len(),
        2,
        "both children resolved the child default"
    );
    for (_, route, reason) in &selections {
        assert_eq!(route, "cheap");
        assert_eq!(*reason, RouteSelectionReason::ChildDefault);
    }
}

/// Checklist: "Removing routing config entirely restores present-day
/// behaviour: the configured provider, no chains, no breaker — stated, not
/// guessed."
#[tokio::test]
async fn no_routing_config_restores_present_day_behaviour_exactly() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "plain",
        "m1",
        vec![Step::Text {
            text: "answered without any routing config",
            quota: None,
        }],
    ));
    // `Runtime::spawn` — no route table constructor at all, exactly what every
    // pre-Phase-15 caller still uses.
    let runtime = Runtime::spawn(
        vec![provider.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("cwd"),
        })
        .await
        .expect("open");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new("plain"),
            model: ModelId::new("m1"),
        })
        .await
        .expect("create session");
    wait_for(&runtime, |snapshot| snapshot.session.is_some()).await;

    let terminal = send_and_wait(&runtime, "just answer").await;
    assert!(matches!(terminal, SmedEvent::RunFinished { .. }));

    let snapshot = runtime.snapshot();
    assert!(
        snapshot.route.is_none(),
        "no chain: nothing attached a route"
    );
    assert!(
        snapshot.breakers.is_empty(),
        "no breaker state exists without a route"
    );
    assert_eq!(snapshot.provider, Some(ProviderId::new("plain")));
    assert_eq!(snapshot.model, Some(ModelId::new("m1")));

    let session = snapshot.session.expect("session");
    let history = store.events(session).await.expect("history");
    assert!(
        !history.iter().any(|stored| matches!(
            stored.event,
            SmedEvent::RouteSelected { .. }
                | SmedEvent::RouteAdvanced { .. }
                | SmedEvent::RouteExhausted { .. }
                | SmedEvent::BreakerStateChanged { .. }
        )),
        "no routing event of any kind is recorded when no routing config exists"
    );
}

///  checklist: "A role resolves to the same route position
/// Phase 15 would pick directly; removing a role mapping falls back to
/// whatever route the tool/spawn names literally, stated, not guessed."
#[tokio::test]
async fn a_role_lands_on_the_same_hop_a_named_route_would_and_says_it_was_a_role() {
    let store = Arc::new(InMemoryEventStore::new());
    let cheap = Arc::new(ScriptedProvider::new(
        "cheap-provider",
        "c1",
        vec![Step::Text {
            text: "answered on the smol route",
            quota: None,
        }],
    ));

    // One route tagged `smol`, reached only through the role — the session is
    // opened on a different provider entirely, so landing on `cheap-provider`
    // can only be the role's doing.
    let mut table = RouteTable::default();
    table.routes.insert(
        "cheap".to_owned(),
        RouteDefinition {
            name: "cheap".to_owned(),
            roles: vec!["smol".to_owned()],
            persona: None,
            hops: vec![RouteHop {
                provider: ProviderId::new("cheap-provider"),
                model: ModelId::new("c1"),
            }],
        },
    );
    let refused = table.reindex_roles();
    assert!(refused.is_empty(), "the fixture's role must index cleanly");

    let runtime = Runtime::spawn_with_routes(
        vec![cheap.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(table),
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("cwd"),
        })
        .await
        .expect("open");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new("cheap-provider"),
            model: ModelId::new("c1"),
        })
        .await
        .expect("create session");
    wait_for(&runtime, |snapshot| snapshot.session.is_some()).await;

    runtime
        .dispatch(SmedCommand::AttachRoute {
            route: None,
            role: Some("smol".to_owned()),
            task_class: "default".to_owned(),
        })
        .await
        .expect("attach by role");
    wait_for(&runtime, |snapshot| snapshot.route.is_some()).await;

    let snapshot = runtime.snapshot();
    let route = snapshot.route.expect("route attached");
    assert_eq!(
        route.route, "cheap",
        "the role resolved to its tagged route"
    );
    assert_eq!(route.position, 0, "a role lands on hop zero like any name");
    assert_eq!(snapshot.provider, Some(ProviderId::new("cheap-provider")));

    let session = snapshot.session.expect("session");
    let history = store.events(session).await.expect("history");
    let reason = history
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::RouteSelected { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("a RouteSelected event was recorded");
    assert_eq!(
        reason,
        smed::core::routing::RouteSelectionReason::Role("smol".to_owned()),
        "the evidence names the role, not just the route it landed on"
    );
}

/// The other half of the same checklist line: an unmapped role does not
/// silently vanish, it falls back to the literal name and records that it did.
#[tokio::test]
async fn an_unmapped_role_falls_back_to_the_named_route_and_records_the_fallback() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider = Arc::new(ScriptedProvider::new(
        "plain",
        "m1",
        vec![Step::Text {
            text: "answered on the literal route",
            quota: None,
        }],
    ));

    // `main` carries no role tags at all, so requesting `smol` finds nothing.
    let runtime = Runtime::spawn_with_routes(
        vec![provider.clone() as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
        Arc::new(route_table(&[("plain", "m1")])),
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("cwd"),
        })
        .await
        .expect("open");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new("plain"),
            model: ModelId::new("m1"),
        })
        .await
        .expect("create session");
    wait_for(&runtime, |snapshot| snapshot.session.is_some()).await;

    runtime
        .dispatch(SmedCommand::AttachRoute {
            route: Some("main".to_owned()),
            role: Some("smol".to_owned()),
            task_class: "default".to_owned(),
        })
        .await
        .expect("attach with an unmapped role");
    wait_for(&runtime, |snapshot| snapshot.route.is_some()).await;

    let snapshot = runtime.snapshot();
    assert_eq!(
        snapshot.route.expect("route").route,
        "main",
        "the literal name applied when the role mapped to nothing"
    );

    let session = snapshot.session.expect("session");
    let history = store.events(session).await.expect("history");
    let reason = history
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::RouteSelected { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("a RouteSelected event was recorded");
    assert_eq!(
        reason,
        smed::core::routing::RouteSelectionReason::NamedAfterUnmappedRole("smol".to_owned()),
        "the fallback is stated in the evidence rather than left to be guessed"
    );
}

/// Anti-pattern guard: "A routing advance must never widen policy, budgets,
/// or the approval tier of anything in flight."
#[tokio::test]
async fn a_route_advance_never_widens_policy_or_budgets() {
    let store = Arc::new(InMemoryEventStore::new());
    let first = Arc::new(ScriptedProvider::new("narrow", "n1", vec![Step::Fail]));
    let second = Arc::new(ScriptedProvider::new(
        "wide",
        "w1",
        vec![Step::Text {
            text: "landed on the fallback",
            quota: None,
        }],
    ));
    let runtime = Runtime::spawn_with_routes(
        vec![
            first.clone() as Arc<dyn Provider>,
            second.clone() as Arc<dyn Provider>,
        ],
        store as Arc<dyn EventStore>,
        Arc::new(route_table(&[("narrow", "n1"), ("wide", "w1")])),
    );
    open_and_attach(&runtime, "narrow", "n1").await;
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: smed::core::policy::PolicyMode::ReadOnly,
        })
        .await
        .expect("set policy");
    wait_for(&runtime, |snapshot| {
        snapshot.policy == smed::core::policy::PolicyMode::ReadOnly
    })
    .await;

    let before = runtime.snapshot();

    let terminal = send_and_wait(&runtime, "advance past the failure").await;
    assert!(matches!(terminal, SmedEvent::RunFinished { .. }));

    let after = runtime.snapshot();
    assert_eq!(
        before.policy, after.policy,
        "advancing a route must not change the session's policy tier"
    );
    assert_eq!(
        before.budget.max_provider_turns, after.budget.max_provider_turns,
        "advancing a route must not widen the provider-turn budget"
    );
    assert_eq!(
        before.budget.max_tool_calls, after.budget.max_tool_calls,
        "advancing a route must not widen the tool-call budget"
    );
    assert_eq!(after.provider, Some(ProviderId::new("wide")));
}

/// `QuotaReserveStatus` resets across a route advance rather than a stale
/// reserve from the old provider governing the new one (Phase 10 rule: a
/// provider's window cannot govern another provider).
#[tokio::test]
async fn a_route_advance_resets_the_quota_reserve_for_the_new_provider() {
    let store = Arc::new(InMemoryEventStore::new());
    let first = Arc::new(ScriptedProvider::new("a", "a1", vec![Step::Fail]));
    let second = Arc::new(ScriptedProvider::new(
        "b",
        "b1",
        vec![Step::Text {
            text: "on the new provider",
            quota: None,
        }],
    ));
    let runtime = Runtime::spawn_with_routes(
        vec![
            first.clone() as Arc<dyn Provider>,
            second.clone() as Arc<dyn Provider>,
        ],
        store as Arc<dyn EventStore>,
        Arc::new(route_table(&[("a", "a1"), ("b", "b1")])),
    );
    open_and_attach(&runtime, "a", "a1").await;
    assert!(matches!(
        terminal_after_send(&runtime).await,
        SmedEvent::RunFinished { .. }
    ));
    assert_eq!(
        runtime.snapshot().quota_reserve,
        QuotaReserveStatus::default()
    );
}

async fn terminal_after_send(runtime: &Runtime) -> SmedEvent {
    send_and_wait(runtime, "go").await
}

/// A disposable git repository, exactly as `tests/integration_subagents.rs`
/// builds one: `spawn_subagent` requires a real worktree, not a temp
/// directory that happens to exist.
fn git_repository() -> tempfile::TempDir {
    let repository = tempfile::tempdir().expect("temporary repository");
    std::fs::write(repository.path().join("README.md"), "base\n").expect("seed file");
    git(repository.path(), &["init", "-q"]);
    git(repository.path(), &["config", "user.name", "smed Test"]);
    git(
        repository.path(),
        &["config", "user.email", "smed-test@localhost"],
    );
    git(repository.path(), &["add", "README.md"]);
    git(repository.path(), &["commit", "-q", "-m", "seed"]);
    repository
}

fn git(root: &std::path::Path, arguments: &[&str]) {
    let status = std::process::Command::new("git")
        .args(arguments)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {arguments:?} failed");
}
