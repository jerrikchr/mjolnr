//! Generic OpenAI-compatible chat-completions adapter (Phase 16).
//!
//! One wire implementation, many named endpoints. Unlike the bespoke MVP
//! adapters , the providers in [`CATALOG`] genuinely share the
//! OpenAI chat-completions SSE shape — same request body, same delta frames,
//! same `[DONE]` terminator — and differ only in base URL, credential, and
//! default model. Pretending each needs its own adapter would be the opposite
//! lie to the one §30.3 warns about.
//!
//! Base URLs may be overridden per provider with `SMED_<ID>_BASE_URL`
//! (hyphens become underscores), which is how account-scoped gateways and
//! non-default local ports are configured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::{FinishReason, ProviderEvent};
use crate::core::message::{ContentBlock, Role, ToolCall, ToolOutcome};
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId, Usage};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::{CredentialKind, ResolvedCredential, SecretError, SecretStore};

/// Whether an endpoint refuses unauthenticated requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatAuth {
    /// A missing or blank key is an auth error before any request is sent.
    Required,
    /// Local servers (vLLM, LM Studio) commonly run keyless; send the bearer
    /// header only when a key resolves.
    Optional,
}

/// A named OpenAI-compatible endpoint.
#[derive(Debug)]
pub struct CompatDescriptor {
    /// Stable provider id: credential key, routing target, ledger identity.
    pub id: &'static str,
    /// Human label used in model listings and protocol error details.
    pub label: &'static str,
    /// Default endpoint root (the adapter appends `/chat/completions`).
    /// Empty means "no usable default" — the URL is account-specific and must
    /// come from `SMED_<ID>_BASE_URL`.
    pub default_base_url: &'static str,
    /// Model requested when the user has not picked one.
    pub default_model: &'static str,
    /// Display name for the default model entry.
    pub model_display: &'static str,
    pub auth: CompatAuth,
}

