//! Forge gateway adapter — an OpenAI-compatible aggregator for premium model
//! providers. Since Phase 16, a named instance of the generic OpenAI-compatible
//! adapter in [`super::openai_compat`].

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::SecretStore;
use crate::providers::openai_compat::{CompatAuth, CompatDescriptor, OpenAiCompatProvider};

pub const PROVIDER_ID: &str = "forge";
pub const DEFAULT_MODEL: &str = "claude-sonnet-5";

/// Curated Forge models for coding tasks.
///
/// Format: (`model_id`, `display_name`, `context_tokens`, `max_output_tokens`)
const MODELS: &[(&str, &str, u32, u32)] = &[
    // Anthropic
    ("claude-sonnet-5", "Claude Sonnet 5", 1_000_000, 64_000),
    ("claude-sonnet-4-6", "Claude Sonnet 4.6", 1_000_000, 64_000),
    (
        "claude-sonnet-4-6-thinking",
        "Claude Sonnet 4.6 Thinking",
        1_000_000,
        64_000,
    ),
    (
        "claude-opus-4-5-20251101",
        "Claude Opus 4.5",
        200_000,
        64_000,
    ),
    (
        "claude-sonnet-4-5-20250929",
        "Claude Sonnet 4.5",
        200_000,
        64_000,
    ),
    (
        "claude-haiku-4-5-20251001",
        "Claude Haiku 4.5",
        200_000,
        64_000,
    ),
    // OpenAI
    ("gpt-5.6-luna", "GPT-5.6 Luna", 1_050_000, 32_768),
    ("gpt-5.6-sol", "GPT-5.6 Sol", 1_050_000, 32_768),
    ("gpt-5.6-terra", "GPT-5.6 Terra", 1_050_000, 32_768),
    ("gpt-5.5", "GPT-5.5", 128_000, 32_768),
    ("gpt-5.3-codex", "GPT-5.3 Codex", 400_000, 32_768),
    // DeepSeek
    ("deepseek-r1", "DeepSeek R1", 128_000, 65_536),
    ("deepseek-v4-flash", "DeepSeek V4 Flash", 1_000_000, 65_536),
    ("deepseek-v4-pro", "DeepSeek V4 Pro", 1_000_000, 65_536),
    ("deepseek-v3.2", "DeepSeek V3.2", 163_000, 65_536),
    ("deepseek-v3.1", "DeepSeek V3.1", 128_000, 65_536),
    ("deepseek-v3", "DeepSeek V3", 128_000, 65_536),
    // xAI
    ("grok-4.5", "Grok 4.5", 500_000, 32_768),
    ("grok-4.3", "Grok 4.3", 1_000_000, 32_768),
    ("grok-build-0.1", "Grok Build 0.1", 256_000, 32_768),
    // Google
    ("gemini-3-pro-preview", "Gemini 3 Pro", 1_000_000, 65_536),
    ("gemini-3.5-flash", "Gemini 3.5 Flash", 1_000_000, 65_536),
    // Moonshot
    ("kimi-k3", "Kimi K3", 1_000_000, 65_536),
    ("kimi-k2.7-code", "Kimi K2.7 Code", 256_000, 65_536),
    ("kimi-k2.6", "Kimi K2.6", 262_000, 65_536),
    ("kimi-k2.5", "Kimi K2.5", 256_000, 65_536),
    // Tencent
    ("tencent/hy3", "Tencent HY3", 262_000, 32_768),
    // Xiaomi
    ("mimo-v2.5", "Mimo V2.5", 1_000_000, 32_768),
    ("mimo-v2.5-pro", "Mimo V2.5 Pro", 1_000_000, 32_768),
    // MiniMax
    ("MiniMax-M3", "MiniMax M3", 1_000_000, 32_768),
    ("MiniMax-M2.5", "MiniMax M2.5", 204_000, 32_768),
    // GLM
    ("glm-5.2", "GLM 5.2", 1_000_000, 32_768),
];

static DESCRIPTOR: CompatDescriptor = CompatDescriptor {
    id: PROVIDER_ID,
    label: "Forge",
    default_base_url: "https://forge-gateway-api.fly.dev/v1",
    default_model: DEFAULT_MODEL,
    model_display: "Forge Claude Sonnet 5",
    auth: CompatAuth::Required,
    // The inner compat adapter must return raw rows so the reviewed
    // capability table below can do the intersecting.
    catalog_trust: crate::providers::openai_compat::CatalogTrust::ReviewedOnly,
};

#[derive(Debug)]
pub struct ForgeProvider {
    inner: OpenAiCompatProvider,
}

impl ForgeProvider {
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(&DESCRIPTOR, secrets),
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.inner = self.inner.with_base_url(base_url);
        self
    }
}

#[async_trait]
impl Provider for ForgeProvider {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    fn credentialed(&self) -> bool {
        self.inner.credentialed()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        MODELS
            .iter()
            .map(|(id, display_name, context, max_output)| {
                let id = ModelId::new(*id);
                let provider = self.id();
                ModelDescriptor {
                    tier: crate::core::model::ModelTier::curated(&provider, &id),
                    id,
                    provider,
                    display_name: (*display_name).to_owned(),
                    capabilities: ModelCapabilities::text_and_tools(),
                    context_tokens: Some(*context),
                    max_output_tokens: Some(*max_output),
                }
            })
            .collect()
    }

    async fn discover_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        let discovered = self.inner.discover_models(cancel).await?;
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

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        self.inner.stream(request, events, cancel).await
    }
}
