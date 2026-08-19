//! The model-facing tool that proposes loading a discovered extension
//! .
//!
//! This is the other half of the load act: `/load-extension` is the human's
//! direct command, and this tool is the *agent loop* proposing to extend itself.
//! It follows the same shape as [`activate_skill`](super::activate) — a
//! catalog-backed tool whose `requires_workspace_trust` defers to the catalog —
//! so a model-proposed load of a project extension passes the same trust gate a
//! project skill does, which is the guard that matters when no human typed the
//! command. Under full-auto that gate auto-resolves; under `ask` it holds for a
//! human, and under an untrusted project it raises the trust prompt even in
//! full-auto (`tool_loop` forces `Ask` when trust is required and absent).
//!
//! The tool only *proposes*: its `execute` validates that the name is real and
//! returns success. The runtime performs the registration and records the
//! `ExtensionLoaded` event when the call completes, because only the actor holds
//! the live tool registry — the same division `spawn_subagent` uses.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::context::extensions::ExtensionCatalog;
use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};

pub(crate) const TOOL_NAME: &str = "load_extension";

#[derive(Debug)]
pub(super) struct LoadExtension {
    catalog: Arc<ExtensionCatalog>,
    project_root: PathBuf,
}

impl LoadExtension {
    pub(super) fn new(catalog: Arc<ExtensionCatalog>, project_root: PathBuf) -> Self {
        Self {
            catalog,
            project_root,
        }
    }

    fn name_argument(arguments: &serde_json::Value) -> Result<&str, ToolError> {
        arguments
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ToolError::SchemaInvalid {
                detail: "`name` must identify a discovered extension".to_owned(),
            })
    }

    fn require_matching_workspace(&self, context: &ToolContext) -> Result<(), ToolError> {
        if context.workspace_root == self.project_root {
            return Ok(());
        }
        Err(ToolError::Refused {
            code: ReasonCode::PathOutsideWorkspace,
            detail: "extension catalog belongs to a different canonical workspace".to_owned(),
        })
    }
}

#[async_trait]
impl Tool for LoadExtension {
    fn name(&self) -> &'static str {
        TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Load a discovered extension so its tool becomes callable this session; every call it makes is still gated at Execute tier"
    }

    /// Loading adds a callable capability, so the proposal itself is `Execute`:
    /// under full-auto it auto-resolves, and under `ask` a human sees it. An
    /// extension is not knowledge like a skill, so this is not `Read`.
    fn tier(&self) -> ToolTier {
        ToolTier::Execute
    }

    fn requires_workspace_trust(&self, arguments: &serde_json::Value) -> bool {
        Self::name_argument(arguments).is_ok_and(|name| self.catalog.requires_project_trust(name))
    }

    fn schema(&self) -> serde_json::Value {
        let names = self
            .catalog
            .summaries()
            .iter()
            .map(|extension| serde_json::Value::String(extension.name.clone()))
            .collect::<Vec<_>>();
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "name": { "type": "string", "enum": names }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<String, ToolError> {
        self.require_matching_workspace(context)?;
        let name = Self::name_argument(arguments)?;
        let Some(definition) = self.catalog.get(name) else {
            return Err(ToolError::Refused {
                code: ReasonCode::SchemaInvalid,
                detail: format!("no discovered extension named `{name}`"),
            });
        };
        let program = definition.program();
        let trust = if self.catalog.requires_project_trust(name) {
            " Trust this workspace before loading its project extensions."
        } else {
            ""
        };
        Ok(format!(
            "load extension `{name}` (runs `{program}`).{trust} Every call it makes is gated at Execute tier."
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        self.require_matching_workspace(&context)?;
        let name = Self::name_argument(&arguments)?;
        // Validate the name is real; the runtime does the registration when this
        // call completes, because only the actor holds the tool registry.
        if self.catalog.get(name).is_none() {
            return Err(ToolError::Refused {
                code: ReasonCode::SchemaInvalid,
                detail: format!("no discovered extension named `{name}`"),
            });
        }
        Ok(ToolResult::ok(format!(
            "extension `{name}` loaded; it is now callable this session"
        )))
    }
}
