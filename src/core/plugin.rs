//! The plugin declaration and manifest contract (ADR-0016, Master Implementation Plan §3.1).
//!
//! A plugin is third-party code running as a local subprocess speaking JSON-RPC 2.0
//! over stdio. This module defines the fail-closed manifest (`mjolnr-plugin.yaml`)
//! format, declared hooks, tool schemas, and data-only view descriptors.
//!
//! # Governed Security Properties (ADR-0016 §3, §4, §5)
//! - **Fail-closed:** unknown fields refuse (`deny_unknown_fields`).
//! - **Fixed tier:** a plugin cannot self-declare tool tiers; all plugin tools
//!   are pinned at `ToolTier::Execute`.
//! - **No pass-env:** credentials must be explicitly named in `required_credentials`
//!   and granted by the owner before injection.
//! - **Hooks as observers:** hooks receive context and emit annotations only;
//!   they cannot mutate tool arguments or bypass gates.
//! - **No schema references:** tool schemas must be locally self-contained (no `$ref`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The active JSON-RPC stdio plugin protocol version supported by mjolnr.
pub const PLUGIN_PROTOCOL_VERSION: u32 = 1;

/// Maximum number of tools a single plugin may declare.
pub const MAX_PLUGIN_TOOLS: usize = 32;

/// Maximum number of required credentials a single plugin may request.
pub const MAX_PLUGIN_CREDENTIALS: usize = 16;

/// Subscribed observer lifecycle hook events (ADR-0016 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHook {
    /// Session created or resumed.
    SessionStart,
    /// User submitted a prompt, before model execution.
    UserPromptSubmit,
    /// Model proposed a tool call, immediately before approval/execution.
    PreToolCall,
    /// Tool completed execution, with result outcome.
    PostToolCall,
    /// Model finished its turn.
    PostTurn,
}

/// Structured observer feedback emitted by a plugin hook (ADR-0016 §5).
/// Hooks are observers only; annotations provide context suggestions and notices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginObserverResult {
    #[serde(default)]
    pub annotations: Vec<String>,
    #[serde(default)]
    pub notices: Vec<String>,
}

/// A tool declared by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginToolDeclaration {
    pub name: String,
    pub description: String,
    #[serde(default = "default_tool_schema")]
    pub parameters: serde_json::Value,
}

fn default_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

/// A data-only view contribution descriptor for the client UI (ADR-0016 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginViewDeclaration {
    pub id: String,
    pub title: String,
    pub view_type: String,
}

/// The declarative manifest of a plugin (`mjolnr-plugin.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    pub protocol_version: u32,
    pub run: PluginRunCommand,
    #[serde(default)]
    pub tools: Vec<PluginToolDeclaration>,
    #[serde(default)]
    pub hooks: Vec<PluginHook>,
    #[serde(default)]
    pub required_credentials: Vec<String>,
    #[serde(default)]
    pub views: Vec<PluginViewDeclaration>,
    #[serde(default)]
    pub source_url: Option<String>,
}

/// Executable and argv specification for launching the plugin subprocess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRunCommand {
    pub program: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

/// Client-facing summary of a discovered plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub description: String,
    pub tool_count: usize,
    pub hook_count: usize,
    pub required_credentials: Vec<String>,
    pub source_url: Option<String>,
}

impl PluginManifest {
    /// Parse and strictly validate a plugin manifest YAML string.
    ///
    /// # Errors
    /// Returns a human-readable reason on any malformed field, schema reference,
    /// protocol mismatch, or path traversal attempt.
    pub fn parse(contents: &str) -> Result<Self, String> {
        let manifest = serde_yaml_ng::from_str::<Self>(contents)
            .map_err(|error| format!("invalid plugin YAML: {error}"))?;

        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate manifest fields against security and protocol constraints.
    pub fn validate(&self) -> Result<(), String> {
        validate_plugin_name(&self.name)?;
        validate_text(&self.version, "version", 32)?;
        validate_text(&self.publisher, "publisher", 64)?;
        validate_text(&self.description, "description", 1_024)?;

        if self.protocol_version != PLUGIN_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported plugin protocol version {}; expected {PLUGIN_PROTOCOL_VERSION}",
                self.protocol_version
            ));
        }

        let program = self.run.program.trim();
        if program.is_empty() {
            return Err("`run.program` must not be empty".to_owned());
        }
        if program.contains("..") {
            return Err("`run.program` cannot contain path traversal (`..`)".to_owned());
        }

        if self.tools.len() > MAX_PLUGIN_TOOLS {
            return Err(format!(
                "a plugin may declare at most {MAX_PLUGIN_TOOLS} tools; found {}",
                self.tools.len()
            ));
        }

        let mut tool_names = BTreeSet::new();
        for tool in &self.tools {
            validate_tool_name(&tool.name)?;
            if !tool_names.insert(&tool.name) {
                return Err(format!("duplicate tool name `{}`", tool.name));
            }
            validate_text(
                &tool.description,
                &format!("tool `{}` description", tool.name),
                1_024,
            )?;
            validate_schema(&tool.name, &tool.parameters)?;
        }

        if self.required_credentials.len() > MAX_PLUGIN_CREDENTIALS {
            return Err(format!(
                "a plugin may request at most {MAX_PLUGIN_CREDENTIALS} credentials; found {}",
                self.required_credentials.len()
            ));
        }

        for cred in &self.required_credentials {
            validate_credential_name(cred)?;
        }

        Ok(())
    }

    /// Convert manifest to client-facing summary.
    #[must_use]
    pub fn summary(&self) -> PluginSummary {
        PluginSummary {
            name: self.name.clone(),
            version: self.version.clone(),
            publisher: self.publisher.clone(),
            description: self.description.clone(),
            tool_count: self.tools.len(),
            hook_count: self.hooks.len(),
            required_credentials: self.required_credentials.clone(),
            source_url: self.source_url.clone(),
        }
    }
}

fn validate_text(value: &str, field: &str, maximum: usize) -> Result<(), String> {
    let length = value.trim().chars().count();
    if length == 0 || length > maximum {
        return Err(format!(
            "`{field}` must contain 1-{maximum} characters; found {length}"
        ));
    }
    Ok(())
}

fn validate_plugin_name(name: &str) -> Result<(), String> {
    validate_text(name, "name", 64)?;
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' || c == '_')
    {
        return Err("plugin name may contain only lowercase ASCII letters, digits, dots, hyphens, and underscores".to_owned());
    }
    if name.starts_with('.') || name.ends_with('.') {
        return Err("plugin name cannot start or end with a dot".to_owned());
    }
    Ok(())
}

fn validate_tool_name(name: &str) -> Result<(), String> {
    validate_text(name, "tool name", 64)?;
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(
            "tool name may contain only lowercase ASCII letters, digits, and underscores"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_credential_name(name: &str) -> Result<(), String> {
    validate_text(name, "credential name", 64)?;
    if !name
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "credential name `{name}` must be uppercase environment variable format (e.g. `GITHUB_TOKEN`)"
        ));
    }
    Ok(())
}

fn validate_schema(tool_name: &str, schema: &serde_json::Value) -> Result<(), String> {
    let schema_str = schema.to_string();
    if schema_str.contains("\"$ref\"") {
        return Err(format!(
            "tool `{tool_name}` schema contains forbidden `$ref`; schemas must be self-contained"
        ));
    }
    if !jsonschema::meta::is_valid(schema) {
        return Err(format!(
            "tool `{tool_name}` schema is not a valid JSON Schema"
        ));
    }
    Ok(())
}
