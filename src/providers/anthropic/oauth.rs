//! smed-owned Claude Pro/Max subscription OAuth lifecycle.
//!
//! Same ownership rules as the Codex module: the refresh mutex is the
//! boundary that stops two requests consuming one refresh generation, and a
//! refreshed access token is never returned until the replacement chain is
//! durably stored.
//!
//! Flow shape differs from Codex: Anthropic uses an authorization-code PKCE
//! flow where the browser lands on a console page that *displays* the code
//! for the user to paste back (`code#state`), so there is no polling loop and
//! no local callback server. Token requests are JSON, not form-encoded, and
//! access tokens are opaque (`sk-ant-oat…`), not JWTs — expiry comes only
//! from `expires_in`.
//!
//! Endpoint and header facts verified against oh-my-pi's implementation
//! (`registry/oauth/anthropic.ts`, 2026-07-21): authorize on `claude.ai`,
//! token on `api.anthropic.com/v1/oauth/token`, refresh carries
//! `anthropic-beta: oauth-2025-04-20`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tokio::sync::Mutex;

use crate::core::error::ProviderError;
use crate::core::model::ProviderId;
use crate::core::secrets::{
    Credential, CredentialKind, OAuthCredential, Secret, SecretError, SecretStore,
};

/// Claude Code's public OAuth client id; mjolnr authenticates the same way the
/// official CLI does, against the user's own subscription.
pub(crate) const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const DEFAULT_AUTHORIZE_BASE_URL: &str = "https://claude.ai";
const DEFAULT_API_BASE_URL: &str = "https://api.anthropic.com";
/// The console callback renders the authorization code for manual paste.
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
/// `user:inference` is the scope that allows Messages calls with the OAuth
/// token; the other two mirror what the official client requests.
const SCOPES: &str = "org:create_api_key user:profile user:inference";
pub(crate) const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";
pub(crate) const SUBSCRIPTION_BETA_HEADERS: &str = "claude-code-20250219,oauth-2025-04-20";
pub(crate) const SUBSCRIPTION_USER_AGENT: &str = "claude-cli/2.0.0 (external, cli)";
const SUBSCRIPTION_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

pub(crate) fn system_blocks(
    subscription: bool,
    system: Option<&str>,
) -> Option<Vec<super::wire::SystemBlock>> {
    let mut blocks = Vec::new();
    if subscription {
        blocks.push(super::wire::SystemBlock::text(SUBSCRIPTION_IDENTITY));
    }
    if let Some(system) = system.filter(|text| !text.is_empty()) {
        blocks.push(super::wire::SystemBlock::text(system));
    }
    (!blocks.is_empty()).then_some(blocks)
}

const EXPIRY_SKEW_SECONDS: i64 = 60;

#[derive(Debug, Clone)]
pub(crate) struct OAuthEndpoints {
    authorize_base_url: String,
    api_base_url: String,
}

impl Default for OAuthEndpoints {
    fn default() -> Self {
        Self {
            authorize_base_url: DEFAULT_AUTHORIZE_BASE_URL.to_owned(),
            api_base_url: DEFAULT_API_BASE_URL.to_owned(),
        }
    }
}

impl OAuthEndpoints {
    #[cfg(test)]
    pub(crate) fn for_test(base_url: impl Into<String>) -> Self {
        let base = base_url.into();
        Self {
            authorize_base_url: base.clone(),
            api_base_url: base,
        }
    }

    fn authorize_url(&self, challenge: &str, state: &str) -> String {
        let mut url = format!("{}/oauth/authorize?code=true", self.authorize_base_url);
        for (name, value) in [
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ] {
            url.push('&');
            url.push_str(name);
            url.push('=');
            encode_query_component(value, &mut url);
        }
        url
    }

    fn token_url(&self) -> String {
        format!("{}/v1/oauth/token", self.api_base_url)
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

/// What the CLI shows before asking for the pasted code. No token material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastePrompt {
    pub authorize_url: String,
}

#[derive(Debug, Serialize)]
struct ExchangeRequest<'a> {
    grant_type: &'a str,
    client_id: &'a str,
    code: &'a str,
    state: &'a str,
    redirect_uri: &'a str,
    code_verifier: &'a str,
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    grant_type: &'a str,
    client_id: &'a str,
    refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    account: Option<AccountWire>,
}

#[derive(Debug, Deserialize)]
struct AccountWire {
    uuid: Option<String>,
}

/// Run the paste-code login and persist smed's own credential copy.
///
/// `announce` shows the authorize URL; `read_code` returns what the user
/// pasted from the console callback page (`code` or `code#state`).
pub async fn paste_login<F, R>(
    secrets: Arc<dyn SecretStore>,
    announce: F,
    read_code: R,
) -> Result<i64, OAuthError>
where
    F: FnOnce(PastePrompt),
    R: FnOnce() -> Result<String, OAuthError>,
{
    paste_login_at(
        reqwest::Client::new(),
        OAuthEndpoints::default(),
        secrets,
        announce,
        read_code,
    )
    .await
}

