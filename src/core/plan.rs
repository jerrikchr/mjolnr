//! Typed plan contracts and append-only workflow authority (plan Phase A1).
//!
//! Replaces plan-shaped prose and UI convention with append-only runtime truth.
//! Delivers the state machine needed for questions, proposals, advisory review,
//! human approval, and handoff.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::error::{MjolnrError, MjolnrResult};

/// Upper bound on the owner brief carried into an interview.
pub const MAX_INTERVIEW_GOAL_CHARS: usize = 8_000;
/// Upper bound on clarification questions in one interview.
pub const MAX_INTERVIEW_QUESTIONS: usize = 8;
/// Upper bound on one model-authored interview field.
pub const MAX_INTERVIEW_FIELD_CHARS: usize = 4_000;
/// Upper bound on requirements in one generated PRD.
pub const MAX_PRD_REQUIREMENTS: usize = 32;

/// Stable identity for a generated product-requirements document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PrdId(Uuid);

impl PrdId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for PrdId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PrdId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One requirement in the generated PRD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrdRequirement {
    pub id: String,
    pub title: String,
    pub description: String,
}

/// The durable artifact produced by the interview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductRequirementsDocument {
    pub id: PrdId,
    pub plan_id: PlanId,
    pub title: String,
    pub problem: String,
    pub users: Vec<String>,
    pub requirements: Vec<PrdRequirement>,
    pub acceptance_criteria: Vec<String>,
    pub non_goals: Vec<String>,
    pub constraints: Vec<String>,
    pub created_at: OffsetDateTime,
}

impl ProductRequirementsDocument {
    /// Render the exact bounded artifact text supplied to the advisory council.
    #[must_use]
    pub fn render_markdown(&self) -> String {
        use std::fmt::Write as _;

        let mut output = format!("# {}\n\n## Problem\n{}\n", self.title, self.problem);
        output.push_str("\n## Users\n");
        for user in &self.users {
            let _ = writeln!(output, "- {user}");
        }
        output.push_str("\n## Requirements\n");
        for requirement in &self.requirements {
            let _ = writeln!(
                output,
                "### {} — {}\n{}\n",
                requirement.id, requirement.title, requirement.description
            );
        }
        output.push_str("\n## Acceptance criteria\n");
        for criterion in &self.acceptance_criteria {
            let _ = writeln!(output, "- {criterion}");
        }
        output.push_str("\n## Non-goals\n");
        for non_goal in &self.non_goals {
            let _ = writeln!(output, "- {non_goal}");
        }
        output.push_str("\n## Constraints\n");
        for constraint in &self.constraints {
            let _ = writeln!(output, "- {constraint}");
        }
        output
    }
}

/// The only two response shapes accepted from the bounded interview model.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InterviewResponse {
    Question {
        prompt: String,
        options: Vec<String>,
        is_multi_select: bool,
    },
    Prd {
        title: String,
        problem: String,
        users: Vec<String>,
        requirements: Vec<PrdRequirement>,
        acceptance_criteria: Vec<String>,
        non_goals: Vec<String>,
        constraints: Vec<String>,
    },
}

/// The only response shape accepted from the plan-synthesis model.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDraft {
    pub title: String,
    pub summary: String,
    pub steps: Vec<PlanStepDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStepDraft {
    pub title: String,
    pub description: String,
}

/// Parse and validate one model response. Markdown fences are accepted because
/// models commonly add them around JSON, but no prose or second object is.
pub fn parse_interview_response(text: &str) -> Result<InterviewResponse, String> {
    let payload = json_payload(text)?;
    let response: InterviewResponse = serde_json::from_str(payload)
        .map_err(|error| format!("interview response is not the required JSON object: {error}"))?;
    validate_interview_response(&response)?;
    Ok(response)
}

