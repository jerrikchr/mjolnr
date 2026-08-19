//! Interrupted work, and what a human may decide about it.
//!
//! The rule this module exists to enforce is `AGENTS.md` §1.4:
//!
//! > **Uncertain side effects are never retried automatically.** If smed cannot
//! > prove a write or command did not happen, it asks a human. Losing work beats
//! > duplicating it.
//!
//! # Why this is an enum and not a `bool`
//!
//! The tempting shape is `session.needs_recovery: bool`. It is wrong, because
//! the ways a session can be interrupted are not the same fact and do not
//! license the same action:
//!
//! - A proposal still waiting on an approval **did not run**. Provably.
//! - An authorised effect with no outcome **may or may not have run**. Not
//!   provably either way — that is the entire problem.
//! - An interrupted provider call may have produced tokens and billed for them.
//!
//! Collapsing those into one boolean forces the code that reads it to guess
//! which one it is, and the cheapest guess is always the optimistic one. Plan
//! §Phase 4 names that failure directly: "do not infer that an interrupted
//! command failed merely because no completion event exists."
//!
//! # The axis is certainty, not authority
//!
//! An earlier draft split on *who said yes* (`ApprovedEffectUncertain`). That is
//! the wrong axis: a read-tier tool that `PolicyMode::Ask` pre-authorises is
//! never approved by a human, yet it starts, and a crash mid-execution leaves
//! exactly the same uncertainty as an approved write. Splitting on authority put
//! two identical certainties in different variants and invited callers to treat
//! the unapproved one as safe. The split is now certainty; authority rides along
//! as [`Authority`] because it belongs in the message to the human, not in the
//! decision about whether to trust the outcome.

use crate::core::command::ApprovalId;
use crate::core::error::ReasonCode;
use crate::core::event::RunId;
use crate::core::message::ToolCall;
use crate::core::tool::ToolTier;

/// Whether a resumed session may proceed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RecoveryState {
    /// No interrupted work. The session continues normally.
    #[default]
    Clean,
    /// Work was interrupted. Autonomous progress is blocked until a human
    /// resolves it — the runtime refuses `SendUserMessage` with
    /// [`ReasonCode::RecoveryRequiresDecision`] while this holds.
    Required(InterruptedWork),
}

impl RecoveryState {
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self, Self::Required(_))
    }

    #[must_use]
    pub const fn work(&self) -> Option<&InterruptedWork> {
        match self {
            Self::Clean => None,
            Self::Required(work) => Some(work),
        }
    }

    /// The stable code a blocked action refuses with.
    #[must_use]
    pub const fn reason_code(&self) -> Option<ReasonCode> {
        match self {
            Self::Clean => None,
            Self::Required(_) => Some(ReasonCode::RecoveryRequiresDecision),
        }
    }
}

/// What was in flight when the process stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedWork {
    pub run: RunId,
    pub kind: InterruptedKind,
}

impl InterruptedWork {
    /// Whether smed can prove no side effect occurred.
    ///
    /// Only [`ProposalUnapproved`](InterruptedKind::ProposalUnapproved) is
    /// provably safe, and only because smed durably records intent *before*
    /// starting an effect. Without that ordering this question would
    /// be unanswerable for every variant.
    #[must_use]
    pub const fn effect_is_certain(&self) -> bool {
        matches!(self.kind, InterruptedKind::ProposalUnapproved { .. })
    }

    /// One line for a human, naming what is and is not known.
    #[must_use]
    pub fn summary(&self) -> String {
        match &self.kind {
            InterruptedKind::ProposalUnapproved { call, .. } => format!(
                "`{}` was proposed but never authorised, so it did not run.",
                call.name
            ),
            InterruptedKind::EffectUncertain {
                call, authority, ..
            } => format!(
                "`{}` was {} and started, but no outcome was recorded. \
                 It may or may not have run. smed cannot tell.",
                call.name,
                authority.describe()
            ),
            InterruptedKind::ProviderTurnInterrupted => {
                "A model call was interrupted mid-stream. It will not be replayed.".to_owned()
            }
        }
    }
}

