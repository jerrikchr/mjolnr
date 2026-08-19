//! The tool boundary.
//!
//! Phase 1 defines the contract; Phase 3 implements the tools and the policy
//! gate that stands in front of them. The trait exists now so the runtime is
//! built against it rather than retrofitted around it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::message::ToolResult;

/// How dangerous a tool is, and therefore what gate it must pass.
///
/// Ordering matters: `Execute` is the most privileged. When a tier cannot be
/// determined it resolves *here*, to `Execute` — see [`ToolTier::default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolTier {
    Read,
    Write,
    Execute,
}

/// A provider-neutral function definition sent with every agent turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

/// One exact argv command. No shell string exists to expand twice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandSpec {
    pub program: String,
    pub arguments: Vec<String>,
}

impl CommandSpec {
    #[must_use]
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.arguments.iter().map(String::as_str))
            .map(quote_argument)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn quote_argument(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._/:=@".contains(character))
    {
        return argument.to_owned();
    }

    format!("'{}'", argument.replace('\'', "'\\''"))
}

/// SHA-256 versions of files observed in this session.
#[derive(Debug, Default)]
pub struct ReadSet {
    files: RwLock<BTreeMap<PathBuf, String>>,
}

impl ReadSet {
    pub fn observe(&self, path: PathBuf, sha256: String) -> Result<(), ToolError> {
        let mut files = self.files.write().map_err(|_| ToolError::Execution {
            detail: "session read set is unavailable".to_owned(),
        })?;
        files.insert(path, sha256);
        Ok(())
    }

    pub fn version(&self, path: &Path) -> Result<Option<String>, ToolError> {
        let files = self.files.read().map_err(|_| ToolError::Execution {
            detail: "session read set is unavailable".to_owned(),
        })?;
        Ok(files.get(path).cloned())
    }

    pub fn clear(&self) -> Result<(), ToolError> {
        let mut files = self.files.write().map_err(|_| ToolError::Execution {
            detail: "session read set is unavailable".to_owned(),
        })?;
        files.clear();
        Ok(())
    }

    /// Every observed file version, for checkpointing.
    ///
    /// Ordered by path, since the backing map is a `BTreeMap` — a checkpoint
    /// that reordered its read set on every write would produce a different
    /// `state_json` for identical state, which makes diffing a stored session
    /// useless.
    pub fn entries(&self) -> Result<Vec<(PathBuf, String)>, ToolError> {
        let files = self.files.read().map_err(|_| ToolError::Execution {
            detail: "session read set is unavailable".to_owned(),
        })?;
        Ok(files
            .iter()
            .map(|(path, sha256)| (path.clone(), sha256.clone()))
            .collect())
    }

    /// Rebuild a read set from a checkpoint.
    pub fn restore(entries: Vec<(PathBuf, String)>) -> Result<Self, ToolError> {
        let set = Self::default();
        for (path, sha256) in entries {
            set.observe(path, sha256)?;
        }
        Ok(set)
    }
}

impl Default for ToolTier {
    /// **Unknown means `Execute`.**
    ///
    ///  and §Phase 3: "unknown classifications fail closed as execute"
    /// and "unknown tool tier requires execute approval". A tool whose tier we
    /// cannot establish is treated as the most dangerous thing it could be, not
    /// the least. This is [`Default`] rather than a helper so that any code path
    /// that forgets to classify still lands on the safe answer.
    fn default() -> Self {
        Self::Execute
    }
}

/// What a tool is given when it runs.
///
/// Phase 3 adds the workspace root, the read set, and the policy decision that
/// authorised this call. Kept minimal here rather than speculatively designed.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub workspace_root: std::path::PathBuf,
    pub read_set: Arc<ReadSet>,
    pub max_output_bytes: usize,
    pub command_timeout: Duration,
}

/// A tool the model may propose.
#[async_trait]
pub trait Tool: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    fn tier(&self) -> ToolTier;

    /// Whether this exact proposal needs the project-skill trust gate.
    ///
    /// Trust admits advisory instructions only. It never bypasses the normal
    /// policy decision for scripts or commands those instructions mention.
    fn requires_workspace_trust(&self, _arguments: &serde_json::Value) -> bool {
        false
    }

    /// JSON Schema for this tool's arguments.
    fn schema(&self) -> serde_json::Value;

    /// Human-review material produced before a side-effect approval. The
    /// returned text is display only and is never applied as a patch or shell.
    async fn preview(
        &self,
        arguments: &serde_json::Value,
        context: &ToolContext,
    ) -> Result<String, ToolError>;

    /// Run the tool.
    ///
    /// Contract:
    ///
    /// - Arguments are validated against [`schema`](Tool::schema) **immediately
    ///   before this call**, after every policy or hook transformation (plan
    ///   §8.4). Validating earlier and then mutating defeats validation.
    /// - `cancel` must terminate the work, including any child process group.
    /// - A refusal is a [`ToolResult`] with a refused outcome, not an `Err`.
    ///   `Err` is for the tool malfunctioning, not for the tool saying no.
    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_tier_fails_closed_to_execute() {
        // The guard is the `Default` impl: anything that forgets to classify a
        // tool still gets the most restrictive gate.
        assert_eq!(ToolTier::default(), ToolTier::Execute);
    }

    #[test]
    fn tiers_order_by_privilege() {
        assert!(ToolTier::Read < ToolTier::Write);
        assert!(ToolTier::Write < ToolTier::Execute);
    }

    #[test]
    fn command_display_quotes_without_creating_shell_syntax() {
        let command = CommandSpec {
            program: "cargo".to_owned(),
            arguments: vec!["test".to_owned(), "a value".to_owned()],
        };
        assert_eq!(command.display(), "cargo test 'a value'");
    }
}
