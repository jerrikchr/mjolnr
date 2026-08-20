//! Phase 7 provider contract tests.

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
use mjolnr::providers::{gemini, ollama, openrouter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Debug)]
struct Secrets(&'static str);

impl SecretStore for Secrets {
    fn resolve(
        &self,
        provider: &ProviderId,
        _kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        Ok(ResolvedCredential {
            credential: Credential::ApiKey(Secret::new(self.0.to_owned())),
            source: SecretSource::Environment,
        })
        .inspect(|_| assert!(["gemini", "openrouter"].contains(&provider.as_str())))
    }

    fn store(&self, _provider: &ProviderId, _credential: Credential) -> Result<(), SecretError> {
        Ok(())
    }

    fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
        Ok(())
    }
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

#[tokio::test]
async fn gemini_uses_header_auth_and_sse_terminal_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-2.5-flash:streamGenerateContent"))
        .and(header("x-goog-api-key", "gemini-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2}}\n\n",
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let provider =
        gemini::GeminiProvider::new(Arc::new(Secrets("gemini-secret"))).with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(32);
    let result = provider
        .stream(request(gemini::DEFAULT_MODEL), tx, CancellationToken::new())
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
    assert!(!format!("{events:?}").contains("gemini-secret"));
}

#[tokio::test]
async fn openrouter_ignores_comment_frames_and_assembles_tool_arguments() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer router-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            concat!(
                ": OPENROUTER PROCESSING\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"id\":\"call_1\",\"index\":0,\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"\"}}]}}]}\n\n",
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"src/lib.rs\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":5}}

"#,
                "data: [DONE]\n\n"
            ),
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let provider = openrouter::OpenRouterProvider::new(Arc::new(Secrets("router-secret")))
        .with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(32);
    let result = provider
        .stream(
            request(openrouter::DEFAULT_MODEL),
            tx,
            CancellationToken::new(),
        )
        .await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert_eq!(result.unwrap().reason, FinishReason::ToolCalls);
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::ToolCallCompleted { call } if call.name == "read_file")));
}

#[tokio::test]
async fn ollama_decodes_ndjson_and_maps_final_token_counts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "{\"model\":\"llama3.2\",\"created_at\":\"now\",\"message\":{\"role\":\"assistant\",\"content\":\"local\"},\"done\":false}\n{\"model\":\"llama3.2\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":7,\"eval_count\":3}\n",
            "application/x-ndjson",
        ))
        .mount(&server)
        .await;
    let provider = ollama::OllamaProvider::new().with_base_url(server.uri());
    let (tx, mut rx) = mpsc::channel(32);
    let result = provider
        .stream(request(ollama::DEFAULT_MODEL), tx, CancellationToken::new())
        .await;
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    let completion = result.unwrap();
    assert_eq!(completion.reason, FinishReason::Stop);
    assert_eq!(completion.usage.unwrap().input_tokens, 7);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TextDelta { text } if text == "local"))
    );
}

#[tokio::test]
async fn ollama_discovers_installed_tool_models_and_ignores_plain_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [
                {"model": "qwen-tool:latest"},
                {"model": "embed-only:latest"}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if body["model"] == "qwen-tool:latest" {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "capabilities": ["completion", "tools", "vision"],
                    "model_info": {"qwen.context_length": 32768}
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "capabilities": ["embedding"],
                    "model_info": {}
                }))
            }
        })
        .mount(&server)
        .await;
    let provider = ollama::OllamaProvider::new().with_base_url(server.uri());

    let models = provider
        .discover_models(CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id.as_str(), "qwen-tool:latest");
    assert!(models[0].capabilities.tools);
    assert!(models[0].capabilities.images_in);
    assert_eq!(models[0].context_tokens, Some(32_768));
}

#[test]
fn phase7_provider_errors_remain_typed() {
    let error = ProviderError::Protocol {
        detail: "http 404".to_owned(),
    };
    assert_eq!(error.reason_code().as_str(), "PROVIDER_PROTOCOL");
}
