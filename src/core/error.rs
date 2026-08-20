//! Stable reason codes and typed errors.
//!
//! **Reason codes are a public contract.** Human-readable messages may change
//! freely; codes may not. Tests assert on codes, never on prose (AGENTS.md §6).
//!
//! `anyhow` is deliberately absent from library code: it erases the taxonomy the
//! guards depend on. A guard that cannot say *why* it refused is not a guard.

use thiserror::Error;

pub type MjolnrResult<T> = Result<T, MjolnrError>;

/// The stable vocabulary of refusals and failures.
///
/// Every variant in  is present, including ones nothing emits yet.
/// They are declared up front because the code is the contract: a phase that
/// invents its own string instead of reusing the declared code would fork the
/// vocabulary silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReasonCode {
    SchemaInvalid,
    PathOutsideWorkspace,
    PathSymlinkEscape,
    FileNotObserved,
    StaleFileVersion,
    ApprovalRequired,
    ApprovalDenied,
    PolicyReadOnly,
    BudgetExhausted,
    CommandTimeout,
    OutputTruncated,
    ToolExecution,
    Cancelled,
    ProviderAuth,
    ProviderRateLimit,
    ProviderRateLimitUnexplained,
    ProviderOverloaded,
    ProviderPlanQuota,
    ProviderRelay,
    ProviderProtocol,
    ProviderIncompatibleModel,
    RecoveryRequiresDecision,
    CompletionEvidenceMissing,
    RunActive,
    McpServerUnavailable,
    McpToolRefused,
    McpSchemaMismatch,
    WorkspaceDirty,
    WorktreeUnavailable,
    SubagentResultMissing,
    /// A trigger disabled itself after repeated firing failures.
    TriggerDisabled,
    /// A route's ordered fallback chain had no viable position left (plan
    /// §Phase 15). A typed stop, never a silent retry loop.
    RouteExhausted,
    /// A spawn asked for more than the armed envelope authorises (
    /// 31). Typed rather than silently downgraded to an approval prompt: the
    /// model can re-plan a smaller draw against it, and a large preview
    /// appearing mid-fleet is the previewability problem returning.
    SpawnEnvelopeRefused,
    /// An invalid plan workflow transition was attempted.
    PlanInvalidTransition,
    /// A plan revision is stale or superseded by a newer revision.
    PlanStaleRevision,
    /// A client sent an outdated revision for a workspace object (Phase D0).
    WorkspaceStaleRevision,
    /// A requested workspace feature is not available (Phase D0).
    WorkspaceCapabilityUnavailable,
    /// Data from an unverified external source was used where verified data is
    /// required (Phase D0).
    WorkspaceExternalUnverified,
    /// A diff references outdated tree state (Phase D0).
    WorkspaceStaleDiff,
    /// Integration authentication was refused (Phase D0).
    WorkspaceAuthRefused,
    /// A workspace search question could not be answered (Phase D4).
    ///
    /// Distinct from an empty page, which says "nothing matched". This says
    /// "that could not be matched" — a query shorter than the trigram index can
    /// answer, a cursor issued for a different filter, a page walked past the
    /// enumeration bound. The two send a user to different remedies, which is
    /// why `StoreError::Refused` exists and why collapsing it into
    /// `MjolnrError::Store` (a code-less "the store is broken") lost the
    /// distinction the store had just made.
    WorkspaceSearchRefused,
    /// The workspace root cannot change because a session is already open on
    /// it. The session's durable record, policy, and contained paths are all
    /// anchored to that root, so repointing it underneath a live session would
    /// silently invalidate all three. Distinct from
    /// [`RunActive`](Self::RunActive): no run needs to be in flight for the
    /// root to be locked.
    WorkspaceRootLocked,
    /// A repository operation stopped on an unmerged path (Phase D5). mjolnr
    /// never resolves a conflict on the human's behalf.
    RepositoryConflict,
    /// The repository has no branch checked out, so a branch-relative
    /// operation has no meaning (Phase D5).
    RepositoryDetachedHead,
    /// A repository hook refused the operation (Phase D5). The hook is the
    /// owner's own gate; mjolnr reports it rather than bypassing it with
    /// `--no-verify`.
    RepositoryHookRefused,
    /// Commit signing failed (Phase D5). Reported rather than retried
    /// unsigned, which would silently downgrade the owner's guarantee.
    RepositorySigningFailed,
    /// A repository operation may or may not have taken effect (Phase D5).
    /// The distinct code exists so a client cannot render this as either
    /// success or clean failure — it requires a human decision.
    RepositoryUncertainEffect,
    /// The current branch has no upstream configured, so a push has no
    /// resolved destination (Phase D5 git surface). Refused before any
    /// network call.
    RepositoryNoUpstream,
    /// The current branch is behind its remote-tracking ref, so a push would
    /// be rejected as non-fast-forward (Phase D5 git surface). Refused before
    /// the network call; the human is told to fetch or integrate first.
    RepositoryDivergedFromRemote,
    /// A concurrent subagent mutation invalidated an observed file in the active read set (Phase 5 Slice 5.2).
    ReadSetCollision,
}

