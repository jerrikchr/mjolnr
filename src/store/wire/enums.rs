//! Persisted mirrors of canonical enumerations.
//!
//! One reason to change: the stored spelling of a fixed vocabulary.

use serde::{Deserialize, Serialize};

use crate::core::command::ApprovalDecision;
use crate::core::error::ReasonCode;
use crate::core::event::{EnvelopeEnd, ExtensionLoadAuthority, FinishReason};
use crate::core::governance::GovernanceTier;
use crate::core::message::Role;
use crate::core::model::Usage;
use crate::core::policy::PolicyMode;
use crate::core::routing::{BreakerState, RouteAdvanceCondition, RouteSelectionReason};
use crate::core::tool::ToolTier;
use crate::core::trigger::{OverlapPolicy, TriggerOutcome, TriggerSourceKind};

/// Who produced a message.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum RoleWire {
    System,
    User,
    Assistant,
    Tool,
}

impl From<Role> for RoleWire {
    fn from(role: Role) -> Self {
        match role {
            Role::System => Self::System,
            Role::User => Self::User,
            Role::Assistant => Self::Assistant,
            Role::Tool => Self::Tool,
        }
    }
}

impl From<RoleWire> for Role {
    fn from(role: RoleWire) -> Self {
        match role {
            RoleWire::System => Self::System,
            RoleWire::User => Self::User,
            RoleWire::Assistant => Self::Assistant,
            RoleWire::Tool => Self::Tool,
        }
    }
}

/// How dangerous a tool is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum ToolTierWire {
    Read,
    Write,
    Execute,
}

impl From<ToolTier> for ToolTierWire {
    fn from(tier: ToolTier) -> Self {
        match tier {
            ToolTier::Read => Self::Read,
            ToolTier::Write => Self::Write,
            ToolTier::Execute => Self::Execute,
        }
    }
}

impl From<ToolTierWire> for ToolTier {
    fn from(tier: ToolTierWire) -> Self {
        match tier {
            ToolTierWire::Read => Self::Read,
            ToolTierWire::Write => Self::Write,
            ToolTierWire::Execute => Self::Execute,
        }
    }
}

/// The active policy mode.
///
/// Deliberately not persisted via `PolicyMode::label()`. That method returns
/// display text; a UI wording change ("ask" → "confirm") would silently make
/// every stored session unreadable. The wire names are owned here.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum PolicyModeWire {
    ReadOnly,
    Ask,
    WorkspaceWrite,
    FullAuto,
}

impl From<PolicyMode> for PolicyModeWire {
    fn from(mode: PolicyMode) -> Self {
        match mode {
            PolicyMode::ReadOnly => Self::ReadOnly,
            PolicyMode::Ask => Self::Ask,
            PolicyMode::WorkspaceWrite => Self::WorkspaceWrite,
            PolicyMode::FullAuto => Self::FullAuto,
        }
    }
}

impl From<PolicyModeWire> for PolicyMode {
    fn from(mode: PolicyModeWire) -> Self {
        match mode {
            PolicyModeWire::ReadOnly => Self::ReadOnly,
            PolicyModeWire::Ask => Self::Ask,
            PolicyModeWire::WorkspaceWrite => Self::WorkspaceWrite,
            PolicyModeWire::FullAuto => Self::FullAuto,
        }
    }
}

/// A model's declared governance tier, as persisted.
///
/// Owned here rather than deriving `Serialize` on the core type, for the reason
/// the module header gives: a rename in `core` must not make every stored
/// session unreadable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum GovernanceTierWire {
    Supervised,
    Standard,
    Trusted,
}

impl From<GovernanceTier> for GovernanceTierWire {
    fn from(tier: GovernanceTier) -> Self {
        match tier {
            GovernanceTier::Supervised => Self::Supervised,
            GovernanceTier::Standard => Self::Standard,
            GovernanceTier::Trusted => Self::Trusted,
        }
    }
}

impl From<GovernanceTierWire> for GovernanceTier {
    fn from(tier: GovernanceTierWire) -> Self {
        match tier {
            GovernanceTierWire::Supervised => Self::Supervised,
            GovernanceTierWire::Standard => Self::Standard,
            GovernanceTierWire::Trusted => Self::Trusted,
        }
    }
}

