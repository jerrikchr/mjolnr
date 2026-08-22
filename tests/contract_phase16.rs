//! Phase 16 OpenAI-compat catalog contract tests.
//!
//! The streaming wire itself is covered by the OpenRouter tests in
//! `contract_phase7.rs` (OpenRouter is now an instance of the same adapter).
//! These tests cover what the catalog adds: keyless local endpoints,
//! bearer-keyed catalog endpoints, and the typed refusal when an
//! account-scoped gateway has no configured URL.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::Arc;

use mjolnr::core::error::ProviderError;
use mjolnr::core::event::{FinishReason, ProviderEvent};
use mjolnr::core::message::CanonicalMessage;
use mjolnr::core::model::{ModelId, ProviderId};
use mjolnr::core::provider::{Provider, ProviderRequest};
use mjolnr::core::secrets::{
    Credential, CredentialKind, ResolvedCredential, Secret, SecretError, SecretSource, SecretStore,
};
use mjolnr::providers::forge::ForgeProvider;
use mjolnr::providers::openai_compat::{CATALOG, OpenAiCompatProvider, persist_lm_studio_base_url};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug)]
struct Secrets(&'static str);

impl SecretStore for Secrets {
    fn resolve(
        &self,
        _provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        Ok(ResolvedCredential {
            credential: Credential::ApiKey(Secret::new(self.0.to_owned())),
            source: SecretSource::Environment,
        })
    }

    fn store(&self, _provider: &ProviderId, _credential: Credential) -> Result<(), SecretError> {
        Ok(())
    }

    fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
        Ok(())
    }
}

/// A store with nothing in it, as on a machine that never ran `auth login`.
#[derive(Debug)]
struct EmptySecrets;

impl SecretStore for EmptySecrets {
    fn resolve(
        &self,
        provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        Err(SecretError::NotFound {
            provider: provider.clone(),
        })
    }

    fn store(&self, _provider: &ProviderId, _credential: Credential) -> Result<(), SecretError> {
        Ok(())
    }

    fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
        Ok(())
    }
}

fn descriptor(id: &str) -> &'static mjolnr::providers::openai_compat::CompatDescriptor {
    CATALOG
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("catalog id")
}

fn request(model: &str) -> ProviderRequest {
    ProviderRequest {
        model: ModelId::new(model),
        messages: vec![CanonicalMessage::user("hello")],
        system: Some("be concise".to_owned()),
        tools: Vec::new(),
        images: mjolnr::core::image::ImageSidecar::new(),
    }
}

const STREAM_BODY: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\n",
    "data: [DONE]\n\n"
);

#[tokio::test]
async fn a_local_keyless_endpoint_streams_without_a_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(STREAM_BODY, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(descriptor("lm-studio"), Arc::new(EmptySecrets))
        .with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(32);
    let result = provider
        .stream(request("llama-3-8b"), tx, CancellationToken::new())
        .await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert_eq!(result.unwrap().reason, FinishReason::Stop);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TextDelta { text } if text == "hi"))
    );
    // No key resolved, so no authorization header was demanded by the mock and
    // none should have been sent; the mock matching without a header assertion
    // plus a successful stream is the observable contract here.
}

#[tokio::test]
async fn lm_studio_surfaces_a_root_stream_error_without_choices() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            concat!(
                "data: {\"error\":{\"code\":\"context_length_exceeded\",",
                "\"message\":\"loaded context is too small\"},",
                "\"message\":\"loaded context is too small\"}\n\n"
            ),
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(descriptor("lm-studio"), Arc::new(EmptySecrets))
        .with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(32);
    let result = provider
        .stream(request("gemma"), tx, CancellationToken::new())
        .await;
    while rx.recv().await.is_some() {}

    assert!(matches!(
        result,
        Err(ProviderError::Protocol { detail })
            if detail == "\"context_length_exceeded\""
    ));
}

#[tokio::test]
async fn lm_studio_discovers_only_llms_with_native_capabilities() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [
                {
                    "type": "llm",
                    "key": "google/gemma-4-12b-qat",
                    "display_name": "Gemma 4 12B",
                    "max_context_length": 131_072,
                    "capabilities": {
                        "vision": true,
                        "trained_for_tool_use": true,
                        "reasoning": false
                    }
                },
                {
                    "type": "embedding",
                    "key": "nomic/embed",
                    "display_name": "Nomic Embed",
                    "max_context_length": 8192
                }
            ]
        })))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(descriptor("lm-studio"), Arc::new(EmptySecrets))
        .with_base_url(format!("{}/v1", server.uri()));

    let models = provider
        .discover_models(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id.as_str(), "google/gemma-4-12b-qat");
    assert!(models[0].capabilities.tools);
    assert!(models[0].capabilities.images_in);
    assert_eq!(models[0].context_tokens, Some(131_072));
}

#[tokio::test]
async fn lm_studio_model_discovery_sends_an_optional_api_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .and(header("authorization", "Bearer local-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": []
        })))
        .mount(&server)
        .await;
    let provider =
        OpenAiCompatProvider::new(descriptor("lm-studio"), Arc::new(Secrets("local-token")))
            .with_base_url(format!("{}/v1", server.uri()));

    assert!(
        provider
            .discover_models(CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn lm_studio_reloads_the_project_endpoint_before_discovery() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": []
        })))
        .mount(&server)
        .await;
    let workspace = tempfile::tempdir().unwrap();
    let provider = OpenAiCompatProvider::for_workspace(
        descriptor("lm-studio"),
        Arc::new(EmptySecrets),
        workspace.path(),
    );
    persist_lm_studio_base_url(workspace.path(), &server.uri()).unwrap();

    assert!(
        provider
            .discover_models(CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_generic_catalog_does_not_invent_tool_capability_for_unknown_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer nim-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {
                    "id": "nvidia/llama-3.1-nemotron-70b-instruct"
                },
                {
                    "id": "provider/unknown-embedding-or-chat-model"
                }
            ]
        })))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(descriptor("nvidia"), Arc::new(Secrets("nim-secret")))
        .with_base_url(server.uri());

    let models = provider
        .discover_models(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(
        models.first().expect("reviewed model").id.as_str(),
        "nvidia/llama-3.1-nemotron-70b-instruct"
    );
}