/// Parse and validate the plan produced after council review.
pub fn parse_plan_draft(text: &str) -> Result<PlanDraft, String> {
    let payload = json_payload(text)?;
    let draft: PlanDraft = serde_json::from_str(payload)
        .map_err(|error| format!("plan response is not the required JSON object: {error}"))?;
    if draft.title.trim().is_empty() || draft.summary.trim().is_empty() {
        return Err("plan title and summary must not be empty".to_owned());
    }
    if draft.steps.is_empty() || draft.steps.len() > MAX_PRD_REQUIREMENTS {
        return Err(format!(
            "plan must contain between 1 and {MAX_PRD_REQUIREMENTS} steps"
        ));
    }
    for step in &draft.steps {
        validate_text(&step.title, "plan step title")?;
        validate_text(&step.description, "plan step description")?;
    }
    Ok(draft)
}

fn json_payload(text: &str) -> Result<&str, String> {
    let trimmed = text.trim();
    if let Some(fenced) = trimmed.strip_prefix("```") {
        let body = fenced
            .strip_prefix("json")
            .or_else(|| fenced.strip_prefix("JSON"))
            .ok_or_else(|| "interview response fence must be labelled json".to_owned())?
            .trim_start();
        return body
            .strip_suffix("```")
            .map(str::trim)
            .ok_or_else(|| "interview response JSON fence is unterminated".to_owned());
    }
    Ok(trimmed)
}

fn validate_interview_response(response: &InterviewResponse) -> Result<(), String> {
    match response {
        InterviewResponse::Question {
            prompt,
            options,
            is_multi_select: _,
        } => {
            validate_text(prompt, "interview question")?;
            if options.len() > 8 {
                return Err("an interview question may have at most 8 options".to_owned());
            }
            for option in options {
                validate_text(option, "interview option")?;
            }
        }
        InterviewResponse::Prd {
            title,
            problem,
            users,
            requirements,
            acceptance_criteria,
            non_goals,
            constraints,
        } => {
            validate_text(title, "PRD title")?;
            validate_text(problem, "PRD problem")?;
            validate_list(users, "PRD user")?;
            if requirements.is_empty() || requirements.len() > MAX_PRD_REQUIREMENTS {
                return Err(format!(
                    "PRD must contain between 1 and {MAX_PRD_REQUIREMENTS} requirements"
                ));
            }
            for requirement in requirements {
                validate_text(&requirement.id, "requirement id")?;
                validate_text(&requirement.title, "requirement title")?;
                validate_text(&requirement.description, "requirement description")?;
            }
            validate_list(acceptance_criteria, "acceptance criterion")?;
            validate_list(non_goals, "non-goal")?;
            validate_list(constraints, "constraint")?;
        }
    }
    Ok(())
}

fn validate_list(values: &[String], label: &str) -> Result<(), String> {
    if values.len() > MAX_PRD_REQUIREMENTS {
        return Err(format!("{label} list is too large"));
    }
    for value in values {
        validate_text(value, label)?;
    }
    Ok(())
}

fn validate_text(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.chars().count() > MAX_INTERVIEW_FIELD_CHARS {
        return Err(format!(
            "{label} exceeds {MAX_INTERVIEW_FIELD_CHARS} characters"
        ));
    }
    Ok(())
}

/// Unique identifier for a plan workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PlanId(Uuid);

impl PlanId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for PlanId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Revision number for a plan proposal (1-indexed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RevisionId(u32);

impl RevisionId {
    #[must_use]
    pub const fn initial() -> Self {
        Self(1)
    }

    #[must_use]
    pub const fn new(revision: u32) -> Self {
        Self(revision)
    }

    #[must_use]
    pub const fn value(&self) -> u32 {
        self.0
    }

    #[must_use]
    pub fn next(&self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl std::fmt::Display for RevisionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "v{}", self.0)
    }
}

/// Unique identifier for a clarification question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QuestionId(Uuid);

impl QuestionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for QuestionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for QuestionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A clarification question asked to resolve ambiguity before or during planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: QuestionId,
    pub prompt: String,
    pub options: Vec<String>,
    pub is_multi_select: bool,
    pub created_at: OffsetDateTime,
}

