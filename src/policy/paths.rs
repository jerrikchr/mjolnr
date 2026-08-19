//! Canonical workspace containment.

use std::path::{Component, Path, PathBuf};

use crate::core::error::ReasonCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRefusal {
    pub code: ReasonCode,
    pub detail: String,
}

pub fn canonical_root(root: &Path) -> Result<PathBuf, PathRefusal> {
    let canonical = std::fs::canonicalize(root).map_err(|error| PathRefusal {
        code: ReasonCode::PathOutsideWorkspace,
        detail: format!("cannot open workspace {}: {error}", root.display()),
    })?;
    if !canonical.is_dir() {
        return Err(outside(root, "workspace root is not a directory"));
    }
    Ok(canonical)
}

pub fn existing(root: &Path, requested: &Path) -> Result<PathBuf, PathRefusal> {
    let candidate = lexical_candidate(root, requested)?;
    let canonical = std::fs::canonicalize(&candidate).map_err(|error| PathRefusal {
        code: ReasonCode::PathOutsideWorkspace,
        detail: format!("cannot resolve {}: {error}", requested.display()),
    })?;
    contained(root, requested, &candidate, canonical)
}

/// Resolve a path that may not exist by canonicalizing its nearest existing
/// ancestor, then appending only lexically-safe missing components.
pub fn for_write(root: &Path, requested: &Path) -> Result<PathBuf, PathRefusal> {
    let candidate = lexical_candidate(root, requested)?;
    if candidate.exists() {
        return existing(root, requested);
    }

    let mut ancestor = candidate.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            return Err(outside(requested, "path has no existing ancestor"));
        };
        missing.push(name.to_owned());
        let Some(parent) = ancestor.parent() else {
            return Err(outside(requested, "path has no existing parent"));
        };
        ancestor = parent;
    }

    let canonical_ancestor = std::fs::canonicalize(ancestor).map_err(|error| PathRefusal {
        code: ReasonCode::PathOutsideWorkspace,
        detail: format!("cannot resolve parent of {}: {error}", requested.display()),
    })?;
    if !canonical_ancestor.starts_with(root) {
        return Err(PathRefusal {
            code: ReasonCode::PathSymlinkEscape,
            detail: format!(
                "{} resolves through a symlink outside the workspace",
                requested.display()
            ),
        });
    }

    let mut resolved = canonical_ancestor;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn lexical_candidate(root: &Path, requested: &Path) -> Result<PathBuf, PathRefusal> {
    if requested
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(outside(
            requested,
            "parent-directory components are forbidden",
        ));
    }

    let candidate = if requested.is_absolute() {
        requested.to_owned()
    } else {
        root.join(requested)
    };

    if !candidate.starts_with(root) {
        return Err(outside(requested, "path is outside the workspace"));
    }
    Ok(candidate)
}

fn contained(
    root: &Path,
    requested: &Path,
    lexical: &Path,
    canonical: PathBuf,
) -> Result<PathBuf, PathRefusal> {
    if canonical.starts_with(root) {
        return Ok(canonical);
    }

    let code = if lexical.starts_with(root) {
        ReasonCode::PathSymlinkEscape
    } else {
        ReasonCode::PathOutsideWorkspace
    };
    Err(PathRefusal {
        code,
        detail: format!("{} resolves outside the workspace", requested.display()),
    })
}

fn outside(requested: &Path, reason: &str) -> PathRefusal {
    PathRefusal {
        code: ReasonCode::PathOutsideWorkspace,
        detail: format!("{}: {reason}", requested.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_components_are_rejected_before_filesystem_resolution() {
        let root = std::env::temp_dir();
        let result = for_write(&root, Path::new("../escape"));
        assert_eq!(
            result.expect_err("must refuse").code,
            ReasonCode::PathOutsideWorkspace
        );
    }

    #[test]
    fn workspace_root_must_be_a_directory() {
        let path = std::env::current_exe().expect("test executable path");
        let result = canonical_root(&path);
        assert_eq!(
            result.expect_err("file cannot be a workspace root").code,
            ReasonCode::PathOutsideWorkspace
        );
    }
}
