//! Deterministic Phase 13 subagent transcript with real git worktrees.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use smed::core::command::{ApprovalDecision, SmedCommand};
use smed::core::error::ReasonCode;
use smed::core::event::{FinishReason, SessionId, SmedEvent};
use smed::core::message::{ToolOutcome, ToolResult};
use smed::core::model::{ModelId, ProviderId};
use smed::core::policy::PolicyMode;
use smed::core::provider::Provider;
use smed::core::runtime::{RuntimeSubscription, SmedRuntime};
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::runtime::subagent::cleanup_orphans;
use smed::store::memory::InMemoryEventStore;
use tempfile::TempDir;

const DEADLINE: Duration = Duration::from_secs(30);

struct Harness {
    repository: TempDir,
    runtime: Runtime,
    store: Arc<InMemoryEventStore>,
}

impl Harness {
    async fn new(policy: PolicyMode) -> Self {
        let repository = repository();
        let store = Arc::new(InMemoryEventStore::new());
        let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Subagent));
        let runtime = Runtime::spawn(vec![provider], Arc::clone(&store) as Arc<dyn EventStore>);

        runtime
            .dispatch(SmedCommand::OpenProject {
                root: repository.path().to_path_buf(),
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
        runtime
            .dispatch(SmedCommand::SetPolicy { mode: policy })
            .await
            .expect("set policy");
        wait_ready(&runtime, policy).await;

        Self {
            repository,
            runtime,
            store,
        }
    }

    fn root(&self) -> &Path {
        self.repository.path()
    }
}

async fn wait_ready(runtime: &Runtime, policy: PolicyMode) {
    if runtime.snapshot().session.is_some() && runtime.snapshot().policy == policy {
        return;
    }
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(DEADLINE, async {
        loop {
            let snapshot = snapshots.changed().await.expect("runtime remains open");
            if snapshot.session.is_some() && snapshot.policy == policy {
                break;
            }
        }
    })
    .await
    .expect("session becomes ready");
}

async fn approve_spawn(
    runtime: &Runtime,
    directive: &str,
) -> (RuntimeSubscription, Vec<SmedEvent>) {
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: directive.to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send directive");

    let mut observed = Vec::new();
    let approval = tokio::time::timeout(DEADLINE, async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            let approval = match &event {
                SmedEvent::ToolProposed {
                    approval: Some(approval),
                    call,
                    ..
                } if call.name == "spawn_subagent" => Some(*approval),
                _ => None,
            };
            observed.push(event);
            if let Some(approval) = approval {
                break approval;
            }
        }
    })
    .await
    .expect("spawn proposal arrives");

    runtime
        .dispatch(SmedCommand::ResolveApproval {
            approval,
            decision: ApprovalDecision::ApproveOnce,
        })
        .await
        .expect("approve spawn");
    (events, observed)
}

/// Poll the snapshot until `ready`, or give up loudly.
async fn settle(
    runtime: &Runtime,
    ready: impl Fn(&smed::core::runtime::RuntimeSnapshot) -> bool,
) -> smed::core::runtime::RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

/// Send a directive and collect every event until the run settles.
///
/// Deliberately does not approve anything: used where the point is that no
/// approval was needed, or that the run refused before asking for one.
async fn run_to_completion(runtime: &Runtime, directive: &str) -> Vec<SmedEvent> {
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: directive.to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("send directive");
    tokio::time::timeout(DEADLINE, async {
        let mut seen = Vec::new();
        loop {
            let event = events.recv().await.expect("event feed remains open");
            let terminal = matches!(
                event,
                SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
            );
            seen.push(event);
            if terminal {
                break seen;
            }
        }
    })
    .await
    .expect("the run settles")
}

async fn run_spawn(runtime: &Runtime, directive: &str) -> Vec<SmedEvent> {
    let (mut events, mut observed) = approve_spawn(runtime, directive).await;
    tokio::time::timeout(DEADLINE, async {
        loop {
            let event = events.recv().await.expect("event feed remains open");
            let terminal = matches!(
                event,
                SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
            );
            observed.push(event);
            if terminal {
                break;
            }
        }
    })
    .await
    .expect("parent run settles");
    observed
}

