//! Memory view and summary contracts for client rendering and snapshots
//! (master implementation plan §2.3).
//!
//! Defined in `core` so clients (TUI and Desktop) can render memory views
//! without depending on `memory` or SQLite (AGENTS.md §2.1).

use serde::{Deserialize, Serialize};

/// Summary of memory layers for client rendering and snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummary {
    /// Number of frozen rules in effect for this session.
    pub rules_count: usize,
    /// Whether a user profile (`.mjolnr/USER.md`) was loaded into Tier 1 snapshot.
    pub user_profile_present: bool,
    /// Number of temporal facts in the workspace projection. `None` until a
    /// count query succeeds — unknown is reportable, zero is a claim
    /// (AGENTS.md §1.3).
    pub facts_count: Option<usize>,
    /// Number of consolidated episodes in the workspace projection. `None`
    /// until a count query succeeds.
    pub episodes_count: Option<usize>,
    /// Why the projection counts are unknown, when they are. Surfaced so a
    /// failed refresh reads as a failure, not as "no memory" (§1.3).
    pub projection_error: Option<String>,
    /// Why the Tier 1 rules load refused or failed, when it did. A refusal
    /// must not read as "this workspace declares no rules".
    pub rules_error: Option<String>,
    /// Preview of frozen rule names.
    pub rule_names: Vec<String>,
}

/// A client-facing representation of a memory fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFactView {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: String,
    pub valid_until: Option<String>,
    pub source: String,
}

/// A client-facing representation of an episodic summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEpisodeView {
    pub id: i64,
    pub session_id: String,
    pub summary: String,
    pub key_decisions: String,
    pub source_event_start: u64,
    pub source_event_end: u64,
    pub created_at: String,
}
