//! The session tree: rewinding, branching, and what survives a restart
//! (Pillar 1).
//!
//! # What these tests are for
//!
//! A branch is only worth anything if it is *durable*. Rewinding the active
//! leaf in memory is easy; the hard promise is that a session reopened tomorrow
//! resumes the branch the user left it on, and does not quietly resurrect the
//! messages they branched away from. That promise spans the store's parent
//! pointers, the branch-aware recovery read, and the projection — so it is
//! tested end to end through a real runtime and a real SQLite file, not at any
//! one of those layers alone.
//!
//! The abandoned branch is *retained*, never deleted. Several tests assert that
//! directly: "rewind" must mean "stop following", not "erase".

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely — a failing assertion is a failing test"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use smed::core::command::SmedCommand;
use smed::core::event::SmedEvent;
use smed::core::message::Role;
use smed::core::provider::Provider;
use smed::core::runtime::{RuntimeSnapshot, SmedRuntime};
use smed::core::store::EventStore;
use smed::providers::fake::{FakeProvider, FakeScript};
use smed::runtime::Runtime;
use smed::store::sqlite::SqliteEventStore;
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    database: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().expect("temp dir");
        let database = directory.path().join("smed.sqlite3");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        Self {
            _directory: directory,
            database,
            workspace,
        }
    }

    async fn store(&self) -> Arc<SqliteEventStore> {
        Arc::new(
            SqliteEventStore::open(&self.database)
                .await
                .expect("open database"),
        )
    }
}

fn runtime_for(store: &Arc<SqliteEventStore>) -> Runtime {
    let provider: Arc<dyn Provider> = Arc::new(FakeProvider::new(FakeScript::Text));
    Runtime::spawn(vec![provider], Arc::clone(store) as Arc<dyn EventStore>)
}

async fn settle(runtime: &Runtime, ready: impl Fn(&RuntimeSnapshot) -> bool) -> RuntimeSnapshot {
    for _ in 0..400 {
        let snapshot = runtime.snapshot();
        if ready(&snapshot) {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the runtime never reached the expected state");
}

/// Open a project, create a session, and say `text`, waiting for the reply.
async fn say(runtime: &Runtime, text: &str) -> RuntimeSnapshot {
    let before = runtime.snapshot().messages.len();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: text.to_owned(),
            source: smed::core::directive::DirectiveSource::Human,
        })
        .await
        .expect("dispatch");
    settle(runtime, |snapshot| {
        !snapshot.run_active && snapshot.messages.len() > before + 1
    })
    .await
}

fn user_texts(snapshot: &RuntimeSnapshot) -> Vec<String> {
    snapshot
        .messages
        .iter()
        .filter(|entry| entry.role == Role::User)
        .map(|entry| entry.text())
        .collect()
}

/// The sequence of the user message with this text, as `/tree` would read it.
fn anchor_of(snapshot: &RuntimeSnapshot, text: &str) -> u64 {
    snapshot
        .messages
        .iter()
        .find(|entry| entry.role == Role::User && entry.text() == text)
        .expect("the message must be in the transcript")
        .sequence
        .expect("a live user message must be anchored to its durable event")
}

#[tokio::test]
async fn every_live_message_is_anchored_to_the_event_that_introduced_it() {
    // The precondition for `/tree` existing at all: a client cannot name a
    // branch point it was never told the name of.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: smed::core::model::ProviderId::new(FakeProvider::ID),
            model: smed::core::model::ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    let snapshot = say(&runtime, "first").await;

    let anchors: Vec<Option<u64>> = snapshot
        .messages
        .iter()
        .map(|entry| entry.sequence)
        .collect();
    assert!(
        anchors.iter().all(Option::is_some),
        "every message from a live turn must carry its event's sequence, got {anchors:?}"
    );

    // And the anchors are the real sequences, not positions in the transcript.
    let events = store
        .events(snapshot.session.expect("session"))
        .await
        .expect("events");
    for entry in snapshot.messages.iter() {
        let sequence = entry.sequence.expect("anchored");
        let stored = events
            .iter()
            .find(|stored| stored.sequence == sequence)
            .expect("the anchor must name a real event");
        assert!(
            stored.event.introduces_message(),
            "an anchor must name an event that actually introduced a message, \
             not whichever event happened to sit at that sequence"
        );
    }

    let _ = runtime.close().await;
}

