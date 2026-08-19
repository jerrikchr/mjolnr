//! Fleet orchestration and multi-agent roster types (ADR-0016, Master Implementation Plan Phase 3).
//!
//! Models live subagent and council fleet state for cross-surface visualization
//! (TUI Mission Rail, Jump Palette, and Desktop `SvelteKit` client).

use serde::{Deserialize, Serialize};

use crate::core::event::SessionId;

/// Execution status of an agent in the fleet roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetAgentStatus {
    /// Agent is initialized or standing by.
    Idle,
    /// Agent is actively processing provider turns or executing governed tools.
    Running,
    /// Agent completed its directive and settled results.
    Completed,
    /// Agent failed or was cancelled.
    Failed { reason: String },
}

impl FleetAgentStatus {
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Running | Self::Idle)
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Glassbox summary of an agent participating in the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAgentSummary {
    pub child_session_id: SessionId,
    pub short_name: String,
    pub role: Option<String>,
    pub status: FleetAgentStatus,
    pub latest_activity: String,
    pub feed: Vec<String>,
    pub worktree_branch: Option<String>,
}

/// Live fleet roster summary carried on runtime and client snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetSummary {
    pub visible: bool,
    pub active_count: usize,
    pub agents: Vec<FleetAgentSummary>,
}

impl FleetSummary {
    #[must_use]
    pub fn from_agents(agents: Vec<FleetAgentSummary>) -> Self {
        let active_count = agents.iter().filter(|a| a.status.is_active()).count();
        let visible = agents.len() >= 2 && active_count > 0;
        Self {
            visible,
            active_count,
            agents,
        }
    }
}
