//! The scripted extension tool (ADR 0002).
//!
//! An [`ExtensionDefinition`] is a named view onto one exact-argv command. This
//! wraps one as a [`Tool`] whose `execute` substitutes the validated arguments
//! into the argv and runs it through the *same* path `run_command` uses:
//! [`command::run_process`], the workspace-identity revalidation, and the
//! shared result formatting. That reuse is the design (ADR 0002) — a loaded
//! extension is gated, previewed, and evidenced exactly like the built-in
//! command tool, because it is the built-in command tool wearing a name.

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::extension::ExtensionDefinition;
use crate::core::message::ToolResult;
use crate::core::tool::{CommandSpec, Tool, ToolContext, ToolTier};

/// A loaded, callable extension.
#[derive(Debug)]
pub(crate) struct ExtensionTool {
    definition: ExtensionDefinition,
}

impl ExtensionTool {
    pub(crate) fn new(definition: ExtensionDefinition) -> Self {
        Self { definition }
    }

    /// The validated string values for this call, keyed by parameter name.
    ///
    /// Arguments are validated against [`schema`](Tool::schema) immediately
    /// before `execute`, so every declared parameter is present as a string.
    /// The `ok_or_else` is the fail-loud path for a caller that skipped
    /// validation, not an expected branch.
    fn values(&self, arguments: &serde_json::Value) -> Result<BTreeMap<String, String>, ToolError> {
        let mut values = BTreeMap::new();
        for parameter in self.definition.parameters() {
            let value = arguments
                .get(&parameter.name)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| ToolError::SchemaInvalid {
                    detail: format!("missing string argument `{}`", parameter.name),
                })?;
            values.insert(parameter.name.clone(), value.to_owned());
        }
        Ok(values)
    }

    fn spec(&self, arguments: &serde_json::Value) -> Result<CommandSpec, ToolError> {
        let values = self.values(arguments)?;
        let resolved = self
            .definition
            .resolve(&values)
            .map_err(|detail| ToolError::SchemaInvalid { detail })?;
        Ok(CommandSpec {
            program: self.definition.program().to_owned(),
            arguments: resolved,
        })
    }
}

#[async_trait]
impl Tool for ExtensionTool {
    fn name(&self) -> &str {
        self.definition.name()
    }

    fn description(&self) -> &str {
        self.definition.description()
    }

    /// Unknown provenance fails closed to `Execute` (; MCP
    /// precedent from Phase 11). An extension cannot declare itself lower —
    /// `tier` is not a field it can set (`core::extension`).
    fn tier(&self) -> ToolTier {
        ToolTier::Execute
    }

    fn schema(&self) -> serde_json::Value {
        self.definition.schema()
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok(self.spec(arguments)?.display())
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let spec = self.spec(&arguments)?;
        let workspace_root = super::command::revalidate_workspace(&context.workspace_root).await?;
        let result = super::command::run_process(
            Path::new(&spec.program),
            &spec.arguments,
            &workspace_root,
            context.command_timeout,
            context.max_output_bytes,
            cancel,
        )
        .await?;

        super::command::result_from_process(&result)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::core::message::{ToolEffect, ToolOutcome};
    use crate::core::tool::ReadSet;

    use super::*;

    fn definition(source: &str, name: &str) -> ExtensionDefinition {
        ExtensionDefinition::parse(source, name).expect("valid extension")
    }

    fn context(root: &Path) -> ToolContext {
        // The workspace is revalidated at execute time against its canonical
        // path. A temporary directory reaches us uncanonicalized on macOS
        // (/var is a symlink to /private/var), which the revalidation
        // correctly refuses as a changed command identity.
        let root = std::fs::canonicalize(root).expect("canonical workspace");
        ToolContext {
            workspace_root: root,
            read_set: Arc::new(ReadSet::default()),
            max_output_bytes: 64 * 1024,
            command_timeout: Duration::from_secs(10),
        }
    }

    fn shell_program() -> String {
        if cfg!(windows) {
            std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".to_owned())
        } else {
            "echo".to_owned()
        }
    }

    fn echo_definition() -> String {
        if cfg!(windows) {
            format!(
                "name: echo-name\ndescription: Echo the supplied name back.\nparameters:\n  - name: who\n    description: Who to greet.\nrun:\n  program: '{}'\n  arguments: ['/C', 'echo', 'hello', '${{who}}']\n",
                shell_program()
            )
        } else {
            ECHO.to_owned()
        }
    }

    const ECHO: &str = "name: echo-name
description: Echo the supplied name back.
parameters:
  - name: who
    description: Who to greet.
run:
  program: echo
  arguments: [\"hello\", \"${who}\"]
";

    #[test]
    fn the_tool_reflects_its_definition() {
        let source = echo_definition();
        let tool = ExtensionTool::new(definition(&source, "echo-name"));
        assert_eq!(tool.name(), "echo-name");
        assert_eq!(tool.tier(), ToolTier::Execute);
        assert!(!tool.requires_workspace_trust(&serde_json::json!({})));
        assert_eq!(tool.schema()["required"], serde_json::json!(["who"]));
    }

    #[tokio::test]
    async fn preview_shows_the_substituted_argv() {
        let source = echo_definition();
        let tool = ExtensionTool::new(definition(&source, "echo-name"));
        let root = tempfile::tempdir().expect("tempdir");
        let preview = tool
            .preview(&serde_json::json!({ "who": "ada" }), &context(root.path()))
            .await
            .expect("preview");
        if cfg!(windows) {
            assert!(preview.ends_with("/C echo hello ada"), "{preview}");
        } else {
            assert_eq!(preview, "echo hello ada");
        }
    }

    #[tokio::test]
    async fn execute_runs_the_real_command_at_the_workspace_root() {
        let source = echo_definition();
        let tool = ExtensionTool::new(definition(&source, "echo-name"));
        let root = tempfile::tempdir().expect("tempdir");
        let result = tool
            .execute(
                serde_json::json!({ "who": "ada" }),
                context(root.path()),
                CancellationToken::new(),
            )
            .await
            .expect("execute");
        assert_eq!(result.outcome, ToolOutcome::Ok);
        assert!(result.content.contains("hello ada"), "{}", result.content);
        assert!(matches!(
            result.effect,
            ToolEffect::Command {
                exit_code: Some(0),
                success: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_missing_argument_is_refused_before_spawn() {
        let source = echo_definition();
        let tool = ExtensionTool::new(definition(&source, "echo-name"));
        let root = tempfile::tempdir().expect("tempdir");
        let error = tool
            .execute(
                serde_json::json!({}),
                context(root.path()),
                CancellationToken::new(),
            )
            .await
            .expect_err("a missing required argument must be refused");
        assert!(
            matches!(error, ToolError::SchemaInvalid { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_nonzero_exit_is_reported_as_a_failed_outcome_not_an_error() {
        let source = if cfg!(windows) {
            format!(
                "name: false-tool\ndescription: Always fails.\nrun:\n  program: '{}'\n  arguments: ['/C', 'exit', '1']\n",
                shell_program()
            )
        } else {
            "name: false-tool\ndescription: Always fails.\nrun:\n  program: false\n  arguments: []\n".to_owned()
        };
        let tool = ExtensionTool::new(definition(&source, "false-tool"));
        let root = tempfile::tempdir().expect("tempdir");
        let result = tool
            .execute(
                serde_json::json!({}),
                context(root.path()),
                CancellationToken::new(),
            )
            .await
            .expect("a failing command is a result, not an Err");
        assert!(matches!(result.outcome, ToolOutcome::Failed(_)));
    }
}
