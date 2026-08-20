//! Opt-in live Anthropic smoke test.
//!
//! ```text
//! ANTHROPIC_API_KEY=... cargo test --test live_anthropic -- --ignored --nocapture
//! ```

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::provider::{Provider, ProviderRequest};
use mjolnr::core::secrets::{
    Credential, CredentialKind, ResolvedCredential, Secret, SecretError, SecretSource, SecretStore,
};
use mjolnr::providers::anthropic::{AnthropicProvider, DEFAULT_MODEL};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct EnvironmentOnly;

impl SecretStore for EnvironmentOnly {
    fn resolve(
        &self,
        provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        std::env::var("ANTHROPIC_API_KEY").map_or_else(
            |_| {
                Err(SecretError::NotFound {
                    provider: provider.clone(),
                })
            },
            |value| {
                Ok(ResolvedCredential {
                    credential: Credential::ApiKey(Secret::new(value)),
                    source: SecretSource::Environment,
                })
            },
        )
    }

    fn store(&self, _provider: &ProviderId, _credential: Credential) -> Result<(), SecretError> {
        Err(SecretError::Unavailable {
            detail: "live test is read-only".to_owned(),
        })
    }

    fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
        Err(SecretError::Unavailable {
            detail: "live test is read-only".to_owned(),
        })
    }
}

#[tokio::test]
#[ignore = "requires ANTHROPIC_API_KEY and spends live credits"]
async fn live_anthropic_text_stream_completes() {
    let provider = AnthropicProvider::new(Arc::new(EnvironmentOnly));
    let request = ProviderRequest {
        model: ModelId::new(DEFAULT_MODEL),
        messages: vec![mjolnr::core::message::CanonicalMessage::user(
            "Reply with exactly: mjolnr-anthropic-live-ok",
        )],
        system: None,
        tools: Vec::new(),
        images: mjolnr::core::image::ImageSidecar::new(),
    };
    let (tx, mut rx) = mpsc::channel(64);
    let task =
        tokio::spawn(async move { provider.stream(request, tx, CancellationToken::new()).await });
    let mut text = String::new();
    while let Some(event) = rx.recv().await {
        if let mjolnr::core::event::ProviderEvent::TextDelta { text: delta } = event {
            text.push_str(&delta);
        }
    }
    let completion = task.await.expect("task").expect("live request");
    assert_eq!(completion.reason, mjolnr::core::event::FinishReason::Stop);
    assert!(
        text.contains("mjolnr-anthropic-live-ok"),
        "response: {text}"
    );
}