fn repository() -> TempDir {
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

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn spawn_result(events: &[SmedEvent]) -> &ToolResult {
    events
        .iter()
        .find_map(|event| match event {
            SmedEvent::ToolCompleted { name, result, .. } if name == "spawn_subagent" => {
                Some(result)
            }
            _ => None,
        })
        .expect("spawn result")
}

fn spawned(events: &[SmedEvent]) -> Vec<(SessionId, String, PathBuf, PolicyMode)> {
    events
        .iter()
        .filter_map(|event| match event {
            SmedEvent::SubagentSpawned {
                child,
                branch,
                worktree,
                policy,
                ..
            } => Some((*child, branch.clone(), PathBuf::from(worktree), *policy)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn two_children_use_isolated_worktrees_and_settle_once() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let parent = harness.runtime.snapshot().session.expect("parent session");

    let events = run_spawn(&harness.runtime, "spawn-two:").await;
    let children = spawned(&events);
    assert_eq!(children.len(), 2, "both child boundaries must be visible");
    assert!(spawn_result(&events).outcome.is_ok());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                SmedEvent::ToolCompleted { name, .. } if name == "spawn_subagent"
            ))
            .count(),
        1,
        "one spawn group must produce exactly one settlement"
    );

    for (_, branch, worktree, policy) in &children {
        assert_eq!(*policy, PolicyMode::WorkspaceWrite);
        assert!(!worktree.exists(), "settled worktree must be removed");
        assert!(
            !worktree.with_extension("owner.json").exists(),
            "owner marker must be removed"
        );
        assert!(
            !git(harness.root(), &["branch", "--list", branch])
                .trim()
                .is_empty(),
            "a branch carrying child work must survive"
        );
    }

    let alpha = children
        .iter()
        .find(|(_, branch, _, _)| {
            git(harness.root(), &["ls-tree", "-r", "--name-only", branch])
                .lines()
                .any(|path| path == "alpha.txt")
        })
        .expect("one branch carries alpha.txt");
    let beta = children
        .iter()
        .find(|(_, branch, _, _)| {
            git(harness.root(), &["ls-tree", "-r", "--name-only", branch])
                .lines()
                .any(|path| path == "beta.txt")
        })
        .expect("one branch carries beta.txt");
    assert_ne!(alpha.1, beta.1, "siblings must not share a branch");

    let summaries = harness.store.sessions().await.expect("sessions");
    let linked = summaries
        .iter()
        .filter(|summary| summary.parent == Some(parent))
        .count();
    assert_eq!(linked, 2, "both child sessions must link to the parent");
}

#[tokio::test]
async fn a_dirty_parent_refuses_before_dispatch() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    std::fs::write(harness.root().join("dirty.txt"), "not committed\n").expect("dirty file");

    let events = run_spawn(&harness.runtime, "spawn-two:").await;
    assert_eq!(
        spawn_result(&events).outcome,
        ToolOutcome::Refused(ReasonCode::WorkspaceDirty)
    );
    assert!(spawned(&events).is_empty(), "no child may dispatch");
}

#[tokio::test]
async fn child_policy_is_clamped_to_the_parent() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let events = run_spawn(&harness.runtime, "spawn-clamp:").await;
    let children = spawned(&events);

    assert_eq!(children.len(), 1);
    assert_eq!(children[0].3, PolicyMode::WorkspaceWrite);
    assert_ne!(children[0].3, PolicyMode::FullAuto);
}