/// Answer provided for a clarification question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub question_id: QuestionId,
    pub selected_options: Vec<String>,
    pub freeform_text: Option<String>,
    pub answered_at: OffsetDateTime,
}

/// A single step in a plan proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub index: usize,
    pub title: String,
    pub description: String,
}

/// A proposed plan revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanProposal {
    pub plan_id: PlanId,
    pub revision_id: RevisionId,
    pub title: String,
    pub summary: String,
    pub steps: Vec<PlanStep>,
    pub proposed_at: OffsetDateTime,
}

/// Verdict returned by advisory reviews or human decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewVerdict {
    Approve,
    Iterate,
    Reject,
}

impl std::fmt::Display for ReviewVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Approve => write!(f, "Approve"),
            Self::Iterate => write!(f, "Iterate"),
            Self::Reject => write!(f, "Reject"),
        }
    }
}

/// Advisory review for a plan revision (e.g. model or council review).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReview {
    pub plan_id: PlanId,
    pub revision_id: RevisionId,
    pub reviewer: String,
    pub verdict: ReviewVerdict,
    pub feedback: String,
    pub reviewed_at: OffsetDateTime,
}

/// Durable human approval decision for a plan revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanApproval {
    pub plan_id: PlanId,
    pub revision_id: RevisionId,
    pub approver: String,
    pub decision: ReviewVerdict,
    pub note: Option<String>,
    pub approved_at: OffsetDateTime,
}

/// Handoff record once an approved plan enters execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanHandoff {
    pub plan_id: PlanId,
    pub revision_id: RevisionId,
    pub handoff_note: String,
    pub created_at: OffsetDateTime,
}

/// The durable link between the generated PRD, its advisory council review,
/// and the plan workflow that consumed both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCouncilLink {
    pub plan_id: PlanId,
    pub prd_id: PrdId,
    pub review_id: crate::core::council::CouncilReviewId,
}

/// Current status stage of a plan workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanStage {
    Idle,
    QuestionPending {
        question: Question,
    },
    Proposed {
        proposal: PlanProposal,
    },
    Reviewed {
        proposal: PlanProposal,
        reviews: Vec<PlanReview>,
    },
    Approved {
        proposal: PlanProposal,
        approval: PlanApproval,
    },
    IterateRequested {
        proposal: PlanProposal,
        feedback: String,
    },
    Rejected {
        proposal: PlanProposal,
        reason: String,
    },
    Handoff {
        proposal: PlanProposal,
        handoff: PlanHandoff,
    },
}

/// Deterministic state machine governing plan workflow transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanWorkflow {
    pub plan_id: PlanId,
    pub interview_goal: Option<String>,
    pub questions: Vec<Question>,
    pub answers: Vec<QuestionAnswer>,
    pub prd: Option<ProductRequirementsDocument>,
    pub council_link: Option<PlanCouncilLink>,
    pub active_revision: Option<RevisionId>,
    pub stage: PlanStage,
    pub proposals: Vec<PlanProposal>,
    pub reviews: Vec<PlanReview>,
    pub approvals: Vec<PlanApproval>,
    pub handoffs: Vec<PlanHandoff>,
}

impl PlanWorkflow {
    #[must_use]
    pub fn new(plan_id: PlanId) -> Self {
        Self {
            plan_id,
            interview_goal: None,
            questions: Vec::new(),
            answers: Vec::new(),
            prd: None,
            council_link: None,
            active_revision: None,
            stage: PlanStage::Idle,
            proposals: Vec::new(),
            reviews: Vec::new(),
            approvals: Vec::new(),
            handoffs: Vec::new(),
        }
    }

