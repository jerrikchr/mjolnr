use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::{ToolEffect, ToolOutcome, ToolResult};
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::policy::paths;
use crate::tools::files;
use crate::tools::output;

#[derive(Debug)]
pub(super) struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create a UTF-8 file or replace a file that was read and has not changed"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Write
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1 },
                "content": { "type": "string", "maxLength": 8_388_608 }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<String, ToolError> {
        let requested = files::requested_path(arguments)?;
        let new_text = super::arguments::required_string(arguments, "content")?;
        let root = context.workspace_root.clone();
        let read_set = context.read_set.clone();
        let max_output = context.max_output_bytes;
        files::blocking(move || {
            let path = paths::for_write(&root, &requested).map_err(files::preview_path_error)?;
            let old = if path.exists() {
                match files::observed_text(&path, &read_set) {
                    Ok((text, _)) => text,
                    Err(result) => return Err(files::preview_result_error(result)),
                }
            } else {
                String::new()
            };
            let relative = files::display_path(&root, &path);
            let (diff, _) =
                output::truncate(files::review_diff(&relative, &old, &new_text), max_output);
            Ok(diff)
        })
        .await
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let requested = files::requested_path(&arguments)?;
        let new_text = super::arguments::required_string(&arguments, "content")?;
        let root = context.workspace_root.clone();
        let read_set = context.read_set.clone();

        files::blocking(move || {
            let mut path = match paths::for_write(&root, &requested) {
                Ok(path) => path,
                Err(refusal) => return Ok(files::refusal(refusal)),
            };
            if path.exists()
                && let Err(result) = files::observed_text(&path, &read_set)
            {
                return Ok(result);
            }
            if cancel.is_cancelled() {
                return Err(ToolError::Cancelled);
            }

            let Some(parent) = path.parent() else {
                return Ok(ToolResult::failed(
                    ReasonCode::PathOutsideWorkspace,
                    "write target has no parent",
                ));
            };
            std::fs::create_dir_all(parent).map_err(|error| ToolError::Execution {
                detail: format!("cannot create {}: {error}", parent.display()),
            })?;

            // Parent creation changes the filesystem shape. Resolve again
            // immediately before the effect instead of trusting the preview.
            path = match paths::for_write(&root, &requested) {
                Ok(path) => path,
                Err(refusal) => return Ok(files::refusal(refusal)),
            };
            if cancel.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            std::fs::write(&path, new_text.as_bytes()).map_err(|error| ToolError::Execution {
                detail: format!("cannot write {}: {error}", path.display()),
            })?;

            let sha256 = files::hash(new_text.as_bytes());
            read_set.observe(path.clone(), sha256.clone())?;
            Ok(ToolResult {
                outcome: ToolOutcome::Ok,
                content: format!(
                    "wrote {} (sha256 {sha256})",
                    files::display_path(&root, &path)
                ),
                truncated: false,
                effect: ToolEffect::Mutation {
                    path: files::display_path(&root, &path),
                    sha256,
                },
                evidence_event_id: None,
            })
        })
        .await
    }
}