#[tokio::test]
async fn an_envelope_wider_than_the_session_is_refused_at_arm_time() {
    use smed::core::envelope::SpawnEnvelope;

    let harness = Harness::new(PolicyMode::Ask).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::FullAuto,
                max_children: 8,
                max_per_call: 4,
                max_provider_turns: 40,
                routes: Vec::new(),
                expires_after_turns: 10,
            }),
        })
        .await
        .expect("dispatch arm");

    // Arming one thing and being granted another is the class of surprise the
    // gate exists to remove, so this refuses rather than silently narrowing.
    let snapshot = settle(&harness.runtime, |snapshot| {
        snapshot.envelope_refusal.is_some() || snapshot.envelope.is_some()
    })
    .await;
    assert!(
        snapshot.envelope.is_none(),
        "an ask session must not hold a full-auto envelope"
    );
    assert!(
        snapshot
            .envelope_refusal
            .is_some_and(|detail| detail.contains("wider")),
        "the refusal must say why"
    );
}

#[tokio::test]
async fn a_spawn_inside_the_envelope_needs_no_approval_and_is_charged() {
    use smed::core::envelope::SpawnEnvelope;

    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::WorkspaceWrite,
                max_children: 8,
                max_per_call: 4,
                max_provider_turns: 60,
                routes: Vec::new(),
                expires_after_turns: 10,
            }),
        })
        .await
        .expect("dispatch arm");
    settle(&harness.runtime, |snapshot| snapshot.envelope.is_some()).await;

    // No approval is resolved here: the envelope already authorised this shape,
    // which is the entire point of arming one.
    let observed = run_to_completion(&harness.runtime, "spawn-two:").await;

    let drawn = observed.iter().find_map(|event| match event {
        SmedEvent::SpawnEnvelopeDrawn {
            children,
            children_remaining,
            ..
        } => Some((*children, *children_remaining)),
        _ => None,
    });
    assert_eq!(
        drawn,
        Some((2, 6)),
        "the draw must be recorded against the envelope it spent:\n{observed:#?}"
    );
    assert_eq!(spawned(&observed).len(), 2, "both children must dispatch");
}

#[tokio::test]
async fn a_draw_beyond_the_envelope_is_refused_with_a_typed_code() {
    use smed::core::envelope::SpawnEnvelope;

    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::WorkspaceWrite,
                max_children: 8,
                max_per_call: 1,
                max_provider_turns: 60,
                routes: Vec::new(),
                expires_after_turns: 10,
            }),
        })
        .await
        .expect("dispatch arm");
    settle(&harness.runtime, |snapshot| snapshot.envelope.is_some()).await;

    let observed = run_to_completion(&harness.runtime, "spawn-two:").await;

    // Refused, not downgraded to an approval prompt: a two-child preview here
    // would be the previewability problem coming back through the door the
    // envelope showed it out of.
    let refusal = observed.iter().find_map(|event| match event {
        SmedEvent::ToolCompleted { name, result, .. } if name == "spawn_subagent" => {
            Some(result.clone())
        }
        _ => None,
    });
    let refusal = refusal.expect("the spawn must record an outcome");
    assert_eq!(
        refusal.outcome,
        ToolOutcome::Refused(ReasonCode::SpawnEnvelopeRefused)
    );
    assert!(
        spawned(&observed).is_empty(),
        "a refused draw must dispatch nothing"
    );
}

#[tokio::test]
async fn the_aggregate_turn_budget_bites_before_the_child_count_does() {
    use smed::core::envelope::SpawnEnvelope;

    // Spend is the binding constraint, not headcount: a fleet of cheap children
    // is still a fleet. Eight children remain, but the two this spawn wants
    // would cost more turns than the envelope has left.
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::WorkspaceWrite,
                max_children: 8,
                max_per_call: 4,
                max_provider_turns: 5,
                routes: Vec::new(),
                expires_after_turns: 10,
            }),
        })
        .await
        .expect("dispatch arm");
    settle(&harness.runtime, |snapshot| snapshot.envelope.is_some()).await;

    let events = run_to_completion(&harness.runtime, "spawn-two:").await;

    let refusal = events
        .iter()
        .find_map(|event| match event {
            SmedEvent::ToolCompleted { name, result, .. } if name == "spawn_subagent" => {
                Some(result.clone())
            }
            _ => None,
        })
        .expect("the spawn must record an outcome");
    assert_eq!(
        refusal.outcome,
        ToolOutcome::Refused(ReasonCode::SpawnEnvelopeRefused)
    );
    assert!(
        refusal.content.contains("provider turns"),
        "the refusal must name the bound that bit, not just say no: {}",
        refusal.content
    );
    assert!(spawned(&events).is_empty());
}