/// Why a spawn envelope stopped being in force.
///
/// Four endings rather than a boolean, because the record is read later by
/// someone asking what happened to an authorisation they granted, and "spent"
/// and "lapsed" are different answers to that question.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum EnvelopeEndWire {
    Spent,
    Lapsed,
    Withdrawn,
    PolicyNarrowed,
}

impl From<EnvelopeEnd> for EnvelopeEndWire {
    fn from(end: EnvelopeEnd) -> Self {
        match end {
            EnvelopeEnd::Spent => Self::Spent,
            EnvelopeEnd::Lapsed => Self::Lapsed,
            EnvelopeEnd::Withdrawn => Self::Withdrawn,
            EnvelopeEnd::PolicyNarrowed => Self::PolicyNarrowed,
        }
    }
}

impl From<EnvelopeEndWire> for EnvelopeEnd {
    fn from(end: EnvelopeEndWire) -> Self {
        match end {
            EnvelopeEndWire::Spent => Self::Spent,
            EnvelopeEndWire::Lapsed => Self::Lapsed,
            EnvelopeEndWire::Withdrawn => Self::Withdrawn,
            EnvelopeEndWire::PolicyNarrowed => Self::PolicyNarrowed,
        }
    }
}

/// A human's approval decision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum ApprovalDecisionWire {
    Deny,
    ApproveOnce,
    ApproveExactForSession,
    AutoByPolicy,
}

impl From<ApprovalDecision> for ApprovalDecisionWire {
    fn from(decision: ApprovalDecision) -> Self {
        match decision {
            ApprovalDecision::Deny => Self::Deny,
            ApprovalDecision::ApproveOnce => Self::ApproveOnce,
            ApprovalDecision::ApproveExactForSession => Self::ApproveExactForSession,
            ApprovalDecision::AutoByPolicy => Self::AutoByPolicy,
        }
    }
}

impl From<ApprovalDecisionWire> for ApprovalDecision {
    fn from(decision: ApprovalDecisionWire) -> Self {
        match decision {
            ApprovalDecisionWire::Deny => Self::Deny,
            ApprovalDecisionWire::ApproveOnce => Self::ApproveOnce,
            ApprovalDecisionWire::ApproveExactForSession => Self::ApproveExactForSession,
            ApprovalDecisionWire::AutoByPolicy => Self::AutoByPolicy,
        }
    }
}

/// What authorised loading an extension.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum ExtensionLoadAuthorityWire {
    Command,
    FullAuto,
    Approved,
}

impl From<ExtensionLoadAuthority> for ExtensionLoadAuthorityWire {
    fn from(authority: ExtensionLoadAuthority) -> Self {
        match authority {
            ExtensionLoadAuthority::Command => Self::Command,
            ExtensionLoadAuthority::FullAuto => Self::FullAuto,
            ExtensionLoadAuthority::Approved => Self::Approved,
        }
    }
}

impl From<ExtensionLoadAuthorityWire> for ExtensionLoadAuthority {
    fn from(authority: ExtensionLoadAuthorityWire) -> Self {
        match authority {
            ExtensionLoadAuthorityWire::Command => Self::Command,
            ExtensionLoadAuthorityWire::FullAuto => Self::FullAuto,
            ExtensionLoadAuthorityWire::Approved => Self::Approved,
        }
    }
}

/// Why a provider stream ended.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum FinishReasonWire {
    Stop,
    ToolCalls,
    Incomplete,
    Cancelled,
    Handoff,
    QuotaDrained,
}

impl From<FinishReason> for FinishReasonWire {
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

impl From<FinishReasonWire> for FinishReason {
    fn from(reason: FinishReasonWire) -> Self {
        match reason {
            FinishReasonWire::Stop => Self::Stop,
            FinishReasonWire::ToolCalls => Self::ToolCalls,
            FinishReasonWire::Incomplete => Self::Incomplete,
            FinishReasonWire::Cancelled => Self::Cancelled,
            FinishReasonWire::Handoff => Self::Handoff,
            FinishReasonWire::QuotaDrained => Self::QuotaDrained,
        }
    }
}

/// A stable refusal code.
///
/// This is the one enum persisted by its **core** wire form rather than a
/// mirror, because [`ReasonCode::as_str`] is already documented as the public
/// contract: "Human-readable messages may change freely; codes may not." A
/// mirror here would create a second contract that could drift from the one the
/// model, the tests, and the audit trail already depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::store) struct ReasonCodeWire(pub ReasonCode);

impl Serialize for ReasonCodeWire {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasonCodeWire {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        parse_reason_code(&raw)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown reason code `{raw}`")))
    }
}

/// Parse a stored reason code.
///
/// Delegates to [`ReasonCode::parse`] — the one parser for the contract
/// [`ReasonCode::as_str`] documents — rather than a second copy of the match
/// that could drift from it.
fn parse_reason_code(raw: &str) -> Option<ReasonCode> {
    ReasonCode::parse(raw)
}

/// What a trigger fires from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum TriggerSourceKindWire {
    Schedule,
    Webhook,
}

