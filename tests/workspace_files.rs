//! Contained, paginated file projections (Phase D7 read producer).
//!
//! One reason to change: whether a client can browse and open the open
//! project's files without any read escaping the workspace, and without a
//! binary or over-limit file reaching an editor.
//!
//! Everything here goes through `ClientBridge`, not `crate::workspace_files`
//! directly, so the wire validation, the actor hop, the containment recheck,
//! the git-ignore composition, and the projection are all in the path of every
//! assertion. A correct producer behind a projection nobody wired is the
//! failure this repository has shipped once already.

// `allow-expect-in-tests` covers `#[test]` bodies, not the free helpers these
// tests share. Same allowance, same reason (AGENTS.md §7).
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use smed::core::client::ClientCommand;
use smed::core::client::workspace::{
    DirectoryEntryView, DirectoryPage, FileContentView, FileModeView, FileOpenView, TrustClass,
};
use smed::core::error::ReasonCode;
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::store::EventStore;
use smed::core::workspace_files::MAX_EDITABLE_FILE_BYTES;
use smed::providers::fake::FakeProvider;
use smed::runtime::Runtime;
use smed::runtime::client_bridge::ClientBridge;
use smed::store::memory::InMemoryEventStore;

struct Harness {
    bridge: ClientBridge,
    runtime: Arc<Runtime>,
    store: Arc<InMemoryEventStore>,
}

impl Harness {
    fn start() -> Self {
        let store = Arc::new(InMemoryEventStore::new());
        let runtime = Arc::new(Runtime::spawn(
            vec![Arc::new(FakeProvider::default()) as Arc<dyn Provider>],
            Arc::clone(&store) as Arc<dyn EventStore>,
        ));
        let bridge = ClientBridge::start(Arc::clone(&runtime) as Arc<dyn SmedRuntime>);
        Self {
            bridge,
            runtime,
            store,
        }
    }

    async fn open(&self, root: &Path) {
        self.bridge
            .dispatch(ClientCommand::OpenProject {
                root: root.display().to_string(),
            })
            .await
            .expect("open project");
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.bridge.snapshot().workspace_root.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a project root");
    }

    async fn list(&self, path: &str) -> DirectoryPage {
        self.bridge
            .list_directory(path, 0)
            .await
            .expect("a directory page")
    }

    async fn list_refused(&self, path: &str, page: u32) -> ReasonCode {
        self.bridge
            .list_directory(path, page)
            .await
            .expect_err("must refuse")
            .reason_code()
            .expect("a typed refusal")
    }

    async fn open_file(&self, path: &str) -> FileOpenView {
        self.bridge.open_file(path).await.expect("a file")
    }

    async fn create_session(&self) {
        self.bridge
            .dispatch(ClientCommand::CreateSession {
                provider: FakeProvider::ID.to_owned(),
                model: FakeProvider::MODEL.to_owned(),
            })
            .await
            .expect("create session");
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.bridge.snapshot().session.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a session id");
    }

    async fn open_refused(&self, path: &str) -> ReasonCode {
        self.bridge
            .open_file(path)
            .await
            .expect_err("must refuse")
            .reason_code()
            .expect("a typed refusal")
    }

    async fn close(self) {
        let Self {
            bridge, runtime, ..
        } = self;
        bridge.close().await.expect("close the bridge");
        drop(runtime);
    }
}

fn entry<'a>(page: &'a DirectoryPage, name: &str) -> &'a DirectoryEntryView {
    page.entries
        .items
        .iter()
        .find(|candidate| candidate.name == name)
        .unwrap_or_else(|| panic!("no entry named {name} in {:?}", page.entries.items))
}

fn setup_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("smed-d7-files-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical temp dir");

    git(&dir, &["init", "--initial-branch=main"]);
    git(&dir, &["config", "user.email", "test@smed.invalid"]);
    git(&dir, &["config", "user.name", "smed Test"]);
    git(&dir, &["config", "commit.gpgsign", "false"]);

    fs::create_dir_all(dir.join("src")).expect("src");
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").expect("write");
    fs::write(dir.join("README.md"), "hello\n").expect("write");
    fs::write(dir.join(".gitignore"), "target/\n").expect("write");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
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

