//! Persisted mirrors of recovery state.
//!
//! One reason to change: the stored shape of interrupted work.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::command::ApprovalId;
use crate::core::event::RunId;
use crate::core::recovery::{Authority, InterruptedKind, InterruptedWork, RecoveryDecision};
use crate::store::wire::enums::ToolTierWire;
use crate::store::wire::message::ToolCallWire;

/// What let an interrupted call start.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "authority", rename_all = "snake_case")]
pub(in crate::store) enum AuthorityWire {
    Policy,
    Approval { id: Uuid },
}

impl From<Authority> for AuthorityWire {
    fn from(authority: Authority) -> Self {
        match authority {
            Authority::Policy => Self::Policy,
            Authority::Approval(id) => Self::Approval { id: id.as_uuid() },
        }
    }
}

impl From<AuthorityWire> for Authority {
    fn from(authority: AuthorityWire) -> Self {
        match authority {
            AuthorityWire::Policy => Self::Policy,
            AuthorityWire::Approval { id } => Self::Approval(ApprovalId::from_uuid(id)),
        }
    }
}

/// How work was interrupted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "interrupted", rename_all = "snake_case")]
pub(in crate::store) enum InterruptedKindWire {
    ProposalUnapproved {
        call: ToolCallWire,
        tier: ToolTierWire,
        preview: String,
    },
    EffectUncertain {
        authority: AuthorityWire,
        call: ToolCallWire,
        tier: ToolTierWire,
        preview: String,
    },
    ProviderTurnInterrupted,
}

impl From<InterruptedKind> for InterruptedKindWire {
    fn from(kind: InterruptedKind) -> Self {
        match kind {
            InterruptedKind::ProposalUnapproved {
                call,
                tier,
                preview,
            } => Self::ProposalUnapproved {
                call: call.into(),
                tier: tier.into(),
                preview,
            },
            InterruptedKind::EffectUncertain {
                authority,
                call,
                tier,
                preview,
            } => Self::EffectUncertain {
                authority: authority.into(),
                call: call.into(),
                tier: tier.into(),
                preview,
            },
            InterruptedKind::ProviderTurnInterrupted => Self::ProviderTurnInterrupted,
        }
    }
}

impl From<InterruptedKindWire> for InterruptedKind {
    fn from(kind: InterruptedKindWire) -> Self {
        match kind {
            InterruptedKindWire::ProposalUnapproved {
                call,
                tier,
                preview,
            } => Self::ProposalUnapproved {
                call: call.into(),
                tier: tier.into(),
                preview,
            },
            InterruptedKindWire::EffectUncertain {
                authority,
                call,
                tier,
                preview,
            } => Self::EffectUncertain {
                authority: authority.into(),
                call: call.into(),
                tier: tier.into(),
                preview,
            },
            InterruptedKindWire::ProviderTurnInterrupted => Self::ProviderTurnInterrupted,
        }
    }
}

/// Work a crash interrupted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct InterruptedWorkWire {
    pub run: Uuid,
    pub kind: InterruptedKindWire,
}

impl From<InterruptedWork> for InterruptedWorkWire {
    fn from(work: InterruptedWork) -> Self {
        Self {
            run: work.run.as_uuid(),
            kind: work.kind.into(),
        }
    }
}

impl From<InterruptedWorkWire> for InterruptedWork {
    fn from(work: InterruptedWorkWire) -> Self {
        Self {
            run: RunId::from_uuid(work.run),
            kind: work.kind.into(),
        }
    }
}

/// A human's recovery decision.
///
/// There is no `Retry` here for the same reason there is none in
/// [`RecoveryDecision`]: a wire format that could express an automatic retry
/// would be a way to smuggle one back in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum RecoveryDecisionWire {
    AbandonAndContinue,
    EndSession,
}

impl From<RecoveryDecision> for RecoveryDecisionWire {
    fn from(decision: RecoveryDecision) -> Self {
        match decision {
            RecoveryDecision::AbandonAndContinue => Self::AbandonAndContinue,
            RecoveryDecision::EndSession => Self::EndSession,
        }
    }
}

impl From<RecoveryDecisionWire> for RecoveryDecision {
    fn from(decision: RecoveryDecisionWire) -> Self {
        match decision {
            RecoveryDecisionWire::AbandonAndContinue => Self::AbandonAndContinue,
            RecoveryDecisionWire::EndSession => Self::EndSession,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::message::ToolCall;
    use crate::core::tool::ToolTier;

    fn call() -> ToolCall {
        ToolCall {
            id: "call_1".to_owned(),
            name: "run_command".to_owned(),
            arguments: serde_json::json!({ "program": "git" }),
            provider_signature: None,
        }
    }

    #[test]
    fn every_interrupted_kind_survives_a_round_trip() {
        let kinds = [
            InterruptedKind::ProposalUnapproved {
                call: call(),
                tier: ToolTier::Execute,
                preview: "git diff".to_owned(),
            },
            InterruptedKind::EffectUncertain {
                authority: Authority::Policy,
                call: call(),
                tier: ToolTier::Write,
                preview: "diff".to_owned(),
            },
            InterruptedKind::EffectUncertain {
                authority: Authority::Approval(ApprovalId::new()),
                call: call(),
                tier: ToolTier::Execute,
                preview: "git diff".to_owned(),
            },
            InterruptedKind::ProviderTurnInterrupted,
        ];

        for kind in kinds {
            let work = InterruptedWork {
                run: RunId::new(),
                kind,
            };
            let json =
                serde_json::to_string(&InterruptedWorkWire::from(work.clone())).expect("serialize");
            let decoded: InterruptedWorkWire = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(InterruptedWork::from(decoded), work);
        }
    }

    #[test]
    fn the_wire_format_cannot_express_a_retry_decision() {
        // If someone adds RecoveryDecision::Retry, this fails to compile at the
        // `From` impl above rather than silently gaining a persisted form.
        for decision in [
            RecoveryDecision::AbandonAndContinue,
            RecoveryDecision::EndSession,
        ] {
            let json =
                serde_json::to_string(&RecoveryDecisionWire::from(decision)).expect("serialize");
            let decoded: RecoveryDecisionWire = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(RecoveryDecision::from(decoded), decision);
        }

        assert!(serde_json::from_str::<RecoveryDecisionWire>("\"retry\"").is_err());
    }
}
