//! Core client snapshot and data types.

use serde::{Deserialize, Serialize};

use super::command::{ClientPlanStep, ClientPolicy, ClientReviewVerdict};

pub const MAX_SNAPSHOT_MESSAGES: usize = 200;
pub const MAX_MESSAGE_TEXT: usize = 20_000;
pub const MAX_ACTIVITY_TEXT: usize = 8_000;
pub const MAX_DIRECTIVE_TEXT: usize = 500;

#[must_use]
pub fn truncate_text(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_owned(), false);
    }
    (text.chars().take(max).collect(), true)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientSnapshot {
    pub revision: u64,
    pub session: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub workspace_root: Option<String>,
    pub policy: ClientPolicy,
    pub run_active: bool,
    pub usage: ClientUsage,
    pub budget: ClientBudget,
    /// The full multi-window quota snapshot last reported by a provider (§E1).
    /// `None` until a provider has reported quota at least once this session —
    /// never a guessed or zeroed placeholder.
    pub quota: Option<ClientQuota>,
    pub messages: Vec<ClientMessage>,
    pub messages_omitted: u64,
    pub pending_approval: Option<ClientApproval>,
    pub recovery: ClientRecovery,
    pub store_failure: Option<String>,
    /// Deterministic diagnostics produced by Rust while loading the current
    /// workspace context. The desktop may display these, but they never grant
    /// authority or claim that a file-level language service ran.
    pub context_diagnostics: Vec<ClientContextDiagnostic>,
    pub models: Vec<ClientModelChoice>,
    pub resume_advice: Option<ClientResumeAdvice>,
    /// One entry per provider mjolnr is configured to talk to (§E2). Sourced
    /// from `RuntimeSnapshot::providers`, itself refreshed by
    /// `refresh_provider_catalogs` — never resolved from the credential store
    /// on this conversion path, which runs on every snapshot broadcast and
    /// must not touch the OS keychain.
    pub accounts: Vec<ClientAccount>,
    pub sessions: Vec<ClientSessionSummary>,
    /// The session's explicit persona override, if any (`RuntimeSnapshot::active_persona`).
    ///
    /// `None` means the active route's own persona applies instead, not that no
    /// persona exists at all — same meaning it carries in the TUI's `/persona`.
    pub active_persona: Option<String>,
    /// Personas discovered under `.mjolnr/personas/` and the user config dir
    /// (§Stage 5), for the governance modal's persona picker — the same set
    /// `/persona` already offers in the TUI. Never the file content, which
    /// nothing on this DTO exposes yet.
    pub personas: Vec<ClientPersonaSummary>,
    /// Soul/profile file names in effect (§Stage 5). Names only — reading
    /// file content over the desktop bridge is not wired and this field does
    /// not pretend otherwise.
    pub souls: Vec<String>,
    /// The provider/model routing table (§Stage 5). Sourced from
    /// `RuntimeSnapshot::routes`, itself read-only here: attaching or editing
    /// a route stays a runtime-owned act (`AttachRoute`), this DTO only
    /// renders what is already configured.
    pub routes: Vec<ClientRoute>,
    /// The most recent completed advisory council review, if one ran.
    /// Council output informs a human; it never authorizes an action.
    pub council: Option<ClientCouncilReview>,
    pub plan: Option<ClientPlanWorkflow>,
    pub changes: Option<crate::core::changes::ChangeSet>,
    /// What git last said about the open project, and when (§D5 producer).
    ///
    /// Always present rather than `Option`, because the three states a reader
    /// must distinguish — no project, unreadable, read at a moment — live in
    /// `freshness`, and a `None` would collapse them into one silence.
    pub repository: crate::core::client::workspace::RepositoryState,
    /// Line notes pinned to the diff, oldest first (§D3 producer).
    ///
    /// A `BoundedProjection` rather than a bare `Vec` because
    /// `MAX_REVIEW_THREADS_PER_ITEM` is a real ceiling and a list that hit it
    /// with no way to say so reads as a complete one. Always present: an empty
    /// projection means "no notes", which is a different statement from the
    /// absence a `None` would make.
    pub review_threads: crate::core::client::workspace::BoundedProjection<
        crate::core::client::workspace::ReviewThreadSummary,
    >,
    /// Summary of the workspace memory state.
    pub memory: Option<ClientMemorySummary>,
    /// Summary of discovered third-party plugins.
    pub plugins: Vec<crate::core::plugin::PluginSummary>,
    /// Live multi-agent fleet roster summary.
    pub fleet: Option<crate::core::fleet::FleetSummary>,
    /// Live Studio Preview canvas state.
    pub preview: Option<crate::core::preview::PreviewState>,
    /// External-agent worktrees (Phase D9) — `ExternalUnverified` only.
    pub external_agents: Vec<crate::core::client::external_agent::ExternalAgentView>,
    pub external_agent_capability: crate::core::client::external_agent::ExternalAgentCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientMemorySummary {
    pub rules_count: usize,
    pub user_profile_present: bool,
    pub facts_count: Option<usize>,
    pub episodes_count: Option<usize>,
    pub projection_error: Option<String>,
    pub rules_error: Option<String>,
    pub rule_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilReview {
    pub review_id: String,
    pub question: String,
    pub contributions: Vec<ClientCouncilContribution>,
    pub rounds_conducted: usize,
    pub artifact: Option<ClientCouncilArtifact>,
    pub findings: Vec<ClientCouncilFinding>,
    /// The amendment composed from accepted findings, when a human asked for
    /// one. It is a draft for the editor, not a change on disk.
    pub amendment: Option<ClientCouncilAmendment>,
}

/// A human-reviewable amended artifact. Carrying it to the client is not an
/// approval: the operator still edits it and saves it through the ordinary
/// governed save path, which re-checks the digest before writing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilAmendment {
    pub review_id: String,
    pub path: String,
    pub source_digest: String,
    pub accepted_findings: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilArtifact {
    pub path: String,
    pub source_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilContribution {
    pub role: String,
    pub proposal: String,
    pub critique: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilFinding {
    pub id: String,
    pub section: String,
    pub title: String,
    pub positions: Vec<ClientCouncilPosition>,
    pub disposition: Option<ClientCouncilDispositionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilPosition {
    pub role: String,
    pub response: String,
    pub critique: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientCouncilDispositionRecord {
    pub disposition: super::command::ClientCouncilDisposition,
    pub note: Option<String>,
    pub decided_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientResumeAdvice {
    pub warning: String,
    pub estimated_full_resume_tokens: u64,
    pub has_handoff: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientContextDiagnostic {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientMessage {
    #[serde(rename_all = "camelCase")]
    User {
        id: String,
        text: String,
        text_truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    System {
        id: String,
        text: String,
        text_truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    Assistant {
        id: String,
        text: String,
        text_truncated: bool,
        provider: Option<String>,
        model: Option<String>,
        tool_calls: Vec<ClientToolCallRef>,
    },
    #[serde(rename_all = "camelCase")]
    Tool {
        id: String,
        name: String,
        outcome: ClientToolOutcome,
        reason_code: Option<String>,
        detail: String,
        detail_truncated: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientToolCallRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientToolOutcome {
    Ok,
    Refused,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl ClientUsage {
    #[must_use]
    pub const fn total(self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientBudget {
    pub provider_turns: u32,
    pub max_provider_turns: u32,
    pub tool_calls: u32,
    pub max_tool_calls: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientQuota {
    pub provider: String,
    pub windows: Vec<ClientQuotaWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientQuotaWindow {
    pub label: String,
    pub used_fraction: f32,
    pub resets_at: Option<String>,
    /// Whether this window's pool actually covers the model in use, per
    /// `pool_covers_model` in `src/tui/chrome.rs`. Google's pools are split by
    /// model family (`"gemini"`, `"claude/gpt"`), so "worst across all
    /// windows" can point at a pool the active model never draws from —
    /// computed here rather than in the frontend, same principle as the
    /// session rollup vocabulary above.
    pub is_relevant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientApproval {
    pub id: String,
    pub tool_name: String,
    pub tier: String,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientRecovery {
    Clean,
    #[serde(rename_all = "camelCase")]
    Required {
        run: String,
        kind: String,
        summary: String,
        effect_is_certain: bool,
        tool_name: Option<String>,
        preview: Option<String>,
    },
}

/// Mirrors `ProviderConnectionState` (`src/core/runtime.rs`). Closed set, same
/// reasoning as `ClientRollupStatus`: an unknown value is a wire bug between
/// two components that ship together, not something to paper over client-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientProviderConnectionState {
    Disconnected,
    Discovering,
    Connected,
    NeedsReauth,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientAccount {
    pub provider: String,
    pub state: ClientProviderConnectionState,
    /// Sanitized remedy or failure summary. Never credential material.
    pub detail: Option<String>,
}

/// Mirrors `SkillScope` (`src/core/context.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientPersonaScope {
    Project,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPersonaSummary {
    pub name: String,
    pub description: Option<String>,
    pub scope: ClientPersonaScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientRoute {
    pub name: String,
    pub roles: Vec<String>,
    pub provider: String,
    pub model: String,
    pub persona: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientModelChoice {
    pub provider: String,
    pub model: String,
    pub display_name: String,
}

/// The rollup vocabulary for a session row (Phase D1).
///
/// One source of truth for the values the sidebar groups on. The runtime
/// produces these in `client_bridge::convert`; the frontend only reads them.
/// D1 collapses to four values until later phases introduce blocked / failed /
/// approval-required rollups. There is deliberately no `Archived` variant:
/// `core::store::SessionStatus` has no archived state, so shipping the variant
/// would let the UI render a group the runtime can never populate. Do not add
/// variants without re-reading `docs/integrated-workspace-phases.md` §D1.
///
/// No `#[serde(other)]` catch-all: an unknown value is a wire bug between two
/// components that ship together, so deserialization refuses it rather than
/// silently filing the session under the wrong group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ClientRollupStatus {
    Running,
    Active,
    Draft,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ClientSessionSummary {
    pub id: String,
    pub title: String,
    pub project_root: String,
    pub status: String,
    pub rollup_status: ClientRollupStatus,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub updated_at: String,
    pub event_count: u64,
    pub leased: bool,
    pub parent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanQuestion {
    pub id: String,
    pub prompt: String,
    pub options: Vec<String>,
    pub is_multi_select: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanAnswer {
    pub question_id: String,
    pub selected_options: Vec<String>,
    pub freeform_text: Option<String>,
    pub answered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPrdRequirement {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientProductRequirementsDocument {
    pub id: String,
    pub plan_id: String,
    pub title: String,
    pub problem: String,
    pub users: Vec<String>,
    pub requirements: Vec<ClientPrdRequirement>,
    pub acceptance_criteria: Vec<String>,
    pub non_goals: Vec<String>,
    pub constraints: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanCouncilLink {
    pub plan_id: String,
    pub prd_id: String,
    pub review_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanProposal {
    pub plan_id: String,
    pub revision_id: u32,
    pub title: String,
    pub summary: String,
    pub steps: Vec<ClientPlanStep>,
    pub proposed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanReview {
    pub plan_id: String,
    pub revision_id: u32,
    pub reviewer: String,
    pub verdict: ClientReviewVerdict,
    pub feedback: String,
    pub reviewed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanApproval {
    pub plan_id: String,
    pub revision_id: u32,
    pub approver: String,
    pub decision: ClientReviewVerdict,
    pub note: Option<String>,
    pub approved_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanHandoff {
    pub plan_id: String,
    pub revision_id: u32,
    pub handoff_note: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientWorkspaceSearchFilter {
    pub query: Option<String>,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub work_kind: Option<String>,
    pub event_kind: Option<String>,
    pub status: Option<String>,
    pub provider_model: Option<String>,
    pub reason_code: Option<String>,
    pub file_path: Option<String>,
    pub time_start: Option<String>,
    pub time_end: Option<String>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientWorkspaceSearchResult {
    pub session_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub match_snippet: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientWorkspaceSearchPage {
    pub items: Vec<ClientWorkspaceSearchResult>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub enum ClientPlanStage {
    Idle,
    #[serde(rename_all = "camelCase")]
    QuestionPending {
        question: ClientPlanQuestion,
    },
    #[serde(rename_all = "camelCase")]
    Proposed {
        proposal: ClientPlanProposal,
    },
    #[serde(rename_all = "camelCase")]
    Reviewed {
        proposal: ClientPlanProposal,
        reviews: Vec<ClientPlanReview>,
    },
    #[serde(rename_all = "camelCase")]
    Approved {
        proposal: ClientPlanProposal,
        approval: ClientPlanApproval,
    },
    #[serde(rename_all = "camelCase")]
    IterateRequested {
        proposal: ClientPlanProposal,
        feedback: String,
    },
    #[serde(rename_all = "camelCase")]
    Rejected {
        proposal: ClientPlanProposal,
        reason: String,
    },
    #[serde(rename_all = "camelCase")]
    Handoff {
        proposal: ClientPlanProposal,
        handoff: ClientPlanHandoff,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientPlanWorkflow {
    pub plan_id: String,
    pub interview_goal: Option<String>,
    pub questions: Vec<ClientPlanQuestion>,
    pub answers: Vec<ClientPlanAnswer>,
    pub prd: Option<ClientProductRequirementsDocument>,
    pub council_link: Option<ClientPlanCouncilLink>,
    pub active_revision: Option<u32>,
    pub stage: ClientPlanStage,
    pub proposals: Vec<ClientPlanProposal>,
    pub reviews: Vec<ClientPlanReview>,
    pub approvals: Vec<ClientPlanApproval>,
    pub handoffs: Vec<ClientPlanHandoff>,
}