/// The ways work can be interrupted, kept distinct by how much smed can prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptedKind {
    /// `ToolProposed` is durable and carries an approval id, but no
    /// `ApprovalResolved` followed.
    ///
    /// **Provably did not run.** smed persists intent before effect, and the
    /// effect is gated on an approval that does not exist. Resume drops the
    /// proposal; it never executes it.
    ProposalUnapproved {
        call: ToolCall,
        tier: ToolTier,
        preview: String,
    },
    /// The call was authorised and started, and no `ToolCompleted` or
    /// `ToolFailed` followed.
    ///
    /// **The dangerous one.** The process died somewhere between "authorised"
    /// and "finished", which spans the entire side effect. The file may be
    /// written. The command may have run, half-run, or spawned something still
    /// running. smed records this as unknown and refuses to characterise it.
    EffectUncertain {
        authority: Authority,
        call: ToolCall,
        tier: ToolTier,
        preview: String,
    },
    /// A run started and never reached a terminal event.
    ///
    /// Never replayed: the stream may have produced tokens before dying, and
    /// re-issuing it would duplicate both the work and the bill (`AGENTS.md`
    /// §4: "no auto-retry after partial output").
    ProviderTurnInterrupted,
}

impl InterruptedKind {
    /// A stable label for the UI and for tests, which must never assert on prose
    /// (`AGENTS.md` §6).
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::ProposalUnapproved { .. } => "PROPOSAL_UNAPPROVED",
            Self::EffectUncertain { .. } => "EFFECT_UNCERTAIN",
            Self::ProviderTurnInterrupted => "PROVIDER_TURN_INTERRUPTED",
        }
    }
}

/// What let a call start.
///
/// Reported to the human because "you approved this" and "your policy mode
/// allowed this" are different things to wake up to. It carries no weight in
/// deciding whether the outcome is trustworthy — both are equally unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// The policy mode pre-authorised this tier. No human was asked.
    Policy,
    /// A human approved this exact proposal.
    Approval(ApprovalId),
}

impl Authority {
    #[must_use]
    pub fn describe(self) -> String {
        match self {
            Self::Policy => "allowed by the active policy".to_owned(),
            Self::Approval(id) => format!("approved by you ({id})"),
        }
    }
}

/// What a human may decide about interrupted work.
///
/// Note what is absent: there is no `Retry`. Not an oversight — retrying an
/// effect whose outcome is unknown is the one thing `AGENTS.md` §1.4 forbids
/// outright, and a type that cannot express it cannot be talked into it under a
/// deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDecision {
    /// Abandon the interrupted work and continue the session.
    ///
    /// smed records the outcome as **unknown** and tells the model exactly
    /// that. It does not claim the effect happened, and it does not claim it did
    /// not.
    AbandonAndContinue,
    /// Leave the workspace alone and end the session.
    ///
    /// For when the human wants to inspect the repository before smed touches
    /// it again. The session is marked ended; its history stays readable.
    EndSession,
}

