//! Answering memory queries (`memory_search`, `memory_timeline`, `memory_expand`)
//! from the memory capability module.
//!
//! The actor holds the workspace state and routes to `.mjolnr/data/memory.db`.
//! Memory is a **projection, never authority** (Standing Law #2): queries return
//! structured recall aids, never permissions or policy grants.

use std::fmt::Write as _;
use time::format_description::well_known::Rfc3339;

use crate::core::error::ReasonCode;
use crate::core::event::RunId;
use crate::core::message::{ToolCall, ToolResult};
use crate::memory::store::{DEFAULT_SEARCH_LIMIT, MAX_EXPAND_IDS, MAX_SEARCH_LIMIT, MemoryStore};

use super::Actor;

impl Actor {
    /// Open the workspace's memory store projection.
    async fn open_memory_store(&self) -> Result<MemoryStore, ToolResult> {
        let Some(workspace_root) = self.state.workspace_root.as_ref() else {
            return Err(ToolResult::failed(
                ReasonCode::PathOutsideWorkspace,
                "open a workspace before querying workspace memory",
            ));
        };
        let config_dir = crate::core::paths::resolve_workspace_config_dir(workspace_root);
        let db_path = config_dir.join("data").join("memory.db");
        MemoryStore::open(&db_path)
            .await
            .map_err(|error| ToolResult::failed(ReasonCode::ToolExecution, error.to_string()))
    }

    /// Answer `memory_search` with hybrid-scored one-line summaries.
    pub(super) async fn answer_memory_search(&mut self, run: RunId, call: ToolCall) -> bool {
        let store = match self.open_memory_store().await {
            Ok(store) => store,
            Err(result) => {
                let _ = self.record_tool_result(run, &call, result).await;
                return false;
            }
        };

        let query = call
            .arguments
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        let limit = call
            .arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map_or(DEFAULT_SEARCH_LIMIT, |value| {
                usize::try_from(value).unwrap_or(DEFAULT_SEARCH_LIMIT)
            })
            .clamp(1, MAX_SEARCH_LIMIT);

        let result = match store.search(query, Some(limit)).await {
            Ok(hits) => {
                if hits.is_empty() {
                    ToolResult::ok("No matching facts found in workspace memory.")
                } else {
                    let mut text = format!("# Memory Search Results ({} hit(s))\n\n", hits.len());
                    for hit in hits {
                        let _ = writeln!(
                            text,
                            "- [id: {}] (score: {:.2}) {} {}: {}",
                            hit.id, hit.score, hit.subject, hit.predicate, hit.summary
                        );
                    }
                    text.push_str("\nUse `memory_expand` with fact ids to read full details.\n");
                    ToolResult::ok(text)
                }
            }
            Err(error) => ToolResult::failed(ReasonCode::ToolExecution, error.to_string()),
        };

        let _ = self.record_tool_result(run, &call, result).await;
        false
    }

    /// Answer `memory_timeline` with the full history of a subject.
    pub(super) async fn answer_memory_timeline(&mut self, run: RunId, call: ToolCall) -> bool {
        let store = match self.open_memory_store().await {
            Ok(store) => store,
            Err(result) => {
                let _ = self.record_tool_result(run, &call, result).await;
                return false;
            }
        };

        let subject = call
            .arguments
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

        let result = match store.timeline(subject).await {
            Ok(triples) => {
                if triples.is_empty() {
                    ToolResult::ok(format!(
                        "No timeline entries found for subject \"{subject}\"."
                    ))
                } else {
                    let mut text = format!(
                        "# Memory Timeline for \"{}\" ({} entries, oldest first)\n\n",
                        subject,
                        triples.len()
                    );
                    for triple in triples {
                        let from_str = triple
                            .valid_from
                            .format(&Rfc3339)
                            .unwrap_or_else(|_| "unknown".to_owned());
                        let status_str = match &triple.valid_until {
                            Some(until) => {
                                let until_str = until
                                    .format(&Rfc3339)
                                    .unwrap_or_else(|_| "unknown".to_owned());
                                format!("superseded at {until_str}")
                            }
                            None => "current".to_owned(),
                        };
                        let _ = writeln!(
                            text,
                            "- [id: {}] ({from_str} | {status_str}) {} {}: {} [source: {}]",
                            triple.id,
                            triple.subject,
                            triple.predicate,
                            triple.object,
                            triple.source
                        );
                    }
                    ToolResult::ok(text)
                }
            }
            Err(error) => ToolResult::failed(ReasonCode::ToolExecution, error.to_string()),
        };

        let _ = self.record_tool_result(run, &call, result).await;
        false
    }