/// Phase 16 catalog. Base URLs verified against each provider's published
/// OpenAI-compatible endpoint (2026-07-21); defaults borrowed from oh-my-pi's
/// catalog where smed has no prior opinion.
pub static CATALOG: &[CompatDescriptor] = &[
    CompatDescriptor {
        id: "nvidia",
        label: "NVIDIA NIM",
        default_base_url: "https://integrate.api.nvidia.com/v1",
        default_model: "nvidia/llama-3.1-nemotron-70b-instruct",
        model_display: "NVIDIA NIM Nemotron 70B",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "xai",
        label: "xAI",
        default_base_url: "https://api.x.ai/v1",
        default_model: "grok-4-fast-non-reasoning",
        model_display: "xAI Grok 4 Fast",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "opencode-zen",
        label: "OpenCode Zen",
        default_base_url: "https://opencode.ai/zen/v1",
        default_model: "claude-opus-4-8",
        model_display: "OpenCode Zen default route",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "opencode-go",
        label: "OpenCode Go",
        default_base_url: "https://opencode.ai/zen/go/v1",
        default_model: "kimi-k2.7-code",
        model_display: "OpenCode Go default route",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "vercel-gateway",
        label: "Vercel AI Gateway",
        default_base_url: "https://ai-gateway.vercel.sh/v1",
        default_model: "anthropic/claude-opus-4.8",
        model_display: "Vercel AI Gateway default route",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "cloudflare-gateway",
        label: "Cloudflare AI Gateway",
        // Gateway URLs embed the account and gateway name; there is no
        // meaningful default. Set SMED_CLOUDFLARE_GATEWAY_BASE_URL to the
        // gateway's OpenAI-compatible endpoint.
        default_base_url: "",
        default_model: "anthropic/claude-opus-4-8",
        model_display: "Cloudflare AI Gateway default route",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "vllm",
        label: "vLLM",
        default_base_url: "http://localhost:8000/v1",
        default_model: "gpt-oss-20b",
        model_display: "vLLM local server",
        auth: CompatAuth::Optional,
    },
    CompatDescriptor {
        id: "lm-studio",
        label: "LM Studio",
        default_base_url: "http://localhost:1234/v1",
        default_model: "llama-3-8b",
        model_display: "LM Studio local server",
        auth: CompatAuth::Optional,
    },
    CompatDescriptor {
        id: "deepseek",
        label: "DeepSeek",
        default_base_url: "https://api.deepseek.com/v1",
        default_model: "deepseek-chat",
        model_display: "DeepSeek V3 Chat",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "mistral",
        label: "Mistral AI",
        default_base_url: "https://api.mistral.ai/v1",
        default_model: "mistral-large-latest",
        model_display: "Mistral Large",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "groq",
        label: "Groq",
        default_base_url: "https://api.groq.com/openai/v1",
        default_model: "llama-3.3-70b-versatile",
        model_display: "Groq Llama 3.3 70B",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "together",
        label: "Together AI",
        default_base_url: "https://api.together.xyz/v1",
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        model_display: "Together Llama 3.3 70B",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "fireworks",
        label: "Fireworks AI",
        default_base_url: "https://api.fireworks.ai/inference/v1",
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        model_display: "Fireworks Llama 3.3 70B",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "perplexity",
        label: "Perplexity AI",
        default_base_url: "https://api.perplexity.ai",
        default_model: "sonar-pro",
        model_display: "Perplexity Sonar Pro",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "moonshot",
        label: "Moonshot / Kimi",
        default_base_url: "https://api.moonshot.cn/v1",
        default_model: "moonshot-v1-8k",
        model_display: "Moonshot Kimi v1 8K",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "zhipu",
        label: "Zhipu / GLM",
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        default_model: "glm-4-flash",
        model_display: "GLM-4 Flash",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "qwen",
        label: "Qwen / DashScope",
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        default_model: "qwen-max",
        model_display: "Qwen Max",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "huggingface",
        label: "Hugging Face",
        // The HF Inference Providers router speaks the OpenAI wire; per-model
        // routing lives in the model id, so one base URL serves the catalogue.
        default_base_url: "https://router.huggingface.co/v1",
        default_model: "meta-llama/Llama-3.3-70B-Instruct",
        model_display: "Llama 3.3 70B (HF router)",
        auth: CompatAuth::Required,
    },
    CompatDescriptor {
        id: "tokenrouter",
        label: "TokenRouter",
        // Verified 2026-07-31 against TokenRouter's published quick-start.
        default_base_url: "https://api.tokenrouter.com/v1",
        // A free-tier promotional route at the time this was added — the
        // owner's own choice, not a smed default; TokenRouter's own docs
        // warn free capacity is unguaranteed and can vanish without notice.
        default_model: "moonshotai/kimi-k3-free",
        model_display: "Kimi K3 (TokenRouter free tier)",
        auth: CompatAuth::Required,
    },
];

/// The environment variable that overrides a descriptor's base URL.
#[must_use]
pub fn base_url_variable(id: &str) -> String {
    format!("SMED_{}_BASE_URL", id.to_uppercase().replace('-', "_"))
}

const LM_STUDIO_ID: &str = "lm-studio";
const LM_STUDIO_CONFIG_FILE: &str = "lm-studio.url";

fn environment_base_url(descriptor: &CompatDescriptor) -> Option<String> {
    std::env::var(base_url_variable(descriptor.id))
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
}

/// The diffable project setting used by `smed auth login lm-studio`.
#[must_use]
pub fn lm_studio_config_path(workspace_root: &Path) -> PathBuf {
    crate::core::paths::resolve_workspace_config_dir(workspace_root)
        .join("providers")
        .join(LM_STUDIO_CONFIG_FILE)
}

