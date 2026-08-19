//! Deterministic discovery of the agent Soul and user profile.
//!
//! The Soul is smed's own identity — how the orchestrator behaves and sounds —
//! and the user profile is who it works for. Both are **inert prose**: they
//! shape voice and preference and confer no capability. Every side effect they
//! might inspire still crosses smed's normal deterministic tool and policy
//! gates, exactly as project instructions do. This module owns no gate and
//! grants no tool; it only produces text for the stable system-prompt prefix.
//!
//! Two layers compose, global first so a project can build on the user's
//! standing Soul rather than replace it: the user config directory
//! (`SOUL.md`/`USER.md`) and the project's `.mjolnr/` (`SOUL.md`/`USER.md`).

use std::path::{Path, PathBuf};

use crate::core::context::{ContextDiagnostic, SkillScope};
use crate::core::error::ReasonCode;

/// A starting `SOUL.md` for `mjolnr init` to offer.
///
/// Shipped as a **file the wizard previews and writes**, never as a built-in
/// default applied when the file is absent. That is `AGENTS.md` §11 law 7: a
/// default soul that is not on disk is a hidden config blob the user cannot
/// read, diff, or delete, and the diff-and-revert guarantee is the entire
/// safety case for letting smed evolve its own identity at all. An absent
/// `SOUL.md` therefore keeps meaning exactly what it means today — no Soul.
///
/// The text is voice and preference only. It confers no capability, and every
/// side effect it might inspire still crosses the ordinary gates.
#[must_use]
pub fn default_soul() -> (std::path::PathBuf, String) {
    (
        std::path::PathBuf::from(".mjolnr").join("SOUL.md"),
        DEFAULT_SOUL.to_owned(),
    )
}

/// Deliberately short. This is a starting point the owner edits, not a
/// personality specification — and it rides the stable prompt prefix of every
/// session, where the standing evidence favours less scaffolding.
const DEFAULT_SOUL: &str = "\
# Soul

How smed works, in this project. Edit freely — this file is yours, it is only
text, and it grants nothing. Delete it and smed runs without a Soul.

## Voice

Direct and specific. Say what was done and what was not. Lead with the answer,
then the reasoning if it is needed.

## Working

- State assumptions rather than asking when the answer would not change the
  work; ask when it would.
- Report what was checked and what was not. \"Tests pass\" means they were run.
- Prefer the smallest change that fully does the job, and finish it.
- When something looks wrong with the request itself, say so once, then get on
  with it.

## Preferences

<!-- Project-specific things worth remembering: conventions, review taste,
     what to leave alone. -->
";

/// Which identity file a document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SoulKind {
    /// `SOUL.md` — the orchestrator's standing identity and voice.
    Soul,
    /// `USER.md` — the profile of who smed works for.
    UserProfile,
}

impl SoulKind {
    pub(super) const fn filename(self) -> &'static str {
        match self {
            Self::Soul => "SOUL.md",
            Self::UserProfile => "USER.md",
        }
    }

    /// The XML tag this document is wrapped in inside `<agent_soul>`.
    pub(super) const fn tag(self) -> &'static str {
        match self {
            Self::Soul => "soul",
            Self::UserProfile => "user_profile",
        }
    }
}

/// A discovered identity file, ready to inject.
#[derive(Debug, Clone)]
pub(super) struct SoulDocument {
    pub path: PathBuf,
    pub kind: SoulKind,
    pub scope: SkillScope,
    pub content: String,
}

