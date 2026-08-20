//! The OpenAI Responses adapter.
//!
//! Written against the official OpenAPI specification, not an SDK. What it owns:
//! its wire types, its SSE state machine, its auth header, its tool-call
//! assembly, its error mapping, its usage extraction, its capabilities. It knows
//! nothing about policy, persistence, or the UI.
//!
//! # The rules that shape this file
//!
//! - **Never infer success from HTTP status.** A rate limit arrives *both* as
//!   HTTP 429 and, mid-stream, as `response.failed` with
//!   `error.code = rate_limit_exceeded` under HTTP 200
//!   (`docs/provider-contract.md` §1). Both map to `PROVIDER_RATE_LIMIT`.
//! - **Never retry after output.** A stream that produced tokens and then failed
//!   is not safe to replay, and this adapter has no reconnect path at all
//!   (AGENTS.md §4,  anti-pattern).
//! - **Never log a header or a body.** They carry the credential and the user's
//!   source code respectively.
//! - **Parse tool arguments only at the completion boundary.**

pub(crate) mod schema;
pub(crate) mod stream;
pub mod wire;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::message::{ContentBlock, Role, ToolOutcome};
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::{SecretError, SecretStore};

pub(crate) use stream::{ResponseDialect, decode_stream, is_plan_quota_code};
#[cfg(test)]
use stream::{map_response_error, to_usage};

/// Confirmed from the spec: `servers[0].url` + path `/responses`.
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

pub const PROVIDER_ID: &str = "openai";

/// The models mjolnr offers.
///
/// A static list, not a discovery call. `GET /models` returns hundreds of
/// entries with no capability metadata, so it cannot answer the question that
/// matters ("does this support tools?"). A curated list is honest about what
/// mjolnr has actually been tested against; Phase 8 can revisit.
const MODELS: &[(&str, &str, u32, u32)] = &[
    ("gpt-4o", "GPT-4o", 128_000, 16_384),
    ("gpt-4o-mini", "GPT-4o mini", 128_000, 16_384),
    ("gpt-4.1", "GPT-4.1", 1_047_576, 32_768),
    ("gpt-4.1-mini", "GPT-4.1 mini", 1_047_576, 32_768),
];

#[derive(Debug)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    secrets: Arc<dyn SecretStore>,
}

impl OpenAiProvider {
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            secrets,
        }
    }

    /// Point the adapter at a local mock. Test affordance: contract tests run
    /// against `wiremock`, never the network (AGENTS.md §7).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn id() -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }
}

/// Translate canonical history into the provider's input shape.
pub(crate) fn to_input(request: &ProviderRequest) -> Vec<wire::InputItem> {
    let mut input = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::User => Some("user"),
            Role::Assistant => Some("assistant"),
            Role::System => Some("system"),
            Role::Tool => None,
        };

        let text: String = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // Image parts keep the position the user gave them. Anthropic documents
        // that images before text read better, but moving them would tell the
        // model something different from what the person wrote, and a request
        // that reorders the user's words to score better is the wrong trade.
        let image_parts: Vec<wire::ContentPart> = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ImageRef { source, .. } => {
                    request
                        .images
                        .get(source)
                        .map(|bytes| wire::ContentPart::Image {
                            image_url: bytes.data_uri(),
                            detail: "auto",
                        })
                }
                _ => None,
            })
            .collect();
        if let Some(role) = role
            && (!text.is_empty() || !image_parts.is_empty())
        {
            let content = if image_parts.is_empty() {
                wire::MessageContent::Text(text)
            } else {
                let mut parts = image_parts;
                if !text.is_empty() {
                    parts.push(wire::ContentPart::Text { text });
                }
                wire::MessageContent::Parts(parts)
            };
            input.push(wire::InputItem::Message {
                role: role.to_owned(),
                content,
            });
        }

        for block in &message.blocks {
            match block {
                ContentBlock::ToolCall(call) => {
                    let arguments =
                        serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_owned());
                    input.push(wire::InputItem::FunctionCall {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments,
                    });
                }
                ContentBlock::ToolResult {
                    call_id, result, ..
                } => {
                    let status = match result.outcome {
                        ToolOutcome::Ok => "ok".to_owned(),
                        ToolOutcome::Refused(code) => format!("refused:{}", code.as_str()),
                        ToolOutcome::Failed(code) => format!("failed:{}", code.as_str()),
                    };
                    let output = serde_json::json!({
                        "status": status,
                        "content": result.content,
                        "truncated": result.truncated,
                        "evidence_event_id": result.evidence_event_id,
                    })
                    .to_string();
                    input.push(wire::InputItem::FunctionCallOutput {
                        call_id: call_id.clone(),
                        output,
                    });
                }
                ContentBlock::Text { .. } | ContentBlock::ImageRef { .. } => {}
            }
        }
    }
    input
}