// ---------------------------------------------------------------------------
// The listing reaches a client at all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_client_lists_the_project_root_and_its_children() {
    let dir = setup_repo("list");
    let harness = Harness::start();
    harness.open(&dir).await;

    let root = harness.list("").await;
    assert_eq!(root.path, "");
    assert_eq!(root.trust, TrustClass::OperatorControlled);
    assert!(!root.has_more);
    assert_eq!(entry(&root, "src").kind, "directory");
    assert_eq!(entry(&root, "README.md").kind, "file");
    // Directories before files, which is the order ADR-0009's explorer draws
    // and the one `read_dir` promises nothing about.
    assert_eq!(root.entries.items[0].kind, "directory");

    let nested = harness.list("src").await;
    assert_eq!(nested.path, "src");
    assert_eq!(entry(&nested, "main.rs").path, "src/main.rs");

    harness.close().await;
}

#[tokio::test]
async fn a_client_save_refuses_stale_bytes_records_the_effect_and_refreshes_changes() {
    let dir = setup_repo("save");
    let harness = Harness::start();
    harness.open(&dir).await;
    harness.create_session().await;

    let opened = harness.open_file("README.md").await;
    let session = harness.runtime.snapshot().session.expect("session");
    harness
        .bridge
        .dispatch(ClientCommand::SaveFile {
            path: "README.md".to_owned(),
            expected_digest: opened.digest.clone(),
            text: "saved by the operator\n".to_owned(),
        })
        .await
        .expect("save");

    assert_eq!(
        fs::read_to_string(dir.join("README.md")).expect("saved file"),
        "saved by the operator\n"
    );
    let snapshot = harness.bridge.snapshot();
    let changes = snapshot
        .changes
        .as_ref()
        .expect("save must refresh the Changes view");
    assert!(changes.read_evidence.is_empty());
    assert!(matches!(
        snapshot.repository.freshness,
        smed::core::client::workspace::RepositoryFreshness::CapturedAt {
            ref trigger,
            ..
        } if trigger == "fileSave"
    ));

    let events = harness.store.events(session).await.expect("events");
    assert!(events.iter().any(|stored| matches!(
        &stored.event,
        smed::core::event::SmedEvent::FileSaved { path, .. } if path == "README.md"
    )));

    fs::write(dir.join("README.md"), "changed outside smed\n").expect("external edit");
    let error = harness
        .bridge
        .dispatch(ClientCommand::SaveFile {
            path: "README.md".to_owned(),
            expected_digest: opened.digest,
            text: "must not overwrite\n".to_owned(),
        })
        .await
        .expect_err("stale save");
    assert_eq!(error.reason_code(), Some(ReasonCode::StaleFileVersion));
    assert_eq!(
        fs::read_to_string(dir.join("README.md")).expect("newer file"),
        "changed outside smed\n"
    );

    harness.close().await;
}

/// The E9/D7 listing acceptance: measure the real bridge path on a full page
/// rather than reporting a projection-only number. This is opt-in because the
/// result is evidence for the report, not a threshold that should make a
/// default test suite machine-dependent.
#[tokio::test]
#[ignore = "opt-in latency measurement; run with --ignored --nocapture"]
async fn measure_directory_listing_latency_on_two_hundred_entries() {
    const ENTRIES: usize = 200;
    const SAMPLES: usize = 100;

    let dir = setup_repo("listing-latency");
    for index in 0..ENTRIES {
        fs::write(
            dir.join(format!("entry-{index:03}.txt")),
            format!("entry {index}\n"),
        )
        .expect("write benchmark entry");
    }

    let harness = Harness::start();
    harness.open(&dir).await;
    let first = harness.list("").await;
    assert!(
        first.entries.items.len() >= ENTRIES,
        "the benchmark must exercise a full directory page"
    );

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = std::time::Instant::now();
        let page = harness.list("").await;
        samples.push(started.elapsed());
        assert_eq!(page.entries.items.len(), first.entries.items.len());
    }

    samples.sort_unstable();
    let percentile = |numerator: usize, denominator: usize| {
        let position = samples
            .len()
            .saturating_mul(numerator)
            .saturating_add(denominator - 1)
            / denominator;
        samples
            .get(position.saturating_sub(1))
            .copied()
            .unwrap_or_default()
    };

    println!("--- Phase E9 directory listing latency ---");
    println!("entries: {ENTRIES}");
    println!("samples: {SAMPLES}");
    println!("p50: {:?}", percentile(50, 100));
    println!("p95: {:?}", percentile(95, 100));
    println!("max: {:?}", samples.last().copied().unwrap_or_default());

    harness.close().await;
}

