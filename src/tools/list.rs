use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::policy::paths;
use crate::tools::files;
use crate::tools::output;

// Byte-code and dependency caches are build artifacts of a workspace's
// tooling, not its content; listing them buried real files under noise and
// burned the model's bounded output on `__pycache__` entries.
const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".next",
    "dist",
    "build",
    "__pycache__",
    ".venv",
];

#[derive(Debug)]
pub(super) struct ListFiles;

#[async_trait]
impl Tool for ListFiles {
    fn name(&self) -> &'static str {
        "list_files"
    }

    fn description(&self) -> &'static str {
        "List workspace files in deterministic order without following symlink directories"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "path": { "type": ["string", "null"], "minLength": 1 },
                "recursive": { "type": ["boolean", "null"] },
                "max_results": { "type": ["integer", "null"], "minimum": 1, "maximum": 2000 }
            },
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let path = arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(".");
        Ok(format!("list {path}"))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let requested = PathBuf::from(
            arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("."),
        );
        let recursive = super::arguments::optional_bool(&arguments, "recursive", true);
        let max_results = usize::try_from(super::arguments::optional_u64(
            &arguments,
            "max_results",
            500,
        ))
        .unwrap_or(2000);
        let root = context.workspace_root.clone();
        let max_output = context.max_output_bytes;

        files::blocking(move || {
            let start = match paths::existing(&root, &requested) {
                Ok(path) => path,
                Err(refusal) => return Ok(files::refusal(refusal)),
            };
            if !start.is_dir() {
                return Ok(ToolResult::failed(
                    ReasonCode::ToolExecution,
                    format!("{} is not a directory", requested.display()),
                ));
            }

            let mut found = Vec::new();
            walk(&root, &start, recursive, max_results, &cancel, &mut found)?;
            found.sort();
            let was_truncated = found.len() >= max_results;
            let text = found.join("\n");
            let (listing, bytes_truncated) = output::truncate(text, max_output);
            Ok(ToolResult {
                outcome: crate::core::message::ToolOutcome::Ok,
                content: listing,
                truncated: was_truncated || bytes_truncated,
                effect: crate::core::message::ToolEffect::None,
                evidence_event_id: None,
            })
        })
        .await
    }
}

fn walk(
    root: &Path,
    directory: &Path,
    recursive: bool,
    max_results: usize,
    cancel: &CancellationToken,
    found: &mut Vec<String>,
) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError::Cancelled);
    }
    let entries = std::fs::read_dir(directory).map_err(|error| ToolError::Execution {
        detail: format!("cannot list {}: {error}", directory.display()),
    })?;

    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        if found.len() >= max_results {
            break;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| ToolError::Execution {
            detail: format!("cannot inspect {}: {error}", path.display()),
        })?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let ignored = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| IGNORED_DIRECTORIES.contains(&name));
            if recursive && !ignored {
                walk(root, &path, true, max_results, cancel, found)?;
            }
        } else if metadata.is_file() {
            found.push(files::display_path(root, &path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The noise case from the field: a Python workspace's `__pycache__` and
    /// virtualenv directories buried the real files under byte-code and ate
    /// the bounded output budget. Ignored names must never appear, at any
    /// depth.
    #[test]
    fn walk_skips_cache_and_virtualenv_directories() {
        let workspace = tempfile::tempdir().expect("temp dir");
        let root = workspace.path();
        std::fs::write(root.join("app.py"), b"x").expect("write app.py");
        std::fs::create_dir_all(root.join("__pycache__")).expect("create cache");
        std::fs::write(root.join("__pycache__").join("app.cpython-314.pyc"), b"x")
            .expect("write pyc");
        std::fs::create_dir_all(root.join(".venv").join("lib")).expect("create venv");
        std::fs::write(root.join(".venv").join("lib").join("site.py"), b"x").expect("write venv");
        std::fs::create_dir_all(root.join("src").join("__pycache__")).expect("nested cache");
        std::fs::write(root.join("src").join("main.py"), b"x").expect("write nested");

        let cancel = CancellationToken::new();
        let mut found = Vec::new();
        walk(root, root, true, 500, &cancel, &mut found).expect("walk succeeds");
        found.sort();

        assert_eq!(found, vec!["app.py", "src/main.py"]);
    }
}
