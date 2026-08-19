use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::{ToolOutcome, ToolResult};
use crate::core::tool::ReadSet;
use crate::policy::paths::PathRefusal;

pub(super) const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn read_text(path: &Path) -> Result<(String, String), ToolResult> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        ToolResult::failed(
            ReasonCode::ToolExecution,
            format!("cannot inspect {}: {error}", path.display()),
        )
    })?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ToolResult::failed(
            ReasonCode::OutputTruncated,
            format!("{} exceeds the 8 MiB file limit", path.display()),
        ));
    }

    let bytes = std::fs::read(path).map_err(|error| {
        ToolResult::failed(
            ReasonCode::ToolExecution,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    if bytes.contains(&0) {
        return Err(ToolResult::failed(
            ReasonCode::ToolExecution,
            format!("{} is binary; text tools refuse it", path.display()),
        ));
    }
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        ToolResult::failed(
            ReasonCode::ToolExecution,
            format!("{} is not valid UTF-8", path.display()),
        )
    })?;
    Ok((text, hash(&bytes)))
}

#[must_use]
pub(super) fn hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ToolError> + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ToolError::Execution {
            detail: format!("filesystem task did not complete: {error}"),
        })?
}

pub(super) fn refusal(refusal: PathRefusal) -> ToolResult {
    ToolResult::refused(refusal.code, refusal.detail)
}

pub(super) fn preview_path_error(refusal: PathRefusal) -> ToolError {
    ToolError::Refused {
        code: refusal.code,
        detail: refusal.detail,
    }
}

pub(super) fn preview_result_error(result: ToolResult) -> ToolError {
    match result.outcome {
        ToolOutcome::Refused(code) => ToolError::Refused {
            code,
            detail: result.content,
        },
        ToolOutcome::Failed(code) => ToolError::Failed {
            code,
            detail: result.content,
        },
        ToolOutcome::Ok => ToolError::Execution {
            detail: "preflight unexpectedly received a successful result".to_owned(),
        },
    }
}

pub(super) fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(super) fn requested_path(arguments: &serde_json::Value) -> Result<PathBuf, ToolError> {
    Ok(PathBuf::from(super::arguments::required_string(
        arguments, "path",
    )?))
}

pub(super) fn observed_text(
    path: &Path,
    read_set: &ReadSet,
) -> Result<(String, String), ToolResult> {
    let (text, current) = read_text(path)?;
    let observed = read_set
        .version(path)
        .map_err(|error| ToolResult::failed(ReasonCode::ToolExecution, error.to_string()))?;
    let Some(observed) = observed else {
        return Err(ToolResult::refused(
            ReasonCode::FileNotObserved,
            format!("{} must be read before it can be changed", path.display()),
        ));
    };
    if observed != current {
        return Err(ToolResult::refused(
            ReasonCode::StaleFileVersion,
            format!(
                "{} changed after it was read; read it again",
                path.display()
            ),
        ));
    }
    Ok((text, current))
}

#[must_use]
pub(super) fn review_diff(path: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}
