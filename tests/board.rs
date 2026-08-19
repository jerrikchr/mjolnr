//! Phase E5 step 1: decision tickets and their resolutions.
//!
//! The negative paths matter most here, because this record's worst failure
//! mode is silence: a decision the human believes was recorded and was not.
//! Beside them sits the ADR-0015 constraint made behavioral — resolving a
//! ticket moves the ticket and nothing else, so authority-bearing state is
//! byte-identical before and after.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::board::{DecisionAuthor, DecisionTicketId, DecisionTicketKind};
use smed::core::command::SmedCommand;
use smed::core::error::ReasonCode;
use smed::core::event::{SessionId, SmedEvent};
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::{RuntimeSnapshot, SmedRuntime};
use smed::core::store::EventStore;
use smed::providers::fake::FakeProvider;
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;

async fn open_project() -> (
    Runtime,
    tempfile::TempDir,
    Arc<InMemoryEventStore>,
    SessionId,
) {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: temp.path().to_path_buf(),
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
    let session = settle_until(&runtime, |snap| snap.session.is_some())
        .await
        .session
        .expect("session");
    (runtime, temp, store, session)
}

async fn settle_until(
    runtime: &Runtime,
    predicate: impl Fn(&RuntimeSnapshot) -> bool,
) -> RuntimeSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let snapshot = runtime.snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition settled")
}

/// The `DecisionTicketOpened` event a session recorded, whose id the caller
/// needs for follow-up commands: a command's success is an ack, and the id
/// is part of the durable record, not a reply payload.
async fn opened_ticket(store: &InMemoryEventStore, session: SessionId) -> DecisionTicketId {
    let events = store.events(session).await.expect("events");
    events
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::DecisionTicketOpened { ticket, .. } => Some(ticket.id),
            _ => None,
        })
        .expect("a ticket was recorded")
}

async fn recorded_resolutions(
    store: &InMemoryEventStore,
    session: SessionId,
) -> Vec<smed::core::board::DecisionResolution> {
    store
        .events(session)
        .await
        .expect("events")
        .iter()
        .filter_map(|stored| match &stored.event {
            SmedEvent::DecisionTicketResolved { resolution, .. } => Some(resolution.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn opening_and_resolving_a_ticket_records_durable_verbatim_judgement() {
    let (runtime, _temp, store, session) = open_project().await;

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "Which order for the E5 board?".to_owned(),
            kind: DecisionTicketKind::Research,
            options: vec!["tickets first".to_owned(), "frontier first".to_owned()],
            blocked_by: Vec::new(),
        })
        .await
        .expect("opening records");

    let ticket = opened_ticket(&store, session).await;
    runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 0,
            note: Some("the frontier has nothing to compute over until tickets exist".to_owned()),
        })
        .await
        .expect("resolving records");

    let resolutions = recorded_resolutions(&store, session).await;
    assert_eq!(resolutions.len(), 1);
    let resolution = resolutions.first().expect("one resolution is recorded");
    // ADR-0015's record, verbatim: the question and the options considered —
    // not a summary, and no status word.
    assert_eq!(resolution.question, "Which order for the E5 board?");
    assert_eq!(
        resolution.options,
        vec!["tickets first".to_owned(), "frontier first".to_owned()]
    );
    assert_eq!(resolution.chosen_option, 0);
    assert_eq!(resolution.decided_by, DecisionAuthor::Human);
    assert_eq!(resolution.supersedes, None);
    assert_eq!(
        resolution.note.as_deref(),
        Some("the frontier has nothing to compute over until tickets exist")
    );
}

#[tokio::test]
async fn a_blocker_edge_is_recorded_and_a_dangling_one_is_refused() {
    let (runtime, _temp, store, session) = open_project().await;

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "What does a budget mean without dollars?".to_owned(),
            kind: DecisionTicketKind::Research,
            options: vec!["windows".to_owned(), "turn credits".to_owned()],
            blocked_by: Vec::new(),
        })
        .await
        .expect("open blocker");
    let blocker = opened_ticket(&store, session).await;

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "What does continuation show beside the loop?".to_owned(),
            kind: DecisionTicketKind::Task,
            options: vec!["worst window".to_owned(), "all windows".to_owned()],
            blocked_by: vec![blocker],
        })
        .await
        .expect("the edge records against an existing ticket");

    let missing = DecisionTicketId::new();
    let error = runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "A ticket behind nothing".to_owned(),
            kind: DecisionTicketKind::Task,
            options: vec!["this".to_owned(), "that".to_owned()],
            blocked_by: vec![missing],
        })
        .await
        .expect_err("a dangling blocker must refuse, not fog silently");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    assert!(
        error.to_string().contains(&missing.to_string()),
        "the refusal names what it was waiting on: {error}"
    );
    // The refusal preceded the append: no event exists for it.
    let tickets_opened = store
        .events(session)
        .await
        .expect("events")
        .iter()
        .filter(|stored| matches!(stored.event, SmedEvent::DecisionTicketOpened { .. }))
        .count();
    assert_eq!(tickets_opened, 2, "only the two valid opens persisted");
}

