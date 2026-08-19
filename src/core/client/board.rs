//! Frontend-safe projection of the board: frontier, fog, and settled
//! (Phase E5, step 3).
//!
//! The board is a cross-session read over decision tickets and plans that
//! share the current workspace root, rebuilt per query from durable records.
//! These DTOs carry the answer to "what can I decide right now, and why is
//! the rest fogged" — nothing else. They never carry authority; a node's
//! `provenance` is the same `TrustClass` ADR 0006 maps at every other
//! projection boundary.
//!
//! Wire-number contract: counts and indices are `u32`; see the workspace
//! module doc. Collections are bounded by explicit limits; over-limit
//! conditions produce structured refusals, never silent truncation.

use serde::{Deserialize, Serialize};

use crate::core::client::workspace::TrustClass;

/// Bound on nodes per board, mirroring the frontier's bounded sets.
pub const MAX_BOARD_NODES: u32 = 512;
/// Bound on the blockers named per fogged node.
pub const MAX_BOARD_BLOCKERS_PER_FOGGED: u32 = 64;
/// Bound on the number of cycles the board names.
pub const MAX_BOARD_CYCLES: u32 = 64;
/// Bound on imported task metadata carried alongside board nodes.
pub const MAX_BOARD_IMPORTED_TASKS: u32 = 512;
/// Bound on imported act records crossing the wire with the board.
pub const MAX_BOARD_IMPORTED_ACTS: u32 = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientBoardNode {
    /// The node's address, as its owning record's id renders.
    pub id: String,
    /// What the node settles: a decision or implementation work.
    pub kind: String,
    /// The record's provenance (ADR 0006).
    pub provenance: TrustClass,
    /// Human-readable label: a decision ticket's question or a plan's title.
    /// Falls back to the id when no record text exists.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientImportedTask {
    pub board_id: String,
    pub integration: String,
    pub remote_id: String,
    pub source_url: String,
    pub fetched_revision: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientImportedAct {
    /// Durable ordered identity for this act, rendered as its address.
    pub act_id: String,
    /// The imported item the act was made on, rendered as its board id.
    pub item_board_id: String,
    /// What was sent, today only `pull-request`.
    pub kind: String,
    /// The revision the human approved against.
    pub expected_revision: String,
    pub head_branch: String,
    pub base_branch: String,
    /// One of: a remote url the provider returned, or `uncertain`.
    pub outcome: String,
    /// The remote url when `outcome` is a url, absent when `uncertain`.
    pub remote_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientFoggedNode {
    /// The node itself, provenance intact.
    pub node: ClientBoardNode,
    /// The unresolved blockers that answer "why is this fogged", each with
    /// its own label. Contains only unresolved blockers; a resolved blocker
    /// is no longer a reason.
    pub waits_on: Vec<ClientBoardNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientBoardOverview {
    /// Imported task records behind external board nodes, bounded and ordered by board id.
    pub imported_tasks: Vec<ClientImportedTask>,
    /// Durable act history reporting submitted and uncertain attempts, bounded by `MAX_BOARD_IMPORTED_ACTS`.
    pub imported_acts: Vec<ClientImportedAct>,
    /// Decidable right now.
    pub frontier: Vec<ClientBoardNode>,
    /// Waiting, each with the unresolved blockers that answer "why not yet".
    pub fog: Vec<ClientFoggedNode>,
    /// Resolved; no longer part of what is decidable.
    pub settled: Vec<ClientBoardNode>,
    /// Blocking cycles among unresolved nodes, each named by its members.
    /// Members also appear in `fog`; the cycle is named, not broken.
    pub cycles: Vec<Vec<ClientBoardNode>>,
}