    /// Begin the bounded model-led interview for this workflow.
    pub fn start_interview(&mut self, goal: String) -> MjolnrResult<()> {
        if goal.trim().is_empty() || goal.chars().count() > MAX_INTERVIEW_GOAL_CHARS {
            return Err(MjolnrError::plan_invalid_transition(
                "no interview",
                "start interview",
                "the interview goal is empty or exceeds its bound",
            ));
        }
        if !matches!(self.stage, PlanStage::Idle) || self.interview_goal.is_some() {
            return Err(MjolnrError::plan_invalid_transition(
                "workflow already started",
                "start interview",
                "an interview can only start on a new idle workflow",
            ));
        }
        self.interview_goal = Some(goal);
        Ok(())
    }

    /// Record a question obligation.
    pub fn ask_question(&mut self, question: Question) -> MjolnrResult<()> {
        if self.questions.len() >= MAX_INTERVIEW_QUESTIONS {
            return Err(MjolnrError::plan_invalid_transition(
                "question budget exhausted",
                "ask question",
                "the bounded interview has reached its question limit",
            ));
        }
        match &self.stage {
            PlanStage::Idle | PlanStage::IterateRequested { .. } => {
                self.questions.push(question.clone());
                self.stage = PlanStage::QuestionPending { question };
                Ok(())
            }
            PlanStage::QuestionPending { .. } => Err(MjolnrError::plan_invalid_transition(
                "question pending",
                "ask question",
                "cannot ask a new question while another question is pending",
            )),
            PlanStage::Proposed { .. }
            | PlanStage::Reviewed { .. }
            | PlanStage::Approved { .. }
            | PlanStage::Rejected { .. }
            | PlanStage::Handoff { .. } => Err(MjolnrError::plan_invalid_transition(
                "proposal decision",
                "ask question",
                "questions require an idle workflow or an explicit iterate decision",
            )),
        }
    }

    /// Answer a pending question.
    pub fn answer_question(&mut self, answer: &QuestionAnswer) -> MjolnrResult<()> {
        match &self.stage {
            PlanStage::QuestionPending { question } => {
                if question.id != answer.question_id {
                    return Err(MjolnrError::plan_invalid_transition(
                        "question pending",
                        "answer question",
                        "answer question_id does not match pending question",
                    ));
                }
                self.answers.push(answer.clone());
                self.stage = PlanStage::Idle;
                Ok(())
            }
            _ => Err(MjolnrError::plan_invalid_transition(
                "not question pending",
                "answer question",
                "no question is currently pending",
            )),
        }
    }

    /// Persist the PRD produced after the interview has enough answers.
    pub fn record_prd(&mut self, prd: ProductRequirementsDocument) -> MjolnrResult<()> {
        if prd.plan_id != self.plan_id {
            return Err(MjolnrError::plan_invalid_transition(
                "plan mismatch",
                "record PRD",
                "PRD plan_id does not match workflow plan_id",
            ));
        }
        if self.interview_goal.is_none() || self.prd.is_some() {
            return Err(MjolnrError::plan_invalid_transition(
                "PRD state mismatch",
                "record PRD",
                "a PRD requires an active interview and may only be recorded once",
            ));
        }
        if !matches!(self.stage, PlanStage::Idle) {
            return Err(MjolnrError::plan_invalid_transition(
                "question pending",
                "record PRD",
                "the interview must answer its pending question before producing a PRD",
            ));
        }
        self.prd = Some(prd);
        Ok(())
    }

    /// Attach the completed advisory council review to the generated PRD.
    pub fn link_council_review(&mut self, link: PlanCouncilLink) -> MjolnrResult<()> {
        if link.plan_id != self.plan_id {
            return Err(MjolnrError::plan_invalid_transition(
                "plan mismatch",
                "link council review",
                "council link plan_id does not match workflow plan_id",
            ));
        }
        let prd = self.prd.as_ref().ok_or_else(|| {
            MjolnrError::plan_invalid_transition(
                "no PRD",
                "link council review",
                "a council review cannot link before a PRD exists",
            )
        })?;
        if prd.id != link.prd_id || self.council_link.is_some() {
            return Err(MjolnrError::plan_invalid_transition(
                "council link mismatch",
                "link council review",
                "the review must name the current PRD and may only link once",
            ));
        }
        self.council_link = Some(link);
        Ok(())
    }