/// Resolve LM Studio's current endpoint for an interactive setup prompt.
///
/// This synchronous helper is used only while the TUI is suspended or by the
/// standalone auth command. Provider requests use the cancellable async reader.
pub fn configured_lm_studio_base_url(workspace_root: &Path) -> Result<String, String> {
    if let Some(value) = environment_base_url(
        CATALOG
            .iter()
            .find(|descriptor| descriptor.id == LM_STUDIO_ID)
            .ok_or_else(|| "LM Studio is absent from the provider catalog".to_owned())?,
    ) {
        return normalize_lm_studio_base_url(&value);
    }
    let path = lm_studio_config_path(workspace_root);
    match std::fs::read_to_string(&path) {
        Ok(value) => normalize_lm_studio_base_url(&value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok("http://localhost:1234/v1".to_owned())
        }
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/// Turn an LM Studio host, IP, or URL into the OpenAI-compatible `/v1` root.
///
/// A bare host or IP receives the documented LM Studio port. Other paths are
/// rejected because silently appending chat routes to an arbitrary path would
/// make the saved configuration look valid while every request fails.
pub fn normalize_lm_studio_base_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let supplied_scheme = trimmed.contains("://");
    let candidate = if supplied_scheme {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    let mut url = reqwest::Url::parse(&candidate)
        .map_err(|error| format!("invalid LM Studio server address: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("LM Studio server address must use http or https".to_owned());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "LM Studio server address cannot contain credentials, a query, or a fragment"
                .to_owned(),
        );
    }
    if !supplied_scheme && url.port().is_none() {
        url.set_port(Some(1234))
            .map_err(|()| "LM Studio server address cannot accept port 1234".to_owned())?;
    }
    match url.path().trim_end_matches('/') {
        "" | "/v1" => url.set_path("/v1"),
        _ => return Err("LM Studio server address path must be empty or /v1".to_owned()),
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Persist the non-secret LM Studio endpoint in the current project.
pub fn persist_lm_studio_base_url(workspace_root: &Path, input: &str) -> Result<String, String> {
    let normalized = normalize_lm_studio_base_url(input)?;
    let path = lm_studio_config_path(workspace_root);
    let parent = path
        .parent()
        .ok_or_else(|| "LM Studio configuration path has no parent".to_owned())?;
    let canonical_root = std::fs::canonicalize(workspace_root)
        .map_err(|error| format!("could not resolve {}: {error}", workspace_root.display()))?;
    let smed_dir = crate::core::paths::resolve_workspace_config_dir(workspace_root);
    if smed_dir.exists() {
        let resolved = std::fs::canonicalize(&smed_dir)
            .map_err(|error| format!("could not resolve {}: {error}", smed_dir.display()))?;
        if !resolved.starts_with(&canonical_root) {
            return Err(
                "workspace config directory resolves outside the current project".to_owned(),
            );
        }
    }
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    let resolved_parent = std::fs::canonicalize(parent)
        .map_err(|error| format!("could not resolve {}: {error}", parent.display()))?;
    if !resolved_parent.starts_with(&canonical_root) {
        return Err("LM Studio configuration directory resolves outside the project".to_owned());
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("LM Studio configuration file cannot be a symlink".to_owned());
    }
    std::fs::write(&path, format!("{normalized}\n"))
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(normalized)
}

#[derive(Debug)]
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    descriptor: &'static CompatDescriptor,
    base_url_override: Option<String>,
    workspace_root: Option<PathBuf>,
    secrets: Arc<dyn SecretStore>,
}

impl OpenAiCompatProvider {
    #[must_use]
    pub fn new(descriptor: &'static CompatDescriptor, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url_override: None,
            workspace_root: None,
            descriptor,
            secrets,
        }
    }

    /// Construct a provider that also observes diffable project endpoint
    /// configuration. Only LM Studio currently has such a file.
    #[must_use]
    pub fn for_workspace(
        descriptor: &'static CompatDescriptor,
        secrets: Arc<dyn SecretStore>,
        workspace_root: &Path,
    ) -> Self {
        let mut provider = Self::new(descriptor, secrets);
        provider.workspace_root = Some(workspace_root.to_path_buf());
        provider
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url_override = Some(base_url.into());
        self
    }

    async fn base_url(&self, cancel: &CancellationToken) -> Result<String, ProviderError> {
        if let Some(base_url) = &self.base_url_override {
            return Ok(base_url.clone());
        }
        if let Some(base_url) = environment_base_url(self.descriptor) {
            return if self.descriptor.id == LM_STUDIO_ID {
                normalize_lm_studio_base_url(&base_url)
                    .map_err(|detail| ProviderError::Protocol { detail })
            } else {
                Ok(base_url)
            };
        }
        if self.descriptor.id == LM_STUDIO_ID
            && let Some(workspace_root) = &self.workspace_root
        {
            let path = lm_studio_config_path(workspace_root);
            let read = tokio::select! {
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                result = tokio::fs::read_to_string(&path) => result,
            };
            match read {
                Ok(value) => {
                    return normalize_lm_studio_base_url(&value)
                        .map_err(|detail| ProviderError::Protocol { detail });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ProviderError::Transport {
                        detail: format!(
                            "could not read LM Studio endpoint {}: {error}",
                            path.display()
                        ),
                    });
                }
            }
        }
        Ok(self.descriptor.default_base_url.to_owned())
    }

    /// Resolve the key off the async runtime; keychain access is blocking.
    /// Returns the whole resolved credential because [`Secret`] deliberately
    /// does not implement `Clone`.
    async fn api_key(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<ResolvedCredential>, ProviderError> {
        let secrets = Arc::clone(&self.secrets);
        let provider = self.id();
        let resolution =
            tokio::task::spawn_blocking(move || secrets.resolve(&provider, CredentialKind::ApiKey));
        let resolved = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = resolution => result.map_err(|error| ProviderError::Transport { detail: error.to_string() })?
        };
        match resolved {
            Ok(resolved) => match resolved.credential.api_key() {
                Some(secret) if !secret.is_blank() => Ok(Some(resolved)),
                _ if self.descriptor.auth == CompatAuth::Optional => Ok(None),
                _ => Err(ProviderError::Auth),
            },
            Err(SecretError::NotFound { .. }) if self.descriptor.auth == CompatAuth::Optional => {
                Ok(None)
            }
            Err(SecretError::NotFound { .. } | SecretError::KindMismatch { .. }) => {
                Err(ProviderError::Auth)
            }
            Err(SecretError::Unavailable { detail }) => Err(ProviderError::Transport { detail }),
        }
    }
}

#[derive(Debug, Serialize)]
struct RequestBody {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    stream: bool,
}
/// A chat-completions message's content: bare text, or parts when an image
/// rides along.
///
/// Untagged, so a text-only message serialises exactly as it always did.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CompatContent {
    Text(String),
    Parts(Vec<CompatPart>),
}