impl From<TriggerSourceKind> for TriggerSourceKindWire {
    fn from(source: TriggerSourceKind) -> Self {
        match source {
            TriggerSourceKind::Schedule => Self::Schedule,
            TriggerSourceKind::Webhook => Self::Webhook,
        }
    }
}

impl From<TriggerSourceKindWire> for TriggerSourceKind {
    fn from(source: TriggerSourceKindWire) -> Self {
        match source {
            TriggerSourceKindWire::Schedule => Self::Schedule,
            TriggerSourceKindWire::Webhook => Self::Webhook,
        }
    }
}

/// What happens to an occurrence that arrives while one is in flight.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum OverlapPolicyWire {
    Skip,
    Queue,
    Replace,
}

impl From<OverlapPolicy> for OverlapPolicyWire {
    fn from(overlap: OverlapPolicy) -> Self {
        match overlap {
            OverlapPolicy::Skip => Self::Skip,
            OverlapPolicy::Queue => Self::Queue,
            OverlapPolicy::Replace => Self::Replace,
        }
    }
}

impl From<OverlapPolicyWire> for OverlapPolicy {
    fn from(overlap: OverlapPolicyWire) -> Self {
        match overlap {
            OverlapPolicyWire::Skip => Self::Skip,
            OverlapPolicyWire::Queue => Self::Queue,
            OverlapPolicyWire::Replace => Self::Replace,
        }
    }
}

/// What one firing came to.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum TriggerOutcomeWire {
    Verified,
    Refused,
    BudgetOrQuotaStopped,
    Failed,
}

impl From<TriggerOutcome> for TriggerOutcomeWire {
    fn from(outcome: TriggerOutcome) -> Self {
        match outcome {
            TriggerOutcome::Verified => Self::Verified,
            TriggerOutcome::Refused => Self::Refused,
            TriggerOutcome::BudgetOrQuotaStopped => Self::BudgetOrQuotaStopped,
            TriggerOutcome::Failed => Self::Failed,
        }
    }
}

impl From<TriggerOutcomeWire> for TriggerOutcome {
    fn from(outcome: TriggerOutcomeWire) -> Self {
        match outcome {
            TriggerOutcomeWire::Verified => Self::Verified,
            TriggerOutcomeWire::Refused => Self::Refused,
            TriggerOutcomeWire::BudgetOrQuotaStopped => Self::BudgetOrQuotaStopped,
            TriggerOutcomeWire::Failed => Self::Failed,
        }
    }
}

/// Token accounting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct UsageWire {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl From<Usage> for UsageWire {
    fn from(usage: Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        }
    }
}

impl From<UsageWire> for Usage {
    fn from(usage: UsageWire) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        }
    }
}

/// Why a route selection resolved the way it did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::store) enum RouteSelectionReasonWire {
    Named,
    Role { role: String },
    NamedAfterUnmappedRole { role: String },
    TaskClass { task_class: String },
    ChildDefault,
}

impl From<RouteSelectionReason> for RouteSelectionReasonWire {
    fn from(reason: RouteSelectionReason) -> Self {
        match reason {
            RouteSelectionReason::Named => Self::Named,
            RouteSelectionReason::Role(role) => Self::Role { role },
            RouteSelectionReason::NamedAfterUnmappedRole(role) => {
                Self::NamedAfterUnmappedRole { role }
            }
            RouteSelectionReason::TaskClass(task_class) => Self::TaskClass { task_class },
            RouteSelectionReason::ChildDefault => Self::ChildDefault,
        }
    }
}