async fn paste_login_at<F, R>(
    client: reqwest::Client,
    endpoints: OAuthEndpoints,
    secrets: Arc<dyn SecretStore>,
    announce: F,
    read_code: R,
) -> Result<i64, OAuthError>
where
    F: FnOnce(PastePrompt),
    R: FnOnce() -> Result<String, OAuthError>,
{
    let verifier = random_url_safe(32)?;
    let challenge = base64_url(&sha2::Sha256::digest(verifier.as_bytes()));
    let state = random_url_safe(32)?;

    announce(PastePrompt {
        authorize_url: endpoints.authorize_url(&challenge, &state),
    });
    let pasted = read_code()?;
    // The console page renders `code#state`; accept a bare code too.
    let (code, pasted_state) = match pasted.trim().split_once('#') {
        Some((code, state_part)) if !state_part.is_empty() => {
            (code.to_owned(), state_part.to_owned())
        }
        _ => (pasted.trim().to_owned(), state.clone()),
    };
    if code.is_empty() {
        return Err(OAuthError::Protocol {
            detail: "no authorization code was pasted".to_owned(),
        });
    }

    let response = client
        .post(endpoints.token_url())
        .json(&ExchangeRequest {
            grant_type: "authorization_code",
            client_id: CLIENT_ID,
            code: &code,
            state: &pasted_state,
            redirect_uri: REDIRECT_URI,
            code_verifier: &verifier,
        })
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(status_error("code exchange", response.status()));
    }
    let tokens: TokenResponse = response.json().await?;
    let credential = credential_from_token_response(tokens, None)?;
    let expires_at = credential.expires_at_unix();
    store_credential(secrets, credential).await?;
    Ok(expires_at)
}

fn status_error(operation: &str, status: reqwest::StatusCode) -> OAuthError {
    OAuthError::Protocol {
        detail: format!("{operation} returned HTTP {}", status.as_u16()),
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
    pub(crate) fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoints: OAuthEndpoints::default(),
            secrets,
            refresh_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(secrets: Arc<dyn SecretStore>, base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoints: OAuthEndpoints::for_test(base_url),
            secrets,
            refresh_lock: Mutex::new(()),
        }
    }

    /// Whether a stored subscription credential exists (no refresh, no
    /// validation — an expired token is still a login).
    pub(crate) fn credentialed(&self) -> bool {
        self.secrets
            .resolve(&ProviderId::new(super::PROVIDER_ID), CredentialKind::OAuth)
            .is_ok()
    }

    /// Resolve an access token, refreshing once when needed. Callers must not
    /// cancel after refresh begins (see the Codex module for why).
    pub(crate) async fn access(&self) -> Result<Secret, ProviderError> {
        let credential = self.resolve().await?;
        if credential.is_valid_after(unix_time() + EXPIRY_SKEW_SECONDS) {
            return Ok(access_token(credential));
        }

        let _guard = self.refresh_lock.lock().await;
        let credential = self.resolve().await?;
        if credential.is_valid_after(unix_time() + EXPIRY_SKEW_SECONDS) {
            return Ok(access_token(credential));
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

    async fn refresh(&self, credential: OAuthCredential) -> Result<Secret, ProviderError> {
        let response = self
            .client
            .post(self.endpoints.token_url())
            .header("anthropic-beta", OAUTH_BETA_HEADER)
            .json(&RefreshRequest {
                grant_type: "refresh_token",
                client_id: CLIENT_ID,
                refresh_token: credential.refresh_token().expose(),
            })
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

        // Persist-before-consume, as in the Codex module.
        store_credential(Arc::clone(&self.secrets), replacement)
            .await
            .map_err(|error| ProviderError::Transport {
                detail: error.to_string(),
            })?;
        self.resolve().await.map(access_token)
    }
}

fn map_secret_error(error: SecretError) -> ProviderError {
    match error {
        SecretError::NotFound { .. } | SecretError::KindMismatch { .. } => ProviderError::Auth,
        SecretError::Unavailable { detail } => ProviderError::Transport { detail },
    }
}

fn access_token(credential: OAuthCredential) -> Secret {
    let (access_token, _refresh_token, _expires_at, _account_id) = credential.into_parts();
    access_token
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
    let refresh_token = tokens
        .refresh_token
        .take()
        .filter(|token| !token.is_empty())
        .or_else(|| prior_refresh_token.map(ToOwned::to_owned))
        .ok_or_else(|| OAuthError::Protocol {
            detail: "token response omitted a refresh token".to_owned(),
        })?;
    // Anthropic access tokens are opaque, so `expires_in` is the only expiry.
    let expires_at = unix_time() + tokens.expires_in;
    let account_id = tokens
        .account
        .and_then(|account| account.uuid)
        .unwrap_or_else(|| "unknown".to_owned());

    Ok(OAuthCredential::new(
        Secret::new(tokens.access_token),
        Secret::new(refresh_token),
        expires_at,
        account_id,
    ))
}

fn random_url_safe(bytes: usize) -> Result<String, OAuthError> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer).map_err(|error| OAuthError::Protocol {
        detail: format!("no system randomness for PKCE: {error}"),
    })?;
    Ok(base64_url(&buffer))
}

