//! Client activity event feed and update types.

use serde::{Deserialize, Serialize};

use crate::core::command::ApprovalDecision;
use crate::core::event::FinishReason;

use super::command::{ClientPolicy, ClientRecoveryDecision};
use super::types::{ClientSnapshot, ClientToolOutcome};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "activity", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientEvent {
    #[serde(rename_all = "camelCase")]
    SessionStarted {
        session: String,
        provider: String,
        model: String,
    },
    #[serde(rename_all = "camelCase")]
    RunStarted {
        run: String,
    },
    #[serde(rename_all = "camelCase")]
    TextDelta {
        run: String,
        text: String,
        text_truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    ReasoningDelta {
        run: String,
        text: String,
        text_truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    ToolAssembling {
        run: String,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolProposed {
        run: String,
        approval: Option<String>,
        name: String,
        preview: String,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalResolved {
        run: String,
        approval: String,
        decision: ClientApprovalResolution,
    },
    #[serde(rename_all = "camelCase")]
    ToolCompleted {
        run: String,
        name: String,
        outcome: ClientToolOutcome,
        reason_code: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    RunFinished {
        run: String,
        reason: ClientFinishReason,
    },
    #[serde(rename_all = "camelCase")]
    RunFailed {
        run: String,
        code: String,
        detail: String,
        detail_truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    PolicyChanged {
        policy: ClientPolicy,
    },
    #[serde(rename_all = "camelCase")]
    ModelChanged {
        provider: String,
        model: String,
    },
    #[serde(rename_all = "camelCase")]
    FileSaved {
        path: String,
        observed_digest: String,
        new_digest: String,
        size_bytes: u32,
    },
    /// `child` (not `run`) is the roster key: a subagent's `run` id changes
    /// across its own provider turns, but the TUI's fleet roster
    /// (`src/tui/reducer.rs::apply_fleet_activity`) keys on `child`, the
    /// stable session id for the whole convocation. The desktop client mirrors
    /// that reduction, so it needs the same field.
    #[serde(rename_all = "camelCase")]
    SubagentActivity {
        child: String,
        label: String,
    },
    #[serde(rename_all = "camelCase")]
    SubagentSpawned {
        child: String,
        directive: String,
        directive_truncated: bool,
        branch: String,
        worktree: String,
    },
    #[serde(rename_all = "camelCase")]
    RecoveryRequired {
        work: Box<ClientRecoveryWork>,
    },
    #[serde(rename_all = "camelCase")]
    RecoveryResolved {
        decision: ClientRecoveryDecision,
    },
    SessionEnded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientRecoveryWork {
    pub run: String,
    pub kind: String,
    pub summary: String,
    pub effect_is_certain: bool,
    pub tool_name: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientUpdate {
    #[serde(rename_all = "camelCase")]
    Snapshot {
        snapshot: ClientSnapshot,
    },
    #[serde(rename_all = "camelCase")]
    Event {
        sequence: u64,
        event: ClientEvent,
    },
    #[serde(rename_all = "camelCase")]
    Resync {
        missed: u64,
        snapshot: ClientSnapshot,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientApprovalResolution {
    Deny,
    ApproveOnce,
    ApproveExactForSession,
    AutoByPolicy,
}

impl From<ApprovalDecision> for ClientApprovalResolution {
    fn from(decision: ApprovalDecision) -> Self {
        match decision {
            ApprovalDecision::Deny => Self::Deny,
            ApprovalDecision::ApproveOnce => Self::ApproveOnce,
            ApprovalDecision::ApproveExactForSession => Self::ApproveExactForSession,
            ApprovalDecision::AutoByPolicy => Self::AutoByPolicy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientFinishReason {
    Stop,
    ToolCalls,
    Incomplete,
    Cancelled,
    Handoff,
    QuotaDrained,
}

impl From<FinishReason> for ClientFinishReason {
    fn from(reason: FinishReason) -> Self {
        match reason {
            FinishReason::Stop => Self::Stop,
            FinishReason::ToolCalls => Self::ToolCalls,
            FinishReason::Incomplete => Self::Incomplete,
            FinishReason::Cancelled => Self::Cancelled,
            FinishReason::Handoff => Self::Handoff,
            FinishReason::QuotaDrained => Self::QuotaDrained,
        }
    }
}
