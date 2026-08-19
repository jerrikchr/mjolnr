//! The persisted mirror of a session checkpoint.
//!
//! One reason to change: the stored shape of restorable session state.
//!
//! Like [`SessionCheckpoint`] itself, this type has **no field for approval
//! grants**. Both halves of the boundary have to be missing it for the
//! guarantee to hold: a core type that cannot hold a grant is useless if the
//! wire type reintroduces one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::checkpoint::SessionCheckpoint;
use crate::core::continuation::{
    CommandFact, HandoffCheckpoint, HandoffId, QuotaReserveBasis, QuotaReservePhase,
    QuotaReserveStatus,
};
use crate::core::event::SessionId;
use crate::core::model::{ModelId, ProviderId};
use crate::core::routing::RouteRuntime;
use crate::core::runtime::BudgetStatus;
use crate::core::store::SessionStatus;
use crate::store::wire::enums::{PolicyModeWire, UsageWire};
use crate::store::wire::message::MessageWire;

/// A session's live position on an attached route.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct RouteRuntimeWire {
    route: String,
    position: u32,
}

impl From<RouteRuntime> for RouteRuntimeWire {
    fn from(route: RouteRuntime) -> Self {
        Self {
            route: route.route,
            position: u32::try_from(route.position).unwrap_or(u32::MAX),
        }
    }
}

impl From<RouteRuntimeWire> for RouteRuntime {
    fn from(route: RouteRuntimeWire) -> Self {
        Self {
            route: route.route,
            position: route.position as usize,
        }
    }
}

