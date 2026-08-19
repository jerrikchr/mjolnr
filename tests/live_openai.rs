//! Live OpenAI smoke test.
//!
//! **`#[ignore]`d. Never runs in normal CI, and never required to pass.**
//! Contract tests (`tests/contract_openai.rs`) are mandatory and offline; this
//! exists to catch provider drift that a fixture cannot, because a fixture is a
//! snapshot of a contract that will change.
//!
//! # Running it
//!
//! ```bash
//! export OPENAI_API_KEY=sk-...      # real key; costs a few tokens
//! cargo test --test live_openai -- --ignored --nocapture
//! ```
//!
//! It sends one tiny prompt to the cheapest model and asserts the shape of what
//! comes back, not the content — a model is free to answer however it likes.
//!
//! # If this fails but the contract tests pass
//!
//! **The provider is right and our fixtures are stale.** Recapture, update
//! `docs/provider-contract.md`, and note the drift in the report. Do not
//! edit a fixture to match current behaviour — that inverts the test into a
//! mirror and it will never fail again (`tests/fixtures/providers/README.md`).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use smed::core::event::{FinishReason, ProviderEvent};
use smed::core::message::CanonicalMessage;
use smed::core::model::ModelId;
use smed::core::provider::{Provider, ProviderRequest};
use smed::core::secrets::SecretStore;
use smed::providers::openai::OpenAiProvider;
use smed::store::secrets::OsSecretStore;
use smed::tools::ToolRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The cheapest model that still exercises the real streaming path.
const SMOKE_MODEL: &str = "gpt-4o-mini";

#[tokio::test]
#[ignore = "requires a real OPENAI_API_KEY and spends tokens; run with --ignored"]
async fn live_text_stream_matches_the_documented_contract() {
    let secrets: Arc<dyn SecretStore> = Arc::new(OsSecretStore::new());
    let provider = OpenAiProvider::new(secrets);

    let (tx, mut rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();

    let request = ProviderRequest {
        model: ModelId::new(SMOKE_MODEL),
        messages: vec![CanonicalMessage::user("Reply with exactly the word: ok")],
        system: None,
        tools: Vec::new(),
        images: smed::core::image::ImageSidecar::new(),
    };

    let task = tokio::spawn(async move { provider.stream(request, tx, cancel).await });

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }

    let completion = task
        .await
        .expect("adapter task")
        .expect("the live stream must succeed — if this is an auth error, check OPENAI_API_KEY");

    // Shape, not content.
    assert_eq!(
        completion.reason,
        FinishReason::Stop,
        "a short prompt should stop normally"
    );

    let text: String = events
        .iter()
        .filter_map(|event| match event {
            ProviderEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(!text.is_empty(), "the model produced no text");

    // The claim that matters for drift: usage still arrives, and the documented
    // fields are still populated (`docs/provider-contract.md` §1).
    let usage = completion
        .usage
        .expect("usage must still be reported on response.completed");
    assert!(usage.input_tokens > 0, "input_tokens went missing");
    assert!(usage.output_tokens > 0, "output_tokens went missing");

    // Any unknown event is a drift signal worth seeing, not a failure: the
    // provider adds events continuously and smed tolerates them by design.
    for event in &events {
        if let ProviderEvent::UnknownUpstream { kind } = event {
            println!(
                "note: unknown upstream event `{kind}` — provider may have added an event type"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires a real OpenAI credential and spends tokens; run with --ignored"]
async fn live_builtin_function_schemas_are_accepted() {
    let secrets: Arc<dyn SecretStore> = Arc::new(OsSecretStore::new());
    let provider = OpenAiProvider::new(secrets);
    let (tx, mut rx) = mpsc::channel(64);

    let request = ProviderRequest {
        model: ModelId::new(SMOKE_MODEL),
        messages: vec![CanonicalMessage::user(
            "Reply with exactly the word ok. Do not call a tool.",
        )],
        system: None,
        tools: ToolRegistry::builtins().definitions(),
        images: smed::core::image::ImageSidecar::new(),
    };

    let task =
        tokio::spawn(async move { provider.stream(request, tx, CancellationToken::new()).await });
    while rx.recv().await.is_some() {}

    task.await
        .expect("adapter task")
        .expect("OpenAI must accept every built-in strict function schema");
}

#[tokio::test]
#[ignore = "requires network; sends a deliberately invalid key"]
async fn live_invalid_credentials_map_to_auth() {
    // Proves the 401 path against the real provider rather than a mock, without
    // needing a valid key.
    #[derive(Debug)]
    struct BadKey;

    impl SecretStore for BadKey {
        fn resolve(
            &self,
            _provider: &smed::core::model::ProviderId,
            _kind: smed::core::secrets::CredentialKind,
        ) -> Result<smed::core::secrets::ResolvedCredential, smed::core::secrets::SecretError>
        {
            Ok(smed::core::secrets::ResolvedCredential {
                credential: smed::core::secrets::Credential::ApiKey(
                    smed::core::secrets::Secret::new("sk-obviously-not-a-real-key".to_owned()),
                ),
                source: smed::core::secrets::SecretSource::Environment,
            })
        }

        fn store(
            &self,
            _provider: &smed::core::model::ProviderId,
            _credential: smed::core::secrets::Credential,
        ) -> Result<(), smed::core::secrets::SecretError> {
            Ok(())
        }

        fn delete(
            &self,
            _provider: &smed::core::model::ProviderId,
        ) -> Result<(), smed::core::secrets::SecretError> {
            Ok(())
        }
    }

    let provider = OpenAiProvider::new(Arc::new(BadKey));
    let (tx, _rx) = mpsc::channel(8);

    let error = provider
        .stream(
            ProviderRequest {
                model: ModelId::new(SMOKE_MODEL),
                messages: vec![CanonicalMessage::user("hi")],
                system: None,
                tools: Vec::new(),
                images: smed::core::image::ImageSidecar::new(),
            },
            tx,
            CancellationToken::new(),
        )
        .await
        .expect_err("an invalid key must fail");

    assert_eq!(
        error.reason_code(),
        smed::core::error::ReasonCode::ProviderAuth,
        "a real 401 must still map to PROVIDER_AUTH"
    );

    let rendered = format!("{error} {error:?}");
    assert!(
        !rendered.contains("sk-obviously"),
        "the credential leaked into the error: {rendered}"
    );
}
