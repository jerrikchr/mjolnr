//! OpenRouter chat-completions adapter (Phase 7) — since Phase 16, a named
//! instance of the generic OpenAI-compatible adapter in [`super::openai_compat`].

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::model::{ModelDescriptor, ProviderId};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::SecretStore;
use crate::providers::openai_compat::{CompatAuth, CompatDescriptor, OpenAiCompatProvider};

pub const PROVIDER_ID: &str = "openrouter";
pub const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";

static DESCRIPTOR: CompatDescriptor = CompatDescriptor {
    id: PROVIDER_ID,
    label: "OpenRouter",
    default_base_url: "https://openrouter.ai/api/v1",
    default_model: DEFAULT_MODEL,
    model_display: "OpenRouter GPT-4o mini route",
    auth: CompatAuth::Required,
};

#[derive(Debug)]
pub struct OpenRouterProvider {
    inner: OpenAiCompatProvider,
}

impl OpenRouterProvider {
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
impl Provider for OpenRouterProvider {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    fn credentialed(&self) -> bool {
        self.inner.credentialed()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        self.inner.models()
    }

    async fn discover_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, crate::core::error::ProviderError> {
        self.inner.discover_models(cancel).await
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
