use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::message::{ToolEffect, ToolResult};
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::policy::paths;
use crate::tools::files;
use crate::tools::output;

#[derive(Debug)]
pub(super) struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a bounded UTF-8 file range and record its version before any edit"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1 },
                "start_line": { "type": ["integer", "null"], "minimum": 1 },
                "line_count": { "type": ["integer", "null"], "minimum": 1, "maximum": 1000 }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok(format!(
            "read {}",
            files::requested_path(arguments)?.display()
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let requested = files::requested_path(&arguments)?;
        let start = super::arguments::optional_u64(&arguments, "start_line", 1);
        let count = super::arguments::optional_u64(&arguments, "line_count", 200);
        let root = context.workspace_root.clone();
        let read_set = context.read_set.clone();
        let max_output = context.max_output_bytes;

        files::blocking(move || {
            if cancel.is_cancelled() {
                return Err(ToolError::Cancelled);
            }
            let path = match paths::existing(&root, &requested) {
                Ok(path) => path,
                Err(refusal) => return Ok(files::refusal(refusal)),
            };
            let (text, sha256) = match files::read_text(&path) {
                Ok(read) => read,
                Err(result) => return Ok(result),
            };

            let first = usize::try_from(start.saturating_sub(1)).unwrap_or(usize::MAX);
            let take = usize::try_from(count).unwrap_or(usize::MAX);
            let selected = text
                .lines()
                .enumerate()
                .skip(first)
                .take(take)
                .map(|(index, line)| format!("{:>6}  {line}", index + 1))
                .collect::<Vec<_>>()
                .join("\n");
            let (excerpt, truncated) = output::truncate(selected, max_output);
            read_set.observe(path.clone(), sha256.clone())?;

            Ok(ToolResult {
                outcome: crate::core::message::ToolOutcome::Ok,
                content: excerpt,
                truncated,
                effect: ToolEffect::Read {
                    path: files::display_path(&root, &path),
                    sha256,
                },
                evidence_event_id: None,
            })
        })
        .await
    }
}
