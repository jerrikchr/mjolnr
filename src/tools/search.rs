use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::message::{ToolOutcome, ToolResult};
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::policy::paths;
use crate::tools::command::{find_program, run_process};
use crate::tools::files;
use crate::tools::output;

#[derive(Debug)]
pub(super) struct SearchText;

#[async_trait]
impl Tool for SearchText {
    fn name(&self) -> &'static str {
        "search_text"
    }

    fn description(&self) -> &'static str {
        "Search workspace text with ripgrep when available and a bounded Rust fallback"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1, "maxLength": 4096 },
                "path": { "type": ["string", "null"], "minLength": 1 },
                "max_results": { "type": ["integer", "null"], "minimum": 1, "maximum": 500 }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok(format!(
            "search for {:?}",
            super::arguments::required_string(arguments, "query")?
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let query = super::arguments::required_string(&arguments, "query")?;
        let requested = PathBuf::from(
            arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("."),
        );
        let max_results = usize::try_from(super::arguments::optional_u64(
            &arguments,
            "max_results",
            100,
        ))
        .unwrap_or(500);

        let start = {
            let root = context.workspace_root.clone();
            let requested = requested.clone();
            files::blocking(move || Ok(paths::existing(&root, &requested))).await?
        };
        let start = match start {
            Ok(path) => path,
            Err(refusal) => return Ok(files::refusal(refusal)),
        };

        if let Some(rg) = find_program("rg") {
            let relative = start
                .strip_prefix(&context.workspace_root)
                .unwrap_or(&start)
                .to_string_lossy()
                .into_owned();
            let arguments = vec![
                "--json".to_owned(),
                "--fixed-strings".to_owned(),
                "--line-number".to_owned(),
                "--color".to_owned(),
                "never".to_owned(),
                "--glob".to_owned(),
                "!.git/**".to_owned(),
                "--glob".to_owned(),
                "!target/**".to_owned(),
                query.clone(),
                relative,
            ];
            let rg_output = run_process(
                &rg,
                &arguments,
                &context.workspace_root,
                context.command_timeout,
                context.max_output_bytes.saturating_mul(4),
                cancel.clone(),
            )
            .await?;
            if rg_output.cancelled {
                return Err(ToolError::Cancelled);
            }
            if !rg_output.timed_out && matches!(rg_output.exit_code, Some(0 | 1)) {
                return Ok(render_rg(
                    &rg_output.stdout,
                    max_results,
                    context.max_output_bytes,
                    rg_output.truncated,
                ));
            }
        }

        let root = context.workspace_root.clone();
        let max_output = context.max_output_bytes;
        files::blocking(move || {
            let mut matches = Vec::new();
            fallback_walk(&root, &start, &query, max_results, &cancel, &mut matches)?;
            matches.sort();
            let result_limit = matches.len() >= max_results;
            let (rendered, bytes) = output::truncate(matches.join("\n"), max_output);
            Ok(ToolResult {
                outcome: ToolOutcome::Ok,
                content: rendered,
                truncated: result_limit || bytes,
                effect: crate::core::message::ToolEffect::None,
                evidence_event_id: None,
            })
        })
        .await
    }
}

fn render_rg(
    raw: &str,
    max_results: usize,
    max_bytes: usize,
    upstream_truncated: bool,
) -> ToolResult {
    let mut matches = Vec::new();
    for line in raw.lines() {
        if matches.len() >= max_results {
            break;
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if event.get("type").and_then(serde_json::Value::as_str) != Some("match") {
            continue;
        }
        let data = event.get("data");
        let path = data
            .and_then(|value| value.get("path"))
            .and_then(|value| value.get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        let line_number = data
            .and_then(|value| value.get("line_number"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let snippet = data
            .and_then(|value| value.get("lines"))
            .and_then(|value| value.get("text"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim_end();
        matches.push(format!("{path}:{line_number}:{snippet}"));
    }
    let result_limit = matches.len() >= max_results;
    let (content, bytes) = output::truncate(matches.join("\n"), max_bytes);
    ToolResult {
        outcome: ToolOutcome::Ok,
        content,
        truncated: upstream_truncated || result_limit || bytes,
        effect: crate::core::message::ToolEffect::None,
        evidence_event_id: None,
    }
}

fn fallback_walk(
    root: &Path,
    path: &Path,
    query: &str,
    max_results: usize,
    cancel: &CancellationToken,
    matches: &mut Vec<String>,
) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    if matches.len() >= max_results {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| ToolError::Execution {
        detail: format!("cannot inspect {}: {error}", path.display()),
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        let mut children = std::fs::read_dir(path)
            .map_err(|error| ToolError::Execution {
                detail: format!("cannot search {}: {error}", path.display()),
            })?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            fallback_walk(root, &child, query, max_results, cancel, matches)?;
            if matches.len() >= max_results {
                break;
            }
        }
    } else if metadata.is_file()
        && metadata.len() <= files::MAX_FILE_BYTES
        && let Ok((text, _)) = files::read_text(path)
    {
        for (index, line) in text.lines().enumerate() {
            if line.contains(query) {
                matches.push(format!(
                    "{}:{}:{}",
                    files::display_path(root, path),
                    index + 1,
                    line
                ));
                if matches.len() >= max_results {
                    break;
                }
            }
        }
    }
    Ok(())
}
