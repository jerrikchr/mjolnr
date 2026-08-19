//! Bounded discovery of external-agent profiles (D9).
//!
//! Scans `.mjolnr/external-agent/*.yaml` in the project root. Discovery makes
//! profiles **visible**; launching one requires an explicit human command.

use std::path::{Path, PathBuf};

use crate::context::DiscoveryLimits;
use crate::core::context::{ContextDiagnostic, SkillScope};
use crate::core::error::ReasonCode;

const PROFILE_SUFFIXES: [&str; 2] = ["yaml", "yml"];
const MAX_PROFILE_BYTES: u64 = 32 * 1024;
const MAX_PROFILE_ARGV: usize = 32;
const MAX_PROFILE_ARG_BYTES: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAgentProfile {
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl ExternalAgentProfile {
    pub fn parse(contents: &str) -> Result<Self, String> {
        let profile = serde_yaml_ng::from_str::<Self>(contents)
            .map_err(|e| format!("invalid external-agent profile: {e}"))?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err("profile `name` must be 1-64 characters".to_owned());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(
                "profile name may contain only lowercase letters, digits, '-' and '_'".to_owned(),
            );
        }
        let exe = self.executable.trim();
        if exe.is_empty() || exe.len() > 1_024 {
            return Err("profile `executable` must be 1-1024 characters".to_owned());
        }
        if exe.contains("..") {
            return Err("profile `executable` may not contain `..`".to_owned());
        }
        if self.args.len() > MAX_PROFILE_ARGV {
            return Err(format!("profile may have at most {MAX_PROFILE_ARGV} args"));
        }
        for arg in &self.args {
            if arg.len() > MAX_PROFILE_ARG_BYTES {
                return Err(format!("profile arg exceeds {MAX_PROFILE_ARG_BYTES} bytes"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredExternalAgent {
    pub profile: ExternalAgentProfile,
    pub path: PathBuf,
    pub scope: SkillScope,
}

#[derive(Debug, Clone, Default)]
pub struct ExternalAgentCatalog {
    agents: std::collections::BTreeMap<String, DiscoveredExternalAgent>,
    ordered: Vec<ExternalAgentSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ExternalAgentSummary {
    pub name: String,
    pub executable: String,
    pub args: Vec<String>,
}

impl ExternalAgentCatalog {
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
                Ok(r) => r,
                Err(e) => {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!(
                            "could not resolve external-agent root {}: {e}",
                            root.display()
                        ),
                    });
                    continue;
                }
            };
            if boundary
                .as_ref()
                .is_some_and(|b: &PathBuf| !canonical_root.starts_with(b))
            {
                diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::PathSymlinkEscape,
                    detail: format!(
                        "external-agent root {} escapes workspace boundary",
                        root.display()
                    ),
                });
                continue;
            }
            let mut entries = match std::fs::read_dir(&canonical_root) {
                Ok(e) => e.flatten().collect::<Vec<_>>(),
                Err(e) => {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!(
                            "could not read external-agent directory {}: {e}",
                            root.display()
                        ),
                    });
                    continue;
                }
            };
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                if scanned >= limits.max_skill_directories {
                    exhausted = true;
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::OutputTruncated,
                        detail: format!(
                            "external-agent discovery exceeded file limit of {}",
                            limits.max_skill_directories
                        ),
                    });
                    break;
                }
                let path = entry.path();
                let Ok(ft) = entry.file_type() else { continue };
                if !ft.is_file() {
                    continue;
                }
                let is_yaml = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| PROFILE_SUFFIXES.contains(&e));
                if !is_yaml {
                    continue;
                }
                scanned += 1;
                catalog.ingest_file(&path, scope, diagnostics);
            }
        }
        catalog.ordered = catalog
            .agents
            .values()
            .map(|r| ExternalAgentSummary {
                name: r.profile.name.clone(),
                executable: r.profile.executable.clone(),
                args: r.profile.args.clone(),
            })
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
            Ok(m) => m,
            Err(e) => {
                diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::SchemaInvalid,
                    detail: format!("could not inspect external-agent {}: {e}", path.display()),
                });
                return;
            }
        };
        if metadata.len() > MAX_PROFILE_BYTES {
            diagnostics.push(ContextDiagnostic {
                code: ReasonCode::SchemaInvalid,
                detail: format!(
                    "external-agent profile {} exceeds maximum size of {MAX_PROFILE_BYTES} bytes",
                    path.display()
                ),
            });
            return;
        }
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::SchemaInvalid,
                    detail: format!("could not read external-agent {}: {e}", path.display()),
                });
                return;
            }
        };
        let profile = match ExternalAgentProfile::parse(&contents) {
            Ok(p) => p,
            Err(e) => {
                diagnostics.push(ContextDiagnostic {
                    code: ReasonCode::SchemaInvalid,
                    detail: format!("malformed external-agent {}: {e}", path.display()),
                });
                return;
            }
        };
        if self.agents.contains_key(&profile.name) {
            diagnostics.push(ContextDiagnostic {
                code: ReasonCode::SchemaInvalid,
                detail: format!(
                    "duplicate external-agent name `{}` at {}",
                    profile.name,
                    path.display()
                ),
            });
            return;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem != profile.name {
            diagnostics.push(ContextDiagnostic {
                code: ReasonCode::SchemaInvalid,
                detail: format!(
                    "external-agent file stem `{stem}` must match profile name `{}` at {}",
                    profile.name,
                    path.display()
                ),
            });
            return;
        }
        self.agents.insert(
            profile.name.clone(),
            DiscoveredExternalAgent {
                profile,
                path: path.to_path_buf(),
                scope,
            },
        );
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&DiscoveredExternalAgent> {
        self.agents.get(name)
    }

    #[must_use]
    pub fn list(&self) -> &[ExternalAgentSummary] {
        &self.ordered
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.agents.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_profile_parses() {
        let p = ExternalAgentProfile::parse("name: codex\nexecutable: codex\nargs: [\"--json\"]\n")
            .unwrap();
        assert_eq!(p.name, "codex");
    }

    #[test]
    fn invalid_name_refused() {
        assert!(ExternalAgentProfile::parse("name: BadName\nexecutable: x\n").is_err());
    }

    #[test]
    fn executable_traversal_refused() {
        assert!(ExternalAgentProfile::parse("name: foo\nexecutable: ../bin/codex\n").is_err());
    }

    #[test]
    fn unknown_field_refused() {
        assert!(ExternalAgentProfile::parse("name: foo\nexecutable: x\nunknown: 1\n").is_err());
    }

    #[test]
    fn catalog_rejects_stem_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let ea = dir.path().join(".mjolnr").join("external-agent");
        std::fs::create_dir_all(&ea).unwrap();
        std::fs::write(ea.join("foo.yaml"), "name: bar\nexecutable: x\n").unwrap();
        let mut diags = Vec::new();
        let cat = ExternalAgentCatalog::discover(
            vec![(
                ea,
                SkillScope::Project,
                Some(dir.path().canonicalize().unwrap()),
            )],
            DiscoveryLimits::default(),
            &mut diags,
        );
        assert!(cat.is_empty());
        assert!(diags.iter().any(|d| d.detail.contains("stem")));
    }
}
