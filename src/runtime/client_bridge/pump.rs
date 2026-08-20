//! Background pump task forwarding runtime events and snapshots to client updates.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

use crate::core::client::ClientUpdate;
use crate::core::runtime::MjolnrRuntime;

use super::convert::{event_to_client, snapshot_to_client};

pub(super) async fn pump_updates(
    runtime: Arc<dyn MjolnrRuntime>,
    updates: mpsc::Sender<ClientUpdate>,
    sequence: Arc<AtomicU64>,
) {
    let mut events = runtime.subscribe();
    let mut snapshots = runtime.snapshots();

    let next = move || sequence.fetch_add(1, Ordering::Relaxed);

    let initial = snapshot_to_client(next(), &runtime.snapshot());
    if updates
        .send(ClientUpdate::Snapshot { snapshot: initial })
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            changed = snapshots.changed() => {
                if let Ok(snapshot) = changed {
                    let update = ClientUpdate::Snapshot {
                        snapshot: snapshot_to_client(next(), &snapshot),
                    };
                    if updates.send(update).await.is_err() {
                        return;
                    }
                } else {
                    let _ = updates.send(ClientUpdate::Closed).await;
                    return;
                }
            }
            incoming = events.recv() => {
                match incoming {
                    Ok(event) => {
                        if let Some(event) = event_to_client(&event) {
                            let update = ClientUpdate::Event {
                                sequence: next(),
                                event,
                            };
                            if updates.send(update).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(RecvError::Lagged(missed)) => {
                        let snapshot = snapshot_to_client(next(), &runtime.snapshot());
                        let update = ClientUpdate::Resync { missed, snapshot };
                        if updates.send(update).await.is_err() {
                            return;
                        }
                    }
                    Err(RecvError::Closed) => {
                        let _ = updates.send(ClientUpdate::Closed).await;
                        return;
                    }
                }
            }
        }
    }
}