#[tokio::test]
async fn resolving_an_unknown_ticket_or_an_unrecorded_option_is_refused() {
    let (runtime, _temp, store, session) = open_project().await;

    let error = runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket: DecisionTicketId::new(),
            chosen_option: 0,
            note: None,
        })
        .await
        .expect_err("must refuse");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "two-option question".to_owned(),
            kind: DecisionTicketKind::Grilling,
            options: vec!["a".to_owned(), "b".to_owned()],
            blocked_by: Vec::new(),
        })
        .await
        .expect("open");
    let ticket = opened_ticket(&store, session).await;

    let error = runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 2,
            note: None,
        })
        .await
        .expect_err("an unrecorded option must refuse");
    assert_eq!(error.reason_code(), Some(ReasonCode::SchemaInvalid));
    assert!(
        error.to_string().contains("2 recorded options"),
        "the refusal says what was recorded: {error}"
    );
}

#[tokio::test]
async fn changing_your_mind_records_a_new_resolution_that_supersedes_in_chain() {
    let (runtime, _temp, store, session) = open_project().await;

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "Ship with or without?".to_owned(),
            kind: DecisionTicketKind::Prototype,
            options: vec!["with".to_owned(), "without".to_owned()],
            blocked_by: Vec::new(),
        })
        .await
        .expect("open");
    let ticket = opened_ticket(&store, session).await;

    runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 0,
            note: Some("with, on first reading".to_owned()),
        })
        .await
        .expect("first resolution");
    let first = recorded_resolutions(&store, session)
        .await
        .into_iter()
        .next()
        .expect("one resolution is recorded");

    runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 1,
            note: Some("without — the prototype disagreed".to_owned()),
        })
        .await
        .expect("the reversal records additively");

    let resolutions = recorded_resolutions(&store, session).await;
    assert_eq!(resolutions.len(), 2, "both judgements stay recorded");
    let second = resolutions.last().expect("the reversal is recorded");
    assert_eq!(second.supersedes, Some(first.id));
    // Neither was rewritten: the first record still says what it said.
    assert_eq!(store_copy_choice(&store, session, first.id).await, 0);
    assert_eq!(second.chosen_option, 1);
}

async fn store_copy_choice(
    store: &InMemoryEventStore,
    session: SessionId,
    resolution_id: smed::core::board::DecisionResolutionId,
) -> usize {
    recorded_resolutions(store, session)
        .await
        .into_iter()
        .find(|resolution| resolution.id == resolution_id)
        .expect("the superseded resolution remains addressable")
        .chosen_option
}

#[tokio::test]
async fn resolving_a_ticket_moves_nothing_but_the_ticket_itself() {
    let (runtime, _temp, store, session) = open_project().await;

    // The authority-relevant state, captured before any ticket exists.
    let before = runtime.snapshot();
    let before_policy = before.policy;
    let before_recovery = before.recovery.clone();
    let before_budget = (
        before.budget.provider_turns,
        before.budget.max_provider_turns,
    );

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "Question one?".to_owned(),
            kind: DecisionTicketKind::Task,
            options: vec!["yes".to_owned(), "no".to_owned()],
            blocked_by: Vec::new(),
        })
        .await
        .expect("open");
    let ticket = opened_ticket(&store, session).await;
    runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 1,
            note: None,
        })
        .await
        .expect("resolve");

    let after = runtime.snapshot();
    assert_eq!(
        after.policy, before_policy,
        "a resolution never widens policy"
    );
    assert_eq!(after.recovery, before_recovery);
    assert_eq!(
        (after.budget.provider_turns, after.budget.max_provider_turns),
        before_budget,
        "a resolution spends nothing"
    );

    // And nothing authority-shaped was ever recorded by this flow: no
    // approval, no tool proposal, no policy change, no envelope. The log is
    // the assertion, because a state field can be reverted but a recorded
    // grant cannot be written out of history (ADR-0015 §The trap).
    let events = store.events(session).await.expect("events");
    for stored in &events {
        assert!(
            !matches!(
                stored.event,
                SmedEvent::ApprovalResolved { .. }
                    | SmedEvent::ToolProposed { .. }
                    | SmedEvent::ToolCompleted { .. }
                    | SmedEvent::PolicyChanged { .. }
                    | SmedEvent::SpawnEnvelopeArmed { .. }
            ),
            "nothing authority-bearing entered the log: {:?}",
            stored.event
        );
    }
}

#[tokio::test]
async fn a_replayed_session_remembers_tickets_and_supersedes_in_chain() {
    let (runtime, temp, store, session) = open_project().await;

    runtime
        .dispatch(SmedCommand::OpenDecisionTicket {
            question: "persisted question".to_owned(),
            kind: DecisionTicketKind::Research,
            options: vec!["a".to_owned(), "b".to_owned()],
            blocked_by: Vec::new(),
        })
        .await
        .expect("open");
    let ticket = opened_ticket(&store, session).await;
    runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 0,
            note: None,
        })
        .await
        .expect("first resolution");
    let first = recorded_resolutions(&store, session)
        .await
        .into_iter()
        .next()
        .expect("the first resolution is recorded")
        .clone();
    runtime.close().await.expect("close");

    // A new runtime over the same store knows the ticket and its resolution,
    // because truth lives in the log, not in the process.
    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store.clone() as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    settle_until(&runtime, |snap| snap.session.is_some()).await;

    runtime
        .dispatch(SmedCommand::ResolveDecisionTicket {
            ticket,
            chosen_option: 1,
            note: Some("second thoughts, after a restart".to_owned()),
        })
        .await
        .expect("the replayed state accepts the supersession");

    let resolutions = recorded_resolutions(&store, session).await;
    assert_eq!(resolutions.len(), 2);
    let second = resolutions.last().expect("the resolution after resume");
    assert_eq!(
        second.supersedes,
        Some(first.id),
        "the chain survives a restart: the new resolution names the old one"
    );
}
