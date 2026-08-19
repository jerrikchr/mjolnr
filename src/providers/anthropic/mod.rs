//! Native Anthropic Messages adapter (plan Phase 6).

mod oauth;
mod stream;
mod translate;
mod wire;

pub use oauth::{OAuthError, PastePrompt, paste_login};

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::{SecretError, SecretStore};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const API_VERSION: &str = "2023-06-01";
// smed's request ceiling, not a claim about each model's provider maximum.
// Keeping one bounded client-side limit is intentional until output budgets are
// configurable in the canonical request contract.
const MJOLNR_MAX_OUTPUT_TOKENS: u32 = 16_384;

pub const PROVIDER_ID: &str = "anthropic";
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";

const MODELS: &[(&str, &str, u32, u32)] = &[
    ("claude-sonnet-5", "Claude Sonnet 5", 1_000_000, 16_384),
    ("claude-opus-4-8", "Claude Opus 4.8", 1_000_000, 16_384),
    (DEFAULT_MODEL, "Claude Haiku 4.5", 200_000, 8_192),
];

#[derive(Debug)]
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    secrets: Arc<dyn SecretStore>,
    oauth: oauth::OAuthManager,
}

impl AnthropicProvider {
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            oauth: oauth::OAuthManager::new(Arc::clone(&secrets)),
            secrets,
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn provider_id() -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        Self::provider_id()
    }

    fn credentialed(&self) -> bool {
        // A Pro/Max subscription login or an API key both count as configured.
        self.oauth.credentialed()
            || self
                .secrets
                .resolve(
                    &Self::provider_id(),
                    crate::core::secrets::CredentialKind::ApiKey,
                )
                .is_ok()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        MODELS
            .iter()
            .map(|(id, name, context, max_output)| {
                let id = ModelId::new(*id);
                let provider = Self::provider_id();
                ModelDescriptor {
                    tier: crate::core::model::ModelTier::curated(&provider, &id),
                    id,
                    provider,
                    display_name: (*name).to_owned(),
                    capabilities: ModelCapabilities::text_tools_and_images(),
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
        // Subscription OAuth wins over an API key when both exist: the user
        // who ran the subscription login expects their plan, not metered
        // billing, to serve the request.
        let secrets = Arc::clone(&self.secrets);
        let provider = Self::provider_id();
        let oauth_probe = tokio::task::spawn_blocking(move || {
            secrets.resolve(&provider, crate::core::secrets::CredentialKind::OAuth)
        });
        let oauth_probe = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = oauth_probe => result.map_err(|error| ProviderError::Transport {
                detail: format!("credential resolution task failed: {error}"),
            })?,
        };

        let mut is_subscription = false;
        let mut builder = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("anthropic-version", API_VERSION);
        match oauth_probe {
            // A store may answer any kind with what it holds, so require the
            // credential to actually be OAuth before taking the Bearer path.
            Ok(resolved) if resolved.credential.oauth().is_some() => {
                is_subscription = true;
                // Refresh is not cancellable mid-flight (rotating tokens).
                let token = self.oauth.access().await?;
                builder = builder
                    .header("authorization", format!("Bearer {}", token.expose()))
                    .header("anthropic-beta", oauth::SUBSCRIPTION_BETA_HEADERS)
                    .header(reqwest::header::USER_AGENT, oauth::SUBSCRIPTION_USER_AGENT);
            }
            Ok(_)
            | Err(
                crate::core::secrets::SecretError::NotFound { .. }
                | crate::core::secrets::SecretError::KindMismatch { .. },
            ) => {
                let secrets = Arc::clone(&self.secrets);
                let provider = Self::provider_id();
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
                builder = builder.header("x-api-key", secret.expose());
            }
            Err(crate::core::secrets::SecretError::Unavailable { detail }) => {
                return Err(ProviderError::Transport { detail });
            }
        }

        let max_tokens = MODELS
            .iter()
            .find(|(id, _, _, _)| *id == request.model.as_str())
            .map_or(MJOLNR_MAX_OUTPUT_TOKENS, |(_, _, _, max_out)| *max_out);

        let body = wire::CreateMessage {
            model: request.model.to_string(),
            max_tokens,
            messages: translate::messages(&request),
            system: oauth::system_blocks(is_subscription, request.system.as_deref()),
            tools: translate::tools(&request),
            stream: true,
        };
        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = builder
                .json(&body)
                .send() => result.map_err(|error| ProviderError::Transport {
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
            let retry_after = retry_after_seconds(response.headers());
            let names_a_limit = names_a_limit(response.headers());
            let body = response.text().await.unwrap_or_default();
            return Err(map_http_error(status, &body, retry_after, names_a_limit));
        }
        stream::decode(response, &events, &cancel).await
    }
}

fn map_secret_error(error: SecretError) -> ProviderError {
    match error {
        SecretError::NotFound { .. } | SecretError::KindMismatch { .. } => ProviderError::Auth,
        SecretError::Unavailable { detail } => ProviderError::Transport { detail },
    }
}

fn map_http_error(
    status: reqwest::StatusCode,
    body: &str,
    retry_after_seconds: Option<u64>,
    names_a_limit: bool,
) -> ProviderError {
    let kind = serde_json::from_str::<wire::ErrorResponse>(body)
        .ok()
        .map(|body| body.error.kind);
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => ProviderError::Auth,
        // A throttle names the limit it enforced: a retry-after, a rate-limit
        // window, or `rate_limit_error` in the body. With none of the three,
        // the 429 is a refusal smed cannot attribute to the caller's quota,
        // and saying "wait for the reset" would invent a cause.
        reqwest::StatusCode::TOO_MANY_REQUESTS
            if retry_after_seconds.is_some()
                || names_a_limit
                || kind.as_deref() == Some("rate_limit_error") =>
        {
            ProviderError::RateLimit {
                retry_after_seconds,
            }
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimitUnexplained,
        // 529 is Anthropic being out of capacity, not the caller being
        // throttled. Reporting it as a rate limit blames the wrong party.
        status if status.as_u16() == 529 => ProviderError::Overloaded {
            retry_after_seconds,
        },
        _ => ProviderError::Protocol {
            detail: format!(
                "http {}{}",
                status.as_u16(),
                kind.map(|kind| format!(" ({kind})")).unwrap_or_default()
            ),
        },
    }
}

/// Whether the response carries a rate-limit window at all.
///
/// Anthropic attaches `anthropic-ratelimit-*` headers when it is enforcing a
/// limit. Their absence on a 429 is the signal that nothing was actually
/// throttled.
fn names_a_limit(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .keys()
        .any(|name| name.as_str().starts_with("anthropic-ratelimit-"))
}

fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;
    use reqwest::header::HeaderMap;

    const RATE_LIMITED: &str = r#"{"error":{"type":"rate_limit_error"}}"#;

    #[test]
    fn a_429_that_names_no_limit_is_not_reported_as_the_users_quota() {
        // A subscription token the endpoint declines is answered with a bare
        // 429: no retry-after, no rate-limit window, no rate_limit_error. Read
        // as a throttle, it tells a user with a full quota to wait for a reset
        // that will never come — which is exactly how this reached us.
        let error = map_http_error(StatusCode::TOO_MANY_REQUESTS, "", None, false);
        assert!(matches!(error, ProviderError::RateLimitUnexplained));
        assert!(
            !error.to_string().contains("rate limited"),
            "an unattributable refusal must not read as the caller's limit: {error}"
        );
    }

    #[test]
    fn a_retry_after_is_enough_to_call_it_a_rate_limit() {
        let error = map_http_error(StatusCode::TOO_MANY_REQUESTS, "", Some(30), false);
        assert!(matches!(
            error,
            ProviderError::RateLimit {
                retry_after_seconds: Some(30)
            }
        ));
    }

    #[test]
    fn a_rate_limit_window_is_enough_without_a_retry_after() {
        // Anthropic does not always send retry-after, so the window headers
        // have to count on their own or real throttles get misfiled.
        let error = map_http_error(StatusCode::TOO_MANY_REQUESTS, "", None, true);
        assert!(matches!(error, ProviderError::RateLimit { .. }));
    }

    #[test]
    fn the_body_alone_can_confirm_a_throttle() {
        let error = map_http_error(StatusCode::TOO_MANY_REQUESTS, RATE_LIMITED, None, false);
        assert!(matches!(error, ProviderError::RateLimit { .. }));
    }

    #[test]
    fn rate_limit_headers_are_recognised_by_prefix() {
        let mut headers = HeaderMap::new();
        assert!(!names_a_limit(&headers));
        headers.insert(
            "anthropic-ratelimit-requests-remaining",
            "0".parse().unwrap(),
        );
        assert!(names_a_limit(&headers));
    }

    #[test]
    fn a_529_stays_an_overload_rather_than_an_unexplained_429() {
        let error = map_http_error(StatusCode::from_u16(529).unwrap(), "", None, false);
        assert!(matches!(error, ProviderError::Overloaded { .. }));
    }
}