#[tokio::test]
async fn the_ledger_reconstructs_an_envelopes_whole_life() {
    use smed::core::envelope::SpawnEnvelope;
    use smed::core::event::EnvelopeEnd;

    // The audit's job is to answer "what did this human authorise, and what was
    // done with it?" — from the record alone, without the in-memory state that
    // produced it and that a restart throws away.
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::WorkspaceWrite,
                max_children: 2,
                max_per_call: 4,
                max_provider_turns: 60,
                routes: Vec::new(),
                expires_after_turns: 10,
            }),
        })
        .await
        .expect("dispatch arm");
    let snapshot = settle(&harness.runtime, |snapshot| snapshot.envelope.is_some()).await;
    let session = snapshot.session.expect("an open session");

    // Exactly fills the envelope, so the record should also show it spent.
    run_to_completion(&harness.runtime, "spawn-two:").await;
    settle(&harness.runtime, |snapshot| snapshot.envelope.is_none()).await;

    let stored = harness
        .store
        .events(session)
        .await
        .expect("read the ledger");
    let armed = stored.iter().find_map(|entry| match &entry.event {
        SmedEvent::SpawnEnvelopeArmed {
            max_children,
            ceiling,
            ..
        } => Some((*max_children, *ceiling)),
        _ => None,
    });
    assert_eq!(armed, Some((2, PolicyMode::WorkspaceWrite)));

    let drawn = stored.iter().find_map(|entry| match &entry.event {
        SmedEvent::SpawnEnvelopeDrawn {
            children,
            children_remaining,
            ..
        } => Some((*children, *children_remaining)),
        _ => None,
    });
    assert_eq!(drawn, Some((2, 0)));

    let cleared = stored.iter().find_map(|entry| match &entry.event {
        SmedEvent::SpawnEnvelopeCleared { reason, .. } => Some(*reason),
        _ => None,
    });
    assert_eq!(
        cleared,
        Some(EnvelopeEnd::Spent),
        "an envelope that ran out must be recorded as spent, not merely absent"
    );
}

#[tokio::test]
async fn an_envelope_lapses_and_the_next_spawn_asks_again() {
    use smed::core::envelope::SpawnEnvelope;

    // Property 4: expiry is disclosed and returns the session to asking. An
    // envelope that quietly kept authorising past its turn budget would be the
    // standing grant the expiry exists to prevent.
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::WorkspaceWrite,
                max_children: 8,
                max_per_call: 4,
                max_provider_turns: 60,
                routes: Vec::new(),
                expires_after_turns: 1,
            }),
        })
        .await
        .expect("dispatch arm");
    settle(&harness.runtime, |snapshot| snapshot.envelope.is_some()).await;

    // One completed run is one turn against the envelope's clock.
    run_to_completion(&harness.runtime, "just answer").await;
    let snapshot = settle(&harness.runtime, |snapshot| snapshot.envelope.is_none()).await;
    assert!(
        snapshot.envelope.is_none(),
        "the envelope must lapse on the turn it said it would"
    );

    // And the next spawn is back to needing a human — proven by it holding for
    // an approval rather than dispatching.
    let (_events, observed) = approve_spawn(&harness.runtime, "spawn-two:").await;
    assert!(
        observed.iter().any(|event| matches!(
            event,
            SmedEvent::ToolProposed {
                approval: Some(_),
                ..
            }
        )),
        "a lapsed envelope must not keep authorising spawns"
    );
}

