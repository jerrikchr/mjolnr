use std::path::{Path, PathBuf};

pub fn resolve_executable(
    executable: &str,
    workspace_root: &Path,
) -> Result<PathBuf, crate::core::error::SmedError> {
    use crate::core::error::ReasonCode;
    let exe = executable.trim();
    if exe.is_empty() {
        return Err(crate::core::error::SmedError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            "external-agent profile has no executable",
        ));
    }
    if exe.contains("..") {
        return Err(crate::core::error::SmedError::workspace_refused(
            ReasonCode::WorkspaceCapabilityUnavailable,
            format!("external-agent executable may not contain `..`: {exe}"),
        ));
    }
    if exe.contains('/') {
        let candidate = if exe.starts_with('/') {
            PathBuf::from(exe)
        } else {
            workspace_root.join(exe)
        };
        let resolved = candidate.canonicalize().map_err(|_| {
            crate::core::error::SmedError::workspace_refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                format!("external-agent executable not found: {exe}"),
            )
        })?;
        return Ok(resolved);
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            if let Ok(resolved) = candidate.canonicalize() {
                return Ok(resolved);
            }
            return Ok(candidate);
        }
    }
    Err(crate::core::error::SmedError::workspace_refused(
        ReasonCode::WorkspaceCapabilityUnavailable,
        format!("external-agent executable not found on PATH: {exe}"),
    ))
}
