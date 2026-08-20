//! Phase 25 council acceptance tests.
//!
//! The merged stub emitted hardcoded strings and never called a model. These
//! assert the honest replacement: a council convenes *real* read-only member
//! sessions, so its output carries the members' actual model text; it runs to
//! the round cap with dissent preserved; and an underfunded budget refuses
//! upfront rather than deliberating into insolvency.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::command::MjolnrCommand;
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::provider::Provider;
use mjolnr::core::runtime::{MjolnrRuntime, RuntimeSnapshot};
use mjolnr::core::store::EventStore;
use mjolnr::providers::fake::FakeProvider;
use mjolnr::runtime::Runtime;
use mjolnr::store::memory::InMemoryEventStore;

/// The text `FakeProvider` streams — a real member turn surfaces it; the
/// fabricated council never could, because it never called a model.
const MEMBER_TEXT: &str = "mjolnr streams text incrementally";

async fn open_council_project(council_yaml: &str) -> (Runtime, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let mjolnr_dir = temp.path().join(".mjolnr");
    std::fs::create_dir_all(&mjolnr_dir).expect("create .mjolnr");
    std::fs::write(mjolnr_dir.join("council.yaml"), council_yaml).expect("write council.yaml");

    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(
        vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
        store as Arc<dyn EventStore>,
    );
    runtime
        .dispatch(MjolnrCommand::OpenProject {
            root: temp.path().to_path_buf(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(MjolnrCommand::CreateSession {
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    settle_until(&runtime, |snap| snap.session.is_some()).await;
    (runtime, temp)
}

async fn settle_until(runtime: &Runtime, predicate: impl Fn(&RuntimeSnapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if predicate(&runtime.snapshot()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition settled");
}

fn council_message(snapshot: &RuntimeSnapshot) -> Option<String> {
    snapshot
        .messages
        .iter()
        .map(|entry| entry.text())
        .find(|text| text.contains("MJOLNR COUNCIL"))
}

#[tokio::test]
async fn a_council_convenes_real_members_to_the_round_cap_and_preserves_dissent() {
    let (runtime, _temp) =
        open_council_project("roles: [\"alpha\", \"beta\"]\nmax_rounds: 2\n").await;

    runtime
        .dispatch(MjolnrCommand::ConveneCouncil {
            question: "How should governance scale under concurrency?".to_owned(),
            plan_file: None,
        })
        .await
        .expect("dispatch council");

    settle_until(&runtime, |snap| council_message(snap).is_some()).await;
    let text = council_message(&runtime.snapshot()).expect("council message");

    // Real members ran: the output carries the model's actual streamed text,
    // once per member per round (two members, two rounds).
    assert!(
        text.matches(MEMBER_TEXT).count() >= 2,
        "council output must carry real member model text, not fabricated strings:\n{text}"
    );
    // Both members spoke, and dissent is preserved as its own section.
    assert!(text.contains("[alpha]"), "alpha member is present");
    assert!(text.contains("[beta]"), "beta member is present");
    assert!(text.contains("2 round(s)"), "ran to the two-round cap");
    assert!(
        text.contains("Dissents & critiques"),
        "the critique round's dissent is preserved"
    );
    assert!(
        text.contains("advisory"),
        "the council states it is advisory, not an actor"
    );
    assert!(
        runtime.snapshot().last_council.is_some(),
        "the completed review reaches the runtime projection"
    );

    let review = runtime.snapshot().last_council.clone().expect("review");
    let finding = review.findings.first().expect("structured finding");
    runtime
        .dispatch(MjolnrCommand::ResolveCouncilFinding {
            review_id: review.review_id,
            finding_id: finding.id,
            disposition: mjolnr::core::council::CouncilDisposition::Defer,
            note: Some("Need a human to compare the competing risks.".to_owned()),
        })
        .await
        .expect("record human disposition");
    settle_until(&runtime, |snap| {
        snap.last_council
            .as_ref()
            .and_then(|review| review.findings.first())
            .and_then(|finding| finding.disposition.as_ref())
            .is_some()
    })
    .await;
    assert_eq!(
        runtime.snapshot().last_council.as_ref().and_then(|review| {
            review
                .findings
                .first()
                .and_then(|finding| finding.disposition.as_ref())
                .map(|disposition| disposition.disposition)
        }),
        Some(mjolnr::core::council::CouncilDisposition::Defer)
    );
}

#[tokio::test]
async fn a_council_whose_budget_cannot_fund_the_rounds_refuses_upfront() {
    // Two members × two rounds needs four provider-turns; one cannot fund it.
    let (runtime, _temp) = open_council_project(
        "roles: [\"alpha\", \"beta\"]\nmax_rounds: 2\nbudget_provider_turns: 1\n",
    )
    .await;

    runtime
        .dispatch(MjolnrCommand::ConveneCouncil {
            question: "unfundable".to_owned(),
            plan_file: None,
        })
        .await
        .expect("dispatch council");

    settle_until(&runtime, |snap| {
        snap.messages
            .iter()
            .any(|entry| entry.text().contains("Council refused"))
    })
    .await;

    let refused = runtime
        .snapshot()
        .messages
        .iter()
        .any(|entry| entry.text().contains("Council refused"));
    assert!(refused, "an underfunded council must refuse upfront");
    // It refused before deliberating: no member text was produced.
    assert!(
        council_message(&runtime.snapshot()).is_none(),
        "a refused council must not deliberate"
    );
}