#[tokio::test]
async fn an_endpoint_authoritative_catalog_surfaces_every_listed_model() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer zen-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                { "id": "claude-opus-4-8" },
                { "id": "kimi-k2.7-code" },
                { "id": "some-new-route-the-vendor-shipped-this-week" }
            ]
        })))
        .mount(&server)
        .await;
    let provider =
        OpenAiCompatProvider::new(descriptor("opencode-zen"), Arc::new(Secrets("zen-secret")))
            .with_base_url(server.uri());

    let mut models = provider
        .discover_models(CancellationToken::new())
        .await
        .unwrap();
    models.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    // A subscription gateway's own listing is the plan's route table; a
    // curated mjolnr list would silently hide routes the account can use.
    assert_eq!(models.len(), 3);
    assert_eq!(
        models.first().expect("first model").id.as_str(),
        "claude-opus-4-8"
    );
    assert_eq!(
        models.last().expect("last model").id.as_str(),
        "some-new-route-the-vendor-shipped-this-week"
    );
}

#[test]
fn only_subscription_gateways_own_their_catalog() {
    // Widening this flag must be a reviewed act per endpoint: every other
    // compat endpoint's listing proves availability, not mjolnr's streaming +
    // tool contract, and stays on the reviewed intersection.
    let authoritative: Vec<&str> = CATALOG
        .iter()
        .filter(|descriptor| {
            descriptor.catalog_trust
                == mjolnr::providers::openai_compat::CatalogTrust::EndpointAuthoritative
        })
        .map(|descriptor| descriptor.id)
        .collect();
    assert_eq!(authoritative, vec!["opencode-zen", "opencode-go"]);
}

#[tokio::test]
async fn a_keyed_catalog_endpoint_sends_the_bearer_credential() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer nim-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(STREAM_BODY, "text/event-stream"))
        .mount(&server)
        .await;
    let provider = OpenAiCompatProvider::new(descriptor("nvidia"), Arc::new(Secrets("nim-secret")))
        .with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(32);
    let result = provider
        .stream(
            request("nvidia/llama-3.1-nemotron-70b-instruct"),
            tx,
            CancellationToken::new(),
        )
        .await;
    while rx.recv().await.is_some() {}
    assert_eq!(result.unwrap().reason, FinishReason::Stop);
}

#[tokio::test]
async fn forge_reports_a_typed_relay_failure_instead_of_a_payment_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer forge-secret"))
        .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
            "error": {
                "message": "ERR: 402, Share this with any staff-member: diagnostic-reference",
                "type": "relay_error",
                "code": null,
                "serverissuedkey": "diagnostic-reference"
            }
        })))
        .mount(&server)
        .await;
    let provider =
        ForgeProvider::new(Arc::new(Secrets("forge-secret"))).with_base_url(server.uri());
    let (tx, _rx) = mpsc::channel(32);
    let result = provider
        .stream(request("gpt-5.6-luna"), tx, CancellationToken::new())
        .await;

    assert!(matches!(result, Err(ProviderError::Relay)));
}

#[tokio::test]
async fn forge_reports_an_exhausted_upstream_key_pool_as_a_relay_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer forge-secret"))
        .respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": {
                "message": "No active upstream keys available. Please contact admin."
            }
        })))
        .mount(&server)
        .await;
    let provider =
        ForgeProvider::new(Arc::new(Secrets("forge-secret"))).with_base_url(server.uri());
    let (tx, _rx) = mpsc::channel(32);
    let result = provider
        .stream(request("grok-4.5"), tx, CancellationToken::new())
        .await;

    assert!(matches!(result, Err(ProviderError::Relay)));
}

#[tokio::test]
async fn a_keyed_endpoint_with_no_stored_credential_refuses_before_any_request() {
    let provider = OpenAiCompatProvider::new(descriptor("xai"), Arc::new(EmptySecrets))
        .with_base_url("http://localhost:9");
    let (tx, _rx) = mpsc::channel(32);
    let result = provider
        .stream(
            request("grok-4-fast-non-reasoning"),
            tx,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(ProviderError::Auth)));
}

#[tokio::test]
async fn an_account_scoped_gateway_without_a_url_refuses_with_the_variable_name() {
    let provider =
        OpenAiCompatProvider::new(descriptor("cloudflare-gateway"), Arc::new(EmptySecrets));
    let (tx, _rx) = mpsc::channel(32);
    let result = provider
        .stream(
            request("anthropic/claude-opus-4-8"),
            tx,
            CancellationToken::new(),
        )
        .await;
    match result {
        Err(ProviderError::Protocol { detail }) => {
            assert!(
                detail.contains("MJOLNR_CLOUDFLARE_GATEWAY_BASE_URL"),
                "refusal must name the variable to set, got: {detail}"
            );
        }
        other => panic!("expected a typed protocol refusal, got {other:?}"),
    }
}