/// The de-facto data-URI part shape. **Inferred, not confirmed**
/// (`provider-contract.md` §5.5): "OpenAI-compatible" is a claim each endpoint
/// makes for itself, so this is opt-in per model rather than assumed.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum CompatPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    Image { image_url: CompatImageUrl },
}

#[derive(Debug, Serialize)]
struct CompatImageUrl {
    url: String,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: Option<CompatContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ToolCallWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}
#[derive(Debug, Serialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionDefinition,
}
#[derive(Debug, Serialize)]
struct FunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}
#[derive(Debug, Serialize, Deserialize)]
struct ToolCallWire {
    #[serde(default)]
    id: String,
    index: Option<usize>,
    function: FunctionCallWire,
}
#[derive(Debug, Serialize, Deserialize)]
struct FunctionCallWire {
    name: Option<String>,
    arguments: Option<String>,
}
#[derive(Debug, Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<UsageWire>,
    error: Option<ErrorWire>,
}
#[derive(Debug, Deserialize)]
struct Choice {
    delta: Option<Delta>,
    finish_reason: Option<String>,
}
#[derive(Debug, Deserialize)]
struct Delta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallWire>,
}
#[derive(Debug, Deserialize)]
struct UsageWire {
    prompt_tokens: u64,
    completion_tokens: u64,
}
#[derive(Debug, Deserialize)]
struct ErrorWire {
    code: Option<serde_json::Value>,
    message: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorWire,
}

#[derive(Debug, Deserialize)]
struct CompatModelsResponse {
    #[serde(default)]
    data: Vec<CompatModel>,
}