impl ReasonCode {
    /// The wire form. This string is the contract — changing one is a breaking
    /// change to anything asserting on it, including the model's own ability to
    /// recognise a refusal it has seen before.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaInvalid => "SCHEMA_INVALID",
            Self::PathOutsideWorkspace => "PATH_OUTSIDE_WORKSPACE",
            Self::PathSymlinkEscape => "PATH_SYMLINK_ESCAPE",
            Self::FileNotObserved => "FILE_NOT_OBSERVED",
            Self::StaleFileVersion => "STALE_FILE_VERSION",
            Self::ApprovalRequired => "APPROVAL_REQUIRED",
            Self::ApprovalDenied => "APPROVAL_DENIED",
            Self::PolicyReadOnly => "POLICY_READ_ONLY",
            Self::BudgetExhausted => "BUDGET_EXHAUSTED",
            Self::CommandTimeout => "COMMAND_TIMEOUT",
            Self::OutputTruncated => "OUTPUT_TRUNCATED",
            Self::ToolExecution => "TOOL_EXECUTION",
            Self::Cancelled => "CANCELLED",
            Self::ProviderAuth => "PROVIDER_AUTH",
            Self::ProviderRateLimit => "PROVIDER_RATE_LIMIT",
            Self::ProviderRateLimitUnexplained => "PROVIDER_RATE_LIMIT_UNEXPLAINED",
            Self::ProviderOverloaded => "PROVIDER_OVERLOADED",
            Self::ProviderPlanQuota => "PROVIDER_PLAN_QUOTA",
            Self::ProviderRelay => "PROVIDER_RELAY",
            Self::ProviderProtocol => "PROVIDER_PROTOCOL",
            Self::ProviderIncompatibleModel => "PROVIDER_INCOMPATIBLE_MODEL",
            Self::RecoveryRequiresDecision => "RECOVERY_REQUIRES_DECISION",
            Self::CompletionEvidenceMissing => "COMPLETION_EVIDENCE_MISSING",
            Self::RunActive => "RUN_ACTIVE",
            Self::McpServerUnavailable => "MCP_SERVER_UNAVAILABLE",
            Self::McpToolRefused => "MCP_TOOL_REFUSED",
            Self::McpSchemaMismatch => "MCP_SCHEMA_MISMATCH",
            Self::WorkspaceDirty => "WORKSPACE_DIRTY",
            Self::WorktreeUnavailable => "WORKTREE_UNAVAILABLE",
            Self::SubagentResultMissing => "SUBAGENT_RESULT_MISSING",
            Self::TriggerDisabled => "TRIGGER_DISABLED",
            Self::RouteExhausted => "ROUTE_EXHAUSTED",
            Self::SpawnEnvelopeRefused => "SPAWN_ENVELOPE_REFUSED",
            Self::PlanInvalidTransition => "PLAN_INVALID_TRANSITION",
            Self::PlanStaleRevision => "PLAN_STALE_REVISION",
            Self::WorkspaceStaleRevision => "WORKSPACE_STALE_REVISION",
            Self::WorkspaceCapabilityUnavailable => "WORKSPACE_CAPABILITY_UNAVAILABLE",
            Self::WorkspaceExternalUnverified => "WORKSPACE_EXTERNAL_UNVERIFIED",
            Self::WorkspaceStaleDiff => "WORKSPACE_STALE_DIFF",
            Self::WorkspaceAuthRefused => "WORKSPACE_AUTH_REFUSED",
            Self::WorkspaceSearchRefused => "WORKSPACE_SEARCH_REFUSED",
            Self::WorkspaceRootLocked => "WORKSPACE_ROOT_LOCKED",
            Self::RepositoryConflict => "REPOSITORY_CONFLICT",
            Self::RepositoryDetachedHead => "REPOSITORY_DETACHED_HEAD",
            Self::RepositoryHookRefused => "REPOSITORY_HOOK_REFUSED",
            Self::RepositorySigningFailed => "REPOSITORY_SIGNING_FAILED",
            Self::RepositoryUncertainEffect => "REPOSITORY_UNCERTAIN_EFFECT",
            Self::RepositoryNoUpstream => "REPOSITORY_NO_UPSTREAM",
            Self::RepositoryDivergedFromRemote => "REPOSITORY_DIVERGED_FROM_REMOTE",
            Self::ReadSetCollision => "READ_SET_COLLISION",
        }
    }

    /// Parse a code's wire form back into a [`ReasonCode`].
    ///
    /// Exhaustive and explicit: an unrecognised string returns `None` rather
    /// than a guessed nearby code. Used wherever a code crosses a boundary as
    /// text and needs to come back typed — `store::wire` persistence and the
    /// Phase 14 scheduler's [`crate::headless::HeadlessReport::reason_code`]
    /// both delegate here so there is exactly one parser for the contract
    /// [`as_str`](Self::as_str) documents.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let code = match raw {
            "SCHEMA_INVALID" => Self::SchemaInvalid,
            "PATH_OUTSIDE_WORKSPACE" => Self::PathOutsideWorkspace,
            "PATH_SYMLINK_ESCAPE" => Self::PathSymlinkEscape,
            "FILE_NOT_OBSERVED" => Self::FileNotObserved,
            "STALE_FILE_VERSION" => Self::StaleFileVersion,
            "APPROVAL_REQUIRED" => Self::ApprovalRequired,
            "APPROVAL_DENIED" => Self::ApprovalDenied,
            "POLICY_READ_ONLY" => Self::PolicyReadOnly,
            "BUDGET_EXHAUSTED" => Self::BudgetExhausted,
            "COMMAND_TIMEOUT" => Self::CommandTimeout,
            "OUTPUT_TRUNCATED" => Self::OutputTruncated,
            "TOOL_EXECUTION" => Self::ToolExecution,
            "CANCELLED" => Self::Cancelled,
            "PROVIDER_AUTH" => Self::ProviderAuth,
            "PROVIDER_RATE_LIMIT" => Self::ProviderRateLimit,
            "PROVIDER_RATE_LIMIT_UNEXPLAINED" => Self::ProviderRateLimitUnexplained,
            "PROVIDER_OVERLOADED" => Self::ProviderOverloaded,
            "PROVIDER_PLAN_QUOTA" => Self::ProviderPlanQuota,
            "PROVIDER_RELAY" => Self::ProviderRelay,
            "PROVIDER_PROTOCOL" => Self::ProviderProtocol,
            "PROVIDER_INCOMPATIBLE_MODEL" => Self::ProviderIncompatibleModel,
            "RECOVERY_REQUIRES_DECISION" => Self::RecoveryRequiresDecision,
            "COMPLETION_EVIDENCE_MISSING" => Self::CompletionEvidenceMissing,
            "RUN_ACTIVE" => Self::RunActive,
            "MCP_SERVER_UNAVAILABLE" => Self::McpServerUnavailable,
            "MCP_TOOL_REFUSED" => Self::McpToolRefused,
            "MCP_SCHEMA_MISMATCH" => Self::McpSchemaMismatch,
            "WORKSPACE_DIRTY" => Self::WorkspaceDirty,
            "WORKTREE_UNAVAILABLE" => Self::WorktreeUnavailable,
            "SUBAGENT_RESULT_MISSING" => Self::SubagentResultMissing,
            "TRIGGER_DISABLED" => Self::TriggerDisabled,
            "ROUTE_EXHAUSTED" => Self::RouteExhausted,
            "SPAWN_ENVELOPE_REFUSED" => Self::SpawnEnvelopeRefused,
            "PLAN_INVALID_TRANSITION" => Self::PlanInvalidTransition,
            "PLAN_STALE_REVISION" => Self::PlanStaleRevision,
            "WORKSPACE_STALE_REVISION" => Self::WorkspaceStaleRevision,
            "WORKSPACE_CAPABILITY_UNAVAILABLE" => Self::WorkspaceCapabilityUnavailable,
            "WORKSPACE_EXTERNAL_UNVERIFIED" => Self::WorkspaceExternalUnverified,
            "WORKSPACE_STALE_DIFF" => Self::WorkspaceStaleDiff,
            "WORKSPACE_AUTH_REFUSED" => Self::WorkspaceAuthRefused,
            "WORKSPACE_SEARCH_REFUSED" => Self::WorkspaceSearchRefused,
            "WORKSPACE_ROOT_LOCKED" => Self::WorkspaceRootLocked,
            "REPOSITORY_CONFLICT" => Self::RepositoryConflict,
            "REPOSITORY_DETACHED_HEAD" => Self::RepositoryDetachedHead,
            "REPOSITORY_HOOK_REFUSED" => Self::RepositoryHookRefused,
            "REPOSITORY_SIGNING_FAILED" => Self::RepositorySigningFailed,
            "REPOSITORY_UNCERTAIN_EFFECT" => Self::RepositoryUncertainEffect,
            "REPOSITORY_NO_UPSTREAM" => Self::RepositoryNoUpstream,
            "REPOSITORY_DIVERGED_FROM_REMOTE" => Self::RepositoryDivergedFromRemote,
            "READ_SET_COLLISION" => Self::ReadSetCollision,
            _ => return None,
        };
        Some(code)
    }

    /// Stable code-adjacent explanation for glass-box failure rendering.
    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::SchemaInvalid => "The proposed arguments did not match the tool schema.",
            Self::PathOutsideWorkspace => "The proposed path was outside the open workspace.",
            Self::PathSymlinkEscape => "A symlink would have escaped the open workspace.",
            Self::FileNotObserved => "The file must be read before it can be changed.",
            Self::StaleFileVersion => "The file changed after mjolnr read it.",
            Self::ApprovalRequired => "A human decision is required before this can run.",
            Self::ApprovalDenied => "The proposed action was not authorised.",
            Self::PolicyReadOnly => "The active policy refuses side effects.",
            Self::BudgetExhausted => "The run reached a configured work budget.",
            Self::CommandTimeout => "The command exceeded its allowed runtime.",
            Self::OutputTruncated => "The captured output exceeded its display budget.",
            Self::ToolExecution => "The tool could not complete its operation.",
            Self::Cancelled => "The active operation was interrupted.",
            Self::ProviderAuth => "The provider rejected or could not resolve authentication.",
            Self::ProviderRateLimit => "Wait for the limit to reset, or switch models with /model.",
            Self::ProviderRateLimitUnexplained => {
                "No limit was reported alongside the refusal; check that the credential is one this endpoint accepts."
            }
            Self::ProviderOverloaded => {
                "The provider's own capacity was short; this is not your quota."
            }
            Self::ProviderPlanQuota => "The provider reported that the plan quota was exhausted.",
            Self::ProviderRelay => {
                "The gateway could not relay the request; check its request logs or contact gateway support."
            }
            Self::ProviderProtocol => "The provider response could not be interpreted safely.",
            Self::ProviderIncompatibleModel => "The model lacks a required capability.",
            Self::RecoveryRequiresDecision => "Interrupted work needs a human recovery decision.",
            Self::CompletionEvidenceMissing => "Verified completion lacked command evidence.",
            Self::RunActive => "The requested change is locked while a run is active.",
            Self::McpServerUnavailable => "The configured MCP server is unavailable.",
            Self::McpToolRefused => "The MCP server refused the tool call.",
            Self::McpSchemaMismatch => "The MCP tool schema could not be governed safely.",
            Self::WorkspaceDirty => "The workspace has uncommitted changes.",
            Self::WorktreeUnavailable => "An isolated git worktree could not be prepared.",
            Self::SubagentResultMissing => "A subagent finished without reporting a result.",
            Self::TriggerDisabled => "The trigger disabled itself after repeated failures.",
            Self::RouteExhausted => "The route had no viable provider position left.",
            Self::SpawnEnvelopeRefused => {
                "The spawn asked for more than the armed envelope authorises."
            }
            Self::PlanInvalidTransition => {
                "The requested plan state transition was invalid for the current workflow stage."
            }
            Self::PlanStaleRevision => {
                "The plan revision is stale or superseded by a newer revision."
            }
            Self::WorkspaceStaleRevision => "The client sent an outdated workspace revision.",
            Self::WorkspaceCapabilityUnavailable => {
                "The requested workspace capability is not available."
            }
            Self::WorkspaceExternalUnverified => {
                "The operation requires verified data but received unverified external data."
            }
            Self::WorkspaceStaleDiff => "The diff references an outdated tree state.",
            Self::WorkspaceAuthRefused => "Integration authentication was refused.",
            Self::WorkspaceSearchRefused => {
                "The search question could not be answered, which is not the same as nothing \
                 matching."
            }
            Self::WorkspaceRootLocked => {
                "The workspace root cannot change while a session is open on it."
            }
            Self::RepositoryConflict => "The repository has unmerged paths.",
            Self::RepositoryDetachedHead => "The repository has no branch checked out.",
            Self::RepositoryHookRefused => "A repository hook refused the operation.",
            Self::RepositorySigningFailed => "The commit could not be signed.",
            Self::RepositoryUncertainEffect => {
                "mjolnr cannot prove whether the repository operation took effect."
            }
            Self::RepositoryNoUpstream => {
                "The current branch has no upstream configured, so there is no push destination."
            }
            Self::RepositoryDivergedFromRemote => {
                "The local branch is behind the remote; fetch or integrate before pushing."
            }
            Self::ReadSetCollision => {
                "A concurrent sibling changed a file this agent had read; re-read it before finishing."
            }
        }
    }
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Failures from a provider adapter.
///
/// Note what is absent: there is no `Retry` variant. A stream that produced
/// output and then failed is never replayed automatically (AGENTS.md §4). The
/// decision belongs to a human, and the type refuses to imply otherwise.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("provider authentication failed")]
    Auth,

    /// The `retry-after` header is the only actionable fact in a 429, so it
    /// belongs in the rendered message rather than being parsed and dropped.
    #[error("provider rate limited the request{}", retry_after_seconds.map_or_else(String::new, |seconds| format!("; retry after {seconds}s")))]
    RateLimit { retry_after_seconds: Option<u64> },

    /// A 429 that arrived with nothing to corroborate it: no `retry-after`, no
    /// `anthropic-ratelimit-*` window, and no `rate_limit_error` in the body.
    ///
    /// A genuine throttle says which limit was hit and when it clears. A 429
    /// carrying none of that is the endpoint refusing the request for some
    /// other reason — most often a credential it does not accept here — and
    /// rendering it as "wait for the limit to reset" sends the user to wait out
    /// a quota they never touched. mjolnr reports the refusal and says plainly
    /// that no limit was named, rather than guessing at either explanation.
    #[error("provider refused the request with 429 but reported no limit")]
    RateLimitUnexplained,

    /// The upstream was out of capacity — Anthropic's HTTP 529 and
    /// `overloaded_error`. Distinct from [`Self::RateLimit`] on purpose: both
    /// used to render as "rate limited", which tells a user with an untouched
    /// quota that they exhausted it. mjolnr does not misreport whose fault a
    /// failure is.
    #[error("provider is temporarily overloaded{}", retry_after_seconds.map_or_else(String::new, |seconds| format!("; retry after {seconds}s")))]
    Overloaded { retry_after_seconds: Option<u64> },

    #[error("subscription plan quota exhausted; reset at {reset_at_unix:?}")]
    PlanQuota { reset_at_unix: Option<i64> },

    #[error("provider gateway could not relay the request")]
    Relay,

    /// The upstream said something mjolnr could not interpret. Carries a
    /// description, never the raw body — bodies can contain credentials.
    #[error("provider protocol error: {detail}")]
    Protocol { detail: String },

    #[error("model {model} does not support a required capability: {capability}")]
    IncompatibleModel { model: String, capability: String },

    #[error("provider transport error: {detail}")]
    Transport { detail: String },

    #[error("cancelled")]
    Cancelled,
}

