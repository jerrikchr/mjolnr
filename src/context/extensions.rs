//! Bounded discovery of agent-authored tool extensions.
//!
//! Extensions are discovered the way skills are — canonicalized roots,
//! boundary-checked entries, bounded reads, typed diagnostics on anything
//! malformed — but an extension is a single `.yaml` file rather than a
//! directory, and discovery only makes it **visible**. It does not make it
//! callable: that is the separate, evidenced load act (
//! "Explicit load step"). A malformed file is reported and skipped; it never
//! partially registers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::context::DiscoveryLimits;
use crate::core::context::{ContextDiagnostic, ExtensionSummary, SkillScope};
use crate::core::error::ReasonCode;
use crate::core::extension::ExtensionDefinition;

const EXTENSION_SUFFIXES: [&str; 2] = ["yaml", "yml"];

#[derive(Debug, Clone)]
struct ExtensionRecord {
    definition: ExtensionDefinition,
    summary: ExtensionSummary,
}

/// The extensions discovered under a set of roots, indexed by name.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExtensionCatalog {
    records: BTreeMap<String, ExtensionRecord>,
    ordered: Vec<ExtensionSummary>,
}

impl ExtensionCatalog {
    pub(crate) fn discover(
        roots: Vec<(PathBuf, SkillScope, Option<PathBuf>)>,
        limits: DiscoveryLimits,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) -> Self {
        let mut catalog = Self::default();
        let mut scanned = 0usize;
        let mut exhausted = false;
        for (root, scope, boundary) in roots {
            if exhausted || !root.exists() {
                continue;
            }
            let canonical_root = match root.canonicalize() {
                Ok(root) => root,
                Err(error) => {
                    invalid(
                        diagnostics,
                        format!(
                            "could not resolve extension root {}: {error}",
                            root.display()
                        ),
                    );
                    continue;
                }
            };
            if boundary
                .as_ref()
                .is_some_and(|boundary| !canonical_root.starts_with(boundary))
            {
                escaped(diagnostics, &root);
                continue;
            }
            let mut entries = match std::fs::read_dir(&canonical_root) {
                Ok(entries) => entries.flatten().collect::<Vec<_>>(),
                Err(error) => {
                    invalid(
                        diagnostics,
                        format!("could not scan extension root {}: {error}", root.display()),
                    );
                    continue;
                }
            };
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                if !has_extension_suffix(&path) {
                    continue;
                }
                if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                // Reuse the skill scan cap: an extension file is far smaller
                // than a skill directory, so the same bound is more than
                // generous and avoids a second knob for the same purpose.
                if scanned >= limits.max_skill_directories {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::OutputTruncated,
                        detail: format!("extension scan stopped at {scanned} files"),
                    });
                    exhausted = true;
                    break;
                }
                scanned += 1;
                catalog.inspect(&path, &canonical_root, scope, limits, diagnostics);
            }
        }
        catalog
    }

    fn inspect(
        &mut self,
        path: &Path,
        allowed_root: &Path,
        scope: SkillScope,
        limits: DiscoveryLimits,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) {
        let resolved = match path.canonicalize() {
            Ok(resolved) if resolved.starts_with(allowed_root) => resolved,
            Ok(_) => {
                escaped(diagnostics, path);
                return;
            }
            Err(error) => {
                invalid(
                    diagnostics,
                    format!("could not resolve extension {}: {error}", path.display()),
                );
                return;
            }
        };
        let Some(stem) = resolved.file_stem().and_then(std::ffi::OsStr::to_str) else {
            invalid(
                diagnostics,
                format!("extension file name is not valid UTF-8: {}", path.display()),
            );
            return;
        };
        let contents = match read_bounded(&resolved, limits.max_skill_file_bytes, allowed_root) {
            Ok(contents) => contents,
            Err((code, detail)) => {
                diagnostics.push(ContextDiagnostic { code, detail });
                return;
            }
        };
        let definition = match ExtensionDefinition::parse(&contents, stem) {
            Ok(definition) => definition,
            Err(detail) => {
                invalid(
                    diagnostics,
                    format!("invalid extension {}: {detail}", resolved.display()),
                );
                return;
            }
        };
        let summary = ExtensionSummary {
            name: definition.name().to_owned(),
            description: definition.description().to_owned(),
            location: resolved.to_string_lossy().into_owned(),
            scope,
        };
        if let Some(existing) = self.records.get(&summary.name) {
            invalid(
                diagnostics,
                format!(
                    "extension collision for `{}`: kept {}, ignored {}",
                    summary.name, existing.summary.location, summary.location
                ),
            );
            return;
        }
        self.ordered.push(summary.clone());
        self.records.insert(
            summary.name.clone(),
            ExtensionRecord {
                definition,
                summary,
            },
        );
    }

    pub(crate) fn summaries(&self) -> &[ExtensionSummary] {
        &self.ordered
    }

    /// Whether loading `name` needs the project-skill trust gate.
    ///
    /// A project-scoped extension comes from the workspace, so a model-proposed
    /// load of it must pass the same trust gate a project skill does — the guard
    /// that matters when no human typed the load command. A human's direct
    /// `/load-extension` is its own authorisation and does not consult this.
    pub(crate) fn requires_project_trust(&self, name: &str) -> bool {
        self.records
            .get(name)
            .is_some_and(|record| record.summary.scope == SkillScope::Project)
    }

    /// The parsed definition for `name`, for the load act to build a tool from.
    pub(crate) fn get(&self, name: &str) -> Option<&ExtensionDefinition> {
        self.records.get(name).map(|record| &record.definition)
    }
}

