//! Runtime-owned facts exposed after a repository discovery pass.
//!
//! The durable OKF bundle is the artifact. These bounded values are only the
//! client projection of the most recent pass; they are not authority and do
//! not replace the event store or routing files.

use std::path::PathBuf;

use super::model::{ModelId, ProviderId};

/// The client-facing result of one deterministic discovery pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Workspace-relative bundle path, never an absolute owner path.
    pub bundle_path: PathBuf,
    pub project_name: String,
    pub source_files: usize,
    pub language_counts: Vec<DiscoveryLanguageCount>,
    pub commands: Vec<String>,
    pub convention_files: Vec<PathBuf>,
    pub risk_files: Vec<DiscoveryRisk>,
    pub unresolved_imports: usize,
    pub truncated: bool,
    /// Suggestions only. The owner must accept or edit them in routing config.
    pub model_proposals: Vec<ModelAssignmentProposal>,
}

/// Count of source files handled by one structural parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryLanguageCount {
    pub language: String,
    pub files: usize,
}

/// A deterministic risk indicator derived from importer concentration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRisk {
    pub path: PathBuf,
    pub importer_count: usize,
}

/// A proposed role assignment. This is deliberately not a route mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelAssignmentProposal {
    pub role: String,
    pub provider: ProviderId,
    pub model: ModelId,
    pub basis: String,
}
