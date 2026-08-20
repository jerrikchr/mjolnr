//! Bridge conversion of the board projection (Phase E5, step 3).
//!
//! Maps the core board (`core::frontier::BoardOverview`) to the wire DTOs
//! (`core::client::board`). Provenance crosses here — the core enum becomes
//! ADR 0006's `TrustClass` exactly as every other projection boundary maps
//! it, and never before. Labels already live in the core views; this module
//! only carries them, it does not re-derive them.

use crate::core::client::board::{
    ClientBoardNode, ClientBoardOverview, ClientFoggedNode, ClientImportedAct, ClientImportedTask,
    MAX_BOARD_BLOCKERS_PER_FOGGED, MAX_BOARD_CYCLES, MAX_BOARD_IMPORTED_ACTS,
    MAX_BOARD_IMPORTED_TASKS, MAX_BOARD_NODES,
};
use crate::core::client::workspace::TrustClass;
use crate::core::error::ReasonCode;
use crate::core::frontier::{BoardNodeView, BoardOverview, Provenance};

use super::ClientBridgeError;

/// Project the board to wire shape, re-applying the wire bounds at the last
/// moment before the DTO leaves the bridge (AGENTS.md: validate after the
/// last transformation, immediately before the boundary).
///
/// The core board is bounded already, so an over-limit projection here means
/// the two bounds drifted; refuse loudly rather than truncate silently.
#[allow(
    clippy::too_many_lines,
    reason = "one flat board-to-wire mapping; it grows by one field per projection and splitting it would hide which fields are covered"
)]
pub fn board_overview_to_client(
    board: &BoardOverview,
) -> Result<ClientBoardOverview, ClientBridgeError> {
    let total = board.frontier.len() + board.settled.len() + board.fog.len();
    if total > MAX_BOARD_NODES as usize {
        return Err(ClientBridgeError::RuntimeRefused {
            code: Some(ReasonCode::WorkspaceSearchRefused),
            detail: format!("board has {total} nodes, over the wire limit {MAX_BOARD_NODES}"),
        });
    }
    if board.cycles.len() > MAX_BOARD_CYCLES as usize {
        return Err(ClientBridgeError::RuntimeRefused {
            code: Some(ReasonCode::WorkspaceSearchRefused),
            detail: format!(
                "board names {} cycles, over the wire limit {MAX_BOARD_CYCLES}",
                board.cycles.len()
            ),
        });
    }
    if board.imported_tasks.len() > MAX_BOARD_IMPORTED_TASKS as usize {
        return Err(ClientBridgeError::RuntimeRefused {
            code: Some(ReasonCode::WorkspaceSearchRefused),
            detail: format!(
                "board carries {} imported tasks, over the wire limit {MAX_BOARD_IMPORTED_TASKS}",
                board.imported_tasks.len()
            ),
        });
    }
    if board.imported_acts.len() > MAX_BOARD_IMPORTED_ACTS as usize {
        return Err(ClientBridgeError::RuntimeRefused {
            code: Some(ReasonCode::WorkspaceSearchRefused),
            detail: format!(
                "board carries {} imported acts, over the wire limit {MAX_BOARD_IMPORTED_ACTS}",
                board.imported_acts.len()
            ),
        });
    }
    let imported_tasks = board
        .imported_tasks
        .values()
        .map(|item| ClientImportedTask {
            board_id: item.id.to_string(),
            integration: item.integration.clone(),
            remote_id: item.remote_id.clone(),
            source_url: item.source_url.clone(),
            fetched_revision: item.fetched_revision.clone(),
            title: item.title.clone(),
            state: serde_json::to_value(item.state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned()),
        })
        .collect();
    let imported_acts = board
        .imported_acts
        .values()
        .map(|act| {
            let (outcome, remote_url) = match &act.outcome {
                crate::core::imported::ImportedActOutcome::Submitted { remote_url } => {
                    ("submitted".to_owned(), Some(remote_url.clone()))
                }
                crate::core::imported::ImportedActOutcome::Uncertain => {
                    ("uncertain".to_owned(), None)
                }
            };
            ClientImportedAct {
                act_id: act.act_id.to_string(),
                item_board_id: act.item_id.to_string(),
                kind: match act.kind {
                    crate::core::imported::ImportedActKind::PullRequest => {
                        "pull-request".to_owned()
                    }
                },
                expected_revision: act.expected_revision.clone(),
                head_branch: act.head_branch.clone(),
                base_branch: act.base_branch.clone(),
                outcome,
                remote_url,
            }
        })
        .collect();
    let frontier: Vec<_> = board.frontier.iter().map(node_to_client).collect();
    let settled: Vec<_> = board.settled.iter().map(node_to_client).collect();
    let fog: Vec<_> = board
        .fog
        .iter()
        .map(|fogged| ClientFoggedNode {
            node: node_to_client(&fogged.node),
            waits_on: fogged
                .waits_on
                .iter()
                .take(MAX_BOARD_BLOCKERS_PER_FOGGED as usize)
                .map(node_to_client)
                .collect(),
        })
        .collect();
    let cycles: Vec<_> = board
        .cycles
        .iter()
        .take(MAX_BOARD_CYCLES as usize)
        .map(|cycle| cycle.iter().map(node_to_client).collect())
        .collect();
    Ok(ClientBoardOverview {
        imported_tasks,
        imported_acts,
        frontier,
        fog,
        settled,
        cycles,
    })
}

fn node_to_client(node: &BoardNodeView) -> ClientBoardNode {
    ClientBoardNode {
        id: match node.id {
            crate::core::frontier::NodeId::Decision(id) => id.to_string(),
            crate::core::frontier::NodeId::Plan(id) => id.as_uuid().to_string(),
            crate::core::frontier::NodeId::Imported(id) => id.to_string(),
        },
        kind: match node.kind {
            crate::core::frontier::NodeKind::Decision => "decision".to_owned(),
            crate::core::frontier::NodeKind::Implementation => "implementation".to_owned(),
        },
        provenance: match node.provenance {
            Provenance::MjolnrGoverned => TrustClass::MjolnrGoverned,
            Provenance::ExternalUnverified => TrustClass::ExternalUnverified,
        },
        label: node.label.clone(),
    }
}
