//! Reading this session's own record.
//!
//! smed's session is an append-only event log that the session replays from,
//! and until now the model that produced those events could not read them. It
//! saw the transcript the provider loop assembles and nothing else, so a fact
//! established forty turns ago was re-derived, re-asked, or guessed at.
//!
//! [`QuerySession`] is a **marker**, exactly like
//! [`super::subagent::SpawnSubagent`]: the runtime actor intercepts it after the
//! ordinary schema and policy pipeline and answers from the store it already
//! holds. No tool is given a store handle, and none may be — [`EventStore`]
//! carries `append`, so a tool holding it could write the ledger that evidence,
//! recovery, and the audit all rest on. A tool that could append could forge the
//! evidence a completion is gated on.
//!
//! [`EventStore`]: crate::core::store::EventStore

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};

/// Most entries one query may return.
///
/// A window, not an export: the useful question is nearly always "what happened
/// recently", and a call that could return the whole session would put the thing
/// this feature exists to avoid — an unbounded transcript — back into context.
pub const MAX_ENTRIES: usize = 50;

/// Default when the call names no limit.
pub const DEFAULT_ENTRIES: usize = 20;

/// Longest per-entry summary. Tool results are already bounded when recorded;
/// this bounds them again rather than trusting that they were.
pub const MAX_SUMMARY_CHARS: usize = 240;

#[derive(Debug)]
pub struct QuerySession;

impl QuerySession {
    pub const NAME: &'static str = "query_session";
}

#[async_trait]
impl Tool for QuerySession {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Read this session's own recorded history — what you did, what was approved or refused, and what it produced. Newest first. Covers only this session."
    }

    fn tier(&self) -> ToolTier {
        // Every event this can return describes something that already passed
        // the gate: a refused write is in the log as a refusal, and reading that
        // it was refused is not permission to retry it. Replaying decisions
        // already made grants nothing, so this is an ordinary read.
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_ENTRIES,
                    "default": DEFAULT_ENTRIES,
                    "description": "How many entries to return, newest first."
                },
                "kind": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 64,
                    "description": "Return only entries of this kind, e.g. tool_completed, tool_proposed, message_appended, policy_changed, run_failed. Omit for all kinds."
                }
            },
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let limit = arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_ENTRIES as u64);
        let kind = arguments
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("any kind");
        Ok(format!(
            "read this session's last {limit} recorded event(s), {kind}"
        ))
    }

    /// Never runs: the actor intercepts this tool by name. If a future refactor
    /// breaks that seam, this is a loud failure rather than a silent empty
    /// window — which the model would read as "nothing happened".
    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Execution {
            detail: "query_session is answered by the runtime, not executed as a tool".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;

    #[test]
    fn the_schema_is_local_and_valid() {
        assert!(jsonschema::meta::is_valid(&QuerySession.schema()));
        assert!(!QuerySession.schema().to_string().contains("$ref"));
    }

    #[test]
    fn reading_your_own_record_is_a_read() {
        // The gate consequence: allowed in every policy mode, including
        // read-only, where re-deriving a known fact is most expensive.
        assert_eq!(QuerySession.tier(), ToolTier::Read);
    }

    #[test]
    fn the_window_cannot_be_widened_past_the_cap() {
        let registry = ToolRegistry::new(vec![std::sync::Arc::new(QuerySession)]);
        let tool = registry.get(QuerySession::NAME).expect("registered");
        assert!(
            registry
                .validate(
                    tool.as_ref(),
                    &serde_json::json!({ "limit": MAX_ENTRIES + 1 })
                )
                .is_err(),
            "an unbounded window puts the whole transcript back in context"
        );
        assert!(
            registry
                .validate(tool.as_ref(), &serde_json::json!({ "limit": 0 }))
                .is_err()
        );
    }

    #[test]
    fn there_is_no_way_to_name_another_session() {
        // Scope is structural, not validated: the schema forbids extra
        // properties and has no session parameter, so there is no argument the
        // model can pass to ask about a session that is not its own.
        let registry = ToolRegistry::new(vec![std::sync::Arc::new(QuerySession)]);
        let tool = registry.get(QuerySession::NAME).expect("registered");
        assert!(
            registry
                .validate(
                    tool.as_ref(),
                    &serde_json::json!({ "session": "019f0000-0000-7000-8000-000000000000" })
                )
                .is_err(),
            "a session parameter must not be accepted, even to be ignored"
        );
    }

    #[tokio::test]
    async fn executing_the_marker_is_a_loud_failure() {
        let error = QuerySession
            .execute(
                serde_json::json!({}),
                ToolContext {
                    workspace_root: std::path::PathBuf::from("/tmp"),
                    read_set: std::sync::Arc::default(),
                    max_output_bytes: 1024,
                    command_timeout: std::time::Duration::from_secs(1),
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("the marker must never execute");
        assert!(matches!(error, ToolError::Execution { .. }));
    }
}