/// Map a non-200 status to a typed error.
///
/// The body is parsed for a code where possible, but **never propagated raw**: a
/// provider error body can echo request content back, which may include the
/// user's source.
fn map_http_error(
    status: reqwest::StatusCode,
    body: &str,
    retry_after_seconds: Option<u64>,
) -> ProviderError {
    let parsed: Option<wire::ErrorResponse> = serde_json::from_str(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|response| response.error.code.clone());

    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => ProviderError::Auth,
        reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit {
            retry_after_seconds,
        },
        _ => ProviderError::Protocol {
            detail: format!(
                "http {}{}",
                status.as_u16(),
                code.map(|code| format!(" ({code})")).unwrap_or_default()
            ),
        },
    }
}

/// Read the delta-seconds form of the standard `Retry-After` header.
///
/// The HTTP-date form cannot be represented without a clock-relative
/// calculation, so it remains `None` rather than inventing a duration.
pub(crate) fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn map_secret_error(error: SecretError) -> ProviderError {
    match error {
        // "You have not configured a key" and "the keychain is broken" are
        // different problems and get different words.
        SecretError::NotFound { .. } | SecretError::KindMismatch { .. } => ProviderError::Auth,
        SecretError::Unavailable { detail } => ProviderError::Transport { detail },
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        Self::id()
    }

    fn credentialed(&self) -> bool {
        self.secrets
            .resolve(&Self::id(), crate::core::secrets::CredentialKind::ApiKey)
            .is_ok()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        MODELS
            .iter()
            .map(|(id, display_name, context, max_output)| {
                let id = ModelId::new(*id);
                let provider = Self::id();
                ModelDescriptor {
                    tier: crate::core::model::ModelTier::curated(&provider, &id),
                    id,
                    provider,
                    display_name: (*display_name).to_owned(),
                    capabilities: ModelCapabilities {
                        streaming: true,
                        tools: true,
                        structured_output: true,
                        images_in: true,
                        reasoning_controls: false,
                    },
                    context_tokens: Some(*context),
                    max_output_tokens: Some(*max_output),
                }
            })
            .collect()
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let secrets = Arc::clone(&self.secrets);
        let provider = Self::id();
        let resolution = tokio::task::spawn_blocking(move || {
            secrets.resolve(&provider, crate::core::secrets::CredentialKind::ApiKey)
        });
        let resolved = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = resolution => result.map_err(|error| ProviderError::Transport {
                detail: format!("credential resolution task failed: {error}"),
            })?,
        }
        .map_err(map_secret_error)?;

        let Some(secret) = resolved.credential.api_key() else {
            return Err(ProviderError::Auth);
        };
        if secret.is_blank() {
            return Err(ProviderError::Auth);
        }

        let body = wire::CreateResponse {
            model: request.model.to_string(),
            input: to_input(&request),
            stream: true,
            instructions: request.system.clone(),
            tools: request
                .tools
                .iter()
                .map(|tool| wire::ToolDefinition {
                    kind: "function",
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: schema::strict_parameters(&tool.schema),
                    strict: true,
                })
                .collect(),
        };

        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = self
                .client
                .post(format!("{}/responses", self.base_url))
                // The only place the credential is read. `expose` is greppable
                // precisely so this line is easy to find and audit.
                .bearer_auth(secret.expose())
                .json(&body)
                .send() => result.map_err(|error| ProviderError::Transport {
                    // `reqwest`'s Display includes the URL but never headers.
                    detail: error.without_url().to_string(),
                })?,
        };

        let status = response.status();
        crate::providers::quota::emit_from_headers(
            ProviderId::new(PROVIDER_ID),
            response.headers(),
            &events,
            &cancel,
        )
        .await?;
        if !status.is_success() {
            let retry_after_seconds = retry_after_seconds(response.headers());
            let body = response.text().await.unwrap_or_default();
            return Err(map_http_error(status, &body, retry_after_seconds));
        }

        // 200 means the stream opened. It does **not** mean the request
        // succeeded — that is decided by the terminal event below.
        decode_stream(response, &events, &cancel, ResponseDialect::Api).await
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;
    use crate::core::message::CanonicalMessage;
    use crate::core::secrets::{Credential, CredentialKind, ResolvedCredential};

    #[test]
    fn an_image_becomes_a_data_uri_part_and_text_stays_a_bare_string() {
        use crate::core::image::{ImageBytes, ImageSidecar};
        use crate::core::message::ContentBlock;

        // Two assertions in one, because they constrain each other. The image
        // shape is confirmed against current documentation 2026-07-25
        // (`provider-contract.md` §5.5); the *absence* of parts on a text-only
        // message is what keeps every existing fixture testing what it was
        // written to test.
        let mut with_image = CanonicalMessage::user("what is this?");
        with_image.blocks.push(ContentBlock::ImageRef {
            media_type: "image/png".to_owned(),
            source: "shot.png".to_owned(),
        });
        let mut images = ImageSidecar::new();
        images.insert(
            "shot.png".to_owned(),
            ImageBytes {
                media_type: "image/png".to_owned(),
                bytes: std::sync::Arc::from([1_u8, 2, 3].as_slice()),
            },
        );
        let request = ProviderRequest {
            model: ModelId::new("gpt-5.4"),
            messages: vec![CanonicalMessage::user("plain"), with_image],
            system: None,
            tools: Vec::new(),
            images,
        };

        let encoded = serde_json::to_value(to_input(&request)).expect("encode");
        assert_eq!(
            encoded[0]["content"], "plain",
            "a text-only message must still serialise as a bare string: {encoded}"
        );
        let part = encoded
            .pointer("/1/content/0")
            .expect("the image part leads the message");
        assert_eq!(part["type"], "input_image");
        assert_eq!(part["image_url"], "data:image/png;base64,AQID");
        assert_eq!(encoded.pointer("/1/content/1/type").unwrap(), "input_text");
    }

    #[test]
    fn http_401_maps_to_auth_without_echoing_the_body() {
        // The body of a provider error can echo request content back, which may
        // include the user's source. It must not reach the error.
        let body = r#"{"error":{"type":"invalid_request_error","message":"Incorrect API key sk-secret123 provided","param":null,"code":"invalid_api_key"}}"#;
        let error = map_http_error(reqwest::StatusCode::UNAUTHORIZED, body, None);

        assert!(matches!(error, ProviderError::Auth));
        let rendered = format!("{error} {error:?}");
        assert!(
            !rendered.contains("sk-secret123"),
            "a credential echoed by the provider leaked into our error: {rendered}"
        );
    }

    #[test]
    fn http_429_maps_to_rate_limit() {
        let error = map_http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "{}", None);
        assert_eq!(
            error.reason_code(),
            crate::core::error::ReasonCode::ProviderRateLimit
        );
    }

    #[test]
    fn a_mid_stream_rate_limit_maps_to_rate_limit_not_protocol() {
        // The trap: HTTP was 200. Only `error.code` reveals this. Mapping it to
        // a generic protocol error sends the user retrying into the same wall.
        let response = wire::Response {
            status: Some("failed".to_owned()),
            error: Some(wire::ResponseError {
                code: Some("rate_limit_exceeded".to_owned()),
                message: Some("slow down".to_owned()),
                reset_at_unix: None,
            }),
            incomplete_details: None,
            usage: None,
        };

        assert_eq!(
            map_response_error(&response, ResponseDialect::Api).reason_code(),
            crate::core::error::ReasonCode::ProviderRateLimit
        );
    }

    #[test]
    fn an_unrecognised_failure_code_is_a_protocol_error_naming_the_code() {
        let response = wire::Response {
            status: Some("failed".to_owned()),
            error: Some(wire::ResponseError {
                code: Some("server_error".to_owned()),
                message: Some("boom".to_owned()),
                reset_at_unix: None,
            }),
            incomplete_details: None,
            usage: None,
        };

        let error = map_response_error(&response, ResponseDialect::Api);
        assert_eq!(
            error.reason_code(),
            crate::core::error::ReasonCode::ProviderProtocol
        );
        assert!(error.to_string().contains("server_error"));
    }

    #[test]
    fn usage_ignores_the_breakdowns() {
        let usage = to_usage(&wire::ResponseUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
        });
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total(), 150);
    }

    #[test]
    fn tool_calls_and_results_keep_the_provider_call_id() {
        let request = ProviderRequest {
            model: ModelId::new("gpt-4o-mini"),
            messages: vec![
                CanonicalMessage::user("hello"),
                CanonicalMessage::assistant(
                    vec![ContentBlock::ToolCall(crate::core::message::ToolCall {
                        id: "call_1".to_owned(),
                        name: "read_file".to_owned(),
                        arguments: serde_json::json!({ "path": "src/lib.rs" }),
                        provider_signature: None,
                    })],
                    ProviderId::new(PROVIDER_ID),
                    ModelId::new("gpt-4o-mini"),
                ),
                CanonicalMessage::tool_result(
                    "call_1",
                    "read_file",
                    crate::core::message::ToolResult::refused(
                        crate::core::error::ReasonCode::PathOutsideWorkspace,
                        "outside",
                    ),
                ),
            ],
            system: None,
            tools: Vec::new(),
            images: crate::core::image::ImageSidecar::new(),
        };

        let input = to_input(&request);
        assert_eq!(input.len(), 3);
        assert!(matches!(
            input.get(1),
            Some(wire::InputItem::FunctionCall { call_id, name, .. })
                if call_id == "call_1" && name == "read_file"
        ));
        assert!(matches!(
            input.get(2),
            Some(wire::InputItem::FunctionCallOutput { call_id, output })
                if call_id == "call_1" && output.contains("PATH_OUTSIDE_WORKSPACE")
        ));
    }

    #[test]
    fn empty_messages_are_not_sent() {
        let request = ProviderRequest {
            model: ModelId::new("gpt-4o-mini"),
            messages: vec![CanonicalMessage::assistant(
                vec![],
                ProviderId::new(PROVIDER_ID),
                ModelId::new("gpt-4o-mini"),
            )],
            system: None,
            tools: Vec::new(),
            images: crate::core::image::ImageSidecar::new(),
        };
        assert!(to_input(&request).is_empty());
    }

    /// A store that has nothing. Enough to construct the adapter, and it keeps
    /// `providers` from depending on `store` — the dependency direction applies
    /// to test code too, and a test reaching across a boundary is a hint the
    /// production code could.
    #[derive(Debug)]
    struct NoSecrets;

    impl SecretStore for NoSecrets {
        fn resolve(
            &self,
            provider: &ProviderId,
            _kind: CredentialKind,
        ) -> Result<ResolvedCredential, SecretError> {
            Err(SecretError::NotFound {
                provider: provider.clone(),
            })
        }

        fn store(
            &self,
            _provider: &ProviderId,
            _credential: Credential,
        ) -> Result<(), SecretError> {
            Ok(())
        }

        fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
            Ok(())
        }
    }

    #[test]
    fn every_offered_model_declares_tool_support() {
        // mjolnr's whole premise is a guarded tool loop. Offering a model that
        // cannot call tools would fail confusingly in Phase 3 rather than here.
        let secrets: Arc<dyn SecretStore> = Arc::new(NoSecrets);
        let provider = OpenAiProvider::new(secrets);

        assert!(!provider.models().is_empty());
        for model in provider.models() {
            assert!(
                model.capabilities.tools,
                "{} is offered but does not declare tool support",
                model.id
            );
            assert!(model.capabilities.streaming, "{} cannot stream", model.id);
        }
    }
}
