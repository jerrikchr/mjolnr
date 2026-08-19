//! Bounded discovery of third-party plugins (ADR-0016, Master Implementation Plan §3.1).
//!
//! Scans `.mjolnr/plugins/*.yaml` in the workspace root and the user config directory.
//! Discovery makes plugins **visible** and inspectable; tool registration and
//! execution requires explicit owner authorisation (ADR-0016 §2).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::context::DiscoveryLimits;
use crate::core::context::{ContextDiagnostic, SkillScope};
use crate::core::error::ReasonCode;
use crate::core::plugin::{PluginManifest, PluginSummary};

const PLUGIN_SUFFIXES: [&str; 2] = ["yaml", "yml"];
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    pub summary: PluginSummary,
    pub path: PathBuf,
    pub scope: SkillScope,
}

/// The plugins discovered under project and user roots, indexed by name.
#[derive(Debug, Clone, Default)]
pub struct PluginCatalog {
    plugins: BTreeMap<String, DiscoveredPlugin>,
    ordered: Vec<PluginSummary>,
}

impl PluginCatalog {
    /// Discover plugins from declared roots.
    pub fn discover(
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
                        format!("could not resolve plugin root {}: {error}", root.display()),
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
                        format!(
                            "could not read plugin directory {}: {error}",
                            root.display()
                        ),
                    );
                    continue;
                }
            };
            entries.sort_by_key(std::fs::DirEntry::file_name);

            for entry in entries {
                if scanned >= limits.max_skill_directories {
                    exhausted = true;
                    too_many(diagnostics, limits.max_skill_directories);
                    break;
                }
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_file() {
                    continue;
                }
                let is_plugin_file = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| PLUGIN_SUFFIXES.contains(&ext));
                if !is_plugin_file {
                    continue;
                }

                scanned += 1;
                catalog.ingest_file(&path, scope, diagnostics);
            }
        }

        catalog.ordered = catalog
            .plugins
            .values()
            .map(|record| record.summary.clone())
            .collect();
        catalog
    }

    fn ingest_file(
        &mut self,
        path: &Path,
        scope: SkillScope,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                invalid(
                    diagnostics,
                    format!("could not inspect plugin {}: {error}", path.display()),
                );
                return;
            }
        };
        if metadata.len() > MAX_MANIFEST_BYTES {
            invalid(
                diagnostics,
                format!(
                    "plugin manifest {} exceeds maximum size of {MAX_MANIFEST_BYTES} bytes",
                    path.display()
                ),
            );
            return;
        }

        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) => {
                invalid(
                    diagnostics,
                    format!("could not read plugin {}: {error}", path.display()),
                );
                return;
            }
        };

        let manifest = match PluginManifest::parse(&contents) {
            Ok(manifest) => manifest,
            Err(error) => {
                invalid(
                    diagnostics,
                    format!("malformed plugin {}: {error}", path.display()),
                );
                return;
            }
        };

        if self.plugins.contains_key(&manifest.name) {
            invalid(
                diagnostics,
                format!(
                    "duplicate plugin name `{}` at {}",
                    manifest.name,
                    path.display()
                ),
            );
            return;
        }

        let summary = manifest.summary();
        self.plugins.insert(
            manifest.name.clone(),
            DiscoveredPlugin {
                manifest,
                summary,
                path: path.to_path_buf(),
                scope,
            },
        );
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DiscoveredPlugin> {
        self.plugins.get(name)
    }

    #[must_use]
    pub fn list(&self) -> &[PluginSummary] {
        &self.ordered
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }
}

fn invalid(diagnostics: &mut Vec<ContextDiagnostic>, detail: String) {
    diagnostics.push(ContextDiagnostic {
        code: ReasonCode::SchemaInvalid,
        detail,
    });
}

fn escaped(diagnostics: &mut Vec<ContextDiagnostic>, root: &Path) {
    diagnostics.push(ContextDiagnostic {
        code: ReasonCode::PathSymlinkEscape,
        detail: format!("plugin root {} escapes workspace boundary", root.display()),
    });
}

fn too_many(diagnostics: &mut Vec<ContextDiagnostic>, limit: usize) {
    diagnostics.push(ContextDiagnostic {
        code: ReasonCode::OutputTruncated,
        detail: format!("plugin discovery exceeded file limit of {limit}"),
    });
}
