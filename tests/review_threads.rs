//! Anchored review threads and "send to smed" (Phase D3).
//!
//! One reason to change: whether a human's line note is pinned to the diff it
//! was written against, stays pinned across a restart, and reaches smed as a
//! durable request that claims nothing it has not earned.
//!
//! Everything here goes through `ClientBridge`, not the runtime directly, so
//! the wire validation, the command mapping, the anchor resolver, and the
//! projection are all in the path of every assertion. A correct thread in actor
//! state behind a projection nobody wired is the failure this repository has
//! shipped once already.

// `allow-expect-in-tests` covers `#[test]` bodies, not the free helpers these
// tests share. Same allowance, same reason (AGENTS.md §7).
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use smed::core::client::workspace::ReviewThreadSummary;
use smed::core::client::{ClientCommand, ClientMessage, ClientReviewSide, ClientSnapshot};
use smed::core::error::ReasonCode;
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::providers::fake::FakeProvider;
use smed::runtime::Runtime;
use smed::runtime::client_bridge::ClientBridge;
use smed::store::memory::InMemoryEventStore;

/// A bridge over a live runtime, plus the store both share.
struct Harness {
    bridge: ClientBridge,
    runtime: Arc<Runtime>,
}

impl Harness {
    fn start(store: &Arc<InMemoryEventStore>) -> Self {
        let runtime = Arc::new(Runtime::spawn(
            vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
            Arc::clone(store) as Arc<dyn EventStore>,
        ));
        let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn SmedRuntime>);
        Self { bridge, runtime }
    }

    fn snapshot(&self) -> ClientSnapshot {
        self.bridge.snapshot()
    }

    fn threads(&self) -> Vec<ReviewThreadSummary> {
        self.snapshot().review_threads.items
    }

    /// The digest the human is looking at, read from what the client actually
    /// received — the only place a real client could get it from.
    fn digest(&self) -> String {
        self.snapshot()
            .changes
            .expect("a captured change set")
            .capture_digest
    }

    async fn open_session(&self, root: &Path) {
        self.bridge
            .dispatch(ClientCommand::OpenProject {
                root: root.display().to_string(),
            })
            .await
            .expect("open project");
        self.bridge
            .dispatch(ClientCommand::CreateSession {
                provider: FakeProvider::ID.to_owned(),
                model: FakeProvider::MODEL.to_owned(),
            })
            .await
            .expect("create session");
        self.await_session().await;
    }

    async fn await_session(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.snapshot().session.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a session");
    }

    /// Take one note on line 2 of the new side — the changed line in the
    /// fixture repository.
    async fn add_note(&self, body: &str) -> Result<(), ReasonCode> {
        self.note_at(2, ClientReviewSide::New, &self.digest(), body)
            .await
    }

    async fn note_at(
        &self,
        line: u32,
        side: ClientReviewSide,
        digest: &str,
        body: &str,
    ) -> Result<(), ReasonCode> {
        self.bridge
            .dispatch(ClientCommand::AddReviewNote {
                path: "README.md".to_owned(),
                side,
                line,
                capture_digest: digest.to_owned(),
                body: body.to_owned(),
            })
            .await
            .map_err(|error| error.reason_code().expect("a typed refusal"))
    }

    async fn close(self) {
        let Self { bridge, runtime } = self;
        bridge.close().await.expect("close the bridge");
        drop(runtime);
    }
}

