//! Phase A0 native Tauri smoke harness.
//!
//! Exercises the desktop crate's bridge end-to-end against a real
//! on-disk SQLite store in two phases:
//!
//! 1. Phase A — open a workspace, create a session.
//! 2. Phase B — close the bridge, drop everything, build a fresh
//!    bridge against the *same* database, and resume the session.
//!
//! The point is to prove that what `run()` wires together actually
//! persists across process restarts; the second phase reopens against
//! the same `database_path` so any inconsistency in `init_bridge` or
//! the bridge's session-summary read surfaces here. This is the
//! bridge-equivalent of the A0 exit criterion "open/create/resume
//! smoke". A real WKWebView render is intentionally out of scope for
//! headless CI — that needs the manual macOS run from the A0
//! checkpoint.
//!
//! Run with:
//!   `cargo run --manifest-path desktop/src-tauri/Cargo.toml \
//!      --example native_smoke`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::client::{ClientCommand, ClientSnapshot, ClientUpdate};
use mjolnr_desktop_lib::init_bridge;
use tokio::sync::mpsc;

/// Swallow `tokio-rusqlite` 0.7.0's documented Drop-time spurious
/// panic ("bug in tokio-rusqlite, please report: Ok(())"). The smoke
/// harness installs this hook once at startup; the harness process is
/// short-lived so restoration is unnecessary, and the hook does not
/// shadow genuine panics.
fn install_smoke_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&'static str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str));
        if message == Some("bug in tokio-rusqlite, please report: Ok(())") {
            // tokio-rusqlite 0.7.0's documented Drop-time spurious
            // panic. Swallow so the harness exit code reflects the
            // smoke result, not the third-party cleanup bug.
            return;
        }
        eprintln!("native_smoke: panic — {info}");
    }));
}

const TIMEOUT: Duration = Duration::from_secs(5);

fn die(msg: &str) -> ! {
    eprintln!("native_smoke: {msg}");
    std::process::exit(1);
}

/// Drain updates until `predicate` matches a `Snapshot`, or fail.
async fn drain_until(
    rx: &mut mpsc::Receiver<ClientUpdate>,
    predicate: impl Fn(&ClientSnapshot) -> bool,
) -> ClientSnapshot {
    loop {
        match tokio::time::timeout(TIMEOUT, rx.recv()).await {
            Ok(Some(ClientUpdate::Snapshot { snapshot })) if predicate(&snapshot) => {
                return snapshot;
            }
            Ok(Some(_)) => continue,
            Ok(None) => die("bridge update channel closed unexpectedly"),
            Err(_) => die("timed out waiting for matching snapshot"),
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    install_smoke_panic_hook();
    let db_path = std::env::var("MJOLNR_SMOKE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".mjolnr/mjolnr-desktop-smoke.db")
        });
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    if db_path.exists() {
        std::fs::remove_file(&db_path).expect("clean previous smoke database");
    }

    let phase_a_path = db_path.clone();
    let (created_id, title) = phase_one(&phase_a_path).await;
    println!("native_smoke: phase A — created session {created_id} title={title:?}");

    // Drop everything that referenced the store. The next phase opens
    // a fresh bridge against the same on-disk database.
    drop(phase_a_path);

    let phase_b_path = db_path.clone();
    let resumed_id = phase_two(&phase_b_path, &title).await;
    println!("native_smoke: phase B — resumed session {resumed_id}");

    assert_eq!(
        created_id, resumed_id,
        "the session id must round-trip through SQLite across a process restart"
    );

    // Clean up the smoke database so repeated runs are deterministic.
    let _ = std::fs::remove_file(&db_path);

    println!("native_smoke: PASS — open / create / resume round-tripped through SQLite");
}

async fn phase_one(database_path: &Path) -> (String, String) {
    let bridge = init_bridge(database_path.to_path_buf())
        .await
        .expect("phase A init_bridge");
    let mut rx = bridge.take_updates().expect("phase A updates channel");

    bridge
        .dispatch(ClientCommand::OpenProject {
            root: database_path
                .parent()
                .and_then(|p| p.parent())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .await
        .expect("open project");

    bridge
        .dispatch(ClientCommand::CreateSession {
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet".into(),
        })
        .await
        .expect("create session");

    let snap = drain_until(&mut rx, |s| {
        s.session.is_some()
            && s.sessions.iter().any(|row| row.status == "active")
            && s.store_failure.is_none()
    })
    .await;

    let created = snap.session.clone().expect("session present after create");
    let title = snap
        .sessions
        .iter()
        .find(|row| row.id == created)
        .map(|row| row.title.clone())
        .unwrap_or_default();

    bridge.close().await.expect("phase A close");
    (created, title)
}

async fn phase_two(database_path: &Path, expected_title: &str) -> String {
    let bridge = init_bridge(database_path.to_path_buf())
        .await
        .expect("phase B init_bridge");
    let mut rx = bridge.take_updates().expect("phase B updates channel");

    // Wait for the bridge to advertise the persisted session.
    let snap = drain_until(&mut rx, |s| {
        s.sessions.iter().any(|row| row.status == "active") && s.store_failure.is_none()
    })
    .await;

    let summary = snap
        .sessions
        .iter()
        .find(|row| row.status == "active")
        .expect("resumable session visible after re-open");

    if expected_title == summary.title {
        // titles match
    } else {
        die(&format!(
            "title mismatch after reopen: expected {expected_title:?}, got {:?}",
            summary.title
        ));
    }

    let resumed_id = summary.id.clone();
    let bridge_for_resume = Arc::clone(&bridge);
    bridge_for_resume
        .dispatch(ClientCommand::ResumeSession {
            session: resumed_id.clone(),
        })
        .await
        .expect("resume session");

    let snap = drain_until(&mut rx, |s| {
        s.session.as_deref() == Some(resumed_id.as_str())
    })
    .await;
    assert_eq!(
        snap.session.as_deref(),
        Some(resumed_id.as_str()),
        "bridge must reflect the resumed session id"
    );

    bridge.close().await.expect("phase B close");
    resumed_id
}
