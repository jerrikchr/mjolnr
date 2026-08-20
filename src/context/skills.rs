//! Bounded Agent Skills discovery and activation.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::context::frontmatter;
use crate::context::{ActivatedSkill, DiscoveryLimits, xml};
use crate::core::context::{ContextDiagnostic, SkillScope, SkillSummary};
use crate::core::error::ReasonCode;

const IGNORED_DIRECTORIES: [&str; 6] = [
    ".git",
    ".venv",
    "node_modules",
    "target",
    "vendor",
    "__pycache__",
];

#[derive(Debug, Clone)]
struct SkillRecord {
    summary: SkillSummary,
    directory: PathBuf,
    allowed_root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SkillCatalog {
    records: BTreeMap<String, SkillRecord>,
    ordered: Vec<SkillSummary>,
    limits: DiscoveryLimits,
}

impl SkillCatalog {
    pub(super) fn discover(
        roots: Vec<(PathBuf, SkillScope, Option<PathBuf>)>,
        limits: DiscoveryLimits,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) -> Self {
        let mut catalog = Self {
            records: BTreeMap::new(),
            ordered: Vec::new(),
            limits,
        };
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
                        format!("could not resolve skill root {}: {error}", root.display()),
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
                        format!("could not scan skill root {}: {error}", root.display()),
                    );
                    continue;
                }
            };
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                if !entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() || kind.is_symlink())
                {
                    continue;
                }
                if scanned >= catalog.limits.max_skill_directories {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::OutputTruncated,
                        detail: format!("skill scan stopped at {scanned} directories"),
                    });
                    exhausted = true;
                    break;
                }
                scanned += 1;
                catalog.inspect(&entry.path(), &canonical_root, scope, diagnostics);
            }
        }
        catalog
    }

    fn inspect(
        &mut self,
        path: &Path,
        allowed_root: &Path,
        scope: SkillScope,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) {
        let directory = match path.canonicalize() {
            Ok(directory) if directory.starts_with(allowed_root) => directory,
            Ok(_) => {
                escaped(diagnostics, path);
                return;
            }
            Err(error) => {
                invalid(
                    diagnostics,
                    format!(
                        "could not resolve skill directory {}: {error}",
                        path.display()
                    ),
                );
                return;
            }
        };
        let Some(directory_name) = directory.file_name().and_then(std::ffi::OsStr::to_str) else {
            invalid(
                diagnostics,
                format!("skill directory is not valid UTF-8: {}", path.display()),
            );
            return;
        };
        let location = directory.join("SKILL.md");
        let contents = match read_bounded(&location, self.limits.max_skill_file_bytes, allowed_root)
        {
            Ok(contents) => contents,
            Err((code, detail)) => {
                diagnostics.push(ContextDiagnostic { code, detail });
                return;
            }
        };
        let parsed = match frontmatter::parse(&contents, directory_name) {
            Ok(parsed) => parsed,
            Err(detail) => {
                invalid(
                    diagnostics,
                    format!("invalid {}: {detail}", location.display()),
                );
                return;
            }
        };
        let summary = SkillSummary {
            name: parsed.name.clone(),
            description: parsed.description,
            location: location
                .canonicalize()
                .unwrap_or(location)
                .to_string_lossy()
                .into_owned(),
            scope,
        };
        if let Some(existing) = self.records.get(&summary.name) {
            invalid(
                diagnostics,
                format!(
                    "skill collision for `{}`: kept {}, ignored {}",
                    summary.name, existing.summary.location, summary.location
                ),
            );
            return;
        }
        self.ordered.push(summary.clone());
        self.records.insert(
            summary.name.clone(),
            SkillRecord {
                summary,
                directory,
                allowed_root: allowed_root.to_path_buf(),
            },
        );
    }

    pub(super) fn summaries(&self) -> &[SkillSummary] {
        &self.ordered
    }

    pub(super) fn requires_project_trust(&self, name: &str) -> bool {
        self.records
            .get(name)
            .is_some_and(|record| record.summary.scope == SkillScope::Project)
    }

    pub(super) fn activate(&self, name: &str) -> Result<ActivatedSkill, (ReasonCode, String)> {
        let record = self
            .records
            .get(name)
            .ok_or_else(|| (ReasonCode::SchemaInvalid, format!("unknown skill `{name}`")))?;
        let directory = record.directory.canonicalize().map_err(|error| {
            (
                ReasonCode::SchemaInvalid,
                format!("could not resolve skill `{name}`: {error}"),
            )
        })?;
        if !directory.starts_with(&record.allowed_root) {
            return Err((
                ReasonCode::PathSymlinkEscape,
                format!("skill `{name}` moved outside its discovered root"),
            ));
        }
        let location = directory.join("SKILL.md");
        let contents = read_bounded(
            &location,
            self.limits.max_skill_file_bytes,
            &record.allowed_root,
        )?;
        let parsed = frontmatter::parse(&contents, name).map_err(|detail| {
            (
                ReasonCode::SchemaInvalid,
                format!("skill `{name}` changed: {detail}"),
            )
        })?;
        if parsed.name != record.summary.name || parsed.description != record.summary.description {
            return Err((
                ReasonCode::SchemaInvalid,
                format!("skill `{name}` metadata changed after discovery"),
            ));
        }
        let (resources, truncated) = list_resources(
            &directory,
            &record.allowed_root,
            self.limits.max_resources_per_skill,
        )?;
        let mut content = format!(
            "<skill_content name=\"{}\">\n{}\n\nSkill directory: {}\nRelative paths are resolved from this directory.\n",
            xml(&parsed.name),
            xml(&parsed.body),
            xml(&directory.display().to_string())
        );
        if !resources.is_empty() {
            content.push_str("<skill_resources>\n");
            for resource in resources {
                content.push_str("  <file>");
                content.push_str(&xml(&resource));
                content.push_str("</file>\n");
            }
            if truncated {
                content.push_str("  <truncated>true</truncated>\n");
            }
            content.push_str("</skill_resources>\n");
        }
        content.push_str("Bundled scripts are resources, not permissions; running one still requires mjolnr's run_command policy.\n</skill_content>");
        Ok(ActivatedSkill {
            name: parsed.name,
            project: record.summary.scope == SkillScope::Project,
            content,
        })
    }
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
            format!("{} resolves outside its skill root", path.display()),
        ));
    }
    let length = std::fs::metadata(&resolved)
        .ok()
        .and_then(|metadata| usize::try_from(metadata.len()).ok())
        .unwrap_or(usize::MAX);
    if length > maximum {
        return Err((
            ReasonCode::OutputTruncated,
            format!("{} exceeds the {maximum}-byte skill limit", path.display()),
        ));
    }
    std::fs::read_to_string(&resolved).map_err(|error| {
        (
            ReasonCode::SchemaInvalid,
            format!("{} is not readable UTF-8: {error}", path.display()),
        )
    })
}