fn setup_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("smed-d3-review-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical temp dir");

    git(&dir, &["init", "--initial-branch=main"]);
    git(&dir, &["config", "user.email", "test@smed.invalid"]);
    git(&dir, &["config", "user.name", "smed Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);

    fs::write(dir.join("README.md"), "one\ntwo\nthree\n").expect("write");
    git(&dir, &["add", "README.md"]);
    git(&dir, &["commit", "-m", "init"]);

    // A change to review: line 2 differs on both sides.
    fs::write(dir.join("README.md"), "one\nTWO\nthree\n").expect("write");
    dir
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn a_note_is_pinned_to_the_line_and_the_hunk_smed_captured() {
    let dir = setup_repo("pinned");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;

    harness.add_note("this line needs a comment").await.unwrap();

    let thread = harness.threads().into_iter().next().expect("one thread");
    assert_eq!(thread.anchor.path, "README.md");
    assert_eq!(thread.anchor.line, 2);
    assert_eq!(thread.anchor.side, "new");
    // The hunk context comes from smed's capture, not from the client — which
    // never sent one. That is what stops a note describing a diff that did not
    // exist.
    assert!(
        thread.anchor.hunk_header.starts_with("@@"),
        "expected a real hunk header, got {:?}",
        thread.anchor.hunk_header
    );
    assert_eq!(thread.anchor.capture_digest, harness.digest());
    assert!(!thread.anchor_stale);
    assert_eq!(thread.status, "open");
    assert_eq!(thread.comment_count, 1);
    assert_eq!(
        thread.comments.first().map(|c| c.body.as_str()),
        Some("this line needs a comment")
    );
    // A human's remark about code, not a smed-governed observation.
    assert_eq!(
        thread.trust,
        smed::core::client::workspace::TrustClass::OperatorControlled
    );
    assert!(thread.response_message_id.is_none());

    harness.close().await;
}

/// §D3: "a diff whose base changed is marked stale and cannot accept a line
/// note as if current." The refusal is typed, and nothing is recorded — a
/// half-taken note would be worse than none.
#[tokio::test]
async fn a_note_against_a_moved_diff_is_refused_and_nothing_is_recorded() {
    let dir = setup_repo("stale-refusal");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;
    let stale = harness.digest();

    // The tree moves under the reviewer.
    fs::write(dir.join("README.md"), "one\nTWO\nTHREE\n").expect("write");
    harness
        .bridge
        .dispatch(ClientCommand::RefreshRepository)
        .await
        .expect("refresh");
    assert_ne!(harness.digest(), stale, "the capture must have moved");

    let refusal = harness
        .note_at(2, ClientReviewSide::New, &stale, "against the old diff")
        .await
        .expect_err("a stale digest must refuse");
    assert_eq!(refusal, ReasonCode::WorkspaceStaleDiff);
    assert!(
        harness.threads().is_empty(),
        "a refused note must leave no thread behind"
    );

    harness.close().await;
}

/// The other half of the same bullet: an existing note stays **visible** when
/// the diff moves, and it keeps the line it was taken against. Marked stale,
/// not relocated and not hidden.
#[tokio::test]
async fn an_existing_note_survives_a_moving_diff_without_changing_line() {
    let dir = setup_repo("stale-visible");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;

    harness.add_note("still about line 2").await.unwrap();
    let before = harness.threads().into_iter().next().expect("one thread");
    assert!(!before.anchor_stale);

    // Insert a line above the note's anchor. Whatever is on line 2 now, the
    // note is about what line 2 *was*.
    fs::write(dir.join("README.md"), "zero\none\nTWO\nthree\n").expect("write");
    harness
        .bridge
        .dispatch(ClientCommand::RefreshRepository)
        .await
        .expect("refresh");

    let after = harness
        .threads()
        .into_iter()
        .next()
        .expect("still one thread");
    assert!(
        after.anchor_stale,
        "a moved diff must mark the anchor stale"
    );
    assert_eq!(after.anchor.line, before.anchor.line);
    assert_eq!(after.anchor.side, before.anchor.side);
    assert_eq!(after.anchor.hunk_header, before.anchor.hunk_header);
    assert_eq!(after.anchor.capture_digest, before.anchor.capture_digest);
    assert_eq!(after.comments.len(), 1);

    harness.close().await;
}

#[tokio::test]
async fn a_line_the_diff_never_printed_is_refused() {
    let dir = setup_repo("unprinted-line");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;
    let digest = harness.digest();

    // Line 900 is in no hunk, and line 0 is not a diff line number at all.
    assert_eq!(
        harness
            .note_at(900, ClientReviewSide::New, &digest, "nowhere")
            .await
            .expect_err("an unprinted line must refuse"),
        ReasonCode::SchemaInvalid
    );
    assert_eq!(
        harness
            .note_at(0, ClientReviewSide::New, &digest, "nowhere")
            .await
            .expect_err("line zero must refuse"),
        ReasonCode::SchemaInvalid
    );
    assert!(harness.threads().is_empty());

    harness.close().await;
}

/// §D3: "notes survive restart, keep their original anchor, and link to the
/// resulting smed response." This covers the first two; the send test below
/// covers the third.
#[tokio::test]
async fn notes_survive_a_restart_with_their_original_anchor() {
    let dir = setup_repo("restart");
    let store = Arc::new(InMemoryEventStore::new());

    let first = Harness::start(&store);
    first.open_session(&dir).await;
    first.add_note("keep me across the restart").await.unwrap();
    let before = first.threads().into_iter().next().expect("one thread");
    let session = first.snapshot().session.expect("a session");
    first.close().await;

    let second = Harness::start(&store);
    second
        .bridge
        .dispatch(ClientCommand::ResumeSession {
            session: session.clone(),
        })
        .await
        .expect("resume");
    second.await_session().await;

    // Waits for the capture as well as the note: the first published snapshot
    // of a resumed session carries both, and a reader that accepted one without
    // the other would be asserting against a frame no client sees.
    let after = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = second.snapshot();
            if snapshot.changes.is_some()
                && let Some(thread) = snapshot.review_threads.items.into_iter().next()
            {
                return thread;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the note comes back");

    assert_eq!(after.id, before.id);
    assert_eq!(after.anchor.line, before.anchor.line);
    assert_eq!(after.anchor.side, before.anchor.side);
    assert_eq!(after.anchor.hunk_header, before.anchor.hunk_header);
    assert_eq!(after.anchor.capture_digest, before.anchor.capture_digest);
    assert_eq!(
        after.comments.first().map(|c| c.body.as_str()),
        Some("keep me across the restart")
    );
    // The working tree has not moved, and the resume re-read git, so the
    // restored note is not stale — proving the comparison survived too, not
    // just the text.
    assert!(!after.anchor_stale);

    second.close().await;
}

/// "Send to smed" is a durable human message referencing the thread ids, and
/// the thread links back to what smed answered with.
#[tokio::test]
async fn sending_notes_produces_a_durable_request_and_links_the_answer() {
    let dir = setup_repo("send");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;

    harness
        .add_note("please explain this change")
        .await
        .unwrap();
    let thread_id = harness.threads().into_iter().next().expect("one thread").id;

    harness
        .bridge
        .dispatch(ClientCommand::SendReviewNotes {
            thread_ids: vec![thread_id.clone()],
        })
        .await
        .expect("the request is sent");

    let answered = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(thread) = harness
                .threads()
                .into_iter()
                .find(|thread| thread.response_message_id.is_some())
            {
                return thread;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("smed answers the request");

    assert_eq!(answered.status, "sent");

    let snapshot = harness.snapshot();
    // The request is in the transcript as an ordinary human message, naming the
    // thread it carries — a durable revision request, not a side channel.
    let request = snapshot
        .messages
        .iter()
        .find_map(|message| match message {
            ClientMessage::User { text, .. } if text.contains(&thread_id) => Some(text.clone()),
            _ => None,
        })
        .expect("the review request reached the transcript");
    assert!(request.contains("README.md:2"));
    assert!(request.contains("please explain this change"));

    // And the link points at a message that is actually in the transcript.
    let response = answered.response_message_id.expect("a response id");
    assert!(
        snapshot.messages.iter().any(|message| matches!(
            message,
            ClientMessage::Assistant { id, .. } if *id == response
        )),
        "the linked response must be an assistant message a client can find"
    );

    harness.close().await;
}

/// Negative: sending an unknown thread refuses and marks nothing sent. The
/// failure this guards is a partially-applied send — some threads flagged,
/// no request behind them.
#[tokio::test]
async fn an_unknown_thread_in_a_send_refuses_and_marks_nothing() {
    let dir = setup_repo("unknown-thread");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;

    harness.add_note("a real note").await.unwrap();
    let real = harness.threads().into_iter().next().expect("one thread").id;

    let refusal = harness
        .bridge
        .dispatch(ClientCommand::SendReviewNotes {
            thread_ids: vec![real.clone(), uuid::Uuid::now_v7().to_string()],
        })
        .await
        .expect_err("an unknown thread must refuse");
    assert_eq!(refusal.reason_code(), Some(ReasonCode::SchemaInvalid));

    let thread = harness.threads().into_iter().next().expect("one thread");
    assert_eq!(
        thread.status, "open",
        "a refused send must not mark a thread sent"
    );
    assert!(thread.response_message_id.is_none());

    harness.close().await;
}

/// §D3 asks for negative tests against false promotion. A review thread may say
/// it was written and that a request naming it was sent; nothing on the wire may
/// suggest smed applied, imported, or verified the change.
#[tokio::test]
async fn nothing_a_thread_projects_claims_the_change_was_addressed() {
    let dir = setup_repo("no-promotion");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;

    harness.add_note("a note").await.unwrap();
    let id = harness.threads().into_iter().next().expect("one thread").id;
    harness
        .bridge
        .dispatch(ClientCommand::SendReviewNotes {
            thread_ids: vec![id],
        })
        .await
        .expect("sent");

    let wire = serde_json::to_string(&harness.snapshot().review_threads).expect("serialize");
    for forbidden in ["resolved", "applied", "verified", "imported", "proposed"] {
        assert!(
            !wire.contains(forbidden),
            "a review projection must not claim {forbidden}: {wire}"
        );
    }

    harness.close().await;
}

/// An over-long note is refused at the wire rather than stored and clamped
/// later: the body reaches the durable record and the directive built from it,
/// and a bound applied after storage bounds neither.
#[tokio::test]
async fn an_over_long_note_is_refused_at_the_wire() {
    let dir = setup_repo("over-long");
    let store = Arc::new(InMemoryEventStore::new());
    let harness = Harness::start(&store);
    harness.open_session(&dir).await;
    let digest = harness.digest();

    let body = "x".repeat(smed::core::client::MAX_REVIEW_NOTE_BYTES + 1);
    assert_eq!(
        harness
            .note_at(2, ClientReviewSide::New, &digest, &body)
            .await
            .expect_err("an over-long note must refuse"),
        ReasonCode::SchemaInvalid
    );
    assert!(harness.threads().is_empty());

    // And an empty one, which says nothing and would leave a marker on a line
    // for no stated reason.
    assert_eq!(
        harness
            .note_at(2, ClientReviewSide::New, &digest, "   ")
            .await
            .expect_err("an empty note must refuse"),
        ReasonCode::SchemaInvalid
    );

    harness.close().await;
}
