//! Headless runtime tests.
//!
//! **This file must never import `smed::tui`.** That is the point of it: if the
//! core cannot be exercised without a terminal, the boundary is broken.
//! `tests/architecture.rs` enforces the rule for `src/`; this file demonstrates
//! it for real.
//!
//! No network, no credentials, no sleeps-as-synchronisation.

// AGENTS.md §7: tests may panic freely — clarity beats ceremony, and a
// panicking assertion is a failing test rather than a corrupted terminal.
// `clippy.toml`'s `allow-*-in-tests` options only cover `#[test]` functions and
// `cfg(test)` modules; an integration test is a separate crate, and its *helper*
// functions are neither. So the allowance is stated per file.
#![allow(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::command::SmedCommand;
use smed::core::event::{FinishReason, SmedEvent};
use smed::core::message::Role;
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;

/// Wait for an event matching a predicate, or fail. Never sleeps: it awaits the
/// real feed under a timeout, so it cannot pass by luck on a fast machine.
async fn wait_for(
    events: &mut smed::core::runtime::RuntimeSubscription,
    label: &str,
    mut predicate: impl FnMut(&SmedEvent) -> bool,
) -> SmedEvent {
    let deadline = Duration::from_secs(5);
    let found = tokio::time::timeout(deadline, async {
        loop {
            match events.recv().await {
                Ok(event) if predicate(&event) => return event,
                Ok(_) => {}
                Err(error) => panic!("event feed ended while waiting for {label}: {error}"),
            }
        }
    })
    .await;

    found.unwrap_or_else(|_| panic!("timed out waiting for {label}"))
}

fn harness(script: FakeScript) -> (Runtime, Arc<InMemoryEventStore>) {
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(script));
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);
    (runtime, store)
}

/// Open a project, then a session.
///
/// The order is required, not incidental: every session references a project
/// , and `create_session` refuses without one rather than letting a
/// foreign key refuse later with a less useful message.
async fn open_session(runtime: &Runtime) {
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: std::env::current_dir().expect("current dir"),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
}