#[tokio::test]
async fn narrowing_the_policy_clears_an_envelope_it_no_longer_justifies() {
    use smed::core::envelope::SpawnEnvelope;

    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    harness
        .runtime
        .dispatch(SmedCommand::ArmSpawnEnvelope {
            envelope: Box::new(SpawnEnvelope {
                ceiling: PolicyMode::WorkspaceWrite,
                max_children: 8,
                max_per_call: 4,
                max_provider_turns: 60,
                routes: Vec::new(),
                expires_after_turns: 10,
            }),
        })
        .await
        .expect("dispatch arm");
    settle(&harness.runtime, |snapshot| snapshot.envelope.is_some()).await;

    harness
        .runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: PolicyMode::ReadOnly,
        })
        .await
        .expect("narrow the policy");

    // An envelope outliving the policy that justified it would be a standing
    // grant nobody re-authorised.
    let snapshot = settle(&harness.runtime, |snapshot| snapshot.envelope.is_none()).await;
    assert!(snapshot.envelope.is_none());
}

#[tokio::test]
async fn a_child_reading_its_own_record_cannot_see_its_parents() {
    // `query_session`'s scope is structural — the schema has no session
    // parameter and the actor reads `self.state.session` — but structure is
    // exactly what a later refactor breaks by threading a session through "just
    // for the parent's benefit". This proves it against a real child.
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let events = run_spawn(&harness.runtime, "spawn-query:").await;

    let children = spawned(&events);
    assert_eq!(children.len(), 1, "one child must dispatch");
    let child_session = children[0].0;

    // The child's own session holds its query and the window it got back.
    let child_events = harness
        .store
        .events(child_session)
        .await
        .expect("read the child's session");
    let window = child_events
        .iter()
        .find_map(|stored| match &stored.event {
            SmedEvent::ToolCompleted { name, result, .. } if name == "query_session" => {
                Some(result.content.clone())
            }
            _ => None,
        })
        .expect("the child ran query_session and recorded its answer");

    assert!(
        !window.contains("spawn-query:"),
        "a child must not see the directive that spawned it — that text lives \
         only in the parent's session:\n{window}"
    );
    assert!(
        !window.contains("spawn_subagent"),
        "the parent's spawn proposal must not appear in the child's window:\n{window}"
    );
    assert!(
        window.contains("You are a smed subagent"),
        "the child should still see its own session — the window is scoped, not empty:\n{window}"
    );
    assert!(
        window.contains("policy_changed: read-only"),
        "the child's own clamped policy is part of its record:\n{window}"
    );
}

#[tokio::test]
async fn schema_invalid_child_output_is_a_typed_parent_failure() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let events = run_spawn(&harness.runtime, "spawn-invalid:").await;

    assert_eq!(
        spawn_result(&events).outcome,
        ToolOutcome::Failed(ReasonCode::SchemaInvalid)
    );
}

#[tokio::test]
async fn a_child_that_never_reports_has_a_distinct_failure() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let events = run_spawn(&harness.runtime, "spawn-noreport:").await;

    assert_eq!(
        spawn_result(&events).outcome,
        ToolOutcome::Failed(ReasonCode::SubagentResultMissing)
    );
}

