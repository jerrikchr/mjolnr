//! Provider-neutral policy and approval types.

use crate::core::command::ApprovalId;
use crate::core::tool::ToolTier;

/// The policy modes mjolnr exposes. There is deliberately no unrestricted mode:
/// full-auto changes approval handling, not structural guards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyMode {
    ReadOnly,
    #[default]
    Ask,
    WorkspaceWrite,
    FullAuto,
}

impl PolicyMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Ask => "ask",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAuto => "full-auto",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::ReadOnly | Self::FullAuto => Self::Ask,
            Self::Ask => Self::WorkspaceWrite,
            Self::WorkspaceWrite => Self::ReadOnly,
        }
    }

    #[must_use]
    pub const fn is_full_auto(self) -> bool {
        matches!(self, Self::FullAuto)
    }

    /// How much authority this mode carries, for comparison only.
    ///
    /// `ask` ranks below `workspace-write` because the question is what happens
    /// *without a human*: `ask` auto-resolves nothing. The numbers are not
    /// stable API and nothing should switch on them — they exist so
    /// [`narrower_of`](Self::narrower_of) can be written once instead of as a
    /// sixteen-arm match nobody can audit.
    #[must_use]
    pub const fn width(self) -> u8 {
        match self {
            Self::ReadOnly => 0,
            Self::Ask => 1,
            Self::WorkspaceWrite => 2,
            Self::FullAuto => 3,
        }
    }

    /// The narrower of two modes.
    ///
    /// The only combinator a *ceiling* may use. Distinct from
    /// `subagent::clamp_policy` and [`envelope::clamp_ceiling`] on purpose:
    /// those collapse `ask` to `workspace-write` because a child has no human
    /// to ask, and applying that reasoning to a session with a human attached
    /// would turn a narrowing into a widening.
    ///
    /// [`envelope::clamp_ceiling`]: crate::core::envelope::clamp_ceiling
    #[must_use]
    pub const fn narrower_of(self, other: Self) -> Self {
        if self.width() <= other.width() {
            self
        } else {
            other
        }
    }

    /// The mode a session inherits when it continues this one's work in a new
    /// session — a resume, a fork, or a clone.
    ///
    /// Never wider than `self`: a new session must not be a way to launder a
    /// narrow policy into a wide one. Carrying the mode
    /// forward unchanged already satisfies that, so the only transformation is
    /// full-auto, which is downgraded rather than inherited.
    ///
    /// Full-auto is a thing a human turns on for a stretch of work they are
    /// watching. A session that comes back without them doing anything is not
    /// that stretch of work, and resuming into unattended autonomy is the one
    /// widening that costs nothing to ask for and everything to get wrong.
    #[must_use]
    pub const fn carried_forward(self) -> Self {
        match self {
            Self::FullAuto => Self::Ask,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_carried_policy_is_never_wider_than_its_source() {
        // The anti-laundering rule, stated over every mode: continuing work in
        // a new session must not hand it authority the old one did not have.
        assert_eq!(PolicyMode::ReadOnly.carried_forward(), PolicyMode::ReadOnly);
        assert_eq!(PolicyMode::Ask.carried_forward(), PolicyMode::Ask);
        assert_eq!(
            PolicyMode::WorkspaceWrite.carried_forward(),
            PolicyMode::WorkspaceWrite
        );
        assert_eq!(
            PolicyMode::FullAuto.carried_forward(),
            PolicyMode::Ask,
            "unattended autonomy must be re-armed by a human, never inherited"
        );
    }

    #[test]
    fn narrower_of_is_commutative_and_never_widens() {
        // A ceiling combinator that is not commutative is a ceiling that
        // depends on argument order, which is a bug waiting for the one call
        // site that passes them the other way round.
        const MODES: [PolicyMode; 4] = [
            PolicyMode::ReadOnly,
            PolicyMode::Ask,
            PolicyMode::WorkspaceWrite,
            PolicyMode::FullAuto,
        ];
        for left in MODES {
            for right in MODES {
                let narrowed = left.narrower_of(right);
                assert_eq!(narrowed, right.narrower_of(left));
                assert!(narrowed.width() <= left.width());
                assert!(narrowed.width() <= right.width());
            }
        }
    }

    #[test]
    fn ask_is_narrower_than_workspace_write() {
        // The ordering claim the whole ceiling rests on: what matters is what
        // happens with no human in the loop, and `ask` auto-resolves nothing.
        assert_eq!(
            PolicyMode::Ask.narrower_of(PolicyMode::WorkspaceWrite),
            PolicyMode::Ask
        );
    }

    #[test]
    fn policy_cycle_has_no_unrestricted_state() {
        assert_eq!(PolicyMode::ReadOnly.next(), PolicyMode::Ask);
        assert_eq!(PolicyMode::Ask.next(), PolicyMode::WorkspaceWrite);
        assert_eq!(PolicyMode::WorkspaceWrite.next(), PolicyMode::ReadOnly);
    }

    #[test]
    fn full_auto_is_not_reachable_but_is_easy_to_exit() {
        let mut mode = PolicyMode::ReadOnly;
        for _ in 0..6 {
            mode = mode.next();
            assert_ne!(mode, PolicyMode::FullAuto);
        }
        assert_eq!(PolicyMode::FullAuto.next(), PolicyMode::Ask);
    }
}

/// What the runtime is waiting for a human to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub id: ApprovalId,
    pub tool_name: String,
    pub tier: ToolTier,
    /// Bounded, exact action display. Commands show argv exactly; edits show a
    /// review diff. It is never interpreted as executable syntax.
    pub preview: String,
}
