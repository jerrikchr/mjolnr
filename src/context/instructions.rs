//! Deterministic project-instruction discovery.

use std::path::{Path, PathBuf};

use crate::core::context::ContextDiagnostic;
use crate::core::error::ReasonCode;

#[derive(Debug, Clone)]
pub(super) struct InstructionDocument {
    pub path: PathBuf,
    pub canonical: bool,
    pub content: String,
}

pub(super) fn discover(
    root: &Path,
    working: &Path,
    max_bytes: usize,
) -> Result<(Vec<InstructionDocument>, Vec<ContextDiagnostic>), String> {
    let root = root.canonicalize().map_err(|error| {
        format!(
            "cannot canonicalize project root {}: {error}",
            root.display()
        )
    })?;
    let working = working.canonicalize().map_err(|error| {
        format!(
            "cannot canonicalize working directory {}: {error}",
            working.display()
        )
    })?;
    let relative = working.strip_prefix(&root).map_err(|_| {
        format!(
            "working directory {} is outside project root {}",
            working.display(),
            root.display()
        )
    })?;

    let mut directories = vec![root.clone()];
    let mut current = root.clone();
    for component in relative.components() {
        current.push(component);
        directories.push(current.clone());
    }

    let mut documents = Vec::new();
    let mut diagnostics = Vec::new();
    let mut used = 0usize;
    for directory in directories {
        for (name, canonical) in [("AGENTS.md", true), ("CLAUDE.md", false)] {
            let path = directory.join(name);
            if !path.exists() {
                continue;
            }
            let resolved = match path.canonicalize() {
                Ok(resolved) if resolved.starts_with(&root) => resolved,
                Ok(_) => {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::PathSymlinkEscape,
                        detail: format!(
                            "ignored instruction file outside project root: {}",
                            path.display()
                        ),
                    });
                    continue;
                }
                Err(error) => {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("could not resolve {}: {error}", path.display()),
                    });
                    continue;
                }
            };
            let metadata = match std::fs::metadata(&resolved) {
                Ok(metadata) => metadata,
                Err(error) => {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("could not inspect {}: {error}", path.display()),
                    });
                    continue;
                }
            };
            let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            if used.saturating_add(length) > max_bytes {
                diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::OutputTruncated,
                    detail: format!("instruction budget reached before {}", path.display()),
                });
                continue;
            }
            match std::fs::read_to_string(&resolved) {
                Ok(content) => {
                    used = used.saturating_add(content.len());
                    documents.push(InstructionDocument {
                        path: resolved,
                        canonical,
                        content,
                    });
                }
                Err(error) => diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::SchemaInvalid,
                    detail: format!(
                        "instruction file {} is not readable UTF-8: {error}",
                        path.display()
                    ),
                }),
            }
        }
    }
    Ok((documents, diagnostics))
}