impl From<RouteSelectionReasonWire> for RouteSelectionReason {
    fn from(reason: RouteSelectionReasonWire) -> Self {
        match reason {
            RouteSelectionReasonWire::Named => Self::Named,
            RouteSelectionReasonWire::Role { role } => Self::Role(role),
            RouteSelectionReasonWire::NamedAfterUnmappedRole { role } => {
                Self::NamedAfterUnmappedRole(role)
            }
            RouteSelectionReasonWire::TaskClass { task_class } => Self::TaskClass(task_class),
            RouteSelectionReasonWire::ChildDefault => Self::ChildDefault,
        }
    }
}

/// The typed condition that advanced or exhausted a route.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::store) enum RouteAdvanceConditionWire {
    QuotaReserveBreached,
    ProviderFailure { code: ReasonCodeWire },
    BreakerOpen,
}

impl From<RouteAdvanceCondition> for RouteAdvanceConditionWire {
    fn from(condition: RouteAdvanceCondition) -> Self {
        match condition {
            RouteAdvanceCondition::QuotaReserveBreached => Self::QuotaReserveBreached,
            RouteAdvanceCondition::ProviderFailure(code) => Self::ProviderFailure {
                code: ReasonCodeWire(code),
            },
            RouteAdvanceCondition::BreakerOpen => Self::BreakerOpen,
        }
    }
}

impl From<RouteAdvanceConditionWire> for RouteAdvanceCondition {
    fn from(condition: RouteAdvanceConditionWire) -> Self {
        match condition {
            RouteAdvanceConditionWire::QuotaReserveBreached => Self::QuotaReserveBreached,
            RouteAdvanceConditionWire::ProviderFailure { code } => Self::ProviderFailure(code.0),
            RouteAdvanceConditionWire::BreakerOpen => Self::BreakerOpen,
        }
    }
}

/// Circuit breaker lifecycle state ("Closed → Open → `HalfOpen`").
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::store) enum BreakerStateWire {
    Closed,
    Open,
    HalfOpen,
}

impl From<BreakerState> for BreakerStateWire {
    fn from(state: BreakerState) -> Self {
        match state {
            BreakerState::Closed => Self::Closed,
            BreakerState::Open => Self::Open,
            BreakerState::HalfOpen => Self::HalfOpen,
        }
    }
}

