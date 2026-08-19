//! `ChatGPT` subscription adapter using smed-owned OAuth credentials.
//!
//! This is deliberately a distinct provider from `openai`: it has a different
//! endpoint, credential lifecycle, model catalogue, and quota semantics. Only
//! the documented Responses stream vocabulary is shared.

mod oauth;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::SecretStore;
use crate::providers::openai::{self, ResponseDialect};

pub const PROVIDER_ID: &str = "openai-codex";
pub const DEFAULT_MODEL: &str = "gpt-5.4";
const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

const MODELS: &[(&str, &str, u32, u32)] = &[
    ("gpt-5.4", "GPT-5.4", 400_000, 16_384),
    ("gpt-5.4-mini", "GPT-5.4 mini", 400_000, 16_384),
    (
        "gpt-5.3-codex-spark",
        "GPT-5.3 Codex Spark",
        128_000,
        16_384,
    ),
];

#[derive(Debug, Serialize)]
struct CodexRequest {
    model: String,
    instructions: String,
    input: Vec<openai::wire::InputItem>,
    tools: Vec<openai::wire::ToolDefinition>,
    tool_choice: &'static str,
    parallel_tool_calls: bool,
    store: bool,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct CodexModelsResponse {
    #[serde(default)]
    models: Vec<CodexModel>,
}

#[derive(Debug, Deserialize)]
struct CodexModel {
    slug: String,
    display_name: Option<String>,
    context_window: Option<u32>,
    #[serde(default)]
    input_modalities: Vec<String>,
    #[serde(default)]
    supported_in_api: bool,
    visibility: Option<String>,
    default_reasoning_level: Option<String>,
}

#[derive(Debug)]
pub struct OpenAiCodexProvider {
    client: reqwest::Client,
    base_url: String,
    oauth: oauth::OAuthManager,
}

impl OpenAiCodexProvider {
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            oauth: oauth::OAuthManager::new(secrets),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn for_test(
        secrets: Arc<dyn SecretStore>,
        base_url: impl Into<String>,
        auth_base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            oauth: oauth::OAuthManager::for_test(secrets, auth_base_url),
        }
    }

    fn provider_id() -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }
}

fn request_body(request: &ProviderRequest) -> Result<CodexRequest, ProviderError> {
    let instructions = request
        .system
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderError::Protocol {
            detail: "openai-codex requires non-empty instructions".to_owned(),
        })?;
    Ok(CodexRequest {
        model: request.model.to_string(),
        instructions: instructions.to_owned(),
        input: openai::to_input(request),
        tools: request
            .tools
            .iter()
            .map(|tool| openai::wire::ToolDefinition {
                kind: "function",
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: openai::schema::compatible_parameters(&tool.schema),
                strict: false,
            })
            .collect(),
        tool_choice: "auto",
        parallel_tool_calls: true,
        store: false,
        stream: true,
    })
}

fn map_http_error(
    status: reqwest::StatusCode,
    body: &str,
    retry_after_seconds: Option<u64>,
) -> ProviderError {
    let parsed = serde_json::from_str::<openai::wire::ErrorResponse>(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|response| response.error.code.as_deref())
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|response| response.error.kind.as_deref())
        });
    let reset_at_unix = parsed
        .as_ref()
        .and_then(|response| response.error.reset_at_unix);
    let message = parsed
        .as_ref()
        .and_then(|response| response.error.message.as_deref())
        .map(|value| value.chars().take(500).collect::<String>());

    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => ProviderError::Auth,
        reqwest::StatusCode::TOO_MANY_REQUESTS if code.is_some_and(openai::is_plan_quota_code) => {
            ProviderError::PlanQuota { reset_at_unix }
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit {
            retry_after_seconds,
        },
        _ => ProviderError::Protocol {
            detail: format!(
                "http {}{}{}",
                status.as_u16(),
                code.map(|value| format!(" ({value})")).unwrap_or_default(),
                message
                    .map(|value| format!(": {value}"))
                    .unwrap_or_default()
            ),
        },
    }
}

#[async_trait]
impl Provider for OpenAiCodexProvider {
    fn id(&self) -> ProviderId {
        Self::provider_id()
    }

    fn credentialed(&self) -> bool {
        self.oauth.credentialed(&Self::provider_id())
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        MODELS
            .iter()
            .map(|(id, display_name, context_tokens, max_output)| {
                let id = ModelId::new(*id);
                let provider = Self::provider_id();
                ModelDescriptor {
                    tier: crate::core::model::ModelTier::curated(&provider, &id),
                    id,
                    provider,
                    display_name: (*display_name).to_owned(),
                    capabilities: ModelCapabilities {
                        streaming: true,
                        tools: true,
                        structured_output: true,
                        images_in: false,
                        reasoning_controls: false,
                    },
                    context_tokens: Some(*context_tokens),
                    max_output_tokens: Some(*max_output),
                }
            })
            .collect()
    }

