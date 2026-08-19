//! Third-party plugin tool adapter (ADR-0016 §3, Master Implementation Plan §3.3).
//!
//! Wraps a tool declared in a `PluginManifest` and hosted by a `PluginHost` subprocess.
//!
//! # Governed Security Properties (ADR-0016 §3, AGENTS.md §3)
//! - **Namespacing:** plugin tools are registered as `plugin:<plugin_name>:<tool_name>`.
//! - **Fixed Tier:** strictly pinned at `ToolTier::Execute` — a plugin cannot self-declare
//!   a lower tier (such as Read or Mutate).
//! - **Governed Execution:** every invocation routes through smed's deterministic policy
//!   gate, preview generation, human approval, and post-mutation evidence recording.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::ToolResult;
use crate::core::plugin::PluginToolDeclaration;
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::plugins::PluginHost;
use crate::tools::ToolRegistry;

/// A callable tool provided by a third-party plugin subprocess.
#[derive(Debug)]
pub struct PluginTool {
    namespaced_name: String,
    plugin_name: String,
    remote_tool_name: String,
    description: String,
    schema: serde_json::Value,
    host: Arc<PluginHost>,
}

impl PluginTool {
    /// Create a new plugin tool adapter.
    #[must_use]
    pub fn new(
        plugin_name: String,
        declaration: PluginToolDeclaration,
        host: Arc<PluginHost>,
    ) -> Self {
        let namespaced_name = format!("plugin:{plugin_name}:{}", declaration.name);
        Self {
            namespaced_name,
            plugin_name,
            remote_tool_name: declaration.name,
            description: declaration.description,
            schema: declaration.parameters,
            host,
        }
    }

    #[must_use]
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    #[must_use]
    pub fn remote_tool_name(&self) -> &str {
        &self.remote_tool_name
    }
}

#[async_trait]
impl Tool for PluginTool {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// Pinned at `ToolTier::Execute` per ADR-0016 §3.
    /// Plugins cannot self-declare safety tiers.
    fn tier(&self) -> ToolTier {
        ToolTier::Execute
    }

    fn schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok(format!(
            "Plugin: {}\nTool: {}\nArguments:\n{}",
            self.plugin_name,
            self.remote_tool_name,
            serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string())
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match self
            .host
            .call_tool_with_cancel(
                &self.remote_tool_name,
                arguments,
                context.command_timeout,
                cancel,
            )
            .await
        {
            Ok(result) => {
                let output_text =
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
                Ok(ToolResult::ok(output_text))
            }
            Err(ToolError::Refused { code, detail }) => Ok(ToolResult::refused(code, detail)),
            Err(ToolError::Execution { detail }) => {
                Ok(ToolResult::failed(ReasonCode::ToolExecution, detail))
            }
            Err(err) => Err(err),
        }
    }
}

/// Register all tools declared by an active `PluginHost` into a `ToolRegistry`.
pub fn register_plugin_tools(registry: &mut ToolRegistry, host: &Arc<PluginHost>) {
    let plugin_name = host.manifest().name.clone();
    for tool_decl in &host.manifest().tools {
        let tool = PluginTool::new(plugin_name.clone(), tool_decl.clone(), Arc::clone(host));
        registry.add(Arc::new(tool));
    }
}
