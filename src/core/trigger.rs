//! Trigger display and outcome vocabulary.
//!
//! Two things live here rather than in the `triggers` module that loads and
//! drives them: [`OverlapPolicy`], [`TriggerOutcome`], and [`TriggerStatus`]
//! are values that cross the `core` boundary — [`TriggerStatus`] rides in
//! [`RuntimeSnapshot`](crate::core::runtime::RuntimeSnapshot) so the TUI can
//! render it without depending on the scheduler, exactly as
//! [`McpServerSummary`](crate::core::mcp::McpServerSummary) does for MCP.
//!
//! What is deliberately *not* here: the loaded trigger definition (schedule
//! text, directive, budgets, provider/model). That type reaches into
//! `runtime::budget::BudgetLimits` and `core` may depend on nothing that
//! implements a contract (`AGENTS.md` §2.1), so it lives in the `triggers`
//! module instead.

use time::OffsetDateTime;

use crate::core::error::ReasonCode;

/// What a trigger fires from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSourceKind {
    Schedule,
    Webhook,
}

impl TriggerSourceKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Webhook => "webhook",
        }
    }
}

/// What happens when a firing is still running and its trigger fires again.
///
/// No fourth option. A workflow DSL would grow queue depth, priorities, and
/// merge rules;  forbids exactly that ("no workflow DSL beyond
/// trigger + directive").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapPolicy {
    /// Drop this occurrence; the in-flight firing keeps running.
    Skip,
    /// Hold at most one occurrence until the in-flight firing settles.
    Queue,
    /// Cancel the in-flight firing and start this occurrence.
    Replace,
}

impl OverlapPolicy {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Queue => "queue",
            Self::Replace => "replace",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "skip" => Some(Self::Skip),
            "queue" => Some(Self::Queue),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }
}

/// What one firing came to — the same shape as
/// [`HeadlessOutcome`](crate::headless::HeadlessOutcome), declared
/// independently because `core` may not depend on the top-level `headless`
/// module. The scheduler maps one onto the other at the one call site that
/// needs both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerOutcome {
    Verified,
    Refused,
    BudgetOrQuotaStopped,
    Failed,
}

impl TriggerOutcome {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Refused => "refused",
            Self::BudgetOrQuotaStopped => "budget/quota stopped",
            Self::Failed => "failed",
        }
    }

    /// Whether this outcome counts toward the disable-after-failure counter.
    ///
    /// A refusal or a quota stop is not "the trigger is broken" — the first is
    /// the policy gate working as designed, the second is exactly what the
    /// quota-drain handoff exists to land safely. Only [`Failed`](Self::Failed)
    /// is evidence the directive itself cannot complete.
    #[must_use]
    pub const fn counts_as_failure(self) -> bool {
        matches!(self, Self::Failed)
    }
}

/// What a client may render about one configured trigger.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerStatus {
    pub name: String,
    pub source: TriggerSourceKind,
    pub overlap: OverlapPolicy,
    pub enabled: bool,
    pub disabled_reason: Option<ReasonCode>,
    pub consecutive_failures: u32,
    pub max_consecutive_failures: u32,
    pub last_outcome: Option<TriggerOutcome>,
    pub last_fired_at: Option<OffsetDateTime>,
    /// `None` for a webhook trigger, or a schedule trigger that is disabled.
    pub next_fire_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_policy_round_trips_its_wire_form() {
        for policy in [
            OverlapPolicy::Skip,
            OverlapPolicy::Queue,
            OverlapPolicy::Replace,
        ] {
            assert_eq!(OverlapPolicy::parse(policy.label()), Some(policy));
        }
        assert_eq!(OverlapPolicy::parse("merge"), None);
    }

    #[test]
    fn only_failed_counts_toward_disable() {
        assert!(TriggerOutcome::Failed.counts_as_failure());
        assert!(!TriggerOutcome::Verified.counts_as_failure());
        assert!(!TriggerOutcome::Refused.counts_as_failure());
        assert!(!TriggerOutcome::BudgetOrQuotaStopped.counts_as_failure());
    }
}