/// Live budget counters.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct BudgetWire {
    pub provider_turns: u32,
    pub max_provider_turns: u32,
    pub tool_calls: u32,
    pub max_tool_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct CommandFactWire {
    command: String,
    outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct HandoffWire {
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    status: String,
    provider: String,
    model: String,
    files_read: Vec<PathBuf>,
    files_changed: Vec<PathBuf>,
    commands: Vec<CommandFactWire>,
    usage: UsageWire,
    budget: BudgetWire,
    activated_skills: Vec<String>,
}

impl From<HandoffCheckpoint> for HandoffWire {
    fn from(handoff: HandoffCheckpoint) -> Self {
        Self {
            id: handoff.id.as_uuid(),
            created_at: handoff.created_at,
            status: handoff.status,
            provider: handoff.provider.as_str().to_owned(),
            model: handoff.model.as_str().to_owned(),
            files_read: handoff.files_read,
            files_changed: handoff.files_changed,
            commands: handoff
                .commands
                .into_iter()
                .map(|fact| CommandFactWire {
                    command: fact.command,
                    outcome: fact.outcome,
                })
                .collect(),
            usage: handoff.usage.into(),
            budget: handoff.budget.into(),
            activated_skills: handoff.activated_skills,
        }
    }
}

impl From<HandoffWire> for HandoffCheckpoint {
    fn from(handoff: HandoffWire) -> Self {
        Self {
            id: HandoffId::from_uuid(handoff.id),
            created_at: handoff.created_at,
            status: handoff.status,
            provider: ProviderId::new(handoff.provider),
            model: ModelId::new(handoff.model),
            files_read: handoff.files_read,
            files_changed: handoff.files_changed,
            commands: handoff
                .commands
                .into_iter()
                .map(|fact| CommandFact {
                    command: fact.command,
                    outcome: fact.outcome,
                })
                .collect(),
            usage: handoff.usage.into(),
            budget: handoff.budget.into(),
            activated_skills: handoff.activated_skills,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub(in crate::store) enum QuotaBasisWire {
    ProviderReported { window: String },
    ConfiguredTokens { limit: u64 },
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum QuotaPhaseWire {
    Monitoring,
    Draining,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct QuotaReserveWire {
    basis: QuotaBasisWire,
    used_fraction: Option<f32>,
    soft_threshold: f32,
    hard_threshold: f32,
    #[serde(with = "time::serde::rfc3339::option")]
    resets_at: Option<time::OffsetDateTime>,
    phase: QuotaPhaseWire,
}

impl From<QuotaReserveStatus> for QuotaReserveWire {
    fn from(status: QuotaReserveStatus) -> Self {
        Self {
            basis: match status.basis {
                QuotaReserveBasis::ProviderReported { window } => {
                    QuotaBasisWire::ProviderReported { window }
                }
                QuotaReserveBasis::ConfiguredTokens { limit } => {
                    QuotaBasisWire::ConfiguredTokens { limit }
                }
                QuotaReserveBasis::Unavailable => QuotaBasisWire::Unavailable,
            },
            used_fraction: status.used_fraction,
            soft_threshold: status.soft_threshold,
            hard_threshold: status.hard_threshold,
            resets_at: status.resets_at,
            phase: match status.phase {
                QuotaReservePhase::Monitoring => QuotaPhaseWire::Monitoring,
                QuotaReservePhase::Draining => QuotaPhaseWire::Draining,
                QuotaReservePhase::Stopped => QuotaPhaseWire::Stopped,
            },
        }
    }
}

impl From<QuotaReserveWire> for QuotaReserveStatus {
    fn from(status: QuotaReserveWire) -> Self {
        Self {
            basis: match status.basis {
                QuotaBasisWire::ProviderReported { window } => {
                    QuotaReserveBasis::ProviderReported { window }
                }
                QuotaBasisWire::ConfiguredTokens { limit } => {
                    QuotaReserveBasis::ConfiguredTokens { limit }
                }
                QuotaBasisWire::Unavailable => QuotaReserveBasis::Unavailable,
            },
            used_fraction: status.used_fraction,
            soft_threshold: status.soft_threshold,
            hard_threshold: status.hard_threshold,
            resets_at: status.resets_at,
            phase: match status.phase {
                QuotaPhaseWire::Monitoring => QuotaReservePhase::Monitoring,
                QuotaPhaseWire::Draining => QuotaReservePhase::Draining,
                QuotaPhaseWire::Stopped => QuotaReservePhase::Stopped,
            },
        }
    }
}

impl From<BudgetStatus> for BudgetWire {
    fn from(budget: BudgetStatus) -> Self {
        Self {
            provider_turns: budget.provider_turns,
            max_provider_turns: budget.max_provider_turns,
            tool_calls: budget.tool_calls,
            max_tool_calls: budget.max_tool_calls,
        }
    }
}

impl From<BudgetWire> for BudgetStatus {
    fn from(budget: BudgetWire) -> Self {
        Self {
            provider_turns: budget.provider_turns,
            max_provider_turns: budget.max_provider_turns,
            tool_calls: budget.tool_calls,
            max_tool_calls: budget.max_tool_calls,
        }
    }
}

/// One observed file version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct ReadSetEntryWire {
    pub path: PathBuf,
    pub sha256: String,
}

/// One observed file version and the durable event that recorded it.
///
/// A separate wire struct from [`ReadSetEntryWire`] rather than an extra field
/// on it: the read set is keyed by absolute canonical path and the evidence by
/// the workspace-relative display path, and a single struct would invite the
/// two keys to be treated as interchangeable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct ReadEvidenceEntryWire {
    pub path: String,
    pub sha256: String,
    pub tool_event_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum SessionStatusWire {
    Active,
    Ended,
}

impl From<SessionStatus> for SessionStatusWire {
    fn from(status: SessionStatus) -> Self {
        match status {
            SessionStatus::Active => Self::Active,
            SessionStatus::Ended => Self::Ended,
        }
    }
}

impl From<SessionStatusWire> for SessionStatus {
    fn from(status: SessionStatusWire) -> Self {
        match status {
            SessionStatusWire::Active => Self::Active,
            SessionStatusWire::Ended => Self::Ended,
        }
    }
}

/// Everything about a session that is safe to restore.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct CheckpointWire {
    pub session: Uuid,
    pub status: SessionStatusWire,
    pub project_root: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub messages: Vec<MessageWire>,
    pub usage: UsageWire,
    pub policy: PolicyModeWire,
    pub budget: BudgetWire,
    pub read_set: Vec<ReadSetEntryWire>,
    /// `default` so a checkpoint written before Phase D3 still decodes: it
    /// simply carries no evidence, which is the truth about it.
    #[serde(default)]
    pub read_evidence: Vec<ReadEvidenceEntryWire>,
    /// Carried as `core::review::ReviewThread` directly, the narrow exception
    /// this module's header allows and the event payloads already take for the
    /// plan and review families: the type exists to be a durable record and has
    /// no second reader. `default` so a pre-D3 checkpoint decodes as carrying
    /// no notes, which is the truth about it.
    #[serde(default)]
    pub review_threads: Vec<crate::core::review::ReviewThread>,
    pub last_mutation_sequence: Option<u64>,
    pub successful_command_evidence: BTreeMap<String, u64>,
    #[serde(default)]
    pub activated_skills: Vec<String>,
    #[serde(default)]
    pub workspace_trusted: bool,
    #[serde(default)]
    pub handoff: Option<HandoffWire>,
    #[serde(default)]
    pub quota_reserve: Option<QuotaReserveWire>,
    #[serde(default)]
    pub route: Option<RouteRuntimeWire>,
}

impl From<SessionCheckpoint> for CheckpointWire {
    fn from(checkpoint: SessionCheckpoint) -> Self {
        Self {
            session: checkpoint.session.as_uuid(),
            status: checkpoint.status.into(),
            project_root: checkpoint.project_root,
            provider: checkpoint.provider.map(|id| id.as_str().to_owned()),
            model: checkpoint.model.map(|id| id.as_str().to_owned()),
            messages: checkpoint.messages.into_iter().map(Into::into).collect(),
            usage: checkpoint.usage.into(),
            policy: checkpoint.policy.into(),
            budget: checkpoint.budget.into(),
            read_set: checkpoint
                .read_set
                .into_iter()
                .map(|(path, sha256)| ReadSetEntryWire { path, sha256 })
                .collect(),
            read_evidence: checkpoint
                .read_evidence
                .into_iter()
                .map(|record| ReadEvidenceEntryWire {
                    path: record.path,
                    sha256: record.sha256,
                    tool_event_id: record.tool_event_id,
                })
                .collect(),
            review_threads: checkpoint.review_threads,
            last_mutation_sequence: checkpoint.last_mutation_sequence,
            successful_command_evidence: checkpoint.successful_command_evidence,
            activated_skills: checkpoint.activated_skills,
            workspace_trusted: checkpoint.workspace_trusted,
            handoff: checkpoint.handoff.map(Into::into),
            quota_reserve: Some(checkpoint.quota_reserve.into()),
            route: checkpoint.route.map(Into::into),
        }
    }
}

impl From<CheckpointWire> for SessionCheckpoint {
    fn from(checkpoint: CheckpointWire) -> Self {
        Self {
            session: SessionId::from_uuid(checkpoint.session),
            status: checkpoint.status.into(),
            project_root: checkpoint.project_root,
            provider: checkpoint.provider.map(ProviderId::new),
            model: checkpoint.model.map(ModelId::new),
            messages: checkpoint.messages.into_iter().map(Into::into).collect(),
            usage: checkpoint.usage.into(),
            policy: checkpoint.policy.into(),
            budget: checkpoint.budget.into(),
            read_set: checkpoint
                .read_set
                .into_iter()
                .map(|entry| (entry.path, entry.sha256))
                .collect(),
            read_evidence: checkpoint
                .read_evidence
                .into_iter()
                .map(|entry| crate::core::change_capture::ReadRecord {
                    path: entry.path,
                    sha256: entry.sha256,
                    tool_event_id: entry.tool_event_id,
                })
                .collect(),
            review_threads: checkpoint.review_threads,
            last_mutation_sequence: checkpoint.last_mutation_sequence,
            successful_command_evidence: checkpoint.successful_command_evidence,
            activated_skills: checkpoint.activated_skills,
            workspace_trusted: checkpoint.workspace_trusted,
            handoff: checkpoint.handoff.map(Into::into),
            quota_reserve: checkpoint
                .quota_reserve
                .map_or_else(QuotaReserveStatus::default, Into::into),
            route: checkpoint.route.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::message::CanonicalMessage;
    use crate::core::model::Usage;
    use crate::core::policy::PolicyMode;

    fn populated() -> SessionCheckpoint {
        SessionCheckpoint {
            session: SessionId::new(),
            status: SessionStatus::Ended,
            project_root: Some(PathBuf::from("/tmp/project")),
            provider: Some(ProviderId::new("fake")),
            model: Some(ModelId::new("fake-1")),
            messages: vec![CanonicalMessage::user("hello")],
            usage: Usage {
                input_tokens: 10,
                output_tokens: 3,
            },
            policy: PolicyMode::WorkspaceWrite,
            budget: BudgetStatus {
                provider_turns: 2,
                max_provider_turns: 8,
                tool_calls: 1,
                max_tool_calls: 16,
            },
            read_set: vec![(PathBuf::from("/tmp/project/a.rs"), "abc123".to_owned())],
            read_evidence: vec![crate::core::change_capture::ReadRecord::new(
                "a.rs".to_owned(),
                "abc123".to_owned(),
                "event-0".to_owned(),
            )],
            review_threads: vec![crate::core::review::ReviewThread::open(
                crate::core::review::ReviewThreadId::new(),
                crate::core::review::ReviewAnchor {
                    path: "a.rs".to_owned(),
                    side: crate::core::review::ReviewSide::New,
                    line: 12,
                    hunk_header: "@@ -10,3 +10,4 @@".to_owned(),
                    capture_digest: "9f8e7d".to_owned(),
                    base_object_id: Some("abc123".to_owned()),
                },
                crate::core::review::ReviewComment {
                    body: "handle the None case".to_owned(),
                    created_at: time::OffsetDateTime::UNIX_EPOCH,
                },
            )],
            last_mutation_sequence: Some(12),
            successful_command_evidence: BTreeMap::from([("event-1".to_owned(), 13)]),
            activated_skills: vec!["review".to_owned()],
            workspace_trusted: true,
            handoff: None,
            quota_reserve: QuotaReserveStatus::default(),
            route: Some(RouteRuntime {
                route: "main".to_owned(),
                position: 1,
            }),
        }
    }

    #[test]
    fn a_checkpoint_survives_a_round_trip_with_every_restorable_field() {
        let checkpoint = populated();
        let json =
            serde_json::to_string(&CheckpointWire::from(checkpoint.clone())).expect("serialize");
        let decoded: CheckpointWire = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(SessionCheckpoint::from(decoded), checkpoint);
    }

    #[test]
    fn a_phase_four_checkpoint_defaults_new_context_state_closed() {
        let mut value =
            serde_json::to_value(CheckpointWire::from(populated())).expect("checkpoint value");
        let object = value
            .as_object_mut()
            .expect("a checkpoint serialises to an object");
        object.remove("activated_skills");
        object.remove("workspace_trusted");
        object.remove("handoff");
        object.remove("quota_reserve");
        object.remove("read_evidence");
        object.remove("review_threads");

        let decoded: CheckpointWire = serde_json::from_value(value).expect("older checkpoint");
        let checkpoint = SessionCheckpoint::from(decoded);

        assert!(checkpoint.activated_skills.is_empty());
        assert!(!checkpoint.workspace_trusted);
        assert!(checkpoint.handoff.is_none());
        assert_eq!(checkpoint.quota_reserve, QuotaReserveStatus::default());
        // A checkpoint written before Phase D3 carries no evidence, and the
        // empty list is the truth about it — not a reason to refuse the
        // checkpoint, and not somewhere to invent a citation.
        assert!(checkpoint.read_evidence.is_empty());
        assert!(checkpoint.review_threads.is_empty());
    }

    #[test]
    fn the_persisted_form_carries_no_approval_grant() {
        // The wire half of the guarantee in core::checkpoint. Asserted against
        // the actual JSON, because that is what lands on disk.
        let json = serde_json::to_string(&CheckpointWire::from(populated())).expect("serialize");
        assert!(
            !json.contains("exact_command") && !json.contains("approval"),
            "an approval grant reached the database: {json}"
        );
    }

    #[test]
    fn a_grant_shaped_field_is_refused_on_load() {
        // deny_unknown_fields means a future build cannot quietly add one back
        // and have this build ignore it.
        let mut value = serde_json::to_value(CheckpointWire::from(populated())).expect("value");
        value
            .as_object_mut()
            .expect("a checkpoint serialises to an object")
            .insert(
                "exact_commands".to_owned(),
                serde_json::json!([{ "program": "rm" }]),
            );
        assert!(serde_json::from_value::<CheckpointWire>(value).is_err());
    }
}