#[derive(Debug, Deserialize)]
struct CompatModel {
    id: String,
    name: Option<String>,
    context_length: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct LmStudioModelsResponse {
    #[serde(default)]
    models: Vec<LmStudioModel>,
}

#[derive(Debug, Deserialize)]
struct LmStudioModel {
    #[serde(rename = "type")]
    kind: String,
    key: String,
    display_name: String,
    max_context_length: Option<u32>,
    capabilities: Option<LmStudioCapabilities>,
}

#[derive(Debug, Deserialize)]
struct LmStudioCapabilities {
    #[serde(default)]
    vision: bool,
    #[serde(default)]
    trained_for_tool_use: bool,
    #[serde(default)]
    reasoning: serde_json::Value,
}

fn lm_studio_descriptors(
    payload: LmStudioModelsResponse,
    provider: &ProviderId,
) -> Vec<ModelDescriptor> {
    payload
        .models
        .into_iter()
        .filter_map(|model| {
            let capabilities = model.capabilities.unwrap_or(LmStudioCapabilities {
                vision: false,
                trained_for_tool_use: false,
                reasoning: serde_json::Value::Null,
            });
            if model.kind != "llm" || !capabilities.trained_for_tool_use {
                return None;
            }
            Some(ModelDescriptor {
                id: ModelId::new(model.key),
                provider: provider.clone(),
                display_name: model.display_name,
                capabilities: ModelCapabilities {
                    streaming: true,
                    tools: true,
                    structured_output: false,
                    images_in: capabilities.vision,
                    reasoning_controls: capabilities.reasoning != serde_json::Value::Null
                        && capabilities.reasoning != serde_json::Value::Bool(false),
                },
                context_tokens: model.max_context_length,
                max_output_tokens: None,
                tier: None,
            })
        })
        .collect()
}

fn is_relay_failure(provider: &str, status: reqwest::StatusCode, envelope: &ErrorEnvelope) -> bool {
    envelope.error.kind.as_deref() == Some("relay_error")
        || (provider == crate::providers::forge::PROVIDER_ID
            && status == reqwest::StatusCode::SERVICE_UNAVAILABLE
            && envelope.error.message.as_deref()
                == Some("No active upstream keys available. Please contact admin."))
}

fn translate(request: &ProviderRequest) -> RequestBody {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(Message {
            role: "system".to_owned(),
            content: Some(CompatContent::Text(system.clone())),
            tool_calls: Vec::new(),
            tool_call_id: None,
        });
    }
    for message in &request.messages {
        let role = match message.role {
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
            Role::User => "user",
        };
        let mut content = String::new();
        let mut calls = Vec::new();
        let mut result_id = None;
        let mut image_parts: Vec<CompatPart> = Vec::new();
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => content.push_str(text),
                ContentBlock::ToolCall(call) => calls.push(ToolCallWire {
                    id: call.id.clone(),
                    index: None,
                    function: FunctionCallWire {
                        name: Some(call.name.clone()),
                        arguments: Some(call.arguments.to_string()),
                    },
                }),
                ContentBlock::ToolResult {
                    call_id,
                    name,
                    result,
                } => {
                    result_id = Some(call_id.clone());
                    let status = match result.outcome {
                        ToolOutcome::Ok => "ok",
                        ToolOutcome::Refused(_) => "refused",
                        ToolOutcome::Failed(_) => "failed",
                    };
                    content.push_str(&serde_json::json!({"tool": name, "status": status, "content": result.content, "truncated": result.truncated}).to_string());
                }
                ContentBlock::ImageRef { source, .. } => {
                    if let Some(bytes) = request.images.get(source) {
                        image_parts.push(CompatPart::Image {
                            image_url: CompatImageUrl {
                                url: bytes.data_uri(),
                            },
                        });
                    }
                }
            }
        }
        if !content.is_empty() || !calls.is_empty() || !image_parts.is_empty() {
            let body = if image_parts.is_empty() {
                (!content.is_empty()).then_some(CompatContent::Text(content))
            } else {
                let mut parts = image_parts;
                if !content.is_empty() {
                    parts.push(CompatPart::Text { text: content });
                }
                Some(CompatContent::Parts(parts))
            };
            messages.push(Message {
                role: role.to_owned(),
                content: body,
                tool_calls: calls,
                tool_call_id: result_id,
            });
        }
    }
    let tools = request
        .tools
        .iter()
        .map(|tool| Tool {
            kind: "function",
            function: FunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.schema.clone(),
            },
        })
        .collect();
    RequestBody {
        model: request.model.to_string(),
        messages,
        tools,
        stream: true,
    }
}

