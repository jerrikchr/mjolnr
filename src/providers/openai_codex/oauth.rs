//! mjolnr-owned `ChatGPT` OAuth lifecycle.
//!
//! Refresh tokens rotate and are single-use. The mutex is therefore not a
//! performance optimisation: it is the ownership boundary that prevents two
//! requests consuming the same generation. A refreshed access token is never
//! returned until the replacement chain is durably stored.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::core::error::ProviderError;
use crate::core::model::ProviderId;
use crate::core::secrets::{
    Credential, CredentialKind, OAuthCredential, Secret, SecretError, SecretStore,
};

pub(crate) const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_AUTH_BASE_URL: &str = "https://auth.openai.com";
const DEVICE_LOGIN_TIMEOUT: Duration = Duration::from_mins(15);
const EXPIRY_SKEW_SECONDS: i64 = 60;

#[derive(Debug, Clone)]
pub(crate) struct OAuthEndpoints {
    auth_base_url: String,
}

impl Default for OAuthEndpoints {
    fn default() -> Self {
        Self {
            auth_base_url: DEFAULT_AUTH_BASE_URL.to_owned(),
        }
    }
}

impl OAuthEndpoints {
    #[cfg(test)]
    pub(crate) fn for_test(auth_base_url: impl Into<String>) -> Self {
        Self {
            auth_base_url: auth_base_url.into(),
        }
    }

    fn user_code_url(&self) -> String {
        format!("{}/api/accounts/deviceauth/usercode", self.auth_base_url)
    }

    fn device_token_url(&self) -> String {
        format!("{}/api/accounts/deviceauth/token", self.auth_base_url)
    }

    fn oauth_token_url(&self) -> String {
        format!("{}/oauth/token", self.auth_base_url)
    }

    fn redirect_uri(&self) -> String {
        format!("{}/deviceauth/callback", self.auth_base_url)
    }

    fn verification_url(&self) -> String {
        format!("{}/codex/device", self.auth_base_url)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("OAuth transport failed: {detail}")]
    Transport { detail: String },

    #[error("OAuth protocol failed: {detail}")]
    Protocol { detail: String },

    #[error("OAuth credential persistence failed: {0}")]
    Store(SecretError),
}

impl From<reqwest::Error> for OAuthError {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport {
            detail: error.without_url().to_string(),
        }
    }
}

/// What the CLI may show while it polls. Contains no token material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePrompt {
    pub verification_url: String,
    pub user_code: String,
}

#[derive(Debug, Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;

    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| D::Error::custom("interval must be a non-negative integer")),
        serde_json::Value::String(text) => text
            .parse()
            .map_err(|_| D::Error::custom("interval must be an integer")),
        _ => Err(D::Error::custom("interval must be a number or string")),
    }
}

#[derive(Debug, Serialize)]
struct UserCodeRequest<'a> {
    client_id: &'a str,
}

#[derive(Debug, Serialize)]
struct DeviceTokenRequest<'a> {
    device_auth_id: &'a str,
    user_code: &'a str,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

/// Complete the official Codex device-code flow and persist mjolnr's own copy.
pub async fn device_login<F>(secrets: Arc<dyn SecretStore>, announce: F) -> Result<i64, OAuthError>
where
    F: FnOnce(DevicePrompt),
{
    device_login_at(
        reqwest::Client::new(),
        OAuthEndpoints::default(),
        secrets,
        announce,
    )
    .await
}

async fn device_login_at<F>(
    client: reqwest::Client,
    endpoints: OAuthEndpoints,
    secrets: Arc<dyn SecretStore>,
    announce: F,
) -> Result<i64, OAuthError>
where
    F: FnOnce(DevicePrompt),
{
    let response = client
        .post(endpoints.user_code_url())
        .json(&UserCodeRequest {
            client_id: CLIENT_ID,
        })
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(status_error("device-code request", response.status()));
    }
    let user_code: UserCodeResponse = response.json().await?;
    announce(DevicePrompt {
        verification_url: endpoints.verification_url(),
        user_code: user_code.user_code.clone(),
    });

    let code = poll_for_code(&client, &endpoints, &user_code).await?;
    if code.code_challenge.trim().is_empty() {
        return Err(OAuthError::Protocol {
            detail: "device-code response omitted the PKCE challenge".to_owned(),
        });
    }
    let redirect_uri = endpoints.redirect_uri();
    let response = client
        .post(endpoints.oauth_token_url())
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body(&[
            ("grant_type", "authorization_code"),
            ("code", &code.authorization_code),
            ("redirect_uri", &redirect_uri),
            ("client_id", CLIENT_ID),
            ("code_verifier", &code.code_verifier),
        ]))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(status_error("device-code exchange", response.status()));
    }
    let tokens: TokenResponse = response.json().await?;
    let credential = credential_from_token_response(tokens, None)?;
    let expires_at = credential.expires_at_unix();
    store_credential(secrets, credential).await?;
    Ok(expires_at)
}