    async fn discover_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        // As with streaming, finish a rotating OAuth refresh before observing
        // cancellation so the new refresh token is durably stored.
        let access = self.oauth.access().await?;
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = self.client
                .get(format!(
                    "{}/models?client_version={}",
                    self.base_url,
                    env!("CARGO_PKG_VERSION")
                ))
                .bearer_auth(access.access_token().expose())
                .header("chatgpt-account-id", access.account_id())
                .header("OpenAI-Beta", "responses=experimental")
                .header("originator", "mjolnr")
                .header("version", env!("CARGO_PKG_VERSION"))
                .header(reqwest::header::ACCEPT, "application/json")
                .send() => result.map_err(|error| ProviderError::Transport {
                    detail: error.without_url().to_string(),
                })?,
        };
        let status = response.status();
        let retry_after_seconds = openai::retry_after_seconds(response.headers());
        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Protocol {
                detail: error.to_string(),
            })?;
        if !status.is_success() {
            return Err(map_http_error(status, &body, retry_after_seconds));
        }
        let payload = serde_json::from_str::<CodexModelsResponse>(&body).map_err(|error| {
            ProviderError::Protocol {
                detail: format!("invalid Codex model catalog: {error}"),
            }
        })?;

        Ok(payload
            .models
            .into_iter()
            .filter(|model| model.supported_in_api && model.visibility.as_deref() == Some("list"))
            .map(|model| {
                let id = ModelId::new(&model.slug);
                let provider = Self::provider_id();
                ModelDescriptor {
                    tier: crate::core::model::ModelTier::curated(&provider, &id),
                    id,
                    provider,
                    display_name: model.display_name.unwrap_or(model.slug),
                    capabilities: ModelCapabilities {
                        streaming: true,
                        tools: true,
                        structured_output: true,
                        images_in: model
                            .input_modalities
                            .iter()
                            .any(|modality| modality == "image"),
                        reasoning_controls: model.default_reasoning_level.is_some(),
                    },
                    context_tokens: model.context_window,
                    max_output_tokens: None,
                }
            })
            .collect())
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let body = request_body(&request)?;

        // Auth refresh is deliberately completed before cancellation is
        // observed. Dropping a rotating refresh mid-flight can strand the
        // credential chain before its replacement reaches the keychain.
        let access = self.oauth.access().await?;
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }

        let response = self
            .client
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(access.access_token().expose())
            .header("chatgpt-account-id", access.account_id())
            .header("originator", "mjolnr")
            .header(
                reqwest::header::USER_AGENT,
                concat!("mjolnr/", env!("CARGO_PKG_VERSION")),
            )
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|error| ProviderError::Transport {
                detail: error.without_url().to_string(),
            })?;

        let status = response.status();
        crate::providers::quota::emit_from_headers(
            ProviderId::new(PROVIDER_ID),
            response.headers(),
            &events,
            &cancel,
        )
        .await?;
        if !status.is_success() {
            let retry_after_seconds = openai::retry_after_seconds(response.headers());
            let response_body = response.text().await.unwrap_or_default();
            return Err(map_http_error(status, &response_body, retry_after_seconds));
        }

        openai::decode_stream(response, &events, &cancel, ResponseDialect::Subscription).await
    }
}

