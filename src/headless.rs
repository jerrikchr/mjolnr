//! Non-interactive host for the same governed runtime used by the TUI.

use serde::Serialize;

use crate::core::command::{ApprovalDecision, SmedCommand};
use crate::core::error::{ReasonCode, SmedError};
use crate::core::event::SmedEvent;
use crate::core::message::{ToolEffect, ToolOutcome};
use crate::core::runtime::SmedRuntime;

pub const EXIT_VERIFIED: i32 = 0;
pub const EXIT_REFUSED: i32 = 10;
pub const EXIT_STOPPED: i32 = 20;
pub const EXIT_FAILED: i32 = 30;

/// Stable machine-readable terminal outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessOutcome {
    Verified,
    Refused,
    BudgetOrQuotaStopped,
    Failed,
}

/// One JSON-line summary emitted after a headless run settles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeadlessReport {
    pub session_id: String,
    pub outcome: HeadlessOutcome,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl HeadlessReport {
    #[must_use]
    pub fn setup_failure(code: ReasonCode) -> Self {
        Self {
            session_id: "unknown".to_owned(),
            outcome: HeadlessOutcome::Failed,
            exit_code: EXIT_FAILED,
            reason_code: Some(code.as_str().to_owned()),
        }
    }
}

#[derive(Debug, Default)]
struct Observation {
    session_id: String,
    verified: bool,
    refusal: Option<ReasonCode>,
    failure: Option<ReasonCode>,
    stopped: Option<ReasonCode>,
}

/// Drive one directive to its terminal event. Approval requests are denied
/// immediately using the ordinary runtime command; headless has no hidden yes.
pub async fn run(
    runtime: &dyn SmedRuntime,
    directive: String,
) -> Result<HeadlessReport, SmedError> {
    let mut events = runtime.subscribe();
    runtime
        .dispatch(SmedCommand::SendUserMessage {
            text: directive,
            // `mjolnr exec` is run by the person at the keyboard, or by a CI job
            // they configured. Either way the directive is theirs.
            source: crate::core::directive::DirectiveSource::Human,
        })
        .await?;
    let mut observation = Observation::default();
    loop {
        let event = events.recv().await.map_err(|_| SmedError::RuntimeClosed)?;
        observe(&mut observation, &event);
        if let SmedEvent::ToolProposed {
            approval: Some(approval),
            ..
        } = event
        {
            runtime
                .dispatch(SmedCommand::ResolveApproval {
                    approval,
                    decision: ApprovalDecision::Deny,
                })
                .await?;
        }
        if is_terminal(&event) {
            return Ok(report(observation));
        }
    }
}

fn observe(observation: &mut Observation, event: &SmedEvent) {
    match event {
        SmedEvent::RunStarted { session, .. } => observation.session_id = session.to_string(),
        SmedEvent::ToolCompleted { result, .. } => {
            match result.outcome {
                ToolOutcome::Refused(code) => observation.refusal = Some(code),
                ToolOutcome::Failed(code) => observation.failure = Some(code),
                ToolOutcome::Ok => {}
            }
            if matches!(
                &result.effect,
                ToolEffect::Completion { outcome } if outcome == "verified"
            ) {
                observation.verified = true;
            }
        }
        SmedEvent::ToolFailed { code, .. } | SmedEvent::RunFailed { code, .. } => {
            observation.failure = Some(*code);
        }
        SmedEvent::BudgetExhausted { .. } => {
            observation.stopped = Some(ReasonCode::BudgetExhausted);
        }
        SmedEvent::QuotaBoundaryReached { reserve, .. }
            if reserve.phase == crate::core::continuation::QuotaReservePhase::Stopped =>
        {
            observation.stopped = Some(ReasonCode::ProviderPlanQuota);
        }
        _ => {}
    }
}

fn is_terminal(event: &SmedEvent) -> bool {
    matches!(
        event,
        SmedEvent::RunFinished { .. } | SmedEvent::RunFailed { .. }
    )
}

fn report(mut observation: Observation) -> HeadlessReport {
    let (outcome, exit_code, reason) = if let Some(code) = observation.stopped {
        (
            HeadlessOutcome::BudgetOrQuotaStopped,
            EXIT_STOPPED,
            Some(code),
        )
    } else if let Some(code) = observation.refusal {
        (HeadlessOutcome::Refused, EXIT_REFUSED, Some(code))
    } else if let Some(code) = observation.failure {
        if matches!(
            code,
            ReasonCode::BudgetExhausted | ReasonCode::ProviderPlanQuota
        ) {
            (
                HeadlessOutcome::BudgetOrQuotaStopped,
                EXIT_STOPPED,
                Some(code),
            )
        } else {
            (HeadlessOutcome::Failed, EXIT_FAILED, Some(code))
        }
    } else if observation.verified {
        (HeadlessOutcome::Verified, EXIT_VERIFIED, None)
    } else {
        (
            HeadlessOutcome::Failed,
            EXIT_FAILED,
            Some(ReasonCode::CompletionEvidenceMissing),
        )
    };
    if observation.session_id.is_empty() {
        "unknown".clone_into(&mut observation.session_id);
    }
    HeadlessReport {
        session_id: observation.session_id,
        outcome,
        exit_code,
        reason_code: reason.map(|code| code.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unverified_stop_is_not_success() {
        let report = report(Observation {
            session_id: "session".to_owned(),
            ..Observation::default()
        });
        assert_eq!(report.outcome, HeadlessOutcome::Failed);
        assert_eq!(
            report.reason_code.as_deref(),
            Some(ReasonCode::CompletionEvidenceMissing.as_str())
        );
    }

    #[test]
    fn refusal_and_quota_have_distinct_exit_codes() {
        let refused = report(Observation {
            refusal: Some(ReasonCode::PolicyReadOnly),
            ..Observation::default()
        });
        let stopped = report(Observation {
            stopped: Some(ReasonCode::BudgetExhausted),
            ..Observation::default()
        });
        assert_eq!(refused.exit_code, EXIT_REFUSED);
        assert_eq!(stopped.exit_code, EXIT_STOPPED);
    }
}
