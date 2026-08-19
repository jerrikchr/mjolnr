//! Deterministic policy gates. Models propose; this module disposes.

pub mod paths;

use crate::core::error::ReasonCode;
use crate::core::policy::PolicyMode;
use crate::core::tool::ToolTier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Ask,
    Deny(ReasonCode),
}

/// Decide whether a classified tool may run. Exact-command approval is a
/// session-scoped fact supplied by the runtime; this module never grants it.
#[must_use]
pub const fn decide(
    mode: PolicyMode,
    tier: ToolTier,
    exact_command_approved: bool,
) -> PolicyDecision {
    match (mode, tier) {
        (_, ToolTier::Read)
        | (PolicyMode::WorkspaceWrite, ToolTier::Write)
        | (PolicyMode::FullAuto, ToolTier::Write | ToolTier::Execute) => PolicyDecision::Allow,
        (PolicyMode::ReadOnly, ToolTier::Write | ToolTier::Execute) => {
            PolicyDecision::Deny(ReasonCode::PolicyReadOnly)
        }
        (PolicyMode::Ask | PolicyMode::WorkspaceWrite, ToolTier::Execute)
            if exact_command_approved =>
        {
            PolicyDecision::Allow
        }
        (PolicyMode::Ask, ToolTier::Write | ToolTier::Execute)
        | (PolicyMode::WorkspaceWrite, ToolTier::Execute) => PolicyDecision::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_fail_closed_at_the_documented_boundaries() {
        assert_eq!(
            decide(PolicyMode::ReadOnly, ToolTier::Write, false),
            PolicyDecision::Deny(ReasonCode::PolicyReadOnly)
        );
        assert_eq!(
            decide(PolicyMode::Ask, ToolTier::Execute, false),
            PolicyDecision::Ask
        );
        assert_eq!(
            decide(PolicyMode::WorkspaceWrite, ToolTier::Write, false),
            PolicyDecision::Allow
        );
        assert_eq!(
            decide(PolicyMode::WorkspaceWrite, ToolTier::Execute, false),
            PolicyDecision::Ask
        );
    }

    #[test]
    fn full_auto_allows_gated_tiers() {
        assert_eq!(
            decide(PolicyMode::FullAuto, ToolTier::Write, false),
            PolicyDecision::Allow
        );
        assert_eq!(
            decide(PolicyMode::FullAuto, ToolTier::Execute, false),
            PolicyDecision::Allow
        );
    }
}