#[tokio::test]
async fn text_streams_incrementally_and_coalesces_into_one_message() {
    let (runtime, store) = harness(FakeScript::Text);
    let mut events = runtime.subscribe();
    open_session(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let mut deltas = Vec::new();
    let deadline = Duration::from_secs(5);
    tokio::time::timeout(deadline, async {
        loop {
            match events.recv().await.expect("feed") {
                SmedEvent::TextDelta { text, .. } => deltas.push(text),
                SmedEvent::RunFinished { reason, .. } => {
                    assert_eq!(reason, FinishReason::Stop);
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("run finishes");

    // Incremental: more than one delta, and split mid-word.
    assert!(
        deltas.len() > 1,
        "text must stream incrementally, got {} delta(s)",
        deltas.len()
    );
    assert!(deltas.concat().contains("incrementally"));

    let snapshot = runtime.snapshot();
    let assistant: Vec<_> = snapshot
        .messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect();

    assert_eq!(assistant.len(), 1);
    assert_eq!(
        assistant[0].blocks.len(),
        1,
        "fragments must coalesce into one block, not one block per delta"
    );

    // : text deltas must never reach the store.
    let stored = store
        .events(snapshot.session.expect("session"))
        .await
        .expect("events");
    assert!(
        !stored
            .iter()
            .any(|event| matches!(event.event, SmedEvent::TextDelta { .. })),
        "ephemeral deltas must not be persisted — one row per token is forbidden"
    );
}

#[tokio::test]
async fn tool_arguments_are_parsed_only_at_the_completion_boundary() {
    let (runtime, _store) = harness(FakeScript::TextThenToolCall);
    let mut events = runtime.subscribe();
    open_session(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "read a file".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let terminal = wait_for(&mut events, "run finished", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    // Phase 3 consumes the proposal, returns its schema refusal to the fake,
    // and lets the next provider turn stop normally.
    match terminal {
        SmedEvent::RunFinished { reason, .. } => assert_eq!(reason, FinishReason::Stop),
        other => panic!("expected a finished run, got {other:?}"),
    }

    let snapshot = runtime.snapshot();
    let calls: Vec<_> = snapshot
        .messages
        .iter()
        .flat_map(|entry| entry.tool_calls())
        .collect();

    assert_eq!(calls.len(), 1, "the scripted tool call must survive");
    assert_eq!(calls[0].name, "read_file");
    // The fake streams these arguments as six fragments, split mid-key and
    // mid-value. Getting them back whole proves reassembly happened at the
    // completion boundary and not before.
    assert_eq!(calls[0].arguments["path"], "src/lib.rs");
    assert_eq!(calls[0].arguments["limit"], 40);
}

#[tokio::test]
async fn cancel_stops_the_stream_and_emits_exactly_one_terminal_event() {
    // A slow fake so cancellation lands mid-stream rather than after it.
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(
        FakeProvider::new(FakeScript::Text).with_fragment_delay(Duration::from_millis(40)),
    );
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    let mut events = runtime.subscribe();
    open_session(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    // Cancel once the stream is demonstrably alive.
    wait_for(&mut events, "first delta", |event| {
        matches!(event, SmedEvent::TextDelta { .. })
    })
    .await;

    runtime
        .dispatch(SmedCommand::CancelRun)
        .await
        .expect("cancel");

    let terminal = wait_for(&mut events, "terminal event", |event| {
        matches!(
            event,
            SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
        )
    })
    .await;

    match terminal {
        SmedEvent::RunFinished { reason, .. } => assert_eq!(reason, FinishReason::Cancelled),
        other => panic!("cancellation must finish, not fail: {other:?}"),
    }

    // Exactly one terminal event: drain briefly and prove no second arrives.
    let second = tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            match events.recv().await {
                Ok(SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await;

    assert!(
        second.is_err() || second == Ok(false),
        "a run must emit exactly one terminal event"
    );

    // The session survives cancellation and can be used again.
    assert!(!runtime.snapshot().run_active);
}

#[tokio::test]
async fn a_failure_after_output_records_the_text_and_does_not_retry() {
    let (runtime, _store) = harness(FakeScript::FailMidStream);
    let mut events = runtime.subscribe();
    open_session(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    let terminal = wait_for(&mut events, "terminal event", |event| {
        matches!(
            event,
            SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
        )
    })
    .await;

    match terminal {
        SmedEvent::RunFailed { code, .. } => {
            assert_eq!(code.as_str(), "PROVIDER_PROTOCOL");
        }
        other => panic!("expected a failed run, got {other:?}"),
    }

    // Text the user already watched arrive is real and is kept, even though the
    // run failed afterwards. Discarding it would lose observed work.
    let snapshot = runtime.snapshot();
    let assistant: Vec<_> = snapshot
        .messages
        .iter()
        .filter(|message| message.role == Role::Assistant)
        .collect();
    assert_eq!(
        assistant.len(),
        1,
        "output produced before a failure is kept"
    );

    // And exactly one run happened — no automatic replay (AGENTS.md §4). One
    // user message plus one assistant message means the failed stream was not
    // quietly re-sent behind the user's back.
    let user_messages = snapshot
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .count();
    assert_eq!(
        user_messages, 1,
        "a stream that produced output must never be retried"
    );
}

/// Enough runs to overflow the 256-slot broadcast channel.
///
/// Each run emits roughly 15 events, so 30 runs produce ~450 — comfortably past
/// capacity. An earlier version used 12 runs (~180 events), never overflowed,
/// and hung forever waiting for a lag that could not happen. The number matters;
/// it is not a round guess.
const RUNS_TO_OVERFLOW_THE_FEED: usize = 30;

#[tokio::test]
async fn a_slow_consumer_applies_backpressure_without_unbounded_growth() {
    let (runtime, store) = harness(FakeScript::Text);

    // Subscribe, then never read. The feed is bounded, so it must drop old
    // events and tell this subscriber it lagged — not buffer them forever.
    let mut neglected = runtime.subscribe();
    let mut watcher = runtime.subscribe();

    open_session(&runtime).await;

    for index in 0..RUNS_TO_OVERFLOW_THE_FEED {
        runtime
            .dispatch(SmedCommand::SendUserMessage {
                text: format!("message {index}"),
                source: smed::core::directive::DirectiveSource::Human,
            })
            .await
            .expect("send");

        wait_for(&mut watcher, "run finished", |event| {
            matches!(event, SmedEvent::RunFinished { .. })
        })
        .await;
    }

    // Once a receiver is further behind than the channel's capacity, the very
    // next read reports the gap. Under a timeout so a bug here fails the test
    // rather than hanging it.
    let lagged = tokio::time::timeout(Duration::from_secs(2), neglected.recv())
        .await
        .expect("recv must not block: an overflowed feed reports Lagged immediately");

    assert!(
        matches!(
            lagged,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
        ),
        "a neglected subscriber must be told it lagged; silently buffering is unbounded growth. \
         got: {lagged:?}"
    );

    // And the runtime stayed correct throughout: every run completed and was
    // recorded despite a subscriber that never read a single event.
    let session = runtime.snapshot().session.expect("session");
    let stored = store.events(session).await.expect("events");
    let finished = stored
        .iter()
        .filter(|event| matches!(event.event, SmedEvent::RunFinished { .. }))
        .count();
    assert_eq!(
        finished, RUNS_TO_OVERFLOW_THE_FEED,
        "a slow consumer must not cost durable history"
    );
}

#[tokio::test]
async fn durable_events_reach_the_store_in_order() {
    let (runtime, store) = harness(FakeScript::Text);
    open_session(&runtime).await;

    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    wait_for(&mut events, "run finished", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    let session = runtime.snapshot().session.expect("session");
    let stored = store.events(session).await.expect("events");

    let sequences: Vec<u64> = stored.iter().map(|event| event.sequence).collect();
    let expected: Vec<u64> = (0..sequences.len() as u64).collect();
    assert_eq!(sequences, expected, "sequences must be dense and ordered");

    assert!(matches!(stored[0].event, SmedEvent::SessionCreated { .. }));
    assert!(matches!(
        stored.last().map(|event| &event.event),
        Some(SmedEvent::RunFinished { .. })
    ));
}

#[tokio::test]
async fn a_model_switch_is_refused_while_a_run_is_active() {
    let store = Arc::new(InMemoryEventStore::new());
    let provider: Arc<dyn Provider> = Arc::new(
        FakeProvider::new(FakeScript::Text).with_fragment_delay(Duration::from_millis(40)),
    );
    let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

    let mut events = runtime.subscribe();
    open_session(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "hello".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");

    wait_for(&mut events, "first delta", |event| {
        matches!(event, SmedEvent::TextDelta { .. })
    })
    .await;

    // : switching mid-run must not happen silently.
    runtime
        .dispatch(SmedCommand::SelectModel {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new("some-other-model"),
        })
        .await
        .expect("dispatch");

    let refusal = wait_for(&mut events, "model switch refusal", |event| {
        matches!(
            event,
            SmedEvent::ModelChangeRefused {
                code: smed::core::error::ReasonCode::RunActive,
                ..
            }
        )
    })
    .await;
    assert!(matches!(refusal, SmedEvent::ModelChangeRefused { .. }));

    wait_for(&mut events, "run finished", |event| {
        matches!(event, SmedEvent::RunFinished { .. })
    })
    .await;

    assert_eq!(
        runtime.snapshot().model.expect("model").as_str(),
        FakeProvider::MODEL,
        "the model must not change underneath an active run"
    );
}
