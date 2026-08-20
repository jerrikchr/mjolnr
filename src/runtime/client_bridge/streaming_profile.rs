//! Localized streaming-update profile (Phase A0).
//!
//! Measures the bridge pump under a sustained burst of `TextDelta`
//! events. Two things matter for a TUI/designer client:
//!
//! * **Throughput**: deltas observed per second by the consumer.
//! * **Coalescing**: snapshot updates emitted per delta burst. The
//!   pump emits a snapshot only when the watch fires, so an idle
//!   snapshot during a pure-delta burst must yield *zero* snapshot
//!   updates.
//!
//! AGENTS.md §5 — performance is I/O-bound. The relevant cost here is
//! backpressure and coalescing, not arithmetic.

#![allow(
    clippy::cast_precision_loss,
    reason = "profile measurements use ratios and counts"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::client::{ClientEvent, ClientUpdate};
use crate::core::event::{MjolnrEvent, RunId, SessionId};
use crate::core::runtime::{MjolnrRuntime, RuntimeSnapshot, RuntimeSubscription, SnapshotStream};
use crate::runtime::client_bridge::ClientBridge;

const DELTAS: usize = 2_000;
const CONSUMER_LAG_BUDGET: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct HarnessRuntime {
    snapshot: Arc<tokio::sync::watch::Sender<RuntimeSnapshot>>,
    events: tokio::sync::broadcast::Sender<MjolnrEvent>,
}

impl HarnessRuntime {
    fn new(initial: RuntimeSnapshot) -> Self {
        let (snap_tx, _) = tokio::sync::watch::channel(initial);
        let (events_tx, _) = tokio::sync::broadcast::channel(8192);
        Self {
            snapshot: Arc::new(snap_tx),
            events: events_tx,
        }
    }
}

#[async_trait::async_trait]
impl MjolnrRuntime for HarnessRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.borrow().clone()
    }
    fn snapshots(&self) -> SnapshotStream {
        SnapshotStream::new(self.snapshot.subscribe())
    }
    fn subscribe(&self) -> RuntimeSubscription {
        RuntimeSubscription::new(self.events.subscribe())
    }
    async fn dispatch(
        &self,
        _command: crate::core::command::MjolnrCommand,
    ) -> Result<(), crate::core::error::MjolnrError> {
        Ok(())
    }
    async fn read_workspace_files(
        &self,
        _request: crate::core::workspace_files::WorkspaceFileRequest,
    ) -> Result<crate::core::workspace_files::WorkspaceFileAnswer, crate::core::error::MjolnrError>
    {
        Err(crate::core::error::MjolnrError::workspace_refused(
            crate::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
            "this harness runtime opens no project, so there is nothing to read files from",
        ))
    }

    async fn search_workspace(
        &self,
        _filter: crate::core::store::WorkspaceSearchFilter,
    ) -> Result<crate::core::store::WorkspaceSearchPage, crate::core::error::MjolnrError> {
        Err(crate::core::error::MjolnrError::workspace_refused(
            crate::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
            "workspace search is not yet implemented (contract landed in D4)",
        ))
    }
    async fn query_board(
        &self,
    ) -> Result<crate::core::frontier::BoardOverview, crate::core::error::MjolnrError> {
        Err(crate::core::error::MjolnrError::workspace_refused(
            crate::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
            "this harness runtime opens no project, so there is no board to answer from",
        ))
    }
    async fn query_repository_history(
        &self,
        _limit: u32,
    ) -> Result<crate::core::repository::RepositoryHistory, crate::core::error::MjolnrError> {
        Err(crate::core::error::MjolnrError::workspace_refused(
            crate::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
            "this harness runtime opens no project, so there is no repository history to answer from",
        ))
    }
    async fn close(&self) -> Result<(), crate::core::error::MjolnrError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_update_profile_throughput_and_coalescing() {
    let initial = RuntimeSnapshot::default();
    let runtime = HarnessRuntime::new(initial);
    let events = runtime.events.clone();
    let snapshot = runtime.snapshot.clone();
    let bridge = ClientBridge::start(Arc::new(runtime));
    let mut rx = bridge.take_updates().expect("updates channel");

    // Drop the initial snapshot the pump publishes.
    let _initial = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("initial snapshot arrives")
        .expect("bridge channel open");

    // Producer side: enqueue DELTAS synthetic text deltas in a tight
    // loop. broadcast::Sender::send is non-blocking when there is
    // buffer headroom.
    let producer_start = Instant::now();
    for index in 0..DELTAS {
        let delta = MjolnrEvent::TextDelta {
            session: SessionId::new(),
            run: RunId::new(),
            text: format!("tok{index}"),
        };
        events
            .send(delta)
            .expect("text delta accepted by broadcast");
    }
    let producer_elapsed = producer_start.elapsed();

    // Now publish a snapshot change so the pump is forced to emit a
    // final coalesced snapshot.
    snapshot.send_modify(|snap| snap.run_active = true);

    // Consumer side: drain until the trailing coalesced snapshot
    // arrives, or budget exceeded.
    let consumer_start = Instant::now();
    let mut observed_deltas: u64 = 0;
    let mut observed_snapshots: u64 = 0;
    let mut last_revision: u64 = 0;
    let mut monotonic = true;
    let mut last_trailing_revision: u64 = 0;
    let mut trailing_seen = false;
    loop {
        let remaining = CONSUMER_LAG_BUDGET
            .checked_sub(consumer_start.elapsed())
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(ClientUpdate::Event { event, .. })) => {
                if matches!(event, ClientEvent::TextDelta { .. }) {
                    observed_deltas += 1;
                }
            }
            Ok(Some(ClientUpdate::Snapshot { snapshot })) => {
                observed_snapshots += 1;
                if snapshot.revision <= last_revision {
                    monotonic = false;
                }
                last_revision = snapshot.revision;
                if snapshot.run_active {
                    trailing_seen = true;
                    last_trailing_revision = snapshot.revision;
                }
            }
            Ok(Some(ClientUpdate::Resync { snapshot, .. })) => {
                observed_snapshots += 1;
                if snapshot.revision <= last_revision {
                    monotonic = false;
                }
                last_revision = snapshot.revision;
            }
            Ok(Some(ClientUpdate::Closed) | None) | Err(_) => break,
        }
    }
    let consumer_elapsed = consumer_start.elapsed();

    let throughput = observed_deltas as f64 / consumer_elapsed.as_secs_f64();
    eprintln!(
        "streaming_update_profile: deltas_observed={observed_deltas} \
         snapshots_observed={observed_snapshots} producer_elapsed={producer_elapsed:?} \
         consumer_elapsed={consumer_elapsed:?} throughput={throughput:.0}/s \
         monotonic_revisions={monotonic} trailing_seen={trailing_seen} \
         last_trailing_revision={last_trailing_revision}"
    );

    assert!(
        observed_deltas >= (DELTAS as u64) / 2,
        "at least half of synthetic deltas must reach the receiver — \
         observed {observed_deltas} of {DELTAS}"
    );
    assert!(
        observed_snapshots <= 4,
        "the pump must coalesce snapshots — observed {observed_snapshots} \
         for {DELTAS} pure deltas plus one trailing change"
    );
    assert!(
        monotonic,
        "snapshot revisions must be strictly monotonic across the burst"
    );
    assert!(
        trailing_seen,
        "the trailing snapshot change (run_active=true) must be observed"
    );

    bridge.close().await.expect("bridge closes cleanly");
}