#[tokio::test]
async fn cancelling_the_parent_cancels_and_cleans_the_child() {
    let harness = Harness::new(PolicyMode::WorkspaceWrite).await;
    let (mut events, _) = approve_spawn(&harness.runtime, "spawn-hold:").await;

    let worktree = tokio::time::timeout(DEADLINE, async {
        loop {
            if let SmedEvent::SubagentSpawned { worktree, .. } =
                events.recv().await.expect("event feed remains open")
            {
                break PathBuf::from(worktree);
            }
        }
    })
    .await
    .expect("child dispatches");
    assert!(worktree.exists());

    harness
        .runtime
        .dispatch(SmedCommand::CancelRun)
        .await
        .expect("cancel parent");
    let reason = tokio::time::timeout(DEADLINE, async {
        loop {
            match events.recv().await.expect("event feed remains open") {
                SmedEvent::RunFinished { reason, .. } => break reason,
                SmedEvent::RunFailed { code, detail, .. } => {
                    panic!("cancellation failed as {code}: {detail}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("cancelled run settles");

    assert_eq!(reason, FinishReason::Cancelled);
    assert!(
        !worktree.exists(),
        "cancelled child worktree must be removed"
    );
}

#[tokio::test]
async fn orphan_cleanup_removes_dead_owners_but_preserves_live_ones() {
    let repository = repository();
    let namespace = std::env::temp_dir().join("smed-worktrees");
    std::fs::create_dir_all(&namespace).expect("worktree namespace");
    let dead_path = namespace.join(SessionId::new().to_string());
    let live_path = namespace.join(SessionId::new().to_string());
    let dead_branch = format!("smed/test-dead-{}", SessionId::new());
    let live_branch = format!("smed/test-live-{}", SessionId::new());

    git(
        repository.path(),
        &[
            "worktree",
            "add",
            "-b",
            &dead_branch,
            dead_path.to_str().expect("utf-8 path"),
            "HEAD",
        ],
    );
    git(
        repository.path(),
        &[
            "worktree",
            "add",
            "-b",
            &live_branch,
            live_path.to_str().expect("utf-8 path"),
            "HEAD",
        ],
    );

    let mut exited = if cfg!(windows) {
        Command::new(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_owned()))
            .args(["/C", "exit", "0"])
            .spawn()
    } else {
        Command::new("/bin/sh").args(["-c", "exit 0"]).spawn()
    }
    .expect("short process");
    let dead_pid = exited.id();
    exited.wait().expect("process exits");
    std::fs::write(
        dead_path.with_extension("owner.json"),
        serde_json::json!({ "pid": dead_pid }).to_string(),
    )
    .expect("dead marker");
    std::fs::write(
        live_path.with_extension("owner.json"),
        serde_json::json!({ "pid": std::process::id() }).to_string(),
    )
    .expect("live marker");

    cleanup_orphans(repository.path().to_path_buf()).await;

    assert!(!dead_path.exists(), "dead owner's worktree is orphaned");
    assert!(live_path.exists(), "a live owner must never be disturbed");

    git(
        repository.path(),
        &[
            "worktree",
            "remove",
            "--force",
            live_path.to_str().expect("utf-8 path"),
        ],
    );
    let _ = std::fs::remove_file(live_path.with_extension("owner.json"));
}

/// Phase D2 put the child-run command vocabulary on the wire before the
/// execution exists. This is the guard that proves the gap is a typed refusal
/// rather than the `todo!()` panic it shipped as originally: every child-run
/// command is answered with `WorkspaceCapabilityUnavailable`, and the runtime
/// stays alive to refuse the next one.
#[tokio::test]
async fn child_run_commands_are_refused_not_panicked() {
    let store = Arc::new(InMemoryEventStore::new());
    let runtime = Runtime::spawn(Vec::new(), Arc::clone(&store) as Arc<dyn EventStore>);

    let commands = [
        SmedCommand::CreateWorktree {
            name: "child".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        SmedCommand::ForkWork {
            name: "child".to_owned(),
            base_revision: "HEAD".to_owned(),
        },
        SmedCommand::StartChild {
            name: "child".to_owned(),
            directive: "implement the feature".to_owned(),
            policy_ceiling: None,
            budget: None,
        },
        SmedCommand::CancelChild {
            name: "child".to_owned(),
        },
        SmedCommand::PreserveBranch {
            name: "child".to_owned(),
        },
        SmedCommand::SettleChild {
            name: "child".to_owned(),
        },
        SmedCommand::DiscardSettledWorktree {
            name: "child".to_owned(),
        },
    ];

    for command in commands {
        let error = runtime
            .dispatch(command)
            .await
            .expect_err("a child-run command must be refused while execution is unimplemented");
        assert_eq!(
            error.reason_code(),
            Some(ReasonCode::WorkspaceCapabilityUnavailable),
            "the refusal must carry the typed code: {error}"
        );
        assert!(
            error.to_string().contains("nothing ran"),
            "the refusal must say plainly that nothing happened: {error}"
        );
    }

    // The runtime survived: a subsequent command is answered, not lost to a
    // crashed actor.
    runtime.close().await.expect("runtime closes cleanly");
}