/// Drive a session to three user messages and rewind to the second.
async fn rewound_session(
    fixture: &Fixture,
    store: &Arc<SqliteEventStore>,
) -> smed::core::event::SessionId {
    let runtime = runtime_for(store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: smed::core::model::ProviderId::new(FakeProvider::ID),
            model: smed::core::model::ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    say(&runtime, "keep this").await;
    let snapshot = say(&runtime, "branch here").await;
    say(&runtime, "abandon this").await;

    let sequence = anchor_of(&snapshot, "branch here");
    runtime
        .dispatch(SmedCommand::RewindTo { sequence })
        .await
        .expect("rewind");
    let rewound = settle(&runtime, |snapshot| {
        !user_texts(snapshot).contains(&"branch here".to_owned())
    })
    .await;

    assert_eq!(
        user_texts(&rewound),
        vec!["keep this".to_owned()],
        "a rewind to a message drops that message and everything after it"
    );

    // The next message continues the new branch.
    say(&runtime, "instead, this").await;

    let session = rewound.session.expect("session");
    let _ = runtime.close().await;
    session
}

#[tokio::test]
async fn a_rewind_starts_a_sibling_and_keeps_the_abandoned_branch_in_the_store() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = rewound_session(&fixture, &store).await;

    // The branch reads as the new line...
    let branch = store.branch_events(session).await.expect("branch");
    let said: Vec<String> = branch
        .iter()
        .filter_map(|stored| match &stored.event {
            SmedEvent::MessageAppended { message, .. } if message.role == Role::User => {
                Some(message.text())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        said,
        vec!["keep this".to_owned(), "instead, this".to_owned()]
    );

    // ...while the whole tree, abandoned sibling included, is still on disk.
    let all = store.events(session).await.expect("events");
    let every_text: Vec<String> = all
        .iter()
        .filter_map(|stored| match &stored.event {
            SmedEvent::MessageAppended { message, .. } if message.role == Role::User => {
                Some(message.text())
            }
            _ => None,
        })
        .collect();
    for abandoned in ["branch here", "abandon this"] {
        assert!(
            every_text.contains(&abandoned.to_owned()),
            "a rewind must stop following the old branch, not erase it; \
             `{abandoned}` is missing from {every_text:?}"
        );
    }
}

#[tokio::test]
async fn a_rewind_survives_a_restart() {
    // The promise that makes branching worth having. Resume reads the branch,
    // not the linear history: if it read linearly, the abandoned messages would
    // walk back into the transcript and straight into the next provider request.
    let fixture = Fixture::new();
    let session = {
        let store = fixture.store().await;
        rewound_session(&fixture, &store).await
    };

    let store = fixture.store().await;
    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let resumed = settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    assert_eq!(
        user_texts(&resumed),
        vec!["keep this".to_owned(), "instead, this".to_owned()],
        "a resumed session must restore the branch it was left on"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_resumed_transcript_is_anchored_the_same_way_a_live_one_is() {
    // `/tree` must work after a restart, which means the restored entries carry
    // the same sequences the live ones did — including the entries a checkpoint
    // restored, which the checkpoint itself does not store.
    let fixture = Fixture::new();
    let session = {
        let store = fixture.store().await;
        rewound_session(&fixture, &store).await
    };

    let store = fixture.store().await;
    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let resumed = settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    let anchors: Vec<Option<u64>> = resumed
        .messages
        .iter()
        .map(|entry| entry.sequence)
        .collect();
    assert!(
        anchors.iter().all(Option::is_some),
        "a resumed transcript must offer the same branch points a live one does, got {anchors:?}"
    );

    // And each anchor is on the branch that was restored, not on the sibling.
    let branch: std::collections::BTreeSet<u64> = store
        .branch_events(session)
        .await
        .expect("branch")
        .iter()
        .map(|stored| stored.sequence)
        .collect();
    for anchor in anchors.into_iter().flatten() {
        assert!(
            branch.contains(&anchor),
            "sequence {anchor} anchors a restored message to an event on the abandoned branch"
        );
    }

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_rewind_survives_a_crash_with_no_covering_checkpoint() {
    // The clean-close path above is covered by a checkpoint written from live
    // state, which is already branch-correct. This is the case that actually
    // exercises the recovery read: history on disk, no checkpoint, a branch
    // point in the middle. Replayed linearly, the abandoned messages walk back
    // into the transcript and into the next provider request.
    //
    // The history is written directly for the same reason `persistence_recovery`
    // does it: "what must resume do given these events?" must not depend on how
    // the events came to exist, and killing a live actor mid-turn would race the
    // thing it was trying to interrupt (`AGENTS.md` §7).
    use smed::core::event::SessionId;
    use smed::core::message::CanonicalMessage;
    use smed::core::model::{ModelId, ProviderId};

    let fixture = Fixture::new();
    let store = fixture.store().await;

    let project = store
        .open_project(fixture.workspace.clone())
        .await
        .expect("project");
    let session = SessionId::new();
    store
        .create_session(session, project, "test".to_owned(), None)
        .await
        .expect("session");

    let said = |text: &str| SmedEvent::MessageAppended {
        session,
        message: Box::new(CanonicalMessage::user(text)),
    };

    store
        .append(SmedEvent::SessionCreated {
            session,
            provider: ProviderId::new(FakeProvider::ID),
            model: ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("append");
    let keep = store.append(said("keep this")).await.expect("append");
    store.append(said("abandon this")).await.expect("append");

    // The rewind: the next event branches from `keep` rather than continuing.
    store
        .set_active_leaf(session, Some(keep.sequence))
        .await
        .expect("leaf");
    store
        .append_after(said("instead, this"), Some(keep.sequence))
        .await
        .expect("append after");

    assert!(
        store
            .latest_checkpoint(session)
            .await
            .expect("checkpoint")
            .is_none(),
        "this test is only meaningful with no checkpoint to fall back on"
    );

    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    let resumed = settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    assert_eq!(
        user_texts(&resumed),
        vec!["keep this".to_owned(), "instead, this".to_owned()],
        "resume replays the branch, not the linear history"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn the_tree_shows_the_abandoned_branch_beside_the_one_being_followed() {
    // What `/tree` needs in order to be a tree. The active branch alone cannot
    // answer "what did I branch away from?", so the read walks the whole event
    // tree and marks which turns are on the branch the session is following.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = rewound_session(&fixture, &store).await;

    let tree = store.session_tree(session).await.expect("tree");
    let shape: Vec<(&str, bool)> = tree
        .iter()
        .map(|node| (node.prompt.as_str(), node.on_active_branch))
        .collect();

    assert_eq!(
        shape,
        vec![
            ("keep this", true),
            ("branch here", false),
            ("abandon this", false),
            ("instead, this", true),
        ],
        "every turn is listed; only the ones on the followed branch are marked"
    );

    // And the two branches hang off the same turn, which is what makes the
    // branch point visible rather than implied.
    let parent_of = |prompt: &str| {
        tree.iter()
            .find(|node| node.prompt == prompt)
            .expect("turn")
            .parent
    };
    assert_eq!(
        parent_of("branch here"),
        parent_of("instead, this"),
        "the abandoned turn and its replacement must share a parent"
    );
}

#[tokio::test]
async fn a_turn_parents_to_the_previous_turn_across_the_tool_traffic_between_them() {
    // The tree is a tree of *turns*, not of events. Assistant replies, tool
    // proposals, and results sit between two user messages; if a turn parented
    // to the immediately preceding event, every turn would look like a child of
    // a tool result and the tree would be a straight line of noise.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: smed::core::model::ProviderId::new(FakeProvider::ID),
            model: smed::core::model::ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;

    say(&runtime, "one").await;
    let snapshot = say(&runtime, "two").await;
    let session = snapshot.session.expect("session");
    let _ = runtime.close().await;

    let tree = store.session_tree(session).await.expect("tree");
    assert_eq!(tree.len(), 2);
    let first = tree.first().expect("first turn");
    let second = tree.get(1).expect("second turn");

    assert_eq!(first.parent, None, "the opening turn has no parent turn");
    assert_eq!(
        second.parent,
        Some(first.sequence),
        "the second turn parents to the first, not to whatever event preceded it"
    );
    assert!(
        second.sequence > first.sequence + 1,
        "this test is only meaningful with events between the two turns"
    );
}

#[tokio::test]
async fn following_an_abandoned_branch_restores_it_without_writing_anything() {
    // The inverse of a rewind, and the reason the abandoned branch is retained
    // rather than deleted. Returning to a branch is not a new fact about the
    // session, so it appends nothing — it only changes which branch is read.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = rewound_session(&fixture, &store).await;

    let tree = store.session_tree(session).await.expect("tree");
    let abandoned = tree
        .iter()
        .find(|node| node.prompt == "abandon this")
        .expect("the abandoned turn");

    let events_before = store.events(session).await.expect("events").len();

    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    runtime
        .dispatch(SmedCommand::FollowBranch {
            sequence: abandoned.sequence,
        })
        .await
        .expect("follow branch");
    let followed = settle(&runtime, |snapshot| {
        user_texts(snapshot).contains(&"abandon this".to_owned())
    })
    .await;

    assert_eq!(
        user_texts(&followed),
        vec![
            "keep this".to_owned(),
            "branch here".to_owned(),
            "abandon this".to_owned()
        ],
        "following a branch restores the whole line ending at that turn"
    );
    assert_eq!(
        store.events(session).await.expect("events").len(),
        events_before,
        "returning to a branch is a read, not a new event"
    );

    // The tree on the snapshot follows the move rather than going stale.
    assert!(
        followed
            .tree
            .iter()
            .any(|node| node.prompt == "abandon this" && node.on_active_branch),
        "the turn just followed must now read as being on the active branch"
    );
    assert!(
        followed
            .tree
            .iter()
            .any(|node| node.prompt == "instead, this" && !node.on_active_branch),
        "and the branch just left must read as abandoned"
    );

    let _ = runtime.close().await;
}

/// Drive a session to two turns and return its id plus the runtime still open
/// on it.
async fn two_turn_session(fixture: &Fixture, store: &Arc<SqliteEventStore>) -> Runtime {
    let runtime = runtime_for(store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::CreateSession {
            provider: smed::core::model::ProviderId::new(FakeProvider::ID),
            model: smed::core::model::ModelId::new(FakeProvider::MODEL),
        })
        .await
        .expect("create session");
    settle(&runtime, |snapshot| snapshot.session.is_some()).await;
    say(&runtime, "first").await;
    say(&runtime, "second").await;
    runtime
}

#[tokio::test]
async fn a_clone_carries_the_whole_branch_into_a_session_of_its_own() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = two_turn_session(&fixture, &store).await;
    let original = runtime.snapshot().session.expect("session");

    runtime
        .dispatch(SmedCommand::ForkSession { before: None })
        .await
        .expect("clone");
    // The new session id appears before its carried history does, so settle on
    // the history rather than racing the moment in between.
    let cloned = settle(&runtime, |snapshot| {
        snapshot.session.is_some_and(|session| session != original)
            && user_texts(snapshot).len() == 2
    })
    .await;

    assert_eq!(
        user_texts(&cloned),
        vec!["first".to_owned(), "second".to_owned()],
        "a clone carries the whole branch"
    );

    // The clone's history is its own events, not a pointer at the original's.
    let session = cloned.session.expect("session");
    let events = store.events(session).await.expect("events");
    assert!(
        events.iter().any(
            |stored| matches!(&stored.event, SmedEvent::MessageAppended { message, .. }
                if message.text() == "first")
        ),
        "the carried messages must be this session's own durable events"
    );
    // And every entry is anchored to one of them, not to the original's numbers.
    let own: std::collections::BTreeSet<u64> =
        events.iter().map(|stored| stored.sequence).collect();
    for entry in cloned.messages.iter() {
        let sequence = entry.sequence.expect("a carried entry must be anchored");
        assert!(
            own.contains(&sequence),
            "entry anchored to sequence {sequence}, which is not an event in this session"
        );
    }

    // The original is untouched and still readable.
    let original_tree = store.session_tree(original).await.expect("tree");
    assert_eq!(
        original_tree.len(),
        2,
        "cloning must not disturb the source"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_fork_cuts_where_a_rewind_would_and_leaves_the_original_alone() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = two_turn_session(&fixture, &store).await;
    let snapshot = runtime.snapshot();
    let original = snapshot.session.expect("session");
    let cut = anchor_of(&snapshot, "second");

    runtime
        .dispatch(SmedCommand::ForkSession { before: Some(cut) })
        .await
        .expect("fork");
    let forked = settle(&runtime, |snapshot| {
        snapshot.session.is_some_and(|session| session != original)
            && user_texts(snapshot).len() == 1
    })
    .await;

    assert_eq!(
        user_texts(&forked),
        vec!["first".to_owned()],
        "forking at a turn starts just before it was said, as a rewind does"
    );

    // The source keeps both turns: that is the whole difference from a rewind.
    let original_tree = store.session_tree(original).await.expect("tree");
    let prompts: Vec<&str> = original_tree
        .iter()
        .map(|node| node.prompt.as_str())
        .collect();
    assert_eq!(
        prompts,
        vec!["first", "second"],
        "a fork leaves the session it came from exactly as it was"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn a_fork_cannot_launder_a_narrow_policy_into_a_wide_one() {
    // The rule that makes forking safe to offer. A new session is a new place
    // for authority to appear from nowhere, and read-only must stay read-only
    // rather than resetting to the default.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = two_turn_session(&fixture, &store).await;
    let original = runtime.snapshot().session.expect("session");

    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: smed::core::policy::PolicyMode::ReadOnly,
        })
        .await
        .expect("set policy");
    settle(&runtime, |snapshot| {
        snapshot.policy == smed::core::policy::PolicyMode::ReadOnly
    })
    .await;

    runtime
        .dispatch(SmedCommand::ForkSession { before: None })
        .await
        .expect("clone");
    let cloned = settle(&runtime, |snapshot| {
        snapshot.session.is_some_and(|session| session != original)
    })
    .await;

    assert_eq!(
        cloned.policy,
        smed::core::policy::PolicyMode::ReadOnly,
        "a fork must not widen the policy back to the default"
    );

    let _ = runtime.close().await;
}

/// The same rule, checked against every snapshot the runtime publishes rather
/// than one polled sample.
///
/// `create_session` resets state to defaults and then restores the carried
/// policy. While the restore happened *after* the `SessionCreated` append, that
/// append's own snapshot paired the new session with the wider default — a
/// narrowed policy visibly widening itself, which §11.4 does not permit even
/// for an instant. The sampling test above caught it only when its poll landed
/// inside the window, which is why it read as a flake.
///
/// This observer is honest about its own limit: `watch` coalesces, so it is not
/// guaranteed to see every intermediate state. It cannot prove the window is
/// absent; it can only fail when the window is present and it happened to land
/// there. Both tests are kept because they sample differently.
#[tokio::test]
async fn no_published_snapshot_pairs_a_forked_session_with_a_widened_policy() {
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = two_turn_session(&fixture, &store).await;
    let original = runtime.snapshot().session.expect("session");

    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: smed::core::policy::PolicyMode::ReadOnly,
        })
        .await
        .expect("set policy");
    settle(&runtime, |snapshot| {
        snapshot.policy == smed::core::policy::PolicyMode::ReadOnly
    })
    .await;

    // Subscribe *before* the fork so the observer is watching for the whole
    // transition rather than joining part-way through it.
    let mut snapshots = runtime.snapshots();
    let observer = tokio::spawn(async move {
        let mut widened = Vec::new();
        while let Ok(snapshot) = snapshots.changed().await {
            if let Some(session) = snapshot.session
                && session != original
                && snapshot.policy != smed::core::policy::PolicyMode::ReadOnly
            {
                widened.push(snapshot.policy);
            }
        }
        widened
    });

    runtime
        .dispatch(SmedCommand::ForkSession { before: None })
        .await
        .expect("fork");
    settle(&runtime, |snapshot| {
        snapshot.session.is_some_and(|session| session != original)
    })
    .await;

    let _ = runtime.close().await;
    let widened = observer.await.expect("observer finished");
    assert!(
        widened.is_empty(),
        "the forked session was published at a wider policy than it inherited: {widened:?}"
    );
}

#[tokio::test]
async fn a_fork_does_not_inherit_unattended_autonomy() {
    // Full-auto is armed by a human for a stretch of work they are watching.
    // A forked session is not that stretch of work.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let runtime = two_turn_session(&fixture, &store).await;
    let original = runtime.snapshot().session.expect("session");

    runtime
        .dispatch(SmedCommand::SetPolicy {
            mode: smed::core::policy::PolicyMode::FullAuto,
        })
        .await
        .expect("set policy");
    settle(&runtime, |snapshot| snapshot.policy.is_full_auto()).await;

    runtime
        .dispatch(SmedCommand::ForkSession { before: None })
        .await
        .expect("clone");
    let cloned = settle(&runtime, |snapshot| {
        snapshot.session.is_some_and(|session| session != original)
    })
    .await;

    assert!(
        !cloned.policy.is_full_auto(),
        "full-auto must be re-armed by a human, never inherited across a fork"
    );

    let _ = runtime.close().await;
}

#[tokio::test]
async fn leaving_a_branch_reports_what_was_on_it_without_calling_a_model() {
    // The deterministic branch summary. Every field is read
    // off events that were already written, which is what makes it both free
    // and incapable of describing work that did not happen.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = rewound_session(&fixture, &store).await;

    // The abandoned branch, summarised from its own diverged segment.
    let tree = store.session_tree(session).await.expect("tree");
    let abandoned = tree
        .iter()
        .find(|node| node.prompt == "abandon this")
        .expect("the abandoned turn");
    let summary = store
        .branch_summary(session, abandoned.sequence)
        .await
        .expect("summary");

    assert_eq!(
        summary.origin.as_deref(),
        Some("branch here"),
        "the summary starts at the turn the branch diverged on, \
         not at the beginning of the session"
    );
    assert_eq!(
        summary.turns, 2,
        "only the diverged segment counts; the shared prefix is on both branches"
    );
    assert!(!summary.is_empty());
}

#[tokio::test]
async fn a_summary_covers_the_diverged_segment_and_not_the_shared_prefix() {
    // The distinction that makes the summary worth reading: "keep this" is on
    // both branches, so reporting it would bury the difference in history the
    // user never left.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = rewound_session(&fixture, &store).await;

    let tree = store.session_tree(session).await.expect("tree");
    let abandoned = tree
        .iter()
        .find(|node| node.prompt == "abandon this")
        .expect("turn");
    let summary = store
        .branch_summary(session, abandoned.sequence)
        .await
        .expect("summary");

    assert_ne!(
        summary.origin.as_deref(),
        Some("keep this"),
        "the shared prefix must not appear as this branch's origin"
    );
    assert_eq!(
        summary.turns, 2,
        "two turns diverged; the third is shared and must not be counted"
    );
}

#[tokio::test]
async fn switching_away_puts_the_summary_on_the_snapshot() {
    // Surfaced at the moment of the switch, so work being left behind is
    // stated rather than just scrolling off the screen.
    let fixture = Fixture::new();
    let store = fixture.store().await;
    let session = rewound_session(&fixture, &store).await;

    let runtime = runtime_for(&store);
    runtime
        .dispatch(SmedCommand::OpenProject {
            root: fixture.workspace.clone(),
        })
        .await
        .expect("open project");
    runtime
        .dispatch(SmedCommand::ResumeSession { session })
        .await
        .expect("resume");
    settle(&runtime, |snapshot| snapshot.session == Some(session)).await;

    let tree = store.session_tree(session).await.expect("tree");
    let abandoned = tree
        .iter()
        .find(|node| node.prompt == "abandon this")
        .expect("turn");

    runtime
        .dispatch(SmedCommand::FollowBranch {
            sequence: abandoned.sequence,
        })
        .await
        .expect("follow");
    let followed = settle(&runtime, |snapshot| snapshot.left_branch.is_some()).await;

    let left = followed.left_branch.expect("a summary of what was left");
    assert_eq!(
        left.origin.as_deref(),
        Some("instead, this"),
        "the summary describes the branch walked away from, not the one arrived at"
    );

    let _ = runtime.close().await;
}
