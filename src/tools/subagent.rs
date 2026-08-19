//! Subagent tools.
//!
//! Two tools live here with one boundary between them:
//!
//! - [`SpawnSubagent`] is what a *parent* model proposes. It is a marker: the
//!   runtime actor intercepts it after the ordinary schema/policy/approval
//!   pipeline and hosts the children itself, because spawning needs providers,
//!   the store, and budget state that no tool is given — and must never be
//!   given (a tool that could mint runtimes could mint ungoverned ones).
//! - [`ReportResult`] is what a *child* model calls to hand back its bounded
//!   result. Its schema **is** the parent-supplied result schema, so the
//!   ordinary registry validation performs the mechanical schema check and a
//!   child that reports garbage gets an ordinary `SCHEMA_INVALID` refusal it
//!   can correct within its own budget.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};

/// Most children one spawn call may dispatch **without an armed envelope**.
///
/// Fan-out is frugal on purpose: a swarm is a spend decision, and an
/// unbounded one is not previewable. That reasoning is about what one human can
/// read in one approval, which is why an envelope — a shape approved once, in
/// advance — can raise it and a bigger constant cannot.
pub const MAX_CHILDREN: usize = 4;

/// The schema's ceiling, which an armed envelope may reach.
///
/// The schema has to admit the widest legal call; whether *this* call is legal
/// depends on session state the schema cannot see, so the runtime refuses a draw
/// above [`MAX_CHILDREN`] when no envelope is in force.
pub const MAX_CHILDREN_ENVELOPED: usize = crate::core::envelope::MAX_ENVELOPE_PER_CALL as usize;

/// Default per-child budget slices. Deliberately small: a child is a bounded
/// sub-task, not a second session.
pub const DEFAULT_CHILD_TURNS: u32 = 8;
pub const DEFAULT_CHILD_TOOL_CALLS: u32 = 16;

#[derive(Debug)]
pub struct SpawnSubagent;

impl SpawnSubagent {
    pub const NAME: &'static str = "spawn_subagent";
}

#[async_trait]
impl Tool for SpawnSubagent {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Delegate bounded sub-tasks to isolated child sessions, one git worktree each, and receive schema-validated results"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Execute
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "children": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_CHILDREN_ENVELOPED,
                    "items": {
                        "type": "object",
                        "properties": {
                            "directive": { "type": "string", "minLength": 1, "maxLength": 10000 },
                            "policy": {
                                "type": "string",
                                "enum": ["read-only", "workspace-write", "full-auto"],
                                "default": "read-only"
                            },
                            "max_provider_turns": {
                                "type": "integer", "minimum": 1, "maximum": 50,
                                "default": DEFAULT_CHILD_TURNS
                            },
                            "max_tool_calls": {
                                "type": "integer", "minimum": 1, "maximum": 100,
                                "default": DEFAULT_CHILD_TOOL_CALLS
                            },
                            "result_schema": {
                                "type": "object",
                                "description": "JSON Schema the child's reported result must satisfy"
                            },
                            "route": {
                                "type": "string",
                                "description": "A route named in this project's .mjolnr/routes/. Omit to use the configured child default."
                            },
                            "role": {
                                "type": "string",
                                "description": "A role tag (e.g. default, smol, slow, plan) resolved through this project's routes. Preferred over naming a route literally: the project decides what the role points at. An unmapped role falls back to `route`, then to the configured child default."
                            }
                        },
                        "required": ["directive"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["children"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let children = arguments
            .get("children")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ToolError::SchemaInvalid {
                detail: "children must be an array".to_owned(),
            })?;
        let mut lines = vec![format!("spawn {} subagent(s):", children.len())];
        for (index, child) in children.iter().enumerate() {
            let directive = child
                .get("directive")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let policy = child
                .get("policy")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("read-only");
            let turns = child
                .get("max_provider_turns")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::from(DEFAULT_CHILD_TURNS));
            let calls = child
                .get("max_tool_calls")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::from(DEFAULT_CHILD_TOOL_CALLS));
            let mut summary = directive.chars().take(120).collect::<String>();
            if directive.chars().count() > 120 {
                summary.push('…');
            }
            lines.push(format!(
                "  {}. [{policy}, {turns} turns / {calls} tool calls] {summary}",
                index + 1
            ));
        }
        lines.push("each child works in its own git worktree on its own branch".to_owned());
        Ok(lines.join("\n"))
    }

    /// Never runs: the actor intercepts this tool by name after approval. If a
    /// future refactor breaks that seam, this is a loud failure instead of a
    /// silent no-op child.
    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Execution {
            detail: "spawn_subagent is hosted by the runtime, not executed as a tool".to_owned(),
        })
    }
}

/// Where a child's reported result lands for the orchestrator to collect.
pub type ResultSlot = Arc<Mutex<Option<serde_json::Value>>>;