fn list_resources(
    directory: &Path,
    allowed_root: &Path,
    maximum: usize,
) -> Result<(Vec<String>, bool), (ReasonCode, String)> {
    let mut resources = Vec::new();
    let mut queue = VecDeque::from([(directory.to_path_buf(), 0usize)]);
    let mut truncated = false;
    while let Some((current, depth)) = queue.pop_front() {
        if depth > 3 {
            truncated = true;
            continue;
        }
        let mut entries = std::fs::read_dir(&current)
            .map_err(|error| {
                (
                    ReasonCode::SchemaInvalid,
                    format!("could not list {}: {error}", current.display()),
                )
            })?
            .flatten()
            .collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if IGNORED_DIRECTORIES.contains(&name.as_ref()) {
                continue;
            }
            let resolved = path.canonicalize().map_err(|error| {
                (
                    ReasonCode::SchemaInvalid,
                    format!("could not resolve resource {}: {error}", path.display()),
                )
            })?;
            if !resolved.starts_with(allowed_root) {
                return Err((
                    ReasonCode::PathSymlinkEscape,
                    format!("resource {} escapes the skill root", path.display()),
                ));
            }
            if resolved.is_dir() {
                queue.push_back((resolved, depth + 1));
                continue;
            }
            if path.file_name().and_then(std::ffi::OsStr::to_str) == Some("SKILL.md") {
                continue;
            }
            if resources.len() >= maximum {
                truncated = true;
                continue;
            }
            let relative = resolved.strip_prefix(directory).map_err(|_| {
                (
                    ReasonCode::PathSymlinkEscape,
                    format!("resource {} moved outside its skill", path.display()),
                )
            })?;
            resources.push(relative.to_string_lossy().into_owned());
        }
    }
    resources.sort();
    Ok((resources, truncated))
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
            "ignored skill path outside its declared root: {}",
            path.display()
        ),
    });
}