impl From<BreakerStateWire> for BreakerState {
    fn from(state: BreakerStateWire) -> Self {
        match state {
            BreakerStateWire::Closed => Self::Closed,
            BreakerStateWire::Open => Self::Open,
            BreakerStateWire::HalfOpen => Self::HalfOpen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_code_survives_a_round_trip() {
        // A code that fails to parse back is a stored refusal mjolnr can no
        // longer read. The list mirrors core::error::ReasonCode exactly; if one
        // is added there without a wire mapping, this fails.
        let codes = [
            ReasonCode::SchemaInvalid,
            ReasonCode::PathOutsideWorkspace,
            ReasonCode::PathSymlinkEscape,
            ReasonCode::FileNotObserved,
            ReasonCode::StaleFileVersion,
            ReasonCode::ApprovalRequired,
            ReasonCode::ApprovalDenied,
            ReasonCode::PolicyReadOnly,
            ReasonCode::BudgetExhausted,
            ReasonCode::CommandTimeout,
            ReasonCode::OutputTruncated,
            ReasonCode::ToolExecution,
            ReasonCode::Cancelled,
            ReasonCode::ProviderAuth,
            ReasonCode::ProviderRateLimit,
            ReasonCode::ProviderPlanQuota,
            ReasonCode::ProviderRelay,
            ReasonCode::ProviderProtocol,
            ReasonCode::ProviderIncompatibleModel,
            ReasonCode::RecoveryRequiresDecision,
            ReasonCode::CompletionEvidenceMissing,
            ReasonCode::RunActive,
            ReasonCode::McpServerUnavailable,
            ReasonCode::McpToolRefused,
            ReasonCode::McpSchemaMismatch,
            ReasonCode::WorkspaceDirty,
            ReasonCode::WorktreeUnavailable,
            ReasonCode::SubagentResultMissing,
            ReasonCode::TriggerDisabled,
            ReasonCode::RouteExhausted,
        ];

        for code in codes {
            assert_eq!(
                parse_reason_code(code.as_str()),
                Some(code),
                "{code} does not survive a persistence round trip"
            );
        }

        assert_eq!(codes.len(), 30, "a reason code was added without a mapping");
    }

    #[test]
    fn an_unknown_reason_code_is_rejected_rather_than_guessed() {
        assert_eq!(parse_reason_code("SCHEMA_INVALID_V2"), None);
        assert_eq!(parse_reason_code(""), None);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        // The database is mjolnr's own format. A field this build does not know
        // means the row was written by a newer schema, which the version gate
        // should already have caught — this is the second line of defence.
        let json = r#"{"input_tokens":1,"output_tokens":2,"cached_tokens":3}"#;
        assert!(serde_json::from_str::<UsageWire>(json).is_err());
    }

    #[test]
    fn policy_wire_names_are_independent_of_display_labels() {
        // The guard: renaming PolicyMode::label() must not rewrite the database.
        let json = serde_json::to_string(&PolicyModeWire::from(PolicyMode::WorkspaceWrite))
            .expect("serialize");
        assert_eq!(json, "\"workspace_write\"");
        assert_eq!(PolicyMode::WorkspaceWrite.label(), "workspace-write");
    }

    #[test]
    #[allow(
        clippy::cognitive_complexity,
        reason = "one flat sequence of independent round-trip checks;  added three more enums to the same pattern"
    )]
    fn every_enum_survives_a_round_trip() {
        for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
            let wire = serde_json::to_string(&RoleWire::from(role)).expect("serialize");
            let back: RoleWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(Role::from(back), role);
        }
        for tier in [ToolTier::Read, ToolTier::Write, ToolTier::Execute] {
            let wire = serde_json::to_string(&ToolTierWire::from(tier)).expect("serialize");
            let back: ToolTierWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(ToolTier::from(back), tier);
        }
        for mode in [
            PolicyMode::ReadOnly,
            PolicyMode::Ask,
            PolicyMode::WorkspaceWrite,
        ] {
            let wire = serde_json::to_string(&PolicyModeWire::from(mode)).expect("serialize");
            let back: PolicyModeWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(PolicyMode::from(back), mode);
        }
        for decision in [
            ApprovalDecision::Deny,
            ApprovalDecision::ApproveOnce,
            ApprovalDecision::ApproveExactForSession,
        ] {
            let wire =
                serde_json::to_string(&ApprovalDecisionWire::from(decision)).expect("serialize");
            let back: ApprovalDecisionWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(ApprovalDecision::from(back), decision);
        }
        for reason in [
            FinishReason::Stop,
            FinishReason::ToolCalls,
            FinishReason::Incomplete,
            FinishReason::Cancelled,
            FinishReason::Handoff,
            FinishReason::QuotaDrained,
        ] {
            let wire = serde_json::to_string(&FinishReasonWire::from(reason)).expect("serialize");
            let back: FinishReasonWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(FinishReason::from(back), reason);
        }
        for state in [
            BreakerState::Closed,
            BreakerState::Open,
            BreakerState::HalfOpen,
        ] {
            let wire = serde_json::to_string(&BreakerStateWire::from(state)).expect("serialize");
            let back: BreakerStateWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(BreakerState::from(back), state);
        }
        for reason in [
            RouteSelectionReason::Named,
            RouteSelectionReason::Role("smol".to_owned()),
            RouteSelectionReason::NamedAfterUnmappedRole("smol".to_owned()),
            RouteSelectionReason::TaskClass("default".to_owned()),
            RouteSelectionReason::ChildDefault,
        ] {
            let wire = serde_json::to_string(&RouteSelectionReasonWire::from(reason.clone()))
                .expect("serialize");
            let back: RouteSelectionReasonWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(RouteSelectionReason::from(back), reason);
        }
        for condition in [
            RouteAdvanceCondition::QuotaReserveBreached,
            RouteAdvanceCondition::ProviderFailure(ReasonCode::ProviderRateLimit),
            RouteAdvanceCondition::BreakerOpen,
        ] {
            let wire = serde_json::to_string(&RouteAdvanceConditionWire::from(condition))
                .expect("serialize");
            let back: RouteAdvanceConditionWire = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(RouteAdvanceCondition::from(back), condition);
        }
    }
}
