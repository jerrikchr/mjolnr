use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::message::{ToolEffect, ToolResult};
use crate::core::tool::{Tool, ToolContext, ToolTier};

#[derive(Debug)]
pub(super) struct FinishTask;

#[async_trait]
impl Tool for FinishTask {
    fn name(&self) -> &'static str {
        "finish_task"
    }

    fn description(&self) -> &'static str {
        "Finish the task with an honest outcome, durable evidence IDs, and remaining risks"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "outcome": { "type": "string", "enum": ["verified", "unverified"] },
                "summary": { "type": "string", "minLength": 1, "maxLength": 20000 },
                "evidence_event_ids": {
                    "type": "array",
                    "items": { "type": "string", "minLength": 1 },
                    "maxItems": 100
                },
                "remaining_risks": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": 100
                }
            },
            "required": ["outcome", "summary", "evidence_event_ids", "remaining_risks"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok(format!(
            "finish as {}",
            super::arguments::required_string(arguments, "outcome")?
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let outcome = super::arguments::required_string(&arguments, "outcome")?;
        let summary = super::arguments::required_string(&arguments, "summary")?;
        Ok(ToolResult::ok(summary).with_effect(ToolEffect::Completion { outcome }))
    }
}