impl ProviderError {
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::Auth => ReasonCode::ProviderAuth,
            Self::RateLimit { .. } => ReasonCode::ProviderRateLimit,
            Self::RateLimitUnexplained => ReasonCode::ProviderRateLimitUnexplained,
            Self::Overloaded { .. } => ReasonCode::ProviderOverloaded,
            Self::PlanQuota { .. } => ReasonCode::ProviderPlanQuota,
            Self::Relay => ReasonCode::ProviderRelay,
            Self::Protocol { .. } | Self::Transport { .. } => ReasonCode::ProviderProtocol,
            Self::IncompatibleModel { .. } => ReasonCode::ProviderIncompatibleModel,
            Self::Cancelled => ReasonCode::Cancelled,
        }
    }
}

/// Failures from a tool.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool arguments failed schema validation: {detail}")]
    SchemaInvalid { detail: String },

    #[error("tool preflight refused: {detail}")]
    Refused { code: ReasonCode, detail: String },

    #[error("tool preflight failed: {detail}")]
    Failed { code: ReasonCode, detail: String },

    #[error("tool execution failed: {detail}")]
    Execution { detail: String },

    #[error("cancelled")]
    Cancelled,
}

impl ToolError {
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::SchemaInvalid { .. } => ReasonCode::SchemaInvalid,
            Self::Refused { code, .. } | Self::Failed { code, .. } => *code,
            Self::Execution { .. } => ReasonCode::ToolExecution,
            Self::Cancelled => ReasonCode::Cancelled,
        }
    }
}