fn has_extension_suffix(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|suffix| EXTENSION_SUFFIXES.contains(&suffix))
}

fn read_bounded(path: &Path, maximum: usize, root: &Path) -> Result<String, (ReasonCode, String)> {
    let resolved = path.canonicalize().map_err(|error| {
        (
            ReasonCode::SchemaInvalid,
            format!("missing or unreadable {}: {error}", path.display()),
        )
    })?;
    if !resolved.starts_with(root) {
        return Err((
            ReasonCode::PathSymlinkEscape,
            format!("{} resolves outside its extension root", path.display()),
        ));
    }
    let length = std::fs::metadata(&resolved)
        .ok()
        .and_then(|metadata| usize::try_from(metadata.len()).ok())
        .unwrap_or(usize::MAX);
    if length > maximum {
        return Err((
            ReasonCode::OutputTruncated,
            format!(
                "{} exceeds the {maximum}-byte extension limit",
                path.display()
            ),
        ));
    }
    std::fs::read_to_string(&resolved).map_err(|error| {
        (
            ReasonCode::SchemaInvalid,
            format!("{} is not readable UTF-8: {error}", path.display()),
        )
    })
}

fn invalid(diagnostics: &mut Vec<ContextDiagnostic>, detail: String) {
    diagnostics.push(ContextDiagnostic {
        code: ReasonCode::SchemaInvalid,
        detail,
    });
}

fn escaped(diagnostics: &mut Vec<ContextDiagnostic>, path: &Path) {
    diagnostics.push(ContextDiagnostic {
        code: ReasonCode::PathSymlinkEscape,
        detail: format!(
            "ignored extension path outside its declared root: {}",
            path.display()
        ),
    });
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    fn write(root: &Path, name: &str, contents: &str) {
        let directory = root.join(".mjolnr/extensions");
        std::fs::create_dir_all(&directory).expect("extension directory");
        std::fs::write(directory.join(name), contents).expect("write extension");
    }

    fn project_roots(root: &Path) -> Vec<(PathBuf, SkillScope, Option<PathBuf>)> {
        vec![(
            root.join(".mjolnr/extensions"),
            SkillScope::Project,
            Some(root.to_path_buf()),
        )]
    }

    const COUNT: &str = "name: count-lines
description: Count the lines in a file.
parameters:
  - name: path
    description: File to count.
run:
  program: wc
  arguments: [\"-l\", \"${path}\"]
";

    #[test]
    fn a_valid_extension_is_discovered() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical");
        write(&root, "count-lines.yaml", COUNT);
        let mut diagnostics = Vec::new();
        let catalog = ExtensionCatalog::discover(
            project_roots(&root),
            DiscoveryLimits::default(),
            &mut diagnostics,
        );
        assert_eq!(catalog.summaries().len(), 1);
        assert_eq!(catalog.summaries()[0].name, "count-lines");
        assert_eq!(catalog.summaries()[0].scope, SkillScope::Project);
        assert!(catalog.requires_project_trust("count-lines"));
        assert!(catalog.get("count-lines").is_some());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn a_malformed_extension_is_skipped_with_a_typed_diagnostic_and_never_registers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical");
        write(&root, "count-lines.yaml", COUNT);
        write(
            &root,
            "broken.yaml",
            "name: broken\ndescription: Missing its run block.\n",
        );
        let mut diagnostics = Vec::new();
        let catalog = ExtensionCatalog::discover(
            project_roots(&root),
            DiscoveryLimits::default(),
            &mut diagnostics,
        );
        // The good one survives; the broken one is absent, not half-registered.
        assert_eq!(catalog.summaries().len(), 1);
        assert!(catalog.get("broken").is_none());
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == ReasonCode::SchemaInvalid && d.detail.contains("broken")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn a_file_whose_stem_disagrees_with_its_name_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical");
        // COUNT declares name: count-lines but lives in wrong-name.yaml.
        write(&root, "wrong-name.yaml", COUNT);
        let mut diagnostics = Vec::new();
        let catalog = ExtensionCatalog::discover(
            project_roots(&root),
            DiscoveryLimits::default(),
            &mut diagnostics,
        );
        assert!(catalog.summaries().is_empty());
        assert!(
            diagnostics
                .iter()
                .any(|d| d.detail.contains("does not match file")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn non_yaml_files_are_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical");
        write(&root, "count-lines.yaml", COUNT);
        write(&root, "README.md", "not an extension");
        let mut diagnostics = Vec::new();
        let catalog = ExtensionCatalog::discover(
            project_roots(&root),
            DiscoveryLimits::default(),
            &mut diagnostics,
        );
        assert_eq!(catalog.summaries().len(), 1);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn a_missing_root_is_silently_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical");
        let mut diagnostics = Vec::new();
        let catalog = ExtensionCatalog::discover(
            project_roots(&root),
            DiscoveryLimits::default(),
            &mut diagnostics,
        );
        assert!(catalog.summaries().is_empty());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