    /// Answer `memory_expand` with targeted full detail for named fact ids.
    pub(super) async fn answer_memory_expand(&mut self, run: RunId, call: ToolCall) -> bool {
        let store = match self.open_memory_store().await {
            Ok(store) => store,
            Err(result) => {
                let _ = self.record_tool_result(run, &call, result).await;
                return false;
            }
        };

        let ids: Vec<i64> = call
            .arguments
            .get("ids")
            .and_then(serde_json::Value::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(serde_json::Value::as_i64)
                    .take(MAX_EXPAND_IDS)
                    .collect()
            })
            .unwrap_or_default();

        if ids.is_empty() {
            let result = ToolResult::refused(
                ReasonCode::SchemaInvalid,
                "memory_expand requires at least one integer fact id",
            );
            let _ = self.record_tool_result(run, &call, result).await;
            return false;
        }

        let result = match store.expand(&ids).await {
            Ok(triples) => {
                if triples.is_empty() {
                    ToolResult::ok("None of the requested fact ids were found in workspace memory.")
                } else {
                    let mut text = format!("# Expanded Memory Facts ({} found)\n\n", triples.len());
                    for triple in triples {
                        let from_str = triple
                            .valid_from
                            .format(&Rfc3339)
                            .unwrap_or_else(|_| "unknown".to_owned());
                        let status_str = match &triple.valid_until {
                            Some(until) => {
                                let until_str = until
                                    .format(&Rfc3339)
                                    .unwrap_or_else(|_| "unknown".to_owned());
                                format!("superseded at {until_str}")
                            }
                            None => "current".to_owned(),
                        };
                        let _ = writeln!(
                            text,
                            "## Fact #{}: {} {}\n- **Object:** {}\n- **Valid From:** {}\n- **Status:** {}\n- **Source:** {}\n",
                            triple.id,
                            triple.subject,
                            triple.predicate,
                            triple.object,
                            from_str,
                            status_str,
                            triple.source
                        );
                    }
                    ToolResult::ok(text)
                }
            }
            Err(error) => ToolResult::failed(ReasonCode::ToolExecution, error.to_string()),
        };

        let _ = self.record_tool_result(run, &call, result).await;
        false
    }

    /// Trigger non-blocking background consolidation of recent session events into episodic memory.
    ///
    /// Single-flight: a second trigger while one pass is open skips — two
    /// overlapping passes would both read progress N and append duplicate
    /// episodes for N+1..M. The task shares the handle's shutdown token, so
    /// actor shutdown stops it (AGENTS.md §4). Failures land in the
    /// projection slot, where the snapshot surfaces them, rather than being
    /// swallowed.
    pub(super) fn trigger_background_consolidation(&self) {
        use std::sync::atomic::Ordering;

        let Some(session) = self.state.session else {
            return;
        };
        let Some(workspace_root) = self.state.workspace_root.as_ref() else {
            return;
        };
        if self
            .memory_consolidation_in_flight
            .swap(true, Ordering::AcqRel)
        {
            return;
        }
        let config_dir = crate::core::paths::resolve_workspace_config_dir(workspace_root);
        let db_path = config_dir.join("data").join("memory.db");
        let store_arc = self.store.clone();
        let cancel = self.shutdown.clone();
        let slot = self.state.memory_projection.clone();
        let in_flight = self.memory_consolidation_in_flight.clone();

        tokio::spawn(async move {
            Self::consolidate_and_refresh(store_arc, session, &db_path, &cancel, &slot).await;
            in_flight.store(false, Ordering::Release);
        });
    }

    /// One background pass: consolidate, then refresh the projection counts
    /// either way so the inspector reports what is actually in the database.
    async fn consolidate_and_refresh(
        store_arc: std::sync::Arc<dyn crate::core::store::EventStore>,
        session: crate::core::event::SessionId,
        db_path: &std::path::Path,
        cancel: &tokio_util::sync::CancellationToken,
        slot: &std::sync::Mutex<MemoryProjection>,
    ) {
        let memory_store = match MemoryStore::open(db_path).await {
            Ok(store) => store,
            Err(error) => {
                note_projection_error(slot, format!("memory projection unavailable: {error}"));
                return;
            }
        };
        let events = match store_arc.events(session).await {
            Ok(events) => events,
            Err(error) => {
                note_projection_error(slot, format!("could not read the event ledger: {error}"));
                return;
            }
        };
        let session_id_str = session.to_string();
        if let Err(error) =
            crate::memory::consolidate_events(&memory_store, &session_id_str, &events, cancel).await
        {
            note_projection_error(slot, format!("consolidation failed: {error}"));
        }
        store_counts(slot, memory_store.counts().await);
    }

    /// Load the Tier 1 rules snapshot off the async path, recording any
    /// refusal where the snapshot can show it. A failed load still fails
    /// safe to "no rules" — but it must read as a failure, not as a
    /// workspace that declares nothing (AGENTS.md §1.3).
    pub(super) async fn load_rules_snapshot(&mut self, root: std::path::PathBuf) {
        let outcome =
            tokio::task::spawn_blocking(move || crate::memory::RulesSnapshot::load(&root)).await;
        match outcome {
            Ok(Ok(snapshot)) => {
                self.state.rules_snapshot = snapshot;
                self.state.rules_load_error = None;
            }
            Ok(Err(error)) => {
                self.state.rules_snapshot = crate::memory::RulesSnapshot::default();
                self.state.rules_load_error = Some(error.to_string());
            }
            Err(join_error) => {
                self.state.rules_snapshot = crate::memory::RulesSnapshot::default();
                self.state.rules_load_error =
                    Some(format!("rules load task failed to run: {join_error}"));
            }
        }
    }

    /// Refresh the projection counts for the current workspace, best-effort.
    ///
    /// Read-only by contract: an absent `memory.db` means "no projection
    /// yet", reported as unknown — opening a project must not create files in
    /// the repository (the change-capture test guards exactly that).
    pub(super) async fn refresh_memory_projection(&self) {
        let Some(workspace_root) = self.state.workspace_root.as_ref() else {
            return;
        };
        let config_dir = crate::core::paths::resolve_workspace_config_dir(workspace_root);
        let db_path = config_dir.join("data").join("memory.db");
        if !tokio::fs::try_exists(&db_path).await.unwrap_or(false) {
            return;
        }
        let slot = self.state.memory_projection.clone();
        let result = match MemoryStore::open(&db_path).await {
            Ok(store) => store.counts().await,
            Err(error) => Err(error),
        };
        store_counts(&slot, result);
    }
}

