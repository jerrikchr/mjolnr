//! Explicit MCP client integration over stdio (Phase 11) and remote HTTP
//! (Phase 26). Remote servers connect over the streamable-HTTP transport with
//! bearer/header auth and are governed identically to stdio: `Execute` tier,
//! namespaced tools, bounded results, and honest `Unavailable` on any failure —
//! never a fabricated `Connected`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{Peer, RoleClient, RunningService, RunningServiceCancellationToken};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use serde::Deserialize;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::mcp::{McpConnectionState, McpServerSummary};
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::tools::ToolRegistry;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Tools and display state discovered from explicit project configuration.
#[derive(Debug)]
pub struct McpCatalog {
    pub registry: ToolRegistry,
    pub servers: Arc<Vec<McpServerSummary>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpConfig {
    #[serde(default)]
    servers: Vec<ServerConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ServerConfig {
    name: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pass_env: Vec<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    bearer_token_env_var: Option<String>,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    oauth: Option<OAuthConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OAuthConfig {
    #[serde(default)]
    client_id_env_var: Option<String>,
    #[serde(default)]
    client_secret_env_var: Option<String>,
}

/// Load and connect the configured stdio or remote MCP servers. A missing file means no MCP;
/// malformed configuration fails closed before any process is spawned or HTTP connection made.
pub async fn connect_project(workspace: &Path) -> Result<McpCatalog, ToolError> {
    let config_dir = crate::core::paths::resolve_workspace_config_dir(workspace);
    let path = config_dir.join("mcp.yaml");
    let config = match tokio::fs::read_to_string(&path).await {
        Ok(text) => {
            serde_yaml_ng::from_str::<McpConfig>(&text).map_err(|error| ToolError::Failed {
                code: ReasonCode::McpSchemaMismatch,
                detail: format!("{} is invalid: {error}", path.display()),
            })?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => McpConfig::default(),
        Err(error) => {
            return Err(ToolError::Failed {
                code: ReasonCode::McpServerUnavailable,
                detail: format!("could not read {}: {error}", path.display()),
            });
        }
    };
    validate_config(&config)?;

    let mut registry = ToolRegistry::builtins();
    let mut summaries = Vec::with_capacity(config.servers.len());
    for server in config.servers {
        match connect_server(server).await {
            Ok((tools, summary)) => {
                for tool in tools {
                    registry.add(tool);
                }
                summaries.push(summary);
            }
            Err(summary) => summaries.push(summary),
        }
    }
    Ok(McpCatalog {
        registry,
        servers: Arc::new(summaries),
    })
}

fn validate_config(config: &McpConfig) -> Result<(), ToolError> {
    let mut names = BTreeSet::new();
    for server in &config.servers {
        if server.name.is_empty()
            || !server
                .name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
            || !names.insert(server.name.as_str())
        {
            return Err(ToolError::Failed {
                code: ReasonCode::McpSchemaMismatch,
                detail: "MCP server names must be unique and use [A-Za-z0-9_-]".to_owned(),
            });
        }

        match (&server.command, &server.url) {
            (Some(cmd), None) => {
                if cmd.trim().is_empty() || !Path::new(cmd).is_absolute() {
                    return Err(ToolError::Failed {
                        code: ReasonCode::McpSchemaMismatch,
                        detail: "Stdio MCP server commands must be non-empty absolute paths"
                            .to_owned(),
                    });
                }
            }
            (None, Some(url)) => {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(ToolError::Failed {
                        code: ReasonCode::McpSchemaMismatch,
                        detail: "Remote MCP server URL must begin with http:// or https://"
                            .to_owned(),
                    });
                }
            }
            _ => {
                return Err(ToolError::Failed {
                    code: ReasonCode::McpSchemaMismatch,
                    detail: "MCP server must specify either command (stdio) or url (remote)"
                        .to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Resolve a remote server's bearer token from the named environment variable.
/// The token lives only in the environment — never in project YAML, a log, or
/// `/mcp`.
fn remote_auth_header(bearer_token_env_var: Option<&str>) -> Option<String> {
    let token = bearer_token_env_var.and_then(|var| std::env::var(var).ok());
    bearer_header(token.as_deref())
}

/// Format a resolved token as an `Authorization` value. A blank token is treated
/// as absent so a misconfigured variable does not send a bare `Bearer ` header.
fn bearer_header(token: Option<&str>) -> Option<String> {
    token
        .filter(|token| !token.trim().is_empty())
        .map(|token| format!("Bearer {token}"))
}

fn unavailable_summary(name: &str) -> McpServerSummary {
    McpServerSummary {
        name: name.to_owned(),
        state: McpConnectionState::Unavailable,
        tool_count: 0,
        tier: ToolTier::Execute,
        reason: Some(ReasonCode::McpServerUnavailable),
    }
}

async fn connect_server(
    config: ServerConfig,
) -> Result<(Vec<Arc<dyn Tool>>, McpServerSummary), McpServerSummary> {
    // `validate_config` guarantees exactly one of `url`/`command`, so `url`
    // present means remote and its absence means stdio.
    if config.url.is_some() {
        connect_remote(config).await
    } else {
        connect_stdio(config).await
    }
}

async fn connect_stdio(
    config: ServerConfig,
) -> Result<(Vec<Arc<dyn Tool>>, McpServerSummary), McpServerSummary> {
    let Some(ref cmd_str) = config.command else {
        return Err(unavailable_summary(&config.name));
    };
    let mut command = Command::new(cmd_str);
    command.args(&config.args).env_clear();
    for name in &config.pass_env {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let Ok((transport, _)) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
    else {
        return Err(unavailable_summary(&config.name));
    };
    let Ok(Ok(service)) = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport)).await else {
        return Err(unavailable_summary(&config.name));
    };
    register_served(service, config.name).await
}

/// Connect a remote HTTP (streamable) MCP server under the same governance the
/// stdio path uses. Rung (a): a bearer token, resolved from the
/// named environment variable at connect time and never logged, written to YAML,
/// or shown in `/mcp`, plus any explicit custom headers. A server that does not
/// answer — a bad URL, a refused credential, a hang — is reported `Unavailable`,
/// never a fabricated `Connected`.
async fn connect_remote(
    config: ServerConfig,
) -> Result<(Vec<Arc<dyn Tool>>, McpServerSummary), McpServerSummary> {
    let Some(url) = config.url.clone() else {
        return Err(unavailable_summary(&config.name));
    };
    let auth_header = remote_auth_header(config.bearer_token_env_var.as_deref());
    let mut custom_headers = std::collections::HashMap::new();
    for (key, value) in &config.headers {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) else {
            return Err(unavailable_summary(&config.name));
        };
        custom_headers.insert(name, value);
    }
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(url);
    transport_config.auth_header = auth_header;
    transport_config.custom_headers = custom_headers;
    let transport =
        StreamableHttpClientTransport::with_client(reqwest::Client::new(), transport_config);
    let Ok(Ok(service)) = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport)).await else {
        return Err(unavailable_summary(&config.name));
    };
    register_served(service, config.name).await
}

/// Discover a connected server's tools and register them namespaced at the
/// `Execute` tier — the one path both transports share, so a remote tool is
/// governed exactly as a stdio one.
async fn register_served(
    service: RunningService<RoleClient, ()>,
    server_name: String,
) -> Result<(Vec<Arc<dyn Tool>>, McpServerSummary), McpServerSummary> {
    let peer = service.peer().clone();
    let Ok(Ok(discovered)) = tokio::time::timeout(CONNECT_TIMEOUT, peer.list_all_tools()).await
    else {
        return Err(unavailable_summary(&server_name));
    };
    let lifetime = Arc::new(McpLifetime::new(service.cancellation_token()));
    tokio::spawn(async move {
        let _ = service.waiting().await;
    });
    let discovered_count = discovered.len();
    let mut tools: Vec<Arc<dyn Tool>> = Vec::with_capacity(discovered_count);
    for tool in discovered {
        let schema = serde_json::Value::Object((*tool.input_schema).clone());
        if !jsonschema::meta::is_valid(&schema) || schema.to_string().contains("$ref") {
            continue;
        }
        tools.push(Arc::new(McpTool {
            name: format!("mcp:{server_name}:{}", tool.name),
            server: server_name.clone(),
            remote_name: tool.name.into_owned(),
            description: tool
                .description
                .map_or_else(|| "MCP tool".to_owned(), std::borrow::Cow::into_owned),
            schema,
            peer: peer.clone(),
            _lifetime: Arc::clone(&lifetime),
        }));
    }
    let summary = McpServerSummary {
        name: server_name,
        state: McpConnectionState::Connected,
        tool_count: tools.len(),
        tier: ToolTier::Execute,
        reason: (tools.len() != discovered_count).then_some(ReasonCode::McpSchemaMismatch),
    };
    Ok((tools, summary))
}

struct McpLifetime {
    cancellation: Mutex<Option<RunningServiceCancellationToken>>,
}

impl std::fmt::Debug for McpLifetime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpLifetime(<connection>)")
    }
}

impl McpLifetime {
    fn new(cancellation: RunningServiceCancellationToken) -> Self {
        Self {
            cancellation: Mutex::new(Some(cancellation)),
        }
    }
}

impl Drop for McpLifetime {
    fn drop(&mut self) {
        if let Ok(slot) = self.cancellation.get_mut()
            && let Some(cancellation) = slot.take()
        {
            cancellation.cancel();
        }
    }
}

#[derive(Debug)]
struct McpTool {
    name: String,
    server: String,
    remote_name: String,
    description: String,
    schema: serde_json::Value,
    peer: Peer<RoleClient>,
    _lifetime: Arc<McpLifetime>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

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
            "MCP server: {}\nTool: {}\nArguments: {}",
            self.server, self.remote_name, arguments
        ))
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let Some(arguments) = arguments.as_object().cloned() else {
            return Err(ToolError::Failed {
                code: ReasonCode::McpSchemaMismatch,
                detail: "MCP tool arguments must be an object".to_owned(),
            });
        };
        let request =
            CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);
        let result = tokio::select! {
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            result = tokio::time::timeout(context.command_timeout, self.peer.call_tool(request)) => {
                result.map_err(|_| ToolError::Failed {
                    code: ReasonCode::McpServerUnavailable,
                    detail: format!("MCP server {} did not answer before the deadline", self.server),
                })?.map_err(|_| ToolError::Failed {
                    code: ReasonCode::McpServerUnavailable,
                    detail: format!("MCP server {} became unavailable", self.server),
                })?
            },
        };
        let output = serde_json::to_string(&result).map_err(|_| ToolError::Failed {
            code: ReasonCode::McpSchemaMismatch,
            detail: "MCP result could not be represented safely".to_owned(),
        })?;
        if result.is_error == Some(true) {
            Ok(ToolResult::refused(ReasonCode::McpToolRefused, output))
        } else {
            Ok(ToolResult::ok(output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::core::tool::ReadSet;

    #[test]
    fn bearer_header_formats_a_token_and_treats_blanks_as_absent() {
        assert_eq!(bearer_header(None), None);
        assert_eq!(bearer_header(Some("  ")), None, "blank token is absent");
        assert_eq!(bearer_header(Some("")), None);
        assert_eq!(
            bearer_header(Some("s3cret")).as_deref(),
            Some("Bearer s3cret")
        );
    }

    #[test]
    fn remote_auth_header_is_absent_when_no_variable_is_named() {
        assert_eq!(remote_auth_header(None), None);
    }

    #[test]
    fn config_rejects_duplicate_or_ambiguous_names() {
        let config: McpConfig =
            serde_yaml_ng::from_str("servers:\n  - name: bad:name\n    command: echo\n")
                .expect("fixture");
        assert_eq!(
            validate_config(&config)
                .expect_err("must reject")
                .reason_code(),
            ReasonCode::McpSchemaMismatch
        );
    }

    #[test]
    fn mcp_execute_tier_uses_the_ordinary_policy_gate() {
        use crate::core::policy::PolicyMode;
        use crate::policy::{PolicyDecision, decide};

        assert_eq!(
            decide(PolicyMode::Ask, ToolTier::Execute, false),
            PolicyDecision::Ask
        );
        assert_eq!(
            decide(PolicyMode::ReadOnly, ToolTier::Execute, false),
            PolicyDecision::Deny(ReasonCode::PolicyReadOnly)
        );
        assert_eq!(
            decide(PolicyMode::FullAuto, ToolTier::Execute, false),
            PolicyDecision::Allow
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_server_lists_previews_and_runs_namespaced_tool() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mjolnr_dir = workspace.path().join(".mjolnr");
        std::fs::create_dir(&mjolnr_dir).expect("config dir");
        let script = workspace.path().join("server.sh");
        let body = r#"while IFS= read -r line; do
case "$line" in
*\"initialize\"*) printf '%s\n' '{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}' ;;
*\"tools/list\"*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"echo","description":"echo safely","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}' ;;
*\"tools/call\"*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"fixture-result"}]}}'; printf '%s\n' 'private-stderr-noise' >&2 ;;
esac
done"#;
        std::fs::write(&script, body).expect("script");
        std::fs::write(
            mjolnr_dir.join("mcp.yaml"),
            format!(
                "servers:\n  - name: fixture\n    command: /bin/sh\n    args:\n      - {}\n",
                script.display()
            ),
        )
        .expect("config");

        let catalog = connect_project(workspace.path()).await.expect("connect");
        assert_eq!(
            catalog.servers.first().expect("server").state,
            McpConnectionState::Connected
        );
        let tool = catalog
            .registry
            .get("mcp:fixture:echo")
            .expect("namespaced tool");
        assert_eq!(tool.tier(), ToolTier::Execute);
        let context = ToolContext {
            workspace_root: workspace.path().to_path_buf(),
            read_set: Arc::new(ReadSet::default()),
            max_output_bytes: 1024,
            command_timeout: Duration::from_secs(1),
        };
        let preview = tool
            .preview(&serde_json::json!({"text": "hello"}), &context)
            .await
            .expect("preview");
        assert!(preview.contains("fixture"));
        assert!(preview.contains("hello"));
        let result = tool
            .execute(
                serde_json::json!({"text": "hello"}),
                context,
                CancellationToken::new(),
            )
            .await
            .expect("call");
        assert!(result.content.contains("fixture-result"));
        assert!(!result.content.contains("private-stderr-noise"));
    }

    #[tokio::test]
    async fn unavailable_server_is_status_not_a_hang() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mjolnr_dir = workspace.path().join(".mjolnr");
        std::fs::create_dir(&mjolnr_dir).expect("config dir");
        let missing_command = if cfg!(windows) {
            r"C:\definitely\not\a\server.exe"
        } else {
            "/definitely/not/a/server"
        };
        std::fs::write(
            mjolnr_dir.join("mcp.yaml"),
            format!("servers:\n  - name: gone\n    command: '{missing_command}'\n"),
        )
        .expect("config");
        let catalog = connect_project(workspace.path()).await.expect("catalog");
        assert_eq!(
            catalog.servers.first().expect("server").state,
            McpConnectionState::Unavailable
        );
        assert!(catalog.registry.get("mcp:gone:anything").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn server_death_mid_call_is_typed_unavailable() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mjolnr_dir = workspace.path().join(".mjolnr");
        std::fs::create_dir(&mjolnr_dir).expect("config dir");
        let script = workspace.path().join("dying-server.sh");
        let body = r#"while IFS= read -r line; do
case "$line" in
*\"initialize\"*) printf '%s\n' '{"jsonrpc":"2.0","id":0,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}' ;;
*\"tools/list\"*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"die","inputSchema":{"type":"object"}}]}}' ;;
*\"tools/call\"*) exit 0 ;;
esac
done"#;
        std::fs::write(&script, body).expect("script");
        std::fs::write(
            mjolnr_dir.join("mcp.yaml"),
            format!(
                "servers:\n  - name: dying\n    command: /bin/sh\n    args:\n      - {}\n",
                script.display()
            ),
        )
        .expect("config");
        let catalog = connect_project(workspace.path()).await.expect("connect");
        let tool = catalog.registry.get("mcp:dying:die").expect("tool");
        let context = ToolContext {
            workspace_root: workspace.path().to_path_buf(),
            read_set: Arc::new(ReadSet::default()),
            max_output_bytes: 1024,
            command_timeout: Duration::from_secs(1),
        };
        let error = tool
            .execute(serde_json::json!({}), context, CancellationToken::new())
            .await
            .expect_err("dead server must fail");
        assert_eq!(error.reason_code(), ReasonCode::McpServerUnavailable);
    }
}