/// Discover the Soul and user-profile files, global then project.
///
/// Deterministic and bounded, in the manner of [`super::instructions::discover`]:
/// a file that escapes its base directory through a symlink is diagnosed and
/// skipped, and a shared byte budget stops reading before it is exceeded rather
/// than silently truncating a document mid-way. An empty file is not a
/// document — it is the same as an absent one, so a placeholder `SOUL.md` does
/// not inject a blank section.
pub(super) fn discover(
    project_root: &Path,
    user_config: &Path,
    max_bytes: usize,
) -> (Vec<SoulDocument>, Vec<ContextDiagnostic>) {
    let mut documents = Vec::new();
    let mut diagnostics = Vec::new();
    let mut used = 0usize;

    // Global first, project last: the project layer builds on the user's Soul.
    let config_dir = crate::core::paths::resolve_workspace_config_dir(project_root);
    let locations = [
        (user_config.to_path_buf(), SkillScope::User),
        (config_dir, SkillScope::Project),
    ];
    for (base, scope) in locations {
        for kind in [SoulKind::Soul, SoulKind::UserProfile] {
            let path = base.join(kind.filename());
            if !path.exists() {
                continue;
            }
            let resolved = match path.canonicalize() {
                Ok(resolved) => resolved,
                Err(error) => {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("could not resolve {}: {error}", path.display()),
                    });
                    continue;
                }
            };
            // The file must stay within the base it was found under; a symlink
            // pointing elsewhere is the same escape the instruction loader
            // refuses, and identity prose is no safer to follow off-tree.
            let base_resolved = base.canonicalize().unwrap_or_else(|_| base.clone());
            if !resolved.starts_with(&base_resolved) {
                diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::PathSymlinkEscape,
                    detail: format!(
                        "ignored identity file outside {}: {}",
                        base_resolved.display(),
                        path.display()
                    ),
                });
                continue;
            }
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
                    detail: format!("identity budget reached before {}", path.display()),
                });
                continue;
            }
            match std::fs::read_to_string(&resolved) {
                Ok(content) => {
                    let trimmed = content.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    used = used.saturating_add(content.len());
                    documents.push(SoulDocument {
                        path: resolved,
                        kind,
                        scope,
                        content: trimmed.to_owned(),
                    });
                }
                Err(error) => diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::SchemaInvalid,
                    detail: format!(
                        "identity file {} is not readable UTF-8: {error}",
                        path.display()
                    ),
                }),
            }
        }
    }
    (documents, diagnostics)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    const BUDGET: usize = 512 * 1024;

    #[test]
    fn discovers_project_and_global_identity_files() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".mjolnr")).unwrap();
        std::fs::write(
            config.path().join("SOUL.md"),
            "I am smed, terse and exact.\n",
        )
        .unwrap();
        std::fs::write(
            project.path().join(".mjolnr").join("USER.md"),
            "Jerrik prefers decisive recommendations.\n",
        )
        .unwrap();

        let (documents, diagnostics) = discover(project.path(), config.path(), BUDGET);
        assert!(diagnostics.is_empty(), "clean discovery has no diagnostics");
        // Global first, project last.
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].scope, SkillScope::User);
        assert_eq!(documents[0].kind, SoulKind::Soul);
        assert_eq!(documents[1].scope, SkillScope::Project);
        assert_eq!(documents[1].kind, SoulKind::UserProfile);
        assert!(documents[0].content.contains("terse and exact"));
    }

    #[test]
    fn an_empty_identity_file_is_not_a_document() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".mjolnr")).unwrap();
        std::fs::write(project.path().join(".mjolnr").join("SOUL.md"), "   \n\n").unwrap();

        let (documents, diagnostics) = discover(project.path(), config.path(), BUDGET);
        assert!(documents.is_empty(), "a blank Soul injects nothing");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn the_budget_stops_before_the_next_file() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".mjolnr")).unwrap();
        std::fs::write(config.path().join("SOUL.md"), "x".repeat(4_096)).unwrap();
        std::fs::write(project.path().join(".mjolnr").join("SOUL.md"), "kept out").unwrap();

        // A budget smaller than the first file skips it with a diagnostic
        // rather than truncating it; a later file that still fits is admitted.
        let (documents, diagnostics) = discover(project.path(), config.path(), 10);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == ReasonCode::OutputTruncated),
            "an over-budget file is diagnosed, not silently dropped"
        );
        assert!(
            documents
                .iter()
                .all(|document| document.scope != SkillScope::User),
            "the over-budget global Soul is skipped, not truncated in"
        );
        assert!(
            documents
                .iter()
                .any(|document| document.content == "kept out"),
            "a small later file still fits under the budget"
        );
    }
}