/// The refusal a client hits before opening anything. Without a project there
/// is no containment boundary to check against, so a read would have to run
/// against the process's current directory — which is the whole failure.
#[tokio::test]
async fn reading_files_without_an_open_project_is_refused() {
    let harness = Harness::start();

    assert_eq!(
        harness.list_refused("", 0).await,
        ReasonCode::WorkspaceCapabilityUnavailable
    );
    assert_eq!(
        harness.open_refused("src/main.rs").await,
        ReasonCode::WorkspaceCapabilityUnavailable
    );

    harness.close().await;
}

// ---------------------------------------------------------------------------
// Containment, through the whole stack
// ---------------------------------------------------------------------------

/// §D7: "containment is rechecked immediately before read and save; symlink
/// escapes are refused." Both doors, through the bridge, with the two refusals
/// kept distinct — a climb out is `PATH_OUTSIDE_WORKSPACE`, a link out is
/// `PATH_SYMLINK_ESCAPE`, and a surface that cannot tell them apart cannot tell
/// a typo from an escape attempt.
#[tokio::test]
async fn a_path_that_climbs_out_of_the_project_is_refused_at_the_bridge() {
    let dir = setup_repo("escape-path");
    let harness = Harness::start();
    harness.open(&dir).await;

    assert_eq!(
        harness.list_refused("../..", 0).await,
        ReasonCode::PathOutsideWorkspace
    );
    assert_eq!(
        harness.open_refused("../../etc/passwd").await,
        ReasonCode::PathOutsideWorkspace
    );
    // An absolute path is not silently reread as project-relative.
    assert_eq!(
        harness.open_refused("/etc/passwd").await,
        ReasonCode::PathOutsideWorkspace
    );

    harness.close().await;
}

#[tokio::test]
#[cfg(unix)]
async fn a_symlink_out_of_the_project_is_refused_and_its_target_never_projected() {
    let outside = tempfile::tempdir().expect("outside");
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, "SENSITIVE-CONTENT").expect("write");

    let dir = setup_repo("escape-link");
    std::os::unix::fs::symlink(&secret, dir.join("escape.txt")).expect("symlink");

    let harness = Harness::start();
    harness.open(&dir).await;

    assert_eq!(
        harness.open_refused("escape.txt").await,
        ReasonCode::PathSymlinkEscape
    );

    // The listing still works, and the escaping link is described without
    // anything about where it goes. Both absences are the assertion.
    let root = harness.list("").await;
    let escape = entry(&root, "escape.txt");
    let symlink = escape.symlink.as_ref().expect("a link");
    assert!(symlink.escaping);
    assert_eq!(symlink.target, None);
    assert_eq!(escape.content, FileContentView::NotAFile);

    let rendered = format!("{root:?}");
    assert!(!rendered.contains("SENSITIVE-CONTENT"), "{rendered}");
    assert!(
        !rendered.contains(&secret.display().to_string()),
        "the projection named a path outside the workspace: {rendered}"
    );

    harness.close().await;
}

/// The check-to-use gap AGENTS.md §3 names, through the whole stack: the entry
/// was contained when it was listed, and the link is replaced before the open.
/// A containment check performed only at listing time passes this test's first
/// half and fails its second.
#[tokio::test]
#[cfg(unix)]
async fn containment_is_rechecked_at_the_open_and_not_inherited_from_the_listing() {
    let outside = tempfile::tempdir().expect("outside");
    fs::write(outside.path().join("secret.txt"), "SENSITIVE-CONTENT").expect("write");

    let dir = setup_repo("recheck");
    std::os::unix::fs::symlink(dir.join("README.md"), dir.join("link.md")).expect("symlink");

    let harness = Harness::start();
    harness.open(&dir).await;

    let before = harness.list("").await;
    let link = entry(&before, "link.md");
    assert_eq!(
        link.symlink.as_ref().expect("a link").target.as_deref(),
        Some("README.md")
    );
    assert!(matches!(
        harness.open_file("link.md").await.mode,
        FileModeView::Editable { .. }
    ));

    fs::remove_file(dir.join("link.md")).expect("unlink");
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.join("link.md"))
        .expect("relink");

    assert_eq!(
        harness.open_refused("link.md").await,
        ReasonCode::PathSymlinkEscape
    );

    harness.close().await;
}