async fn poll_for_code(
    client: &reqwest::Client,
    endpoints: &OAuthEndpoints,
    user_code: &UserCodeResponse,
) -> Result<DeviceCodeResponse, OAuthError> {
    let started = tokio::time::Instant::now();
    loop {
        let response = client
            .post(endpoints.device_token_url())
            .json(&DeviceTokenRequest {
                device_auth_id: &user_code.device_auth_id,
                user_code: &user_code.user_code,
            })
            .send()
            .await?;
        if response.status().is_success() {
            return response.json().await.map_err(OAuthError::from);
        }
        if !matches!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND
        ) {
            return Err(status_error("device-code poll", response.status()));
        }
        if started.elapsed() >= DEVICE_LOGIN_TIMEOUT {
            return Err(OAuthError::Protocol {
                detail: "device-code login timed out after 15 minutes".to_owned(),
            });
        }
        tokio::time::sleep(Duration::from_secs(user_code.interval.max(1))).await;
    }
}

fn status_error(operation: &str, status: reqwest::StatusCode) -> OAuthError {
    OAuthError::Protocol {
        detail: format!("{operation} returned HTTP {}", status.as_u16()),
    }
}

#[derive(Debug)]
pub(crate) struct OAuthAccess {
    access_token: Secret,
    account_id: String,
}

impl OAuthAccess {
    pub(crate) fn access_token(&self) -> &Secret {
        &self.access_token
    }

    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }
}

/// Owns refresh serialization for one provider instance.
#[derive(Debug)]
pub(crate) struct OAuthManager {
    client: reqwest::Client,
    endpoints: OAuthEndpoints,
    secrets: Arc<dyn SecretStore>,
    refresh_lock: Mutex<()>,
}

impl OAuthManager {
    /// Whether a stored OAuth credential exists.
    ///
    /// Deliberately does not refresh or validate the token: this answers "has
    /// the user logged in", which is what a picker labels. An expired token is
    /// still a login, and its refresh belongs on the request path.
    pub(crate) fn credentialed(&self, provider: &crate::core::model::ProviderId) -> bool {
        self.secrets
            .resolve(provider, crate::core::secrets::CredentialKind::OAuth)
            .is_ok()
    }

    pub(crate) fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoints: OAuthEndpoints::default(),
            secrets,
            refresh_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        secrets: Arc<dyn SecretStore>,
        auth_base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoints: OAuthEndpoints::for_test(auth_base_url),
            secrets,
            refresh_lock: Mutex::new(()),
        }
    }

    /// Resolve an access token, refreshing once when needed.
    ///
    /// Callers must not cancel this future after refresh begins. Consuming a
    /// rotating token and then dropping the persistence step would strand the
    /// chain. The provider therefore completes auth before observing request
    /// cancellation.
    pub(crate) async fn access(&self) -> Result<OAuthAccess, ProviderError> {
        let credential = self.resolve().await?;
        if credential.is_valid_after(unix_time() + EXPIRY_SKEW_SECONDS) {
            return Ok(to_access(credential));
        }

        let _guard = self.refresh_lock.lock().await;
        let credential = self.resolve().await?;
        if credential.is_valid_after(unix_time() + EXPIRY_SKEW_SECONDS) {
            return Ok(to_access(credential));
        }
        self.refresh(credential).await
    }

    async fn resolve(&self) -> Result<OAuthCredential, ProviderError> {
        let secrets = Arc::clone(&self.secrets);
        let provider = ProviderId::new(super::PROVIDER_ID);
        tokio::task::spawn_blocking(move || secrets.resolve(&provider, CredentialKind::OAuth))
            .await
            .map_err(|error| ProviderError::Transport {
                detail: format!("credential resolution task failed: {error}"),
            })?
            .map_err(map_secret_error)?
            .credential
            .into_oauth()
            .ok_or(ProviderError::Auth)
    }

    async fn refresh(&self, credential: OAuthCredential) -> Result<OAuthAccess, ProviderError> {
        let response = self
            .client
            .post(self.endpoints.oauth_token_url())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", credential.refresh_token().expose()),
                ("client_id", CLIENT_ID),
            ]))
            .send()
            .await
            .map_err(|error| ProviderError::Transport {
                detail: error.without_url().to_string(),
            })?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::Auth);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Protocol {
                detail: format!("OAuth refresh returned HTTP {}", response.status().as_u16()),
            });
        }
        let tokens: TokenResponse =
            response
                .json()
                .await
                .map_err(|error| ProviderError::Protocol {
                    detail: format!("OAuth refresh response was malformed: {error}"),
                })?;
        let replacement =
            credential_from_token_response(tokens, Some(credential.refresh_token().expose()))
                .map_err(|error| ProviderError::Protocol {
                    detail: error.to_string(),
                })?;

        // Persist-before-consume: no request may observe the new access token
        // until the rotated refresh generation is durable.
        store_credential(Arc::clone(&self.secrets), replacement)
            .await
            .map_err(|error| ProviderError::Transport {
                detail: error.to_string(),
            })?;

        // Resolve the exact durable generation rather than retaining a second
        // in-memory owner of the replacement refresh token.
        self.resolve().await.map(to_access)
    }
}