    /// Propose a new or revised plan.
    pub fn propose_plan(&mut self, proposal: PlanProposal) -> MjolnrResult<()> {
        if proposal.plan_id != self.plan_id {
            return Err(MjolnrError::plan_invalid_transition(
                "plan mismatch",
                "propose plan",
                "proposal plan_id does not match workflow plan_id",
            ));
        }

        if !matches!(
            &self.stage,
            PlanStage::Idle | PlanStage::IterateRequested { .. }
        ) {
            return Err(MjolnrError::plan_invalid_transition(
                "stage mismatch",
                "propose plan",
                "a proposal requires an idle workflow or an explicit iterate decision",
            ));
        }

        if let Some(active) = self.active_revision {
            if proposal.revision_id.value() <= active.value() {
                return Err(MjolnrError::plan_stale_revision(
                    proposal.revision_id.value(),
                    active.value(),
                ));
            }
            if proposal.revision_id.value() != active.value() + 1 {
                return Err(MjolnrError::plan_invalid_transition(
                    "proposal",
                    "propose plan",
                    "proposal revision jumps sequence numbers",
                ));
            }
        } else if proposal.revision_id.value() != 1 {
            return Err(MjolnrError::plan_stale_revision(
                proposal.revision_id.value(),
                1,
            ));
        }

        self.active_revision = Some(proposal.revision_id);
        self.stage = PlanStage::Proposed {
            proposal: proposal.clone(),
        };
        self.proposals.push(proposal);
        Ok(())
    }

    /// Record an advisory review.
    pub fn review_plan(&mut self, review: PlanReview) -> MjolnrResult<()> {
        if review.plan_id != self.plan_id {
            return Err(MjolnrError::plan_invalid_transition(
                "plan mismatch",
                "review plan",
                "review plan_id does not match workflow plan_id",
            ));
        }

        let active = self.active_revision.ok_or_else(|| {
            MjolnrError::plan_invalid_transition(
                "idle",
                "review plan",
                "cannot review a plan before a proposal exists",
            )
        })?;

        if review.revision_id != active {
            return Err(MjolnrError::plan_stale_revision(
                review.revision_id.value(),
                active.value(),
            ));
        }

        let proposal = match &self.stage {
            PlanStage::Proposed { proposal } | PlanStage::Reviewed { proposal, .. } => {
                proposal.clone()
            }
            _ => {
                return Err(MjolnrError::plan_invalid_transition(
                    "stage mismatch",
                    "review plan",
                    "can only review a proposed or currently reviewed plan revision",
                ));
            }
        };

        self.reviews.push(review);
        let active_reviews: Vec<PlanReview> = self
            .reviews
            .iter()
            .filter(|r| r.revision_id == active)
            .cloned()
            .collect();

        self.stage = PlanStage::Reviewed {
            proposal,
            reviews: active_reviews,
        };
        Ok(())
    }

    /// Record a human approval or rejection decision.
    pub fn approve_plan(&mut self, approval: PlanApproval) -> MjolnrResult<()> {
        if approval.plan_id != self.plan_id {
            return Err(MjolnrError::plan_invalid_transition(
                "plan mismatch",
                "approve plan",
                "approval plan_id does not match workflow plan_id",
            ));
        }

        let active = self.active_revision.ok_or_else(|| {
            MjolnrError::plan_invalid_transition(
                "idle",
                "approve plan",
                "cannot approve a plan before a proposal exists",
            )
        })?;

        if approval.revision_id != active {
            return Err(MjolnrError::plan_stale_revision(
                approval.revision_id.value(),
                active.value(),
            ));
        }

        let proposal = match &self.stage {
            PlanStage::Proposed { proposal } | PlanStage::Reviewed { proposal, .. } => {
                proposal.clone()
            }
            _ => {
                return Err(MjolnrError::plan_invalid_transition(
                    "stage mismatch",
                    "approve plan",
                    "can only approve a proposed or reviewed plan revision",
                ));
            }
        };

        self.approvals.push(approval.clone());
        match approval.decision {
            ReviewVerdict::Approve => {
                self.stage = PlanStage::Approved { proposal, approval };
            }
            ReviewVerdict::Iterate => {
                self.stage = PlanStage::IterateRequested {
                    proposal,
                    feedback: approval
                        .note
                        .unwrap_or_else(|| "Iteration requested".to_string()),
                };
            }
            ReviewVerdict::Reject => {
                self.stage = PlanStage::Rejected {
                    proposal,
                    reason: approval.note.unwrap_or_else(|| "Plan rejected".to_string()),
                };
            }
        }
        Ok(())
    }

