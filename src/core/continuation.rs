//! Durable continuation state for quota-aware handoffs and compact resumes.

use std::path::PathBuf;

use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::model::{ModelId, ProviderId, Usage};
use crate::core::runtime::BudgetStatus;

/// Stable identity of one model-written handoff artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandoffId(Uuid);

impl HandoffId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for HandoffId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for HandoffId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Mechanically observed command outcome included beside model-authored status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFact {
    pub command: String,
    pub outcome: String,
}

/// Durable, provider-neutral landing point for later continuation.
#[derive(Debug, Clone, PartialEq)]
pub struct HandoffCheckpoint {
    pub id: HandoffId,
    pub created_at: OffsetDateTime,
    /// Model-authored done/remaining/next-steps/open-risks status.
    pub status: String,
    pub provider: ProviderId,
    pub model: ModelId,
    pub files_read: Vec<PathBuf>,
    pub files_changed: Vec<PathBuf>,
    pub commands: Vec<CommandFact>,
    pub usage: Usage,
    pub budget: BudgetStatus,
    pub activated_skills: Vec<String>,
}

impl HandoffCheckpoint {
    /// Canonical, inspectable seed sent during a compact continuation.
    #[must_use]
    pub fn compact_seed(&self) -> String {
        format!(
            "MJOLNR COMPACT RESUME // handoff {} created {}\n\n{}\n\nMechanical facts:\n- files read: {}\n- files changed: {}\n- commands recorded: {}\n- prior usage: {} input + {} output tokens\n- active skills: {}",
            self.id,
            self.created_at,
            self.status,
            self.files_read.len(),
            self.files_changed.len(),
            self.commands.len(),
            self.usage.input_tokens,
            self.usage.output_tokens,
            if self.activated_skills.is_empty() {
                "none".to_owned()
            } else {
                self.activated_skills.join(", ")
            }
        )
    }
}

/// Why the resume advisor is interrupting the normal open path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeWarning {
    QuotaStopped { resets_at: Option<OffsetDateTime> },
    Stale { idle_seconds: u64 },
}

/// Zero-request advice computed solely from durable session records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAdvice {
    pub warning: ResumeWarning,
    pub estimated_full_resume_tokens: u64,
    pub handoff: Option<HandoffId>,
}

/// Explicit answer to a resume warning. Enter is intentionally not an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeChoice {
    Compact,
    NewFromHandoff,
    Full,
}

/// Durable quota-reserve state. Fractions only exist when reported by a
/// provider; configured-token fallback records its token limit explicitly.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaReserveStatus {
    pub basis: QuotaReserveBasis,
    pub used_fraction: Option<f32>,
    pub soft_threshold: f32,
    pub hard_threshold: f32,
    pub resets_at: Option<OffsetDateTime>,
    pub phase: QuotaReservePhase,
}

impl Default for QuotaReserveStatus {
    fn default() -> Self {
        Self {
            basis: QuotaReserveBasis::Unavailable,
            used_fraction: None,
            soft_threshold: 0.8,
            hard_threshold: 0.95,
            resets_at: None,
            phase: QuotaReservePhase::Monitoring,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaReserveBasis {
    ProviderReported { window: String },
    ConfiguredTokens { limit: u64 },
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaReservePhase {
    Monitoring,
    Draining,
    Stopped,
}
