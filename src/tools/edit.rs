use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::{ToolEffect, ToolOutcome, ToolResult};
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::policy::paths;
use crate::tools::files;
use crate::tools::output;

#[derive(Debug)]
pub(super) struct EditFile;

#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace one exact unique string in a file that was read and remains unchanged"
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
                "old": { "type": "string", "minLength": 1, "maxLength": 8_388_608 },
                "new": { "type": "string", "maxLength": 8_388_608 }
            },
            "required": ["path", "old", "new"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<String, ToolError> {
        let requested = files::requested_path(arguments)?;
        let old = super::arguments::required_string(arguments, "old")?;
        let new = super::arguments::required_string(arguments, "new")?;
        let root = context.workspace_root.clone();
        let read_set = context.read_set.clone();
        let max_output = context.max_output_bytes;
        files::blocking(move || {
            let path = paths::existing(&root, &requested).map_err(files::preview_path_error)?;
            let (current, _) = match files::observed_text(&path, &read_set) {
                Ok(value) => value,
                Err(result) => return Err(files::preview_result_error(result)),
            };
            let replacement =
                exact_replacement(&current, &old, &new).map_err(files::preview_result_error)?;
            let relative = files::display_path(&root, &path);
            let (diff, _) = output::truncate(
                files::review_diff(&relative, &current, &replacement),
                max_output,
            );
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
        let old = super::arguments::required_string(&arguments, "old")?;
        let new = super::arguments::required_string(&arguments, "new")?;
        let root = context.workspace_root.clone();
        let read_set = context.read_set.clone();
        files::blocking(move || {
            let path = match paths::existing(&root, &requested) {
                Ok(path) => path,
                Err(refusal) => return Ok(files::refusal(refusal)),
            };
            let (current, _) = match files::observed_text(&path, &read_set) {
                Ok(value) => value,
                Err(result) => return Ok(result),
            };
            let replacement = match exact_replacement(&current, &old, &new) {
                Ok(value) => value,
                Err(result) => return Ok(result),
            };
            if cancel.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            // Re-resolve at the side-effect boundary to catch a renamed parent
            // or a symlink introduced after preview.
            let immediate = match paths::existing(&root, &requested) {
                Ok(path) => path,
                Err(refusal) => return Ok(files::refusal(refusal)),
            };
            if immediate != path {
                return Ok(ToolResult::refused(
                    ReasonCode::StaleFileVersion,
                    "the edit target moved after it was reviewed",
                ));
            }
            std::fs::write(&path, replacement.as_bytes()).map_err(|error| {
                ToolError::Execution {
                    detail: format!("cannot edit {}: {error}", path.display()),
                }
            })?;
            let sha256 = files::hash(replacement.as_bytes());
            read_set.observe(path.clone(), sha256.clone())?;
            Ok(ToolResult {
                outcome: ToolOutcome::Ok,
                content: format!(
                    "edited {} (sha256 {sha256})",
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

fn exact_replacement(current: &str, old: &str, new: &str) -> Result<String, ToolResult> {
    let matches = current.match_indices(old).count();
    if matches != 1 {
        return Err(ToolResult::refused(
            ReasonCode::StaleFileVersion,
            format!("exact edit expected one match, found {matches}"),
        ));
    }
    Ok(current.replacen(old, new, 1))
}
