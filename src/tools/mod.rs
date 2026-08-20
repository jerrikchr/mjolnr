//! mjolnr-owned tool registry and built-in repository tools.

mod arguments;
mod command;
mod edit;
mod extension;
mod files;
mod finish;
pub mod graph_query;
mod list;
pub mod memory;
pub(crate) mod output;
pub mod plugin;
mod read;
mod search;
pub mod session_query;
pub mod subagent;
mod write;

use std::sync::Arc;

use crate::core::error::ToolError;
use crate::core::tool::{Tool, ToolDefinition};

pub use command::RunCommand;
pub(crate) use command::sanitized_environment;
pub(crate) use extension::ExtensionTool;
pub use memory::{MemoryExpand, MemorySearch, MemoryTimeline};
pub use plugin::{PluginTool, register_plugin_tools};

/// `Clone` is cheap: every entry is an `Arc`. The scheduler needs it because
/// each trigger firing needs its own registry value to pass into a fresh
/// [`Runtime`](crate::runtime::Runtime), and a background process fires the
/// same trigger many times over its lifetime.
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn builtins() -> Self {
        Self::new(vec![
            Arc::new(read::ReadFile),
            Arc::new(list::ListFiles),
            Arc::new(search::SearchText),
            Arc::new(graph_query::QueryGraph),
            Arc::new(memory::MemorySearch),
            Arc::new(memory::MemoryTimeline),
            Arc::new(memory::MemoryExpand),
            Arc::new(write::WriteFile),
            Arc::new(edit::EditFile),
            Arc::new(command::RunCommand),
            Arc::new(finish::FinishTask),
        ])
    }

    #[must_use]
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self { tools }
    }

    pub(crate) fn add(&mut self, tool: Arc<dyn Tool>) {
        if self
            .tools
            .iter()
            .all(|existing| existing.name() != tool.name())
        {
            self.tools.push(tool);
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(Arc::clone)
    }

    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| ToolDefinition {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                schema: tool.schema(),
            })
            .collect()
    }

    pub fn validate(
        &self,
        tool: &dyn Tool,
        arguments: &serde_json::Value,
    ) -> Result<(), ToolError> {
        let schema = tool.schema();
        let validator = jsonschema::draft202012::options()
            .build(&schema)
            .map_err(|error| ToolError::SchemaInvalid {
                detail: format!("{} has an invalid local schema: {error}", tool.name()),
            })?;

        validator
            .validate(arguments)
            .map_err(|error| ToolError::SchemaInvalid {
                detail: error.to_string(),
            })
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_schema_is_local_and_valid() {
        let registry = ToolRegistry::builtins();
        for definition in registry.definitions() {
            assert!(
                jsonschema::meta::is_valid(&definition.schema),
                "{} has an invalid schema",
                definition.name
            );
            assert!(
                !definition.schema.to_string().contains("$ref"),
                "{} must not trigger external schema retrieval",
                definition.name
            );
        }
    }

    #[test]
    fn openai_nulls_for_optional_arguments_resolve_to_local_defaults() {
        let registry = ToolRegistry::builtins();
        let cases = [
            (
                "read_file",
                serde_json::json!({
                    "path": "src/lib.rs",
                    "start_line": null,
                    "line_count": null
                }),
            ),
            (
                "list_files",
                serde_json::json!({
                    "path": null,
                    "recursive": null,
                    "max_results": null
                }),
            ),
            (
                "search_text",
                serde_json::json!({
                    "query": "mjolnr",
                    "path": null,
                    "max_results": null
                }),
            ),
        ];

        for (name, arguments) in cases {
            let tool = registry.get(name).expect("built-in tool");
            registry
                .validate(tool.as_ref(), &arguments)
                .unwrap_or_else(|error| panic!("{name} rejected OpenAI-compatible nulls: {error}"));
        }
    }
}