/// Last-known state of the memory projection, as the snapshot reports it.
#[derive(Debug, Clone, Default)]
pub struct MemoryProjection {
    /// Counts from the last successful query. `None` means unknown, never
    /// zero-by-default: zero is a claim (AGENTS.md §1.3).
    pub counts: Option<crate::memory::store::MemoryCounts>,
    /// Why `counts` is stale or unknown, when there is a reason.
    pub error: Option<String>,
}

/// Record a projection failure without discarding the last known counts.
fn note_projection_error(slot: &std::sync::Mutex<MemoryProjection>, detail: String) {
    let mut guard = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.error = Some(detail);
}

/// Store the outcome of a count query: success replaces the counts and clears
/// any stale error; failure keeps the last known counts and records why.
fn store_counts(
    slot: &std::sync::Mutex<MemoryProjection>,
    result: Result<crate::memory::store::MemoryCounts, crate::memory::MemoryError>,
) {
    let mut guard = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match result {
        Ok(counts) => {
            guard.counts = Some(counts);
            guard.error = None;
        }
        Err(error) => guard.error = Some(format!("count refresh failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::store::MemoryCounts;

    fn slot() -> std::sync::Mutex<MemoryProjection> {
        std::sync::Mutex::new(MemoryProjection::default())
    }

    #[test]
    fn unknown_projection_never_reports_zero_by_default() {
        let slot = slot();
        let projection = slot.lock().unwrap().clone();
        assert_eq!(
            projection.counts, None,
            "unknown, not zero (AGENTS.md §1.3)"
        );
        assert_eq!(projection.error, None);
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_known_counts_and_records_why() {
        let slot = slot();
        store_counts(
            &slot,
            Ok(MemoryCounts {
                facts: 3,
                episodes: 1,
            }),
        );
        let refused = Err(crate::memory::MemoryError::QueryRefused {
            detail: "query refused".to_owned(),
        });
        store_counts(&slot, refused);

        let projection = slot.lock().unwrap().clone();
        assert_eq!(
            projection.counts,
            Some(MemoryCounts {
                facts: 3,
                episodes: 1
            }),
            "a stale count is honest; unknown-after-known hides history"
        );
        assert!(projection.error.is_some());
    }

    #[test]
    fn a_successful_refresh_clears_the_stale_error() {
        let slot = slot();
        note_projection_error(&slot, "stale".to_owned());
        store_counts(
            &slot,
            Ok(MemoryCounts {
                facts: 1,
                episodes: 0,
            }),
        );

        let projection = slot.lock().unwrap().clone();
        assert_eq!(projection.error, None);
        assert_eq!(
            projection.counts,
            Some(MemoryCounts {
                facts: 1,
                episodes: 0
            })
        );
    }
}