// ---------------------------------------------------------------------------
// Binary and over-limit files never reach the editor
// ---------------------------------------------------------------------------

/// §D7: "binary and over-limit files open in bounded preview mode, not the
/// editor." Through the bridge, for all three reasons, with the bound on what
/// crosses asserted rather than assumed.
#[tokio::test]
async fn binary_and_over_limit_files_reach_a_client_as_bounded_previews() {
    let dir = setup_repo("preview");
    fs::write(dir.join("logo.png"), b"\x89PNG\r\n\x1a\n\x00\x00IHDR").expect("write");
    fs::write(
        dir.join("huge.txt"),
        vec![b'a'; usize::try_from(MAX_EDITABLE_FILE_BYTES).unwrap() + 1],
    )
    .expect("write");

    let harness = Harness::start();
    harness.open(&dir).await;

    let binary = harness.open_file("logo.png").await;
    let FileModeView::Preview { reason, .. } = &binary.mode else {
        panic!("a binary file must not be editable: {:?}", binary.mode);
    };
    assert_eq!(reason, "binary");

    let huge = harness.open_file("huge.txt").await;
    let FileModeView::Preview {
        reason,
        excerpt,
        excerpt_truncated,
    } = &huge.mode
    else {
        panic!("an over-limit file must not be editable");
    };
    assert_eq!(reason, "tooLarge");
    assert!(excerpt_truncated);
    assert!(
        excerpt.len()
            <= smed::core::client::workspace::MAX_FILE_PREVIEW_BYTES
                .try_into()
                .unwrap(),
        "the preview crossed the wire bound"
    );
    // The digest still describes the whole file, so a later save can compare
    // against what is actually on disk rather than against the excerpt.
    assert_eq!(huge.digest.len(), 64);

    let text = harness.open_file("src/main.rs").await;
    assert_eq!(
        text.mode,
        FileModeView::Editable {
            text: "fn main() {}\n".to_owned(),
            text_truncated: false,
        }
    );
    assert_eq!(text.trust, TrustClass::OperatorControlled);

    harness.close().await;
}

/// An oversized entry in a *listing* is `oversized`, not a binary/text verdict
/// it never looked for. The two send a reader to different remedies, and one
/// `false` for both would say smed looked when it did not.
#[tokio::test]
async fn a_listing_marks_binary_generated_and_oversized_entries_apart() {
    let dir = setup_repo("classify");
    fs::write(dir.join("logo.png"), b"\x00\x01\x02\x03").expect("write");
    fs::write(
        dir.join("wire.rs"),
        "// @generated by schema\npub struct A;\n",
    )
    .expect("write");
    fs::write(
        dir.join("huge.txt"),
        vec![b'a'; usize::try_from(MAX_EDITABLE_FILE_BYTES).unwrap() + 1],
    )
    .expect("write");

    let harness = Harness::start();
    harness.open(&dir).await;
    let root = harness.list("").await;

    assert_eq!(
        entry(&root, "logo.png").content,
        FileContentView::Sniffed {
            binary: true,
            generated: false
        }
    );
    assert_eq!(
        entry(&root, "wire.rs").content,
        FileContentView::Sniffed {
            binary: false,
            generated: true
        }
    );
    assert_eq!(entry(&root, "huge.txt").content, FileContentView::Oversized);
    assert_eq!(
        entry(&root, "README.md").content,
        FileContentView::Sniffed {
            binary: false,
            generated: false
        }
    );

    harness.close().await;
}

// ---------------------------------------------------------------------------
// git's answer, and the absence of one
// ---------------------------------------------------------------------------