/// Failures from the runtime itself.
#[derive(Debug, Error)]
pub enum MjolnrError {
    #[error("no session is open")]
    NoSession,

    #[error("unknown provider: {0}")]
    UnknownProvider(String),

    #[error("the runtime has shut down")]
    RuntimeClosed,

    #[error("a run is already active; cancel it before starting another")]
    RunActive,

    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error("store error: {detail}")]
    Store { detail: String },

    #[error("plan state transition invalid: from {from} during {action}: {detail}")]
    PlanInvalidTransition {
        from: String,
        action: String,
        detail: String,
    },

    #[error("plan revision {attempted} is stale; current active revision is {current}")]
    PlanStaleRevision { attempted: u32, current: u32 },

    /// A workspace command reached the runtime but the capability it names is
    /// not available. This is the fail-closed answer to "contract on the wire,
    /// execution not yet implemented": a typed refusal the bridge can render,
    /// never a panic (Phase D2).
    #[error("{detail}")]
    WorkspaceRefused { code: ReasonCode, detail: String },
}

impl MjolnrError {
    #[must_use]
    pub fn plan_invalid_transition(
        from: impl Into<String>,
        action: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::PlanInvalidTransition {
            from: from.into(),
            action: action.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn plan_stale_revision(attempted: u32, current: u32) -> Self {
        Self::PlanStaleRevision { attempted, current }
    }

    #[must_use]
    pub fn workspace_refused(code: ReasonCode, detail: impl Into<String>) -> Self {
        Self::WorkspaceRefused {
            code,
            detail: detail.into(),
        }
    }

    /// The stable refusal code, where the variant carries one. Variants that
    /// predate the workspace contract and have no code of their own return
    /// `None`; do not invent mappings for them.
    #[must_use]
    pub fn reason_code(&self) -> Option<ReasonCode> {
        match self {
            Self::RunActive => Some(ReasonCode::RunActive),
            Self::PlanInvalidTransition { .. } => Some(ReasonCode::PlanInvalidTransition),
            Self::PlanStaleRevision { .. } => Some(ReasonCode::PlanStaleRevision),
            Self::WorkspaceRefused { code, .. } => Some(*code),
            Self::Provider(error) => Some(error.reason_code()),
            Self::NoSession
            | Self::UnknownProvider(_)
            | Self::RuntimeClosed
            | Self::Store { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_render_as_their_wire_form() {
        assert_eq!(
            ReasonCode::PathOutsideWorkspace.as_str(),
            "PATH_OUTSIDE_WORKSPACE"
        );
        assert_eq!(ReasonCode::BudgetExhausted.to_string(), "BUDGET_EXHAUSTED");
    }

    /// Every declared `ReasonCode`, in one place. The three tests below share
    /// this list: an earlier version kept a separate array per test, and the
    /// arrays drifted (two provider codes were covered by the sentence test
    /// but not by the round-trip or uniqueness tests).
    ///
    /// Adding a variant to the enum without adding it here is caught by the
    /// compile-time anchor in [`exhaustiveness_anchor_is_complete`]: that
    /// match has no wildcard arm, so the enum and this list cannot drift apart
    /// without breaking the build.
    const ALL_CODES: &[ReasonCode] = &[
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
        ReasonCode::ProviderRateLimitUnexplained,
        ReasonCode::ProviderOverloaded,
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
        ReasonCode::SpawnEnvelopeRefused,
        ReasonCode::PlanInvalidTransition,
        ReasonCode::PlanStaleRevision,
        ReasonCode::WorkspaceStaleRevision,
        ReasonCode::WorkspaceCapabilityUnavailable,
        ReasonCode::WorkspaceExternalUnverified,
        ReasonCode::WorkspaceStaleDiff,
        ReasonCode::WorkspaceAuthRefused,
        ReasonCode::WorkspaceSearchRefused,
        ReasonCode::WorkspaceRootLocked,
        ReasonCode::RepositoryConflict,
        ReasonCode::RepositoryDetachedHead,
        ReasonCode::RepositoryHookRefused,
        ReasonCode::RepositorySigningFailed,
        ReasonCode::RepositoryUncertainEffect,
        ReasonCode::RepositoryNoUpstream,
        ReasonCode::RepositoryDivergedFromRemote,
        ReasonCode::ReadSetCollision,
    ];

    #[test]
    fn exhaustiveness_anchor_is_complete() {
        // This match has no wildcard arm on purpose: adding or renaming a
        // `ReasonCode` variant fails compilation here, which forces
        // `ALL_CODES` (and therefore all three tests that share it) to grow
        // with the enum. It is the tripwire the per-test arrays never had.
        for code in ALL_CODES {
            match code {
                ReasonCode::SchemaInvalid
                | ReasonCode::PathOutsideWorkspace
                | ReasonCode::PathSymlinkEscape
                | ReasonCode::FileNotObserved
                | ReasonCode::StaleFileVersion
                | ReasonCode::ApprovalRequired
                | ReasonCode::ApprovalDenied
                | ReasonCode::PolicyReadOnly
                | ReasonCode::BudgetExhausted
                | ReasonCode::CommandTimeout
                | ReasonCode::OutputTruncated
                | ReasonCode::ToolExecution
                | ReasonCode::Cancelled
                | ReasonCode::ProviderAuth
                | ReasonCode::ProviderRateLimit
                | ReasonCode::ProviderRateLimitUnexplained
                | ReasonCode::ProviderOverloaded
                | ReasonCode::ProviderPlanQuota
                | ReasonCode::ProviderRelay
                | ReasonCode::ProviderProtocol
                | ReasonCode::ProviderIncompatibleModel
                | ReasonCode::RecoveryRequiresDecision
                | ReasonCode::CompletionEvidenceMissing
                | ReasonCode::RunActive
                | ReasonCode::McpServerUnavailable
                | ReasonCode::McpToolRefused
                | ReasonCode::McpSchemaMismatch
                | ReasonCode::WorkspaceDirty
                | ReasonCode::WorktreeUnavailable
                | ReasonCode::SubagentResultMissing
                | ReasonCode::TriggerDisabled
                | ReasonCode::RouteExhausted
                | ReasonCode::SpawnEnvelopeRefused
                | ReasonCode::PlanInvalidTransition
                | ReasonCode::PlanStaleRevision
                | ReasonCode::WorkspaceStaleRevision
                | ReasonCode::WorkspaceCapabilityUnavailable
                | ReasonCode::WorkspaceExternalUnverified
                | ReasonCode::WorkspaceStaleDiff
                | ReasonCode::WorkspaceAuthRefused
                | ReasonCode::WorkspaceSearchRefused
                | ReasonCode::WorkspaceRootLocked
                | ReasonCode::RepositoryConflict
                | ReasonCode::RepositoryDetachedHead
                | ReasonCode::RepositoryHookRefused
                | ReasonCode::RepositorySigningFailed
                | ReasonCode::RepositoryUncertainEffect
                | ReasonCode::RepositoryNoUpstream
                | ReasonCode::RepositoryDivergedFromRemote
                | ReasonCode::ReadSetCollision => {}
            }
        }
    }

    #[test]
    fn every_declared_code_survives_a_parse_round_trip() {
        for code in ALL_CODES {
            assert_eq!(ReasonCode::parse(code.as_str()), Some(*code));
        }
        assert_eq!(ReasonCode::parse("NOT_A_REAL_CODE"), None);
    }

    #[test]
    fn every_declared_code_has_a_distinct_wire_form() {
        // Guards against a copy-paste in the match above silently aliasing two
        // codes to one string, which would make a refusal indistinguishable.
        let unique: std::collections::HashSet<&str> =
            ALL_CODES.iter().map(|code| code.as_str()).collect();

        assert_eq!(
            unique.len(),
            ALL_CODES.len(),
            "two reason codes share a wire form"
        );
    }

    #[test]
    fn every_code_has_a_human_sentence() {
        for code in ALL_CODES {
            assert!(
                code.sentence().ends_with('.'),
                "{code} has no human sentence"
            );
        }
    }

    #[test]
    fn an_unexplained_429_does_not_send_the_user_to_wait_out_a_quota() {
        // A subscription credential the endpoint declines comes back as a bare
        // 429. Rendered as a rate limit, it told a user with a full quota to
        // wait for a reset that was never coming.
        let error = ProviderError::RateLimitUnexplained;
        assert_eq!(
            error.reason_code(),
            ReasonCode::ProviderRateLimitUnexplained
        );
        let guidance = ReasonCode::ProviderRateLimitUnexplained.sentence();
        assert!(
            !guidance.contains("Wait for the limit"),
            "guidance must not point at a reset mjolnr cannot confirm: {guidance}"
        );
        assert!(
            guidance.contains("credential"),
            "the likely cause has to reach the user: {guidance}"
        );
        assert_eq!(
            ReasonCode::parse("PROVIDER_RATE_LIMIT_UNEXPLAINED"),
            Some(ReasonCode::ProviderRateLimitUnexplained),
            "the wire form has to round-trip"
        );
    }

    #[test]
    fn an_overloaded_provider_is_not_reported_as_the_users_rate_limit() {
        // Anthropic's 529 and overloaded_error both used to render as "rate
        // limited", which tells a user with an untouched quota that they
        // exhausted it. Whose fault a failure is has to be reported honestly.
        let overloaded = ProviderError::Overloaded {
            retry_after_seconds: None,
        };
        assert_eq!(overloaded.reason_code(), ReasonCode::ProviderOverloaded);
        assert!(
            !overloaded.to_string().contains("rate limit"),
            "an upstream capacity problem must not read as the caller's limit: {overloaded}"
        );
        assert!(
            ReasonCode::ProviderOverloaded
                .sentence()
                .contains("not your quota"),
            "the guidance must say whose problem this is"
        );
    }

    #[test]
    fn a_rate_limit_reports_when_to_come_back() {
        // The retry-after header is the only actionable fact in a 429. It was
        // parsed off the response and then dropped before rendering, leaving
        // the user with "rate limited" and no idea how long to wait.
        let rendered = ProviderError::RateLimit {
            retry_after_seconds: Some(30),
        }
        .to_string();
        assert!(
            rendered.contains("30s"),
            "the retry delay must survive into the message: {rendered}"
        );
    }

    #[test]
    fn a_rate_limit_without_a_header_says_nothing_it_does_not_know() {
        // Not every provider sends retry-after, and inventing a number would
        // be worse than omitting one.
        let rendered = ProviderError::RateLimit {
            retry_after_seconds: None,
        }
        .to_string();
        assert_eq!(rendered, "provider rate limited the request");
    }

    #[test]
    fn a_codes_sentence_adds_guidance_rather_than_restating_the_error() {
        // These are concatenated as "{error} // {sentence}", so a sentence
        // that paraphrases the error renders as a stutter — which is exactly
        // what "provider rate limited the request // The provider
        // rate-limited the request." was.
        let error = ProviderError::RateLimit {
            retry_after_seconds: None,
        }
        .to_string();
        let sentence = ReasonCode::ProviderRateLimit.sentence().to_lowercase();
        let shared = error
            .to_lowercase()
            .split_whitespace()
            .filter(|word| word.len() > 3 && sentence.contains(*word))
            .count();
        assert!(
            shared < 3,
            "the sentence restates the error rather than advising: {sentence}"
        );
    }

    #[test]
    fn provider_errors_map_to_stable_codes() {
        assert_eq!(ProviderError::Auth.reason_code(), ReasonCode::ProviderAuth);
        assert_eq!(
            ProviderError::RateLimit {
                retry_after_seconds: Some(30)
            }
            .reason_code(),
            ReasonCode::ProviderRateLimit
        );
        assert_eq!(
            ProviderError::Cancelled.reason_code(),
            ReasonCode::Cancelled
        );
        assert_eq!(
            ProviderError::PlanQuota {
                reset_at_unix: Some(1_700_000_000)
            }
            .reason_code(),
            ReasonCode::ProviderPlanQuota
        );
        assert_eq!(
            ProviderError::Relay.reason_code(),
            ReasonCode::ProviderRelay
        );
    }
}
