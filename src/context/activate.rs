//! The progressive-disclosure activation tool.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::context::skills::SkillCatalog;
use crate::core::error::ToolError;
use crate::core::message::{ToolEffect, ToolResult};
use crate::core::tool::{Tool, ToolContext, ToolTier};

pub(crate) const TOOL_NAME: &str = "activate_skill";

#[derive(Debug)]
pub(super) struct ActivateSkill {
    catalog: Arc<SkillCatalog>,
    project_root: PathBuf,
}

impl ActivateSkill {
    pub(super) fn new(catalog: Arc<SkillCatalog>, project_root: PathBuf) -> Self {
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
                detail: "`name` must identify a discovered skill".to_owned(),
            })
    }

    fn require_matching_workspace(&self, context: &ToolContext) -> Result<(), ToolError> {
        if context.workspace_root == self.project_root {
            return Ok(());
        }
        Err(ToolError::Refused {
            code: crate::core::error::ReasonCode::PathOutsideWorkspace,
            detail: "skill catalog belongs to a different canonical workspace".to_owned(),
        })
    }
}

#[async_trait]
impl Tool for ActivateSkill {
    fn name(&self) -> &'static str {
        TOOL_NAME
    }

    fn description(&self) -> &'static str {
        "Activate one discovered Agent Skill and load its instructions; resources remain on demand"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn requires_workspace_trust(&self, arguments: &serde_json::Value) -> bool {
        Self::name_argument(arguments).is_ok_and(|name| self.catalog.requires_project_trust(name))
    }

    fn schema(&self) -> serde_json::Value {
        let names = self
            .catalog
            .summaries()
            .iter()
            .map(|skill| serde_json::Value::String(skill.name.clone()))
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
        let trust = if self.catalog.requires_project_trust(name) {
            " Trust this workspace's project skills before loading its instructions."
        } else {
            ""
        };
        Ok(format!(
            "activate skill `{name}`.{trust} Bundled scripts receive no execution authority."
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let name = Self::name_argument(&arguments)?.to_owned();
        self.require_matching_workspace(&context)?;
        let catalog = Arc::clone(&self.catalog);
        let task = tokio::task::spawn_blocking(move || catalog.activate(&name)).await;
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let activated = task
            .map_err(|error| ToolError::Execution {
                detail: format!("skill activation task did not complete: {error}"),
            })?
            .map_err(|(code, detail)| ToolError::Refused { code, detail })?;
        Ok(
            ToolResult::ok(activated.content).with_effect(ToolEffect::SkillActivated {
                name: activated.name,
                project: activated.project,
            }),
        )
    }
}
