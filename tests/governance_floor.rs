//! The per-model governance floor, proved through the runtime.
//!
//! `core::governance` already proves the arithmetic — no tier widens any mode,
//! over the whole cross product. That is not the claim that matters. The claim
//! that matters is that the clamp is *reached*: that a session cannot arrive at
//! a supervised model still holding authority the owner granted for a different
//! one. Arithmetic in a module nothing calls is a comment.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use smed::core::command::SmedCommand;
use smed::core::error::ProviderError;
use smed::core::event::{FinishReason, ProviderEvent, SmedEvent};
use smed::core::governance::GovernanceTier;
use smed::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use smed::core::policy::PolicyMode;
use smed::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use smed::core::runtime::{RuntimeSnapshot, SmedRuntime};
use smed::core::store::EventStore;
use smed::runtime::Runtime;
use smed::store::memory::InMemoryEventStore;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The file `mjolnr init` writes, reduced to the two rows this test needs.
const GOVERNANCE: &str = "\
default: supervised
models:
  - match: { provider: frontier, model: \"frontier-1\" }
    tier: trusted
";

#[derive(Debug)]
struct QuietProvider {
    id: &'static str,
    model: &'static str,
}

#[async_trait]
impl Provider for QuietProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.id)
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(self.model),
            provider: self.id(),
            display_name: self.model.to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: Some(8_192),
            max_output_tokens: Some(4_096),
            tier: None,
        }]
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        _cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        events
            .send(ProviderEvent::TextDelta {
                text: format!("{} answered", self.id),
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

async fn wait_snapshot(
    runtime: &Runtime,
    ready: impl Fn(&RuntimeSnapshot) -> bool,
) -> RuntimeSnapshot {
    let mut snapshots = runtime.snapshots();
    let current = runtime.snapshot();
    if ready(&current) {
        return current;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = snapshots.changed().await.expect("runtime open");
            if ready(&snapshot) {
                return snapshot;
            }
        }
    })
    .await
    .expect("snapshot timeout")
}

/// A workspace, with a governance file if one is wanted.
fn workspace(fixture: &TempDir, governance: Option<&str>) -> std::path::PathBuf {
    let root = fixture.path().join("workspace");
    std::fs::create_dir_all(&root).expect("workspace");
    if let Some(contents) = governance {
        let smed = root.join(".smed");
        std::fs::create_dir_all(&smed).expect(".smed");
        std::fs::write(smed.join("governance.yaml"), contents).expect("governance.yaml");
    }
    root
}

async fn open(root: &std::path::Path) -> (Runtime, Arc<InMemoryEventStore>) {
    let store = Arc::new(InMemoryEventStore::new());
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(QuietProvider {
            id: "frontier",
            model: "frontier-1",
        }),
        Arc::new(QuietProvider {
            id: "quick",
            model: "quick-1",
        }),
    ];
    let runtime = Runtime::spawn(providers, Arc::clone(&store) as Arc<dyn EventStore>);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: root.to_path_buf(),
        })
        .await
        .expect("open");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: ProviderId::new("frontier"),
            model: ModelId::new("frontier-1"),
        })
        .await
        .expect("create");
    (runtime, store)
}

/// Wait until the turn has actually been through the provider.
///
/// Deliberately not `!run_active`: that is true *before* the run starts too,
/// so a policy assertion behind it samples the pre-run snapshot and passes
/// whatever the clamp does. The first draft of this file made that mistake in
/// two tests, one of which was green for the wrong reason.
async fn wait_answered(runtime: &Runtime) -> RuntimeSnapshot {
    wait_snapshot(runtime, |snapshot| {
        !snapshot.run_active
            && snapshot
                .messages
                .iter()
                .any(|message| message.text().contains("answered"))
    })
    .await
}

async fn arm_full_auto(runtime: &Runtime) {
    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::FullAuto,
        })
        .await
        .expect("policy");
    wait_snapshot(runtime, |snapshot| snapshot.policy == PolicyMode::FullAuto).await;
}