    /// Transition approved plan to handoff state.
    pub fn handoff_plan(&mut self, handoff: PlanHandoff) -> MjolnrResult<()> {
        if handoff.plan_id != self.plan_id {
            return Err(MjolnrError::plan_invalid_transition(
                "plan mismatch",
                "handoff plan",
                "handoff plan_id does not match workflow plan_id",
            ));
        }

        let active = self.active_revision.ok_or_else(|| {
            MjolnrError::plan_invalid_transition(
                "idle",
                "handoff plan",
                "cannot handoff a plan before a proposal exists",
            )
        })?;

        if handoff.revision_id != active {
            return Err(MjolnrError::plan_stale_revision(
                handoff.revision_id.value(),
                active.value(),
            ));
        }

        let (proposal, _approval) = match &self.stage {
            PlanStage::Approved { proposal, approval } => (proposal.clone(), approval.clone()),
            _ => {
                return Err(MjolnrError::plan_invalid_transition(
                    "not approved",
                    "handoff plan",
                    "can only handoff an approved plan revision",
                ));
            }
        };

        self.handoffs.push(handoff.clone());
        self.stage = PlanStage::Handoff { proposal, handoff };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_proposal(plan_id: PlanId, revision: u32) -> PlanProposal {
        PlanProposal {
            plan_id,
            revision_id: RevisionId::new(revision),
            title: format!("Plan Revision {revision}"),
            summary: "Testing plan proposal".to_string(),
            steps: vec![PlanStep {
                index: 1,
                title: "Step 1".to_string(),
                description: "Execute step 1".to_string(),
            }],
            proposed_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn full_valid_workflow_lifecycle() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);

        // 1. Question asked
        let q_id = QuestionId::new();
        let question = Question {
            id: q_id,
            prompt: "Which database strategy?".to_string(),
            options: vec!["SQLite".to_string(), "Postgres".to_string()],
            is_multi_select: false,
            created_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.ask_question(question.clone()).is_ok());

        // 2. Question answered
        let answer = QuestionAnswer {
            question_id: q_id,
            selected_options: vec!["SQLite".to_string()],
            freeform_text: None,
            answered_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.answer_question(&answer).is_ok());

        // 3. Plan proposed (v1)
        let proposal_v1 = make_proposal(plan_id, 1);
        assert!(workflow.propose_plan(proposal_v1.clone()).is_ok());
        assert_eq!(workflow.active_revision, Some(RevisionId::new(1)));

        // 4. Advisory review (v1)
        let review = PlanReview {
            plan_id,
            revision_id: RevisionId::new(1),
            reviewer: "Council".to_string(),
            verdict: ReviewVerdict::Approve,
            feedback: "Looks good".to_string(),
            reviewed_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.review_plan(review).is_ok());

        // 5. Human approval (v1)
        let approval = PlanApproval {
            plan_id,
            revision_id: RevisionId::new(1),
            approver: "Jerrik".to_string(),
            decision: ReviewVerdict::Approve,
            note: Some("Approved for launch".to_string()),
            approved_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.approve_plan(approval).is_ok());

        // 6. Handoff
        let handoff = PlanHandoff {
            plan_id,
            revision_id: RevisionId::new(1),
            handoff_note: "Handoff to execution engine".to_string(),
            created_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.handoff_plan(handoff).is_ok());
        assert!(matches!(workflow.stage, PlanStage::Handoff { .. }));
    }

    #[test]
    fn stale_revision_approval_refused() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);