async fn emit(
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    event: ProviderEvent,
) -> Result<(), ProviderError> {
    tokio::select! { () = cancel.cancelled() => Err(ProviderError::Cancelled), result = events.send(event) => result.map_err(|_| ProviderError::Cancelled) }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.descriptor.id)
    }

    fn credentialed(&self) -> bool {
        self.descriptor.auth == CompatAuth::Optional
            || self
                .secrets
                .resolve(&self.id(), CredentialKind::ApiKey)
                .is_ok()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(self.descriptor.default_model),
            provider: self.id(),
            display_name: self.descriptor.model_display.to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: None,
            max_output_tokens: None,
            tier: None,
        }]
    }

    async fn discover_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        let base_url = self.base_url(&cancel).await?;
        if base_url.is_empty() {
            return Err(ProviderError::Protocol {
                detail: format!(
                    "{} has no endpoint; set {}",
                    self.descriptor.label,
                    base_url_variable(self.descriptor.id)
                ),
            });
        }
        let key = self.api_key(&cancel).await?;
        let url = if self.descriptor.id == "lm-studio" {
            format!("{}/api/v1/models", base_url.trim_end_matches("/v1"))
        } else {
            format!("{base_url}/models")
        };
        let mut builder = self.client.get(url);
        if let Some(secret) = key
            .as_ref()
            .and_then(|resolved| resolved.credential.api_key())
        {
            builder = builder.bearer_auth(secret.expose());
        }
        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = builder.send() => result.map_err(|error| ProviderError::Transport {
                detail: error.without_url().to_string(),
            })?,
        };
        let status = response.status();
        if !status.is_success() {
            return Err(match status {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                    ProviderError::Auth
                }
                reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit {
                    retry_after_seconds: None,
                },
                _ => ProviderError::Protocol {
                    detail: format!("{} model discovery http {}", self.descriptor.label, status),
                },
            });
        }

        if self.descriptor.id == "lm-studio" {
            let payload = response
                .json::<LmStudioModelsResponse>()
                .await
                .map_err(|error| ProviderError::Protocol {
                    detail: format!("invalid LM Studio model catalog: {error}"),
                })?;
            return Ok(lm_studio_descriptors(payload, &self.id()));
        }

        let payload = response
            .json::<CompatModelsResponse>()
            .await
            .map_err(|error| ProviderError::Protocol {
                detail: format!("invalid {} model catalog: {error}", self.descriptor.label),
            })?;
        let discovered = payload
            .data
            .into_iter()
            .map(|model| ModelDescriptor {
                id: ModelId::new(&model.id),
                provider: self.id(),
                display_name: model.name.unwrap_or(model.id),
                capabilities: ModelCapabilities::text_and_tools(),
                context_tokens: model.context_length,
                max_output_tokens: None,
                tier: None,
            })
            .collect::<Vec<_>>();
        if self.descriptor.id == crate::providers::forge::PROVIDER_ID {
            // Forge's wrapper intersects these names with its larger reviewed
            // capability table. Returning raw rows here does not publish them.
            return Ok(discovered);
        }

        // A generic `/models` row proves availability, not smed's required
        // streaming + tool contract. Keep only the descriptor this named
        // adapter has reviewed; provider-specific adapters may apply richer
        // official metadata or their own curated intersection.
        let known = self.models();
        Ok(discovered
            .into_iter()
            .filter_map(|model| {
                known
                    .iter()
                    .find(|candidate| candidate.id == model.id)
                    .cloned()
            })
            .collect())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "SSE routing and indexed tool assembly share one decoder state"
    )]
    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let label = self.descriptor.label;
        let base_url = self.base_url(&cancel).await?;
        if base_url.is_empty() {
            return Err(ProviderError::Protocol {
                detail: format!(
                    "{label} has no default endpoint; set {} to its OpenAI-compatible URL",
                    base_url_variable(self.descriptor.id)
                ),
            });
        }
        let key = self.api_key(&cancel).await?;
        let mut builder = self
            .client
            .post(format!("{base_url}/chat/completions"))
            .json(&translate(&request));
        if let Some(secret) = key
            .as_ref()
            .and_then(|resolved| resolved.credential.api_key())
        {
            builder = builder.bearer_auth(secret.expose());
        }
        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = builder.send() => result.map_err(|error| ProviderError::Transport { detail: error.without_url().to_string() })?
        };
        crate::providers::quota::emit_from_headers(self.id(), response.headers(), &events, &cancel)
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::PAYMENT_REQUIRED
                || (self.id().as_str() == crate::providers::forge::PROVIDER_ID
                    && status == reqwest::StatusCode::SERVICE_UNAVAILABLE)
            {
                let relay_failure = response
                    .json::<ErrorEnvelope>()
                    .await
                    .ok()
                    .is_some_and(|body| is_relay_failure(self.id().as_str(), status, &body));
                if relay_failure {
                    return Err(ProviderError::Relay);
                }
                return Err(ProviderError::Protocol {
                    detail: format!("http {}", status.as_u16()),
                });
            }
            return Err(match status {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                    ProviderError::Auth
                }
                reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit {
                    retry_after_seconds: None,
                },
                status => ProviderError::Protocol {
                    detail: format!("http {}", status.as_u16()),
                },
            });
        }
        let mut stream = response.bytes_stream().eventsource();
        emit(&events, &cancel, ProviderEvent::Started).await?;
        let mut pending: HashMap<usize, (String, String, String)> = HashMap::new();
        let mut usage = None;
        let mut saw_tool = false;
        loop {
            let Some(frame) = (tokio::select! { () = cancel.cancelled() => return Err(ProviderError::Cancelled), frame = stream.next() => frame })
            else {
                return Err(ProviderError::Protocol {
                    detail: format!("{label} stream ended without [DONE]"),
                });
            };
            let frame = frame.map_err(|error| ProviderError::Transport {
                detail: error.to_string(),
            })?;
            if frame.data.trim() == "[DONE]" {
                continue;
            }
            let chunk: Chunk = serde_json::from_str(&frame.data).map_err(|error| {
                let keys = serde_json::from_str::<serde_json::Value>(&frame.data)
                    .ok()
                    .and_then(|value| {
                        value
                            .as_object()
                            .map(|object| object.keys().cloned().collect::<Vec<_>>().join(", "))
                    })
                    .unwrap_or_else(|| "non-object".to_owned());
                ProviderError::Protocol {
                    detail: format!("malformed {label} chunk ({keys}): {error}"),
                }
            })?;
            if let Some(error) = chunk.error {
                let detail = error
                    .code
                    .map(|code| code.to_string())
                    .or(error.message)
                    .unwrap_or_else(|| "stream error".to_owned());
                emit(
                    &events,
                    &cancel,
                    ProviderEvent::Failed {
                        detail: detail.clone(),
                    },
                )
                .await?;
                return Err(ProviderError::Protocol { detail });
            }
            if let Some(wire_usage) = chunk.usage {
                usage = Some(Usage {
                    input_tokens: wire_usage.prompt_tokens,
                    output_tokens: wire_usage.completion_tokens,
                });
            }
            for choice in chunk.choices {
                if let Some(delta) = choice.delta {
                    if let Some(text) = delta.content {
                        emit(&events, &cancel, ProviderEvent::TextDelta { text }).await?;
                    }
                    for call in delta.tool_calls {
                        saw_tool = true;
                        let index = call.index.unwrap_or(0);
                        let entry = pending.entry(index).or_insert_with(|| {
                            (
                                call.id.clone(),
                                call.function.name.clone().unwrap_or_default(),
                                String::new(),
                            )
                        });
                        if !call.id.is_empty() {
                            entry.0 = call.id;
                        }
                        if let Some(name) = call.function.name {
                            entry.1 = name;
                        }
                        if let Some(arguments) = call.function.arguments {
                            emit(
                                &events,
                                &cancel,
                                ProviderEvent::ToolArgumentsDelta {
                                    id: entry.0.clone(),
                                    fragment: arguments.clone(),
                                },
                            )
                            .await?;
                            entry.2.push_str(&arguments);
                        }
                    }
                }
                if let Some(reason) = choice.finish_reason {
                    if reason == "error" {
                        return Err(ProviderError::Protocol {
                            detail: format!("{label} stream reported finish_reason=error"),
                        });
                    }
                    let finish = if saw_tool {
                        FinishReason::ToolCalls
                    } else if reason == "length" {
                        FinishReason::Incomplete
                    } else {
                        FinishReason::Stop
                    };
                    for (_, (id, name, args)) in pending.drain() {
                        let arguments = serde_json::from_str(&args).map_err(|error| {
                            ProviderError::Protocol {
                                detail: format!(
                                    "{label} tool arguments invalid at finish: {error}"
                                ),
                            }
                        })?;
                        emit(
                            &events,
                            &cancel,
                            ProviderEvent::ToolCallCompleted {
                                call: ToolCall {
                                    id,
                                    name,
                                    arguments,
                                    provider_signature: None,
                                },
                            },
                        )
                        .await?;
                    }
                    if let Some(usage) = usage {
                        emit(&events, &cancel, ProviderEvent::Usage { usage }).await?;
                    }
                    emit(&events, &cancel, ProviderEvent::Finished { reason: finish }).await?;
                    return Ok(ProviderCompletion {
                        reason: finish,
                        usage,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_stable() {
        let mut seen = std::collections::HashSet::new();
        for descriptor in CATALOG {
            assert!(seen.insert(descriptor.id), "duplicate id {}", descriptor.id);
            assert_eq!(
                descriptor.id,
                descriptor.id.to_lowercase(),
                "ids are lowercase so credential keys and env vars stay predictable"
            );
        }
    }

    #[test]
    fn remote_endpoints_are_https_and_local_ones_are_loopback() {
        for descriptor in CATALOG {
            if descriptor.default_base_url.is_empty() {
                continue;
            }
            let local = descriptor.default_base_url.starts_with("http://localhost");
            assert!(
                local || descriptor.default_base_url.starts_with("https://"),
                "{} would send a bearer key over cleartext",
                descriptor.id
            );
            if local {
                assert_eq!(
                    descriptor.auth,
                    CompatAuth::Optional,
                    "{} is local and should not demand a key",
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn base_url_variables_are_valid_shell_identifiers() {
        for descriptor in CATALOG {
            let variable = base_url_variable(descriptor.id);
            assert!(
                variable
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{variable} is not settable from a POSIX shell"
            );
        }
    }

    #[test]
    fn lm_studio_address_accepts_an_ip_or_full_v1_url() {
        assert_eq!(
            normalize_lm_studio_base_url("192.168.1.40").unwrap(),
            "http://192.168.1.40:1234/v1"
        );
        assert_eq!(
            normalize_lm_studio_base_url("http://10.0.0.2:5555/v1/").unwrap(),
            "http://10.0.0.2:5555/v1"
        );
        assert!(normalize_lm_studio_base_url("file:///tmp/socket").is_err());
        assert!(normalize_lm_studio_base_url("http://host:1234/not-v1").is_err());
    }

    #[test]
    fn lm_studio_address_is_saved_as_diffable_project_configuration() {
        let workspace = tempfile::tempdir().unwrap();
        let normalized = persist_lm_studio_base_url(workspace.path(), "127.0.0.1:4321").unwrap();
        assert_eq!(normalized, "http://127.0.0.1:4321/v1");
        assert_eq!(
            std::fs::read_to_string(lm_studio_config_path(workspace.path())).unwrap(),
            "http://127.0.0.1:4321/v1\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn lm_studio_address_refuses_a_smed_directory_outside_the_project() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), workspace.path().join(".mjolnr")).unwrap();

        assert!(
            persist_lm_studio_base_url(workspace.path(), "localhost").is_err(),
            "declared configuration must not follow .mjolnr outside the project"
        );
    }
}
