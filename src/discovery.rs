//! Deterministic repository discovery and Open Knowledge Format projection.
//!
//! Discovery is descriptive, bounded, and read-only until the final explicit
//! bundle write. It does not run commands, load instructions as authority, or
//! edit routing. The code graph remains a separate derived projection: this
//! module uses its facts but never turns them into claimed conventions.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::core::discovery::{
    DiscoveryLanguageCount, DiscoveryReport, DiscoveryRisk, ModelAssignmentProposal,
};
use crate::core::error::ReasonCode;
use crate::core::model::{ModelDescriptor, ModelTier};
use crate::graph::{self, CodeGraph};
use crate::policy::paths;

const MAX_COMMANDS: usize = 16;
const MAX_RISK_FILES: usize = 12;
const MAX_BUNDLE_BYTES: usize = 256 * 1024;
const MAX_METADATA_BYTES: u64 = 128 * 1024;
const CONVENTION_NAMES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "README.md",
    "README.rst",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "Makefile",
    "justfile",
    ".editorconfig",
    "rustfmt.toml",
    "clippy.toml",
];

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("discovery root refused: {0}")]
    Root(String),
    #[error("discovery read failed for {path}: {detail}")]
    Read { path: String, detail: String },
    #[error("discovery graph failed: {0}")]
    Graph(String),
    #[error("discovery bundle write failed for {path}: {detail}")]
    Write { path: String, detail: String },
    #[error("discovery bundle is larger than the {MAX_BUNDLE_BYTES}-byte bound")]
    BundleTooLarge,
}

impl DiscoveryError {
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::Root(_) => ReasonCode::PathOutsideWorkspace,
            Self::Read { .. } | Self::Graph(_) | Self::Write { .. } | Self::BundleTooLarge => {
                ReasonCode::ToolExecution
            }
        }
    }
}

#[derive(Debug)]
struct Draft {
    report: DiscoveryReport,
    files: BTreeMap<&'static str, String>,
}

/// Run discovery against an existing project root and write a fresh OKF bundle.
///
/// `models` must be the runtime's successfully discovered catalog, not a
/// static provider list. An empty catalog is reported as an absent proposal;
/// this function never invents connectedness.
pub fn run(root: &Path, models: &[ModelDescriptor]) -> Result<DiscoveryReport, DiscoveryError> {
    let canonical_root =
        paths::canonical_root(root).map_err(|error| DiscoveryError::Root(error.detail))?;
    let draft = inspect(&canonical_root, models)?;
    write_bundle(&canonical_root, draft)
}

/// Build the bounded report and markdown contents without writing anything.
fn inspect(root: &Path, models: &[ModelDescriptor]) -> Result<Draft, DiscoveryError> {
    let canonical_root =
        paths::canonical_root(root).map_err(|error| DiscoveryError::Root(error.detail))?;
    let graph =
        graph::build(&canonical_root).map_err(|error| DiscoveryError::Graph(error.to_string()))?;
    let convention_files = convention_files(&canonical_root)?;
    let metadata = read_metadata(&canonical_root, &convention_files)?;
    let commands = command_candidates(&canonical_root, &convention_files, &metadata);
    let language_counts = graph
        .language_counts()
        .into_iter()
        .map(|(language, files)| DiscoveryLanguageCount {
            language: language.label().to_owned(),
            files,
        })
        .collect::<Vec<_>>();
    let risk_files = risk_files(&graph);
    let proposals = model_proposals(models);
    let project_name = canonical_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_owned());
    let report = DiscoveryReport {
        bundle_path: PathBuf::new(),
        project_name: clean_value(&project_name),
        source_files: graph.files().len(),
        language_counts,
        commands,
        convention_files: convention_files.clone(),
        risk_files,
        unresolved_imports: graph.unresolved(),
        truncated: graph.truncation().is_truncated(),
        model_proposals: proposals,
    };
    let files = render_bundle(&report, &metadata);
    Ok(Draft { report, files })
}