/// `ignored` is git's answer, composed by the runtime rather than guessed by
/// the file producer. `target/` is in `.gitignore`; `src/` is not.
#[tokio::test]
async fn git_decides_what_is_ignored_and_the_producer_never_guesses() {
    let dir = setup_repo("ignored");
    fs::create_dir_all(dir.join("target")).expect("target");
    fs::write(dir.join("target/build.log"), "noise").expect("write");
    // A directory named `build` that git does *not* ignore. The guard: a
    // name-based heuristic would mark it, and mislabel hand-written source.
    fs::create_dir_all(dir.join("build")).expect("build");
    fs::write(dir.join("build/handwritten.rs"), "pub struct B;").expect("write");

    let harness = Harness::start();
    harness.open(&dir).await;
    let root = harness.list("").await;

    assert!(entry(&root, "target").ignored, "git ignores target/");
    assert!(!entry(&root, "build").ignored, "git does not ignore build/");
    assert!(!entry(&root, "src").ignored);

    harness.close().await;
}

/// A project that is not a repository still lists. `ignored` is false
/// throughout, which `DirectoryEntry::ignored` documents as also meaning
/// "nobody was there to ask" — an explorer that refused to draw a directory
/// because git was unavailable would be useless on every non-repository
/// project.
#[tokio::test]
async fn a_project_that_is_not_a_repository_still_lists_its_files() {
    let dir = std::env::temp_dir().join("smed-d7-files-no-git");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("temp dir");
    let dir = dir.canonicalize().expect("canonical");
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").expect("write");

    let harness = Harness::start();
    harness.open(&dir).await;

    let root = harness.list("").await;
    let source = entry(&root, "src");
    assert_eq!(source.kind, "directory");
    assert!(!source.ignored);

    assert!(matches!(
        harness.open_file("src/main.rs").await.mode,
        FileModeView::Editable { .. }
    ));

    harness.close().await;
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// A page past the end is refused, not answered with an empty page. An empty
/// page and "this directory is empty" are the same bytes on the wire and
/// different facts.
#[tokio::test]
async fn a_page_past_the_end_is_refused_rather_than_answered_empty() {
    let dir = setup_repo("paging");
    let harness = Harness::start();
    harness.open(&dir).await;

    let root = harness.list("").await;
    assert!(!root.has_more);
    assert_eq!(root.entries.limit, 200);
    assert!(!root.entries.truncated);

    assert_eq!(harness.list_refused("", 7).await, ReasonCode::SchemaInvalid);

    harness.close().await;
}

/// Asking a file to be a directory, or a directory to be a file, is a schema
/// refusal and not a containment one. Reporting a typo as an escape attempt is
/// both wrong and alarming.
#[tokio::test]
async fn the_wrong_kind_is_a_schema_refusal_not_a_containment_one() {
    let dir = setup_repo("kind");
    let harness = Harness::start();
    harness.open(&dir).await;

    assert_eq!(
        harness.list_refused("README.md", 0).await,
        ReasonCode::SchemaInvalid
    );
    assert_eq!(harness.open_refused("src").await, ReasonCode::SchemaInvalid);

    harness.close().await;
}

/// The bridge's own bounds, refused before the actor is asked. A NUL byte
/// matters most: it truncates a C string, so a value that reads as
/// `README.md\0/../..` on the wire could reach a syscall as `README.md` and
/// defeat the containment check applied to the whole string.
#[tokio::test]
async fn an_over_long_or_nul_bearing_path_is_refused_at_the_wire() {
    let dir = setup_repo("wire-bounds");
    let harness = Harness::start();
    harness.open(&dir).await;

    let long = "a".repeat(2_000);
    assert_eq!(harness.open_refused(&long).await, ReasonCode::SchemaInvalid);
    assert_eq!(
        harness.open_refused("README.md\0/../../etc/passwd").await,
        ReasonCode::SchemaInvalid
    );
    assert_eq!(
        harness.list_refused("src\0", 0).await,
        ReasonCode::SchemaInvalid
    );

    harness.close().await;
}

/// Nothing a client receives names the machine the project lives on. Not a
/// secret, but host detail a frontend has no use for, and the habit of not
/// shipping it is what stops a later projection shipping something that is.
#[tokio::test]
async fn no_projection_carries_the_absolute_workspace_root() {
    let dir = setup_repo("no-absolutes");
    let harness = Harness::start();
    harness.open(&dir).await;

    let root = format!("{:?}", harness.list("").await);
    let file = format!("{:?}", harness.open_file("src/main.rs").await);
    let absolute = dir.display().to_string();

    assert!(!root.contains(&absolute), "{root}");
    assert!(!file.contains(&absolute), "{file}");

    harness.close().await;
}