#[tokio::test]
async fn switching_to_a_supervised_model_drops_full_auto_and_says_so() {
    // The hole this phase closes. Before it, this switch changed who was
    // acting and nothing about what they were allowed to do.
    let fixture = TempDir::new().expect("fixture");
    let root = workspace(&fixture, Some(GOVERNANCE));
    let (runtime, store) = open(&root).await;
    arm_full_auto(&runtime).await;

    runtime
        .dispatch(SmedCommand::SelectModel {
            provider: ProviderId::new("quick"),
            model: ModelId::new("quick-1"),
        })
        .await
        .expect("switch");

    let after = wait_snapshot(&runtime, |snapshot| {
        snapshot
            .model
            .as_ref()
            .is_some_and(|id| id.as_str() == "quick-1")
            && snapshot.policy != PolicyMode::FullAuto
    })
    .await;
    assert_eq!(
        after.policy,
        PolicyMode::WorkspaceWrite,
        "a supervised model may still write; what it may not do is run unattended"
    );

    // And it is in the ledger, with the reason. A narrowing nobody can read
    // later is a session lying about its own state after the fact.
    let session = after.session.expect("session");
    let events = store.events(session).await.expect("events");
    let clamp = events
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::PolicyClamped {
                from,
                to,
                tier,
                model,
                ..
            } => Some((*from, *to, *tier, model.clone())),
            _ => None,
        })
        .expect("the narrowing must be recorded, not applied quietly");
    assert_eq!(clamp.0, PolicyMode::FullAuto);
    assert_eq!(clamp.1, PolicyMode::WorkspaceWrite);
    assert_eq!(clamp.2, GovernanceTier::Supervised);
    assert_eq!(clamp.3.as_str(), "quick-1");
}

#[tokio::test]
async fn a_trusted_model_keeps_the_policy_the_owner_set() {
    // The other half of "it only clamps": a tier must not be a reason to
    // narrow a session that was already within its ceiling.
    let fixture = TempDir::new().expect("fixture");
    let root = workspace(&fixture, Some(GOVERNANCE));
    let (runtime, _store) = open(&root).await;
    arm_full_auto(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "do the work".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let done = wait_answered(&runtime).await;
    assert_eq!(
        done.policy,
        PolicyMode::FullAuto,
        "the trusted tier is the absence of a ceiling, not a narrowing of its own"
    );
}

#[tokio::test]
async fn a_project_with_no_governance_file_is_untouched() {
    // The upgrade case. Taking full-auto away from every project that has
    // never heard of this feature would be a breaking change wearing a safety
    // argument.
    let fixture = TempDir::new().expect("fixture");
    let root = workspace(&fixture, None);
    let (runtime, _store) = open(&root).await;
    arm_full_auto(&runtime).await;

    runtime
        .dispatch(SmedCommand::SelectModel {
            provider: ProviderId::new("quick"),
            model: ModelId::new("quick-1"),
        })
        .await
        .expect("switch");
    let after = wait_snapshot(&runtime, |snapshot| {
        snapshot
            .model
            .as_ref()
            .is_some_and(|id| id.as_str() == "quick-1")
    })
    .await;
    assert_eq!(after.policy, PolicyMode::FullAuto);
}

#[tokio::test]
async fn an_unreadable_governance_file_is_narrowest_not_absent() {
    // A typo must not silently restore the authority the file exists to
    // withhold — and this is the path where that would actually bite, because
    // the owner who wrote the file believes it is in force.
    let fixture = TempDir::new().expect("fixture");
    let root = workspace(&fixture, Some("default: [not, a, tier]\n"));
    let (runtime, _store) = open(&root).await;
    arm_full_auto(&runtime).await;

    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: "do the work".to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send");
    let done = wait_answered(&runtime).await;
    assert_eq!(
        done.policy,
        PolicyMode::WorkspaceWrite,
        "an unreadable judgement is not the absence of one — even for the \
         model the file would have trusted"
    );
}