fn base64_url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let sextet_char = |buffer: u32, shift: u32| {
        let index = usize::try_from((buffer >> shift) & 63).unwrap_or(0);
        char::from(ALPHABET.get(index).copied().unwrap_or(b'A'))
    };
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let buffer = u32::from(chunk.first().copied().unwrap_or(0)) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        output.push(sextet_char(buffer, 18));
        output.push(sextet_char(buffer, 12));
        if chunk.len() > 1 {
            output.push(sextet_char(buffer, 6));
        }
        if chunk.len() > 2 {
            output.push(sextet_char(buffer, 0));
        }
    }
    output
}

fn encode_query_component(value: &str, output: &mut String) {
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(char::from(byte));
            }
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

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secrets::{ResolvedCredential, SecretSource};
    use std::sync::Mutex as StdMutex;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Secrets are not clonable by design, so the store keeps raw parts and
    /// rebuilds a credential per resolve, as a keychain would.
    #[derive(Debug, Default)]
    struct MemoryStore {
        parts: StdMutex<Option<(String, String, i64, String)>>,
    }

    impl SecretStore for MemoryStore {
        fn resolve(
            &self,
            provider: &ProviderId,
            _kind: CredentialKind,
        ) -> Result<ResolvedCredential, SecretError> {
            let held = self.parts.lock().expect("lock");
            let Some((access, refresh, expires_at, account)) = held.as_ref() else {
                return Err(SecretError::NotFound {
                    provider: provider.clone(),
                });
            };
            Ok(ResolvedCredential {
                credential: Credential::OAuth(OAuthCredential::new(
                    Secret::new(access.clone()),
                    Secret::new(refresh.clone()),
                    *expires_at,
                    account.clone(),
                )),
                source: SecretSource::Keyring,
            })
        }

        fn store(&self, _provider: &ProviderId, credential: Credential) -> Result<(), SecretError> {
            let oauth = credential.into_oauth().expect("OAuth credential");
            let (access, refresh, expires_at, account) = oauth.into_parts();
            *self.parts.lock().expect("lock") = Some((
                access.expose().to_owned(),
                refresh.expose().to_owned(),
                expires_at,
                account,
            ));
            Ok(())
        }

        fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
            *self.parts.lock().expect("lock") = None;
            Ok(())
        }
    }

    #[tokio::test]
    async fn paste_login_exchanges_the_code_and_stores_the_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(body_partial_json(serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": CLIENT_ID,
                "code": "the-code",
                "state": "the-state",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "sk-ant-oat01-access",
                "refresh_token": "sk-ant-ort01-refresh",
                "expires_in": 3600,
                "account": {"uuid": "acct-1"}
            })))
            .mount(&server)
            .await;

        let store = Arc::new(MemoryStore::default());
        let mut seen_url = None;
        let expires_at = paste_login_at(
            reqwest::Client::new(),
            OAuthEndpoints::for_test(server.uri()),
            Arc::clone(&store) as Arc<dyn SecretStore>,
            |prompt| seen_url = Some(prompt.authorize_url),
            || Ok("the-code#the-state".to_owned()),
        )
        .await
        .expect("login");

        assert!(expires_at > unix_time());
        let url = seen_url.expect("announced");
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.contains(CLIENT_ID), "{url}");
        let stored = store
            .resolve(
                &ProviderId::new(super::super::PROVIDER_ID),
                CredentialKind::OAuth,
            )
            .expect("stored");
        assert!(stored.credential.oauth().is_some());
    }

    #[tokio::test]
    async fn an_expired_credential_refreshes_with_the_beta_header_before_use() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(header("anthropic-beta", OAUTH_BETA_HEADER))
            .and(body_partial_json(serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": "sk-ant-ort01-old",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "sk-ant-oat01-new",
                "refresh_token": "sk-ant-ort01-new",
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;

        let store = Arc::new(MemoryStore::default());
        store
            .store(
                &ProviderId::new(super::super::PROVIDER_ID),
                Credential::OAuth(OAuthCredential::new(
                    Secret::new("sk-ant-oat01-old".to_owned()),
                    Secret::new("sk-ant-ort01-old".to_owned()),
                    unix_time() - 10,
                    "acct-1".to_owned(),
                )),
            )
            .expect("seed");

        let manager =
            OAuthManager::for_test(Arc::clone(&store) as Arc<dyn SecretStore>, server.uri());
        let access = manager.access().await.expect("refresh");
        assert_eq!(access.expose(), "sk-ant-oat01-new");
    }

    #[test]
    fn base64_url_matches_the_rfc_alphabet_without_padding() {
        assert_eq!(
            base64_url(b"any carnal pleasur"),
            "YW55IGNhcm5hbCBwbGVhc3Vy"
        );
        assert!(!base64_url(&[0xfb, 0xff]).contains('='));
        assert!(base64_url(&[0xfb, 0xef, 0xff]).contains('_'));
    }
}