fn map_secret_error(error: SecretError) -> ProviderError {
    match error {
        SecretError::NotFound { .. } | SecretError::KindMismatch { .. } => ProviderError::Auth,
        SecretError::Unavailable { detail } => ProviderError::Transport { detail },
    }
}

fn to_access(credential: OAuthCredential) -> OAuthAccess {
    let (access_token, _refresh_token, _expires_at, account_id) = credential.into_parts();
    OAuthAccess {
        access_token,
        account_id,
    }
}

async fn store_credential(
    secrets: Arc<dyn SecretStore>,
    credential: OAuthCredential,
) -> Result<(), OAuthError> {
    let provider = ProviderId::new(super::PROVIDER_ID);
    tokio::task::spawn_blocking(move || secrets.store(&provider, Credential::OAuth(credential)))
        .await
        .map_err(|error| OAuthError::Transport {
            detail: format!("credential persistence task failed: {error}"),
        })?
        .map_err(OAuthError::Store)
}

fn credential_from_token_response(
    mut tokens: TokenResponse,
    prior_refresh_token: Option<&str>,
) -> Result<OAuthCredential, OAuthError> {
    let claims = token_claims(&tokens.access_token)?;
    let account_id = claims.account_id.ok_or_else(|| OAuthError::Protocol {
        detail: "access token omitted the ChatGPT account id claim".to_owned(),
    })?;
    let expires_at = claims
        .expires_at_unix
        .or_else(|| tokens.expires_in.map(|seconds| unix_time() + seconds))
        .ok_or_else(|| OAuthError::Protocol {
            detail: "token response omitted an expiry".to_owned(),
        })?;
    let refresh_token = tokens
        .refresh_token
        .take()
        .or_else(|| prior_refresh_token.map(ToOwned::to_owned))
        .ok_or_else(|| OAuthError::Protocol {
            detail: "token response omitted a refresh token".to_owned(),
        })?;

    Ok(OAuthCredential::new(
        Secret::new(tokens.access_token),
        Secret::new(refresh_token),
        expires_at,
        account_id,
    ))
}

fn form_body(fields: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (position, (name, value)) in fields.iter().enumerate() {
        if position > 0 {
            body.push('&');
        }
        encode_form_component(name, &mut body);
        body.push('=');
        encode_form_component(value, &mut body);
    }
    body
}

fn encode_form_component(value: &str, output: &mut String) {
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(char::from(byte));
            }
            b' ' => output.push('+'),
            _ => {
                output.push('%');
                output.push(hex_digit(byte >> 4));
                output.push(hex_digit(byte & 0x0f));
            }
        }
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'A' + (value - 10)),
        _ => '?',
    }
}

#[derive(Debug)]
struct TokenClaims {
    account_id: Option<String>,
    expires_at_unix: Option<i64>,
}

fn token_claims(token: &str) -> Result<TokenClaims, OAuthError> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| OAuthError::Protocol {
            detail: "access token was not a JWT".to_owned(),
        })?;
    let decoded = decode_base64_url(payload)?;
    let value: serde_json::Value =
        serde_json::from_slice(&decoded).map_err(|_| OAuthError::Protocol {
            detail: "access-token claims were not JSON".to_owned(),
        })?;
    let account_id = value
        .get("chatgpt_account_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
                .and_then(serde_json::Value::as_str)
        })
        .map(ToOwned::to_owned);
    let expires_at_unix = value.get("exp").and_then(serde_json::Value::as_i64);
    Ok(TokenClaims {
        account_id,
        expires_at_unix,
    })
}

fn decode_base64_url(value: &str) -> Result<Vec<u8>, OAuthError> {
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in value.bytes().take_while(|byte| *byte != b'=') {
        let sextet = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a' + 26),
            b'0'..=b'9' => u32::from(byte - b'0' + 52),
            b'-' => 62,
            b'_' => 63,
            _ => {
                return Err(OAuthError::Protocol {
                    detail: "access-token claims used invalid base64url".to_owned(),
                });
            }
        };
        buffer = (buffer << 6) | sextet;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            let octet = u8::try_from(buffer >> bits).map_err(|_| OAuthError::Protocol {
                detail: "access-token claims overflowed base64url decoding".to_owned(),
            })?;
            output.push(octet);
            if bits == 0 {
                buffer = 0;
            } else {
                buffer &= (1_u32 << bits) - 1;
            }
        }
    }
    Ok(output)
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