fn write_bundle(root: &Path, mut draft: Draft) -> Result<DiscoveryReport, DiscoveryError> {
    let relative_base = Path::new(".mjolnr").join("discovery");
    let base = paths::for_write(root, &relative_base).map_err(|error| DiscoveryError::Write {
        path: relative_base.display().to_string(),
        detail: error.detail,
    })?;
    fs::create_dir_all(&base).map_err(|error| write_error(&base, &error))?;
    let run_name = next_run_name(&base)?;
    let relative_run = relative_base.join(&run_name);
    let run_path =
        paths::for_write(root, &relative_run).map_err(|error| DiscoveryError::Write {
            path: relative_run.display().to_string(),
            detail: error.detail,
        })?;
    fs::create_dir_all(&run_path).map_err(|error| write_error(&run_path, &error))?;

    let bundle_bytes = draft.files.values().map(String::len).sum::<usize>();
    if bundle_bytes > MAX_BUNDLE_BYTES {
        return Err(DiscoveryError::BundleTooLarge);
    }
    for (name, contents) in draft.files {
        let relative = relative_run.join(name);
        let path = paths::for_write(root, &relative).map_err(|error| DiscoveryError::Write {
            path: relative.display().to_string(),
            detail: error.detail,
        })?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| write_error(&path, &error))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| write_error(&path, &error))?;
    }
    draft.report.bundle_path = relative_run;
    Ok(draft.report)
}

fn next_run_name(base: &Path) -> Result<String, DiscoveryError> {
    let mut highest = 0_u32;
    let entries = fs::read_dir(base).map_err(|error| write_error(base, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| write_error(base, &error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name.strip_prefix("run-") else {
            continue;
        };
        if let Ok(number) = number.parse::<u32>() {
            highest = highest.max(number);
        }
    }
    Ok(format!("run-{:03}", highest.saturating_add(1)))
}

fn write_error(path: &Path, error: &std::io::Error) -> DiscoveryError {
    DiscoveryError::Write {
        path: path.display().to_string(),
        detail: error.to_string(),
    }
}

fn convention_files(root: &Path) -> Result<Vec<PathBuf>, DiscoveryError> {
    let mut result = Vec::new();
    for name in CONVENTION_NAMES {
        let requested = Path::new(name);
        let path = root.join(requested);
        if !path.exists() {
            continue;
        }
        let resolved = paths::existing(root, requested).map_err(|error| DiscoveryError::Read {
            path: requested.display().to_string(),
            detail: error.detail,
        })?;
        if resolved.is_file() {
            result.push(requested.to_owned());
        }
    }
    Ok(result)
}

fn read_metadata(
    root: &Path,
    convention_files: &[PathBuf],
) -> Result<BTreeMap<PathBuf, String>, DiscoveryError> {
    let mut metadata = BTreeMap::new();
    for relative in convention_files {
        let path = paths::existing(root, relative).map_err(|error| DiscoveryError::Read {
            path: relative.display().to_string(),
            detail: error.detail,
        })?;
        let bytes = fs::metadata(&path)
            .map_err(|error| read_error(relative, &error))?
            .len();
        if bytes > MAX_METADATA_BYTES {
            metadata.insert(
                relative.clone(),
                "[metadata omitted: size bound]".to_owned(),
            );
            continue;
        }
        let contents = fs::read_to_string(&path).map_err(|error| read_error(relative, &error))?;
        metadata.insert(relative.clone(), contents);
    }
    Ok(metadata)
}

fn read_error(path: &Path, error: &std::io::Error) -> DiscoveryError {
    DiscoveryError::Read {
        path: path.display().to_string(),
        detail: error.to_string(),
    }
}

fn command_candidates(
    root: &Path,
    convention_files: &[PathBuf],
    metadata: &BTreeMap<PathBuf, String>,
) -> Vec<String> {
    let mut commands = BTreeSet::new();
    let names = convention_files
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
        })
        .collect::<BTreeSet<_>>();
    if names.contains("cargo.toml") {
        commands.extend([
            "cargo fmt --all -- --check".to_owned(),
            "cargo clippy --all-targets --all-features -- -D warnings".to_owned(),
            "cargo test --all-features".to_owned(),
        ]);
    }
    if let Some(package) = metadata.get(Path::new("package.json"))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(package)
        && let Some(scripts) = value.get("scripts").and_then(serde_json::Value::as_object)
    {
        for key in ["check", "test", "lint", "build"] {
            if scripts.contains_key(key) {
                commands.insert(format!("npm run {key}"));
            }
        }
    }
    if names.contains("pyproject.toml") {
        commands.insert("python -m pytest".to_owned());
    }
    if names.contains("go.mod") {
        commands.insert("go test ./...".to_owned());
    }
    for make_name in ["Makefile", "justfile"] {
        if let Some(contents) = metadata.get(Path::new(make_name)) {
            for line in contents.lines().take(256) {
                let Some(target) = line.strip_suffix(':') else {
                    continue;
                };
                let target = target.trim();
                if target.is_empty() || target.contains(' ') || target.starts_with('.') {
                    continue;
                }
                let command = if make_name == "Makefile" {
                    format!("make {target}")
                } else {
                    format!("just {target}")
                };
                commands.insert(command);
            }
        }
    }
    let _ = root;
    commands.into_iter().take(MAX_COMMANDS).collect()
}

