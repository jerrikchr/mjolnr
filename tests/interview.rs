//! Stage 3 acceptance: a bounded interview produces a durable PRD, routes it
//! through an advisory council, and synthesizes a human-approved handoff.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use smed::core::command::SmedCommand;
use smed::core::event::{SmedEvent, StoredEvent};
use smed::core::model::{ModelId, ProviderId};
use smed::core::plan::{PlanApproval, PlanHandoff, PlanStage, ReviewVerdict};
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tempfile::TempDir;

async fn settle(runtime: &Runtime, ready: impl Fn(&smed::core::runtime::RuntimeSnapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if ready(&runtime.snapshot()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime did not settle");
}

async fn open_session(
    store: Arc<InMemoryEventStore>,
) -> (Runtime, TempDir, smed::core::event::SessionId) {
    let workspace = tempfile::tempdir().expect("workspace");
    let smed_dir = workspace.path().join(".smed");
    std::fs::create_dir_all(&smed_dir).expect(".smed");
    std::fs::write(
        smed_dir.join("council.yaml"),
        "roles: [\"plan\"]\nmax_rounds: 1\n",
    )
    .expect("council config");

    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::PlanInterview));
    let runtime = Runtime::spawn(vec![provider], store.clone() as Arc<dyn EventStore>);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: workspace.path().to_owned(),
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
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;
    let session = runtime.snapshot().session.expect("session");
    (runtime, workspace, session)
}

fn event_index(events: &[StoredEvent], predicate: impl Fn(&SmedEvent) -> bool) -> usize {
    events
        .iter()
        .position(|stored| predicate(&stored.event))
        .expect("event in durable history")
}

fn assert_event_order(
    events: &[StoredEvent],
    plan_id: smed::core::plan::PlanId,
    prd_id: smed::core::plan::PrdId,
) {
    let started = event_index(events, |event| {
        matches!(event, SmedEvent::PlanInterviewStarted { .. })
    });
    let asked = event_index(events, |event| {
        matches!(event, SmedEvent::PlanQuestionAsked { .. })
    });
    let answered = event_index(events, |event| {
        matches!(event, SmedEvent::PlanQuestionAnswered { .. })
    });
    let prd_proposed = event_index(events, |event| {
        matches!(event, SmedEvent::PlanPrdProposed { .. })
    });
    let council = event_index(
        events,
        |event| matches!(event, SmedEvent::CouncilReviewed { review, .. } if review.plan_id == Some(plan_id) && review.prd_id == Some(prd_id)),
    );
    let proposed = event_index(
        events,
        |event| matches!(event, SmedEvent::PlanProposed { proposal, .. } if proposal.plan_id == plan_id),
    );
    let approved = event_index(
        events,
        |event| matches!(event, SmedEvent::PlanApproved { approval, .. } if approval.plan_id == plan_id),
    );
    let handoff = event_index(
        events,
        |event| matches!(event, SmedEvent::PlanHandoffCreated { handoff, .. } if handoff.plan_id == plan_id),
    );
    assert!(started < asked && asked < answered && answered < prd_proposed);
    assert!(
        prd_proposed < council && council < proposed && proposed < approved && approved < handoff
    );
}

async fn approve_and_handoff_generated_plan(runtime: &Runtime) {
    let (plan_id, revision_id) = runtime
        .snapshot()
        .plan
        .as_ref()
        .and_then(|plan| match &plan.stage {
            PlanStage::Proposed { proposal } => Some((plan.plan_id, proposal.revision_id)),
            _ => None,
        })
        .expect("generated proposal revision");
    runtime
        .dispatch(SmedCommand::ApprovePlan {
            approval: PlanApproval {
                plan_id,
                revision_id,
                approver: "Human".to_owned(),
                decision: ReviewVerdict::Approve,
                note: Some("Ready for the execution owner".to_owned()),
                approved_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("approve generated plan");
    settle(runtime, |snapshot| {
        snapshot
            .plan
            .as_ref()
            .is_some_and(|plan| matches!(plan.stage, PlanStage::Approved { .. }))
    })
    .await;
    runtime
        .dispatch(SmedCommand::HandoffPlan {
            handoff: PlanHandoff {
                plan_id,
                revision_id,
                handoff_note: "Hand off the approved plan to execution".to_owned(),
                created_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("handoff generated plan");
    settle(runtime, |snapshot| {
        snapshot
            .plan
            .as_ref()
            .is_some_and(|plan| matches!(plan.stage, PlanStage::Handoff { .. }))
    })
    .await;
}

#[tokio::test]
async fn interview_prd_council_and_approved_handoff_are_durable_and_linked() {
    let store = Arc::new(InMemoryEventStore::new());
    let (runtime, workspace, session) = open_session(store.clone()).await;

    runtime
        .dispatch(SmedCommand::StartPlanInterview {
            goal: "Turn a user idea into a reviewed implementation plan".to_owned(),
        })
        .await
        .expect("start interview");
    settle(&runtime, |snapshot| {
        snapshot
            .plan
            .as_ref()
            .is_some_and(|plan| matches!(plan.stage, PlanStage::QuestionPending { .. }))
    })
    .await;

    let (plan_id, question) = runtime
        .snapshot()
        .plan
        .as_ref()
        .and_then(|plan| match &plan.stage {
            PlanStage::QuestionPending { question } => Some((plan.plan_id, question.clone())),
            _ => None,
        })
        .expect("interview question");
    runtime
        .dispatch(SmedCommand::AnswerPlanQuestion {
            plan_id,
            answer: smed::core::plan::QuestionAnswer {
                question_id: question.id,
                selected_options: vec!["Narrow vertical slice".to_owned()],
                freeform_text: Some("Keep the first release local-first".to_owned()),
                answered_at: time::OffsetDateTime::now_utc(),
            },
        })
        .await
        .expect("answer interview");

    settle(&runtime, |snapshot| {
        snapshot.plan.as_ref().is_some_and(|plan| {
            plan.prd.is_some()
                && plan.council_link.is_some()
                && matches!(plan.stage, PlanStage::Proposed { .. })
        })
    })
    .await;

    approve_and_handoff_generated_plan(&runtime).await;

    let snapshot = runtime.snapshot();
    let plan = snapshot.plan.as_ref().expect("plan snapshot");
    let prd = plan.prd.as_ref().expect("durable PRD");
    let link = plan.council_link.as_ref().expect("PRD council link");
    let prd_id = prd.id;
    let review_id = link.review_id;
    assert_eq!(link.plan_id, plan.plan_id);
    assert_eq!(link.prd_id, prd.id);

    let events = store.events(session).await.expect("durable events");
    assert_event_order(&events, plan.plan_id, prd_id);

    runtime.close().await.expect("close runtime");
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::PlanInterview));
    let resumed = Runtime::spawn(vec![provider], store.clone() as Arc<dyn EventStore>);
    resumed
        .dispatch(SmedCommand::OpenProject {
            root: workspace.path().to_owned(),
        })
        .await
        .expect("reopen project");
    resumed
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume session");
    settle(&resumed, |snapshot| {
        snapshot.session == Some(session)
            && snapshot.plan.as_ref().is_some_and(|plan| {
                plan.prd
                    .as_ref()
                    .is_some_and(|replayed| replayed.id == prd_id)
                    && plan
                        .council_link
                        .as_ref()
                        .is_some_and(|link| link.review_id == review_id)
                    && matches!(plan.stage, PlanStage::Handoff { .. })
            })
    })
    .await;
    assert_eq!(resumed.snapshot().plan, snapshot.plan);
    resumed.close().await.expect("close resumed runtime");
}