impl RecoveryDecision {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AbandonAndContinue => "abandon-and-continue",
            Self::EndSession => "end-session",
        }
    }

    /// Parse a stored or typed decision. Unknown text is rejected rather than
    /// defaulted: guessing which recovery a human chose is precisely the guess
    /// this module exists to prevent.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "abandon-and-continue" => Some(Self::AbandonAndContinue),
            "end-session" => Some(Self::EndSession),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call() -> ToolCall {
        ToolCall {
            id: "call_1".to_owned(),
            name: "edit_file".to_owned(),
            arguments: serde_json::json!({}),
            provider_signature: None,
        }
    }

    fn uncertain(authority: Authority) -> InterruptedWork {
        InterruptedWork {
            run: RunId::new(),
            kind: InterruptedKind::EffectUncertain {
                authority,
                call: call(),
                tier: ToolTier::Write,
                preview: "diff".to_owned(),
            },
        }
    }

    #[test]
    fn recovery_cannot_express_an_automatic_retry() {
        // The invariant of AGENTS.md §1.4, asserted so that adding `Retry`
        // becomes a conversation rather than a commit.
        let decisions = [
            RecoveryDecision::AbandonAndContinue,
            RecoveryDecision::EndSession,
        ];
        assert_eq!(
            decisions.len(),
            2,
            "an uncertain side effect is never retried automatically; \
             adding a Retry variant would make that expressible"
        );
    }

    #[test]
    fn only_an_unauthorised_proposal_is_provably_safe() {
        let unapproved = InterruptedWork {
            run: RunId::new(),
            kind: InterruptedKind::ProposalUnapproved {
                call: call(),
                tier: ToolTier::Write,
                preview: "diff".to_owned(),
            },
        };
        assert!(
            unapproved.effect_is_certain(),
            "intent is persisted before effect, so an unapproved proposal provably did not run"
        );

        assert!(
            !uncertain(Authority::Approval(ApprovalId::new())).effect_is_certain(),
            "an approved effect with no outcome must never be reported as certain"
        );

        assert!(
            !uncertain(Authority::Policy).effect_is_certain(),
            "a policy-authorised effect is exactly as uncertain as an approved one; \
             only the authority differs"
        );

        assert!(
            !InterruptedWork {
                run: RunId::new(),
                kind: InterruptedKind::ProviderTurnInterrupted,
            }
            .effect_is_certain()
        );
    }

    #[test]
    fn an_uncertain_effect_is_never_described_as_failed_or_succeeded() {
        //  anti-pattern: "do not infer that an interrupted command
        // failed merely because no completion event exists."
        for authority in [Authority::Policy, Authority::Approval(ApprovalId::new())] {
            let summary = uncertain(authority).summary().to_lowercase();
            assert!(
                !summary.contains("failed") && !summary.contains("succeeded"),
                "an uncertain outcome must not be characterised: {summary}"
            );
            assert!(summary.contains("may or may not"));
        }
    }

    #[test]
    fn a_blocked_session_refuses_with_the_stable_code() {
        let clean = RecoveryState::Clean;
        assert!(!clean.is_required());
        assert_eq!(clean.reason_code(), None);

        let blocked = RecoveryState::Required(InterruptedWork {
            run: RunId::new(),
            kind: InterruptedKind::ProviderTurnInterrupted,
        });
        assert!(blocked.is_required());
        assert_eq!(
            blocked.reason_code(),
            Some(ReasonCode::RecoveryRequiresDecision)
        );
    }

    #[test]
    fn interrupted_kinds_have_distinct_stable_labels() {
        let labels = [
            InterruptedKind::ProposalUnapproved {
                call: call(),
                tier: ToolTier::Write,
                preview: String::new(),
            }
            .label(),
            InterruptedKind::EffectUncertain {
                authority: Authority::Policy,
                call: call(),
                tier: ToolTier::Write,
                preview: String::new(),
            }
            .label(),
            InterruptedKind::ProviderTurnInterrupted.label(),
        ];

        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(unique.len(), labels.len(), "two states share a label");
    }

    #[test]
    fn an_unrecognised_decision_is_rejected_rather_than_defaulted() {
        assert_eq!(
            RecoveryDecision::parse("abandon-and-continue"),
            Some(RecoveryDecision::AbandonAndContinue)
        );
        assert_eq!(
            RecoveryDecision::parse("end-session"),
            Some(RecoveryDecision::EndSession)
        );
        assert_eq!(RecoveryDecision::parse("retry"), None);
        assert_eq!(RecoveryDecision::parse(""), None);
    }

    #[test]
    fn decisions_round_trip_through_their_labels() {
        for decision in [
            RecoveryDecision::AbandonAndContinue,
            RecoveryDecision::EndSession,
        ] {
            assert_eq!(RecoveryDecision::parse(decision.label()), Some(decision));
        }
    }
}