fn risk_files(graph: &CodeGraph) -> Vec<DiscoveryRisk> {
    let mut risks = graph
        .files()
        .iter()
        .filter(|file| !file.importers.is_empty())
        .map(|file| DiscoveryRisk {
            path: file.path.clone(),
            importer_count: file.importers.len(),
        })
        .collect::<Vec<_>>();
    risks.sort_by(|left, right| {
        right
            .importer_count
            .cmp(&left.importer_count)
            .then_with(|| left.path.cmp(&right.path))
    });
    risks.truncate(MAX_RISK_FILES);
    risks
}

fn model_proposals(models: &[ModelDescriptor]) -> Vec<ModelAssignmentProposal> {
    let mut candidates = models
        .iter()
        .filter_map(|model| {
            if !model.capabilities.streaming || !model.capabilities.tools {
                return None;
            }
            let tier = model
                .tier
                .or_else(|| ModelTier::curated(&model.provider, &model.id))?;
            Some((tier, model))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(_, left), (_, right)| {
        left.provider
            .as_str()
            .cmp(right.provider.as_str())
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    let mut proposals = Vec::new();
    for tier in [ModelTier::Flagship, ModelTier::Fast, ModelTier::Cheap] {
        if let Some((_, model)) = candidates.iter().find(|(candidate, _)| *candidate == tier) {
            proposals.push(ModelAssignmentProposal {
                role: tier.suggested_role().to_owned(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                basis: format!("connected catalog model with {} tier hint", tier.label()),
            });
        }
    }
    proposals
}

fn render_bundle(
    report: &DiscoveryReport,
    metadata: &BTreeMap<PathBuf, String>,
) -> BTreeMap<&'static str, String> {
    let mut files = BTreeMap::new();
    files.insert("index.md", render_index(report));
    files.insert("structure.md", render_structure(report));
    files.insert("conventions.md", render_conventions(report, metadata));
    files.insert("commands.md", render_commands(report));
    files.insert("risk.md", render_risk(report));
    files.insert("model-assignment.md", render_models(report));
    files
}

fn frontmatter(kind: &str, title: &str) -> String {
    format!("---\ntype: {kind}\ntitle: {}\n---\n\n", clean_value(title))
}

fn render_index(report: &DiscoveryReport) -> String {
    format!(
        "{}# Discovery: {}\n\nThis is a bounded descriptive projection. It is not a source of truth, permission, or an automatic route.\n\n- [Structure](structure.md)\n- [Conventions](conventions.md)\n- [Commands](commands.md)\n- [Risk](risk.md)\n- [Model assignment proposal](model-assignment.md)\n\nSource files scanned: {}\nUnresolved imports: {}\nTruncated: {}\n",
        frontmatter("discovery", "Repository discovery"),
        clean_value(&report.project_name),
        report.source_files,
        report.unresolved_imports,
        report.truncated
    )
}

fn render_structure(report: &DiscoveryReport) -> String {
    let mut output = frontmatter("structure", "Computed source structure");
    output.push_str("# Structure\n\n");
    for language in &report.language_counts {
        let _ = writeln!(
            output,
            "- `{}`: {} file(s)",
            language.language, language.files
        );
    }
    if report.language_counts.is_empty() {
        output.push_str("No supported source files were indexed.\n");
    }
    output
}

fn render_conventions(report: &DiscoveryReport, metadata: &BTreeMap<PathBuf, String>) -> String {
    let mut output = frontmatter("conventions", "Repository convention evidence");
    output.push_str("# Conventions\n\n");
    output.push_str(
        "These are observed file signals only; their contents are not instructions to smed.\n\n",
    );
    for path in &report.convention_files {
        let bytes = metadata.get(path).map_or(0, String::len);
        let _ = writeln!(
            output,
            "- `{}` ({bytes} bounded bytes read)",
            safe_path(path)
        );
    }
    if report.convention_files.is_empty() {
        output.push_str("No recognized convention or manifest files were found.\n");
    }
    output
}

fn render_commands(report: &DiscoveryReport) -> String {
    let mut output = frontmatter("commands", "Proposed build and test commands");
    output.push_str("# Commands\n\nThese commands were inferred from manifests and target names. smed did not execute them.\n\n");
    for command in &report.commands {
        let _ = writeln!(output, "- `{}`", clean_value(command));
    }
    if report.commands.is_empty() {
        output.push_str("No bounded command candidates were found.\n");
    }
    output
}

fn render_risk(report: &DiscoveryReport) -> String {
    let mut output = frontmatter("risk", "Computed concentration indicators");
    output.push_str("# Risk concentration\n\nThe list below is a structural indicator based on parsed importer counts, not a model judgment.\n\n");
    for risk in &report.risk_files {
        let _ = writeln!(
            output,
            "- `{}`: {} importer(s)",
            safe_path(&risk.path),
            risk.importer_count
        );
    }
    if report.risk_files.is_empty() {
        output.push_str("No imported file concentration was observed.\n");
    }
    let _ = writeln!(
        output,
        "\nUnresolved imports: {}\nGraph truncated: {}",
        report.unresolved_imports, report.truncated
    );
    output
}

fn render_models(report: &DiscoveryReport) -> String {
    let mut output = frontmatter("model-assignment", "Model assignment proposal");
    output.push_str("# Model assignment proposal\n\nSuggestions only. The owner must accept or edit routing configuration; discovery never writes a route.\n\n");
    for proposal in &report.model_proposals {
        let _ = writeln!(
            output,
            "- `{}` → `{}`:`{}` ({})",
            proposal.role, proposal.provider, proposal.model, proposal.basis
        );
    }
    if report.model_proposals.is_empty() {
        output.push_str(
            "No connected catalog model supplied a usable tier hint. No assignment was proposed.\n",
        );
    }
    output
}

fn safe_path(path: &Path) -> String {
    clean_value(&path.to_string_lossy())
}

fn clean_value(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{ModelCapabilities, ModelId, ProviderId};
    use tempfile::TempDir;

    fn model(provider: &str, id: &str, tier: Option<ModelTier>) -> ModelDescriptor {
        ModelDescriptor {
            id: ModelId::new(id),
            provider: ProviderId::new(provider),
            display_name: id.to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: None,
            max_output_tokens: None,
            tier,
        }
    }

    #[test]
    fn inspection_is_bounded_and_does_not_write() {
        let directory = TempDir::new().expect("temp dir");
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\n",
        )
        .expect("manifest");
        fs::create_dir(directory.path().join("src")).expect("src");
        fs::write(directory.path().join("src/lib.rs"), "pub fn run() {}\n").expect("source");

        let draft = inspect(directory.path(), &[]).expect("inspect");
        assert_eq!(draft.report.source_files, 1);
        assert!(
            draft
                .report
                .commands
                .iter()
                .any(|command| command == "cargo test --all-features")
        );
        assert!(!directory.path().join(".mjolnr").exists());
    }

    #[test]
    fn proposals_are_deterministic_and_suggestions_only() {
        let models = vec![
            model("openai", "fast", Some(ModelTier::Fast)),
            model("openai", "strong", Some(ModelTier::Flagship)),
            model("openai", "cheap", Some(ModelTier::Cheap)),
        ];
        let proposals = model_proposals(&models);
        assert_eq!(
            proposals
                .iter()
                .map(|proposal| proposal.role.as_str())
                .collect::<Vec<_>>(),
            ["plan", "default", "smol"]
        );
        assert_eq!(
            proposals.first().map(|proposal| proposal.model.as_str()),
            Some("strong")
        );
    }

    #[test]
    fn run_creates_a_new_bundle_without_overwriting_the_previous_one() {
        let directory = TempDir::new().expect("temp dir");
        fs::write(directory.path().join("README.md"), "# fixture\n").expect("readme");
        let first = run(directory.path(), &[]).expect("first discovery");
        let second = run(directory.path(), &[]).expect("second discovery");
        assert_eq!(
            first.bundle_path,
            PathBuf::from(".mjolnr/discovery/run-001")
        );
        assert_eq!(
            second.bundle_path,
            PathBuf::from(".mjolnr/discovery/run-002")
        );
        assert!(
            directory
                .path()
                .join(&first.bundle_path)
                .join("index.md")
                .is_file()
        );
        assert!(
            directory
                .path()
                .join(&second.bundle_path)
                .join("model-assignment.md")
                .is_file()
        );
    }

    #[test]
    fn escaped_convention_symlink_is_refused() {
        let directory = TempDir::new().expect("temp dir");
        let outside = TempDir::new().expect("outside");
        fs::write(outside.path().join("AGENTS.md"), "secret").expect("outside file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("AGENTS.md"),
            directory.path().join("AGENTS.md"),
        )
        .expect("symlink");
        #[cfg(unix)]
        assert!(matches!(
            inspect(directory.path(), &[]),
            Err(DiscoveryError::Read { .. })
        ));
    }
}