pub use oauth::{DevicePrompt, device_login};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::message::CanonicalMessage;
    use crate::core::secrets::{
        Credential, CredentialKind, OAuthCredential, ResolvedCredential, Secret, SecretError,
        SecretSource,
    };
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const EXPIRED_ACCESS: &str =
        "e30.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2NvdW50LTEiLCJleHAiOjE3MDAwMDAwMDB9.sig";
    const FRESH_ACCESS: &str =
        "e30.eyJjaGF0Z3B0X2FjY291bnRfaWQiOiJhY2NvdW50LTEiLCJleHAiOjQxMDI0NDQ4MDB9.sig";

    struct TestTokens {
        access: String,
        refresh: String,
        expires_at: i64,
        account: String,
    }

    impl std::fmt::Debug for TestTokens {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("TestTokens(<redacted>)")
        }
    }

    #[derive(Debug)]
    struct TestSecrets {
        tokens: StdMutex<TestTokens>,
        stores: AtomicUsize,
        fail_store: bool,
    }

    impl TestSecrets {
        fn expired(fail_store: bool) -> Arc<Self> {
            Arc::new(Self {
                tokens: StdMutex::new(TestTokens {
                    access: EXPIRED_ACCESS.to_owned(),
                    refresh: "refresh-old".to_owned(),
                    expires_at: 1_700_000_000,
                    account: "account-1".to_owned(),
                }),
                stores: AtomicUsize::new(0),
                fail_store,
            })
        }
    }

    impl SecretStore for TestSecrets {
        fn resolve(
            &self,
            provider: &ProviderId,
            kind: CredentialKind,
        ) -> Result<ResolvedCredential, SecretError> {
            assert_eq!(provider.as_str(), PROVIDER_ID);
            assert_eq!(kind, CredentialKind::OAuth);
            let tokens = self.tokens.lock().expect("test token lock");
            Ok(ResolvedCredential {
                credential: Credential::OAuth(OAuthCredential::new(
                    Secret::new(tokens.access.clone()),
                    Secret::new(tokens.refresh.clone()),
                    tokens.expires_at,
                    tokens.account.clone(),
                )),
                source: SecretSource::Keyring,
            })
        }

        fn store(&self, _provider: &ProviderId, credential: Credential) -> Result<(), SecretError> {
            self.stores.fetch_add(1, Ordering::SeqCst);
            if self.fail_store {
                return Err(SecretError::Unavailable {
                    detail: "injected persistence failure".to_owned(),
                });
            }
            let oauth = credential.into_oauth().expect("OAuth credential");
            let (access, refresh, expires_at, account) = oauth.into_parts();
            *self.tokens.lock().expect("test token lock") = TestTokens {
                access: access.expose().to_owned(),
                refresh: refresh.expose().to_owned(),
                expires_at,
                account,
            };
            Ok(())
        }

        fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
            Ok(())
        }
    }

    fn request(system: Option<&str>) -> ProviderRequest {
        ProviderRequest {
            model: ModelId::new(DEFAULT_MODEL),
            messages: vec![CanonicalMessage::user("hello")],
            system: system.map(ToOwned::to_owned),
            tools: Vec::new(),
            images: crate::core::image::ImageSidecar::new(),
        }
    }

    #[test]
    fn request_envelope_is_stateless_and_streaming() {
        let body = request_body(&request(Some("govern every effect"))).expect("valid request");
        let value = serde_json::to_value(body).expect("serialize request");

        assert_eq!(value.get("store"), Some(&serde_json::Value::Bool(false)));
        assert_eq!(value.get("stream"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(
            value.get("tool_choice").and_then(|v| v.as_str()),
            Some("auto")
        );
        assert_eq!(
            value.get("parallel_tool_calls"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn request_tools_preserve_dynamic_schema_arguments_in_non_strict_mode() {
        let mut request = request(Some("govern every effect"));
        request.tools.push(crate::core::tool::ToolDefinition {
            name: "spawn_subagent".to_owned(),
            description: "delegate".to_owned(),
            schema: serde_json::json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "result_schema": {
                        "type": "object"
                    },
                    "route": {
                        "type": "string"
                    }
                },
                "required": ["result_schema"],
                "additionalProperties": false
            }),
        });

        let body = request_body(&request).expect("valid request");
        let value = serde_json::to_value(body).expect("serialize request");
        let tool = value
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .and_then(|tools| tools.first())
            .expect("one tool");
        let parameters = tool.get("parameters").expect("parameters");

        assert_eq!(tool.get("strict"), Some(&serde_json::Value::Bool(false)));
        assert!(parameters.get("$schema").is_none());
        assert_eq!(
            parameters.get("required"),
            Some(&serde_json::json!(["result_schema"]))
        );
        assert!(
            parameters
                .get("properties")
                .and_then(|properties| properties.get("result_schema"))
                .and_then(|schema| schema.get("additionalProperties"))
                .is_none()
        );
    }

    #[test]
    fn blank_instructions_are_refused_before_auth_or_network() {
        let error = request_body(&request(Some("  "))).expect_err("blank instructions");
        assert_eq!(
            error.reason_code(),
            crate::core::error::ReasonCode::ProviderProtocol
        );
    }

    #[test]
    fn plan_quota_is_distinct_and_keeps_reset_time() {
        let body = r#"{"error":{"type":"usage_limit_reached","code":"usage_limit_reached","reset_at":1700000000}}"#;
        let error = map_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, Some(10));

        assert!(matches!(
            &error,
            ProviderError::PlanQuota {
                reset_at_unix: Some(1_700_000_000)
            }
        ));
        assert_eq!(
            error.reason_code(),
            crate::core::error::ReasonCode::ProviderPlanQuota
        );
    }

    #[test]
    fn ordinary_api_rate_limit_stays_distinct_from_plan_quota() {
        let body = r#"{"error":{"type":"rate_limit_exceeded","code":"rate_limit_exceeded"}}"#;
        let error = map_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, body, Some(10));
        assert!(matches!(
            error,
            ProviderError::RateLimit {
                retry_after_seconds: Some(10)
            }
        ));
    }

    #[test]
    fn only_gpt_five_family_models_are_exposed() {
        let provider = OpenAiCodexProvider::for_test(
            TestSecrets::expired(false),
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        );
        let models = provider.models();

        assert!(!models.is_empty());
        assert!(
            models
                .iter()
                .all(|model| model.id.as_str().starts_with("gpt-5"))
        );
        assert!(models.iter().all(|model| model.capabilities.tools));
    }

    #[tokio::test]
    async fn discovery_refreshes_auth_and_uses_the_account_catalog() {
        let server = MockServer::start().await;
        mount_refresh(&server, 1).await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", format!("Bearer {FRESH_ACCESS}")))
            .and(header("chatgpt-account-id", "account-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [
                    {
                        "slug": "gpt-5.6-sol",
                        "display_name": "GPT-5.6-Sol",
                        "context_window": 272_000,
                        "input_modalities": ["text", "image"],
                        "supported_in_api": true,
                        "visibility": "list",
                        "default_reasoning_level": "low"
                    },
                    {
                        "slug": "codex-auto-review",
                        "display_name": "Codex Auto Review",
                        "supported_in_api": true,
                        "visibility": "hide"
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider =
            OpenAiCodexProvider::for_test(TestSecrets::expired(false), server.uri(), server.uri());

        let models = provider
            .discover_models(CancellationToken::new())
            .await
            .expect("catalog");

        assert_eq!(models.len(), 1);
        let model = models.first().expect("visible model");
        assert_eq!(model.id.as_str(), "gpt-5.6-sol");
        assert!(model.capabilities.images_in);
        assert!(model.capabilities.reasoning_controls);
        assert_eq!(model.context_tokens, Some(272_000));
    }

    fn successful_stream() -> &'static str {
        concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"error\":null,\"incomplete_details\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        )
    }

    async fn mount_refresh(server: &MockServer, expected: u64) {
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": FRESH_ACCESS,
                "refresh_token": "refresh-new",
                "expires_in": 3600
            })))
            .expect(expected)
            .mount(server)
            .await;
    }

    async fn mount_response(server: &MockServer, expected: u64) {
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("authorization", format!("Bearer {FRESH_ACCESS}")))
            .and(header("chatgpt-account-id", "account-1"))
            .and(header("originator", "mjolnr"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(successful_stream()),
            )
            .expect(expected)
            .mount(server)
            .await;
    }

    async fn run_provider(provider: Arc<OpenAiCodexProvider>) -> Result<(), ProviderError> {
        let (tx, mut rx) = mpsc::channel(16);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        provider
            .stream(
                request(Some("govern every effect")),
                tx,
                CancellationToken::new(),
            )
            .await?;
        drain.await.expect("event drain");
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_expiry_performs_one_rotating_refresh() {
        let server = MockServer::start().await;
        mount_refresh(&server, 1).await;
        mount_response(&server, 2).await;
        let secrets = TestSecrets::expired(false);
        let provider = Arc::new(OpenAiCodexProvider::for_test(
            Arc::clone(&secrets) as Arc<dyn SecretStore>,
            server.uri(),
            server.uri(),
        ));

        let (first, second) = tokio::join!(
            run_provider(Arc::clone(&provider)),
            run_provider(Arc::clone(&provider))
        );

        first.expect("first request");
        second.expect("second request");
        assert_eq!(secrets.stores.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_persistence_failure_prevents_provider_effect() {
        let server = MockServer::start().await;
        mount_refresh(&server, 1).await;
        mount_response(&server, 0).await;
        let secrets = TestSecrets::expired(true);
        let provider = Arc::new(OpenAiCodexProvider::for_test(
            Arc::clone(&secrets) as Arc<dyn SecretStore>,
            server.uri(),
            server.uri(),
        ));

        let error = run_provider(provider)
            .await
            .expect_err("persistence failure must stop the request");

        assert_eq!(
            error.reason_code(),
            crate::core::error::ReasonCode::ProviderProtocol
        );
        assert_eq!(secrets.stores.load(Ordering::SeqCst), 1);
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(EXPIRED_ACCESS));
        assert!(!rendered.contains(FRESH_ACCESS));
        assert!(!rendered.contains("refresh-old"));
        assert!(!rendered.contains("refresh-new"));
    }
}
