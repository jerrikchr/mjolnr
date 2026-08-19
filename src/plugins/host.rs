//! Plugin Host managing lifecycle, tool execution, and observer hooks (ADR-0016).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::error::ToolError;
use crate::core::plugin::{
    PLUGIN_PROTOCOL_VERSION, PluginHook, PluginManifest, PluginObserverResult,
};
use crate::plugins::transport::PluginTransport;

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// An active plugin host instance managing an isolated subprocess.
#[derive(Debug, Clone)]
pub struct PluginHost {
    manifest: Arc<PluginManifest>,
    transport: Arc<Mutex<PluginTransport>>,
    subscribed_hooks: BTreeSet<PluginHook>,
}

impl PluginHost {
    /// Start a plugin subprocess, initialize JSON-RPC handshake, and verify protocol compatibility.
    pub async fn start(
        manifest: PluginManifest,
        workspace: &Path,
        granted_credentials: BTreeMap<String, String>,
        cancel: CancellationToken,
    ) -> Result<Self, ToolError> {
        let subscribed_hooks: BTreeSet<PluginHook> = manifest.hooks.iter().copied().collect();
        let mut transport = PluginTransport::spawn(
            &manifest.run.program,
            &manifest.run.arguments,
            workspace,
            &granted_credentials,
            cancel,
        )?;

        // Initialize handshake
        let init_params = serde_json::json!({
            "client_version": env!("CARGO_PKG_VERSION"),
            "protocol_version": PLUGIN_PROTOCOL_VERSION,
            "plugin_name": manifest.name,
        });
        let _ = transport
            .call("initialize", init_params, INITIALIZE_TIMEOUT)
            .await?;

        Ok(Self {
            manifest: Arc::new(manifest),
            transport: Arc::new(Mutex::new(transport)),
            subscribed_hooks,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    #[must_use]
    pub fn is_subscribed_to(&self, hook: PluginHook) -> bool {
        self.subscribed_hooks.contains(&hook)
    }

    /// Dispatch an observer hook to the plugin subprocess (ADR-0016 §5).
    ///
    /// Observer hooks receive structured context and return annotations/notices.
    /// If the plugin fails or times out, the error is swallowed and logged so
    /// a third-party plugin cannot fail the primary agent run.
    pub async fn notify_hook(
        &self,
        hook: PluginHook,
        payload: serde_json::Value,
    ) -> Option<PluginObserverResult> {
        if !self.is_subscribed_to(hook) {
            return None;
        }

        let method = match hook {
            PluginHook::SessionStart => "session_start",
            PluginHook::UserPromptSubmit => "user_prompt_submit",
            PluginHook::PreToolCall => "pre_tool_call",
            PluginHook::PostToolCall => "post_tool_call",
            PluginHook::PostTurn => "post_turn",
        };

        let mut transport = self.transport.lock().await;
        match transport.call(method, payload, HOOK_TIMEOUT).await {
            Ok(result) => serde_json::from_value::<PluginObserverResult>(result).ok(),
            Err(_) => None,
        }
    }

    /// Call a declared tool on the plugin subprocess (ADR-0016 §3).
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ToolError> {
        self.call_tool_with_cancel(tool_name, arguments, timeout, CancellationToken::new())
            .await
    }

    /// Call a declared tool on the plugin subprocess with per-call cancellation token.
    pub async fn call_tool_with_cancel(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, ToolError> {
        let is_declared = self
            .manifest
            .tools
            .iter()
            .any(|tool| tool.name == tool_name);
        if !is_declared {
            return Err(ToolError::Execution {
                detail: format!(
                    "plugin `{}` does not declare tool `{tool_name}`",
                    self.manifest.name
                ),
            });
        }

        let params = serde_json::json!({
            "name": tool_name,
            "parameters": arguments,
        });

        let mut transport = self.transport.lock().await;
        transport
            .call_with_cancel("call_tool", params, timeout, cancel)
            .await
    }

    /// Cleanly shut down the plugin subprocess.
    pub async fn shutdown(&self) -> Result<(), ToolError> {
        let mut transport = self.transport.lock().await;
        transport.shutdown().await
    }
}