/// The child-side result channel. Registered only in child registries.
#[derive(Debug)]
pub struct ReportResult {
    schema: serde_json::Value,
    slot: ResultSlot,
}

impl ReportResult {
    pub const NAME: &'static str = "report_result";

    #[must_use]
    pub fn new(schema: serde_json::Value, slot: ResultSlot) -> Self {
        Self { schema, slot }
    }
}

#[async_trait]
impl Tool for ReportResult {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Report this subagent's final result to the session that spawned it. Call exactly once, before finish_task."
    }

    fn tier(&self) -> ToolTier {
        // Reporting a result has no side effect outside the parent's own
        // settlement machinery; gating it would let an ask-less child hang.
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn preview(
        &self,
        _arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok("report the subagent result".to_owned())
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
        let mut slot = self.slot.lock().map_err(|_| ToolError::Execution {
            detail: "result slot is unavailable".to_owned(),
        })?;
        // Last write wins, disclosed: the settlement reports what it received.
        let replaced = slot.replace(arguments).is_some();
        drop(slot);
        Ok(ToolResult::ok(if replaced {
            "result recorded (replaced an earlier report)"
        } else {
            "result recorded"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;

    #[test]
    fn a_governance_tier_never_widens_the_standing_fan_out_cap() {
        // Same arrangement, same reason: `core::governance` carries its own copy
        // of MAX_CHILDREN because it may not import `tools`. If the two drift,
        // a tier that is supposed to leave the cap alone starts raising it —
        // which is the one thing a *ceiling* must never do.
        use crate::core::governance::GovernanceTier;
        for tier in [
            GovernanceTier::Supervised,
            GovernanceTier::Standard,
            GovernanceTier::Trusted,
        ] {
            assert!(
                tier.fan_out() as usize <= MAX_CHILDREN,
                "{} raised the un-enveloped cap to {}",
                tier.label(),
                tier.fan_out()
            );
        }
        assert_eq!(
            GovernanceTier::Trusted.fan_out() as usize,
            MAX_CHILDREN,
            "the most trusted tier is the absence of a narrowing, not a wider cap"
        );
    }

    #[test]
    fn the_envelope_budgets_a_child_the_same_turns_a_child_actually_gets() {
        // `core` may not import `tools` (AGENTS.md §2.1), so an envelope
        // deriving its turn budget carries its own copy of this number. If the
        // two drift, an envelope silently under- or over-funds every child it
        // authorises, and nothing else would notice.
        assert_eq!(
            crate::core::envelope::DEFAULT_TURNS_PER_CHILD,
            DEFAULT_CHILD_TURNS
        );
    }

    #[test]
    fn spawn_schema_is_local_and_valid() {
        let tool = SpawnSubagent;
        assert!(jsonschema::meta::is_valid(&tool.schema()));
        assert!(!tool.schema().to_string().contains("$ref"));
    }

    #[test]
    fn report_result_validates_against_the_supplied_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "summary": { "type": "string" } },
            "required": ["summary"],
            "additionalProperties": false
        });
        let slot: ResultSlot = Arc::default();
        let registry =
            ToolRegistry::new(vec![Arc::new(ReportResult::new(schema, Arc::clone(&slot)))]);
        let tool = registry.get(ReportResult::NAME).expect("registered");

        assert!(
            registry
                .validate(tool.as_ref(), &serde_json::json!({ "summary": "done" }))
                .is_ok()
        );
        assert!(
            registry
                .validate(tool.as_ref(), &serde_json::json!({ "other": 1 }))
                .is_err(),
            "a result outside the parent-supplied schema must be refused"
        );
    }

    #[test]
    fn unfundable_and_oversized_spawns_fail_schema_validation() {
        let registry = ToolRegistry::new(vec![Arc::new(SpawnSubagent)]);
        let tool = registry.get(SpawnSubagent::NAME).expect("registered");

        let empty = serde_json::json!({ "children": [] });
        assert!(registry.validate(tool.as_ref(), &empty).is_err());

        // The schema admits the widest *legal* call, which an armed envelope can
        // reach. Whether this particular call is legal depends on session state
        // the schema cannot see, so the narrow default is enforced by the
        // runtime — see `runtime::envelope::envelope_draw`.
        let child = serde_json::json!({ "directive": "x" });
        let enveloped = serde_json::json!({
            "children": vec![child.clone(); MAX_CHILDREN + 1]
        });
        assert!(
            registry.validate(tool.as_ref(), &enveloped).is_ok(),
            "a draw an envelope could authorise must survive schema validation"
        );

        let beyond_any_envelope = serde_json::json!({
            "children": vec![child; MAX_CHILDREN_ENVELOPED + 1]
        });
        assert!(
            registry
                .validate(tool.as_ref(), &beyond_any_envelope)
                .is_err(),
            "no envelope can authorise a draw this wide, so the schema refuses it"
        );
    }
}