        let proposal_v1 = make_proposal(plan_id, 1);
        assert!(workflow.propose_plan(proposal_v1).is_ok());

        let iterate = PlanApproval {
            plan_id,
            revision_id: RevisionId::new(1),
            approver: "Jerrik".to_string(),
            decision: ReviewVerdict::Iterate,
            note: Some("Revise it".to_string()),
            approved_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.approve_plan(iterate).is_ok());

        let proposal_v2 = make_proposal(plan_id, 2);
        assert!(workflow.propose_plan(proposal_v2).is_ok());

        // Attempt to approve stale v1 when v2 is active
        let stale_approval = PlanApproval {
            plan_id,
            revision_id: RevisionId::new(1),
            approver: "Jerrik".to_string(),
            decision: ReviewVerdict::Approve,
            note: None,
            approved_at: OffsetDateTime::now_utc(),
        };

        let err = workflow.approve_plan(stale_approval).unwrap_err();
        assert!(matches!(
            err,
            MjolnrError::PlanStaleRevision {
                attempted: 1,
                current: 2
            }
        ));
    }

    #[test]
    fn unapproved_plan_handoff_refused() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);

        let proposal_v1 = make_proposal(plan_id, 1);
        assert!(workflow.propose_plan(proposal_v1).is_ok());

        let handoff = PlanHandoff {
            plan_id,
            revision_id: RevisionId::new(1),
            handoff_note: "Premature handoff".to_string(),
            created_at: OffsetDateTime::now_utc(),
        };

        let err = workflow.handoff_plan(handoff).unwrap_err();
        assert!(matches!(err, MjolnrError::PlanInvalidTransition { .. }));
    }

    #[test]
    fn answer_wrong_question_id_refused() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);

        let question = Question {
            id: QuestionId::new(),
            prompt: "Prompt".to_string(),
            options: vec!["A".to_string()],
            is_multi_select: false,
            created_at: OffsetDateTime::now_utc(),
        };
        assert!(workflow.ask_question(question).is_ok());

        let wrong_answer = QuestionAnswer {
            question_id: QuestionId::new(),
            selected_options: vec!["A".to_string()],
            freeform_text: None,
            answered_at: OffsetDateTime::now_utc(),
        };

        let err = workflow.answer_question(&wrong_answer).unwrap_err();
        assert!(matches!(err, MjolnrError::PlanInvalidTransition { .. }));
    }

    #[test]
    fn question_pending_refuses_a_proposal() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);
        let question = Question {
            id: QuestionId::new(),
            prompt: "Choose".to_string(),
            options: vec!["A".to_string()],
            is_multi_select: false,
            created_at: OffsetDateTime::now_utc(),
        };
        workflow.ask_question(question).expect("question");

        let error = workflow
            .propose_plan(make_proposal(plan_id, 1))
            .expect_err("must refuse");
        assert!(matches!(error, MjolnrError::PlanInvalidTransition { .. }));
    }

    #[test]
    fn iterate_question_preserves_history_and_requires_next_revision() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);
        workflow
            .propose_plan(make_proposal(plan_id, 1))
            .expect("proposal");
        workflow
            .approve_plan(PlanApproval {
                plan_id,
                revision_id: RevisionId::new(1),
                approver: "Jerrik".to_string(),
                decision: ReviewVerdict::Iterate,
                note: Some("Clarify scope".to_string()),
                approved_at: OffsetDateTime::now_utc(),
            })
            .expect("iterate");
        let question_id = QuestionId::new();
        workflow
            .ask_question(Question {
                id: question_id,
                prompt: "Which scope?".to_string(),
                options: vec!["Narrow".to_string()],
                is_multi_select: false,
                created_at: OffsetDateTime::now_utc(),
            })
            .expect("follow-up question");
        workflow
            .answer_question(&QuestionAnswer {
                question_id,
                selected_options: vec!["Narrow".to_string()],
                freeform_text: None,
                answered_at: OffsetDateTime::now_utc(),
            })
            .expect("answer");

        assert_eq!(workflow.proposals.len(), 1);
        assert!(matches!(
            workflow.propose_plan(make_proposal(plan_id, 1)),
            Err(MjolnrError::PlanStaleRevision { .. })
        ));
        workflow
            .propose_plan(make_proposal(plan_id, 2))
            .expect("next revision");
    }

    #[test]
    fn approved_and_handed_off_workflows_are_terminal() {
        let plan_id = PlanId::new();
        let mut workflow = PlanWorkflow::new(plan_id);
        workflow
            .propose_plan(make_proposal(plan_id, 1))
            .expect("proposal");
        workflow
            .approve_plan(PlanApproval {
                plan_id,
                revision_id: RevisionId::new(1),
                approver: "Jerrik".to_string(),
                decision: ReviewVerdict::Approve,
                note: None,
                approved_at: OffsetDateTime::now_utc(),
            })
            .expect("short-plan approval");

        assert!(matches!(
            workflow.propose_plan(make_proposal(plan_id, 2)),
            Err(MjolnrError::PlanInvalidTransition { .. })
        ));
        workflow
            .handoff_plan(PlanHandoff {
                plan_id,
                revision_id: RevisionId::new(1),
                handoff_note: "Execute".to_string(),
                created_at: OffsetDateTime::now_utc(),
            })
            .expect("handoff");
        assert!(matches!(
            workflow.ask_question(Question {
                id: QuestionId::new(),
                prompt: "Too late?".to_string(),
                options: Vec::new(),
                is_multi_select: false,
                created_at: OffsetDateTime::now_utc(),
            }),
            Err(MjolnrError::PlanInvalidTransition { .. })
        ));
    }

    #[test]
    fn bounded_interview_parser_accepts_question_and_prd_json() {
        let question = parse_interview_response(
            r#"{"kind":"question","prompt":"Which users?","options":["owners"],"is_multi_select":false}"#,
        )
        .expect("question JSON");
        assert!(matches!(question, InterviewResponse::Question { .. }));

        let prd = parse_interview_response(
            r#"```json
{"kind":"prd","title":"Title","problem":"Problem","users":["owner"],"requirements":[{"id":"REQ-1","title":"Need","description":"Do it"}],"acceptance_criteria":["It works"],"non_goals":["No automation"],"constraints":["Local"]}
```"#,
        )
        .expect("PRD JSON fence");
        assert!(matches!(prd, InterviewResponse::Prd { .. }));
    }

    #[test]
    fn bounded_interview_parser_rejects_prose_and_oversized_fields() {
        assert!(parse_interview_response(
            "Here is the answer: {\"kind\":\"question\",\"prompt\":\"x\",\"options\":[],\"is_multi_select\":false}"
        )
        .is_err());

        let oversized = "x".repeat(MAX_INTERVIEW_FIELD_CHARS + 1);
        let response = format!(
            "{{\"kind\":\"question\",\"prompt\":{oversized:?},\"options\":[],\"is_multi_select\":false}}"
        );
        assert!(parse_interview_response(&response).is_err());
    }

    #[test]
    fn plan_parser_accepts_fenced_draft_and_rejects_unknown_fields() {
        let draft = parse_plan_draft(
            r#"```json
{"title":"Plan","summary":"Bounded","steps":[{"title":"One","description":"Do one thing"}]}
```"#,
        )
        .expect("plan draft");
        assert_eq!(draft.steps.len(), 1);
        assert!(
            parse_plan_draft(r#"{"title":"Plan","summary":"Bounded","steps":[],"approved":true}"#,)
                .is_err()
        );
    }
}
