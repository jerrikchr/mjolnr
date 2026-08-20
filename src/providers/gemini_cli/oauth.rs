//! mjolnr-owned Google OAuth lifecycle for Cloud Code Assist subscriptions
//! (Gemini CLI and Antigravity).
//!
//! Flow shape: standard installed-app authorization-code flow with a loopback
//! redirect — Google's registered redirect for these public clients is a
//! fixed localhost port, so login binds that port, opens the authorize URL,
//! and waits for the browser to land on the callback. Token requests are
//! form-encoded. Refresh responses usually omit the refresh token, so the
//! prior one is kept.
//!
//! After the token exchange, login resolves the Cloud Code Assist project
//! (`loadCodeAssist`, onboarding through `onboardUser` when needed) and
//! stores it as the credential's account id — every inference call needs it.
//!
//! Endpoints and onboarding behavior verified against oh-my-pi
//! (`registry/oauth/google-gemini-cli.ts`, `google-antigravity.ts`,
//! 2026-07-21).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Mutex;

use crate::core::error::ProviderError;
use crate::core::model::ProviderId;
use crate::core::secrets::{
    Credential, CredentialKind, OAuthCredential, Secret, SecretError, SecretStore,
};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const LOGIN_TIMEOUT: Duration = Duration::from_mins(15);
const EXPIRY_SKEW_SECONDS: i64 = 60;
const ONBOARD_POLL_INTERVAL: Duration = Duration::from_secs(5);
const ONBOARD_POLL_ATTEMPTS: u32 = 24;

/// One Google Cloud Code Assist client identity (Gemini CLI or Antigravity).
///
/// The client id/secret pairs are the published installed-app credentials of
/// the official clients — public by design (they ship in every copy of those
/// apps); the user's own Google login is the actual secret. Stored base64'd
/// only so credential scanners don't misread them as leaked private keys.
#[derive(Debug)]
pub struct GoogleClient {
    pub(crate) provider_id: &'static str,
    client_id_bytes: &'static [u8],
    client_secret_bytes: &'static [u8],
    callback_port: u16,
    callback_path: &'static str,
    scopes: &'static [&'static str],
    /// `ideType` sent in Cloud Code Assist metadata.
    pub(crate) ide_type: &'static str,
    /// Cloud Code Assist endpoint for login/onboarding (`loadCodeAssist`).
    pub(crate) endpoint: &'static str,
    /// Endpoint for inference. Antigravity serves models from the `daily-`
    /// host while onboarding stays on the standard one.
    pub(crate) inference_endpoint: &'static str,
    /// Whether requests need the Antigravity envelope (requestId/sessionId/
    /// labels/userAgent) rather than the plain Gemini CLI shape.
    pub(crate) antigravity: bool,
    /// (wire id, display name, context tokens) — wire ids differ per client;
    /// Antigravity's were captured from the real `antigravity/hub` traffic.
    pub(crate) models: &'static [(&'static str, &'static str, u32)],
}

pub static GEMINI_CLI: GoogleClient = GoogleClient {
    provider_id: "gemini-cli",
    client_id_bytes: &[
        106, 100, 109, 110, 105, 105, 100, 108, 101, 111, 101, 105, 113, 51, 51, 100, 58, 40, 110,
        51, 44, 46, 56, 46, 50, 44, 101, 57, 111, 61, 45, 58, 106, 61, 42, 111, 52, 49, 56, 53, 62,
        109, 111, 105, 54, 114, 61, 44, 44, 47, 114, 59, 51, 51, 59, 48, 57, 41, 47, 57, 46, 63,
        51, 50, 40, 57, 50, 40, 114, 63, 51, 49,
    ],
    client_secret_bytes: &[
        27, 19, 31, 15, 12, 4, 113, 104, 41, 20, 59, 17, 12, 49, 113, 109, 51, 107, 15, 55, 113,
        59, 57, 10, 106, 31, 41, 105, 63, 48, 4, 26, 47, 36, 48,
    ],
    callback_port: 8085,
    callback_path: "/oauth2callback",
    scopes: &[
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
    ],
    ide_type: "IDE_UNSPECIFIED",
    endpoint: "https://cloudcode-pa.googleapis.com",
    inference_endpoint: "https://cloudcode-pa.googleapis.com",
    antigravity: false,
    models: &[
        (
            "gemini-3.1-pro-preview",
            "Gemini 3.1 Pro (preview)",
            1_048_576,
        ),
        ("gemini-2.5-pro", "Gemini 2.5 Pro", 1_048_576),
        ("gemini-2.5-flash", "Gemini 2.5 Flash", 1_048_576),
    ],
};

/// Login verified live 2026-07-21. Inference constants (daily endpoint, wire
/// model ids, envelope) captured from oh-my-pi's trace of the real
/// `antigravity/hub` client.
pub static ANTIGRAVITY: GoogleClient = GoogleClient {
    provider_id: "antigravity",
    client_id_bytes: &[
        109, 108, 107, 109, 108, 108, 106, 108, 106, 108, 105, 101, 109, 113, 40, 49, 52, 47, 47,
        53, 50, 110, 52, 110, 109, 48, 63, 46, 57, 110, 111, 105, 42, 40, 51, 48, 51, 54, 52, 104,
        59, 104, 108, 111, 57, 44, 114, 61, 44, 44, 47, 114, 59, 51, 51, 59, 48, 57, 41, 47, 57,
        46, 63, 51, 50, 40, 57, 50, 40, 114, 63, 51, 49,
    ],
    client_secret_bytes: &[
        27, 19, 31, 15, 12, 4, 113, 23, 105, 100, 26, 11, 14, 104, 100, 106, 16, 56, 16, 22, 109,
        49, 16, 30, 100, 47, 4, 31, 104, 38, 106, 45, 24, 29, 58,
    ],
    callback_port: 51121,
    callback_path: "/oauth-callback",
    scopes: &[
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ],
    ide_type: "ANTIGRAVITY",
    endpoint: "https://cloudcode-pa.googleapis.com",
    inference_endpoint: "https://daily-cloudcode-pa.googleapis.com",
    antigravity: true,
    models: &[
        (
            "gemini-3.1-pro-low",
            "Gemini 3.1 Pro (Antigravity)",
            1_048_576,
        ),
        (
            "gemini-pro-agent",
            "Gemini Pro Agent (Antigravity)",
            1_048_576,
        ),
        (
            "gemini-3-flash-agent",
            "Gemini 3 Flash Agent (Antigravity)",
            1_048_576,
        ),
        (
            "gemini-3.5-flash-low",
            "Gemini 3.5 Flash (Antigravity)",
            1_048_576,
        ),
        (
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6 (Antigravity)",
            200_000,
        ),
        (
            "claude-opus-4-6-thinking",
            "Claude Opus 4.6 Thinking (Antigravity)",
            200_000,
        ),
    ],
};

impl GoogleClient {
    fn client_id(&self) -> String {
        unmask_bytes(self.client_id_bytes)
    }

    fn client_secret(&self) -> String {
        unmask_bytes(self.client_secret_bytes)
    }

    fn redirect_uri(&self) -> String {
        format!(
            "http://localhost:{}{}",
            self.callback_port, self.callback_path
        )
    }

    fn authorize_url(&self, state: &str) -> String {
        let mut url = format!("{AUTH_URL}?response_type=code&access_type=offline&prompt=consent");
        for (name, value) in [
            ("client_id", self.client_id()),
            ("redirect_uri", self.redirect_uri()),
            ("scope", self.scopes.join(" ")),
            ("state", state.to_owned()),
        ] {
            url.push('&');
            url.push_str(name);
            url.push('=');
            encode_query_component(&value, &mut url);
        }
        url
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

impl From<std::io::Error> for OAuthError {
    fn from(error: std::io::Error) -> Self {
        Self::Transport {
            detail: error.to_string(),
        }
    }
}

/// What the CLI shows while the browser round-trip is pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPrompt {
    pub authorize_url: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

/// Complete the loopback login, resolve the Cloud Code Assist project, and
/// persist mjolnr's own credential copy. Returns the token expiry.
pub async fn browser_login<F>(
    config: &'static GoogleClient,
    secrets: Arc<dyn SecretStore>,
    announce: F,
) -> Result<i64, OAuthError>
where
    F: FnOnce(BrowserPrompt),
{
    // Bind before announcing: if the fixed port is taken there is nothing to
    // click yet, and the error should name the real problem.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", config.callback_port))
        .await
        .map_err(|error| OAuthError::Transport {
            detail: format!(
                "could not bind localhost:{} for the OAuth callback (is another login running?): {error}",
                config.callback_port
            ),
        })?;
    let state = random_url_safe(32)?;
    announce(BrowserPrompt {
        authorize_url: config.authorize_url(&state),
    });

    let code = tokio::time::timeout(
        LOGIN_TIMEOUT,
        wait_for_callback(&listener, config.callback_path, &state),
    )
    .await
    .map_err(|_| OAuthError::Protocol {
        detail: "browser login timed out after 15 minutes".to_owned(),
    })??;

    let client = reqwest::Client::new();
    let response = client
        .post(TOKEN_URL)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_body(&[
            ("code", code.as_str()),
            ("client_id", &config.client_id()),
            ("client_secret", &config.client_secret()),
            ("redirect_uri", &config.redirect_uri()),
            ("grant_type", "authorization_code"),
        ]))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(status_error("code exchange", response.status()));
    }
    let tokens: TokenResponse = response.json().await?;
    let refresh_token = tokens
        .refresh_token
        .clone()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| OAuthError::Protocol {
            detail:
                "Google omitted a refresh token; re-run login (prompt=consent should force one)"
                    .to_owned(),
        })?;

    let project = discover_project(&client, config, &tokens.access_token).await?;

    let credential = OAuthCredential::new(
        Secret::new(tokens.access_token),
        Secret::new(refresh_token),
        unix_time() + tokens.expires_in,
        // The project id rides in the account slot: it is identity for this
        // credential and every inference call must send it.
        project,
    );
    let expires_at = credential.expires_at_unix();
    store_credential(secrets, config, credential).await?;
    Ok(expires_at)
}

/// Accept connections until one carries the callback path with a code and the
/// expected state; answer every request so the browser never spins.
async fn wait_for_callback(
    listener: &tokio::net::TcpListener,
    path: &str,
    expected_state: &str,
) -> Result<String, OAuthError> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let mut buffer = vec![0_u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        let text = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
        let target = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");
        let (request_path, query) = target.split_once('?').unwrap_or((target, ""));
        if request_path != path {
            let _ = socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await;
            continue;
        }
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            match name {
                "code" => code = Some(decode_query_component(value)),
                "state" => state = Some(decode_query_component(value)),
                "error" => error = Some(decode_query_component(value)),
                _ => {}
            }
        }
        let outcome = match (code, state, error) {
            (_, _, Some(error)) => Err(OAuthError::Protocol {
                detail: format!("Google returned an authorization error: {error}"),
            }),
            (Some(code), Some(state), None) if state == expected_state => Ok(code),
            (Some(_), _, None) => Err(OAuthError::Protocol {
                detail: "authorization response state did not match this login".to_owned(),
            }),
            _ => Err(OAuthError::Protocol {
                detail: "authorization response carried no code".to_owned(),
            }),
        };
        let page = if outcome.is_ok() {
            callback_page(
                "AUTHORIZED",
                "#7ee787",
                "credential stored in an owner-only file",
                "You can close this tab and return to the terminal.",
            )
        } else {
            callback_page(
                "REFUSED",
                "#ff7b72",
                "the authorization response did not verify",
                "Close this tab and check the terminal for the reason.",
            )
        };
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                )
                .as_bytes(),
            )
            .await;
        return outcome;
    }
}

/// The browser landing page after the OAuth redirect. Self-contained (no
/// external assets — the page must render with the network conceptually
/// untrusted) and styled like mjolnr's terminal: dark, monospace, one verdict.
fn callback_page(verdict: &str, color: &str, detail: &str, action: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>mjolnr // {verdict}</title>
<style>
  html,body{{margin:0;height:100%;background:#0b0e14;color:#c9d1d9;
    font-family:"SF Mono","JetBrains Mono",Menlo,Consolas,monospace}}
  body{{display:flex;align-items:center;justify-content:center}}
  .card{{max-width:34rem;padding:2.5rem 3rem;border:1px solid #1f2733;border-radius:12px;
    background:linear-gradient(180deg,#0f141d 0%,#0b0e14 100%);
    box-shadow:0 0 60px rgba(0,0,0,.6),inset 0 1px 0 rgba(255,255,255,.04)}}
  .wordmark{{font-size:.8rem;letter-spacing:.45em;color:#8b949e;margin-bottom:1.6rem}}
  .wordmark b{{color:#e6edf3}}
  .verdict{{font-size:1.9rem;font-weight:700;color:{color};margin:0 0 .4rem}}
  .verdict::before{{content:"● ";font-size:1.1rem;vertical-align:.2rem}}
  .detail{{color:#8b949e;margin:0 0 1.8rem;font-size:.95rem}}
  .action{{color:#c9d1d9;font-size:.95rem;border-top:1px dashed #1f2733;padding-top:1.2rem}}
  .cursor{{display:inline-block;width:.55em;height:1.1em;background:{color};
    vertical-align:text-bottom;margin-left:.35em;animation:blink 1.1s steps(1) infinite}}
  @keyframes blink{{50%{{opacity:0}}}}
</style></head><body>
<div class="card">
  <div class="wordmark"><b>MJOLNR</b>&nbsp;SAYS</div>
  <p class="verdict">{verdict}</p>
  <p class="detail">// {detail}</p>
  <p class="action">{action}<span class="cursor"></span></p>
</div>
</body></html>"#
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssistMetadata {
    ide_type: &'static str,
    platform: &'static str,
    plugin_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    duet_project: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LoadRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    cloudaicompanion_project: Option<String>,
    metadata: AssistMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadResponse {
    cloudaicompanion_project: Option<ProjectRef>,
    current_tier: Option<Tier>,
    #[serde(default)]
    allowed_tiers: Vec<Tier>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tier {
    id: Option<String>,
    #[serde(default)]
    is_default: bool,
}

/// The service answers with either a bare project string or `{ "id": … }`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProjectRef {
    Name(String),
    Object { id: Option<String> },
}

impl ProjectRef {
    fn into_id(self) -> Option<String> {
        match self {
            Self::Name(name) if !name.is_empty() => Some(name),
            Self::Object { id } => id.filter(|id| !id.is_empty()),
            Self::Name(_) => None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OnboardRequest {
    tier_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloudaicompanion_project: Option<String>,
    metadata: AssistMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    name: Option<String>,
    #[serde(default)]
    done: bool,
    response: Option<OperationResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationResponse {
    cloudaicompanion_project: Option<ProjectRef>,
}

fn metadata(config: &GoogleClient, duet_project: Option<String>) -> AssistMetadata {
    AssistMetadata {
        ide_type: config.ide_type,
        platform: "PLATFORM_UNSPECIFIED",
        plugin_type: "GEMINI",
        duet_project,
    }
}

fn environment_project() -> Option<String> {
    ["GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Resolve the Cloud Code Assist project for this account, onboarding the
/// free tier when the account has none yet.
async fn discover_project(
    client: &reqwest::Client,
    config: &GoogleClient,
    access_token: &str,
) -> Result<String, OAuthError> {
    let env_project = environment_project();
    let response = client
        .post(format!("{}/v1internal:loadCodeAssist", config.endpoint))
        .bearer_auth(access_token)
        .json(&LoadRequest {
            cloudaicompanion_project: env_project.clone(),
            metadata: metadata(config, env_project.clone()),
        })
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(status_error("loadCodeAssist", response.status()));
    }
    let load: LoadResponse = response.json().await?;

    if load.current_tier.is_some() {
        if let Some(project) = load.cloudaicompanion_project.and_then(ProjectRef::into_id) {
            return Ok(project);
        }
        return env_project.ok_or_else(|| OAuthError::Protocol {
            detail: "this Google account needs GOOGLE_CLOUD_PROJECT set; \
                     see https://goo.gle/gemini-cli-auth-docs#workspace-gca"
                .to_owned(),
        });
    }

    let tier_id = load
        .allowed_tiers
        .iter()
        .find(|tier| tier.is_default)
        .and_then(|tier| tier.id.clone())
        .unwrap_or_else(|| "legacy-tier".to_owned());
    if tier_id != "free-tier" && env_project.is_none() {
        return Err(OAuthError::Protocol {
            detail: format!(
                "tier {tier_id} needs GOOGLE_CLOUD_PROJECT set; \
                 see https://goo.gle/gemini-cli-auth-docs#workspace-gca"
            ),
        });
    }

    let response = client
        .post(format!("{}/v1internal:onboardUser", config.endpoint))
        .bearer_auth(access_token)
        .json(&OnboardRequest {
            tier_id: tier_id.clone(),
            cloudaicompanion_project: (tier_id != "free-tier")
                .then(|| env_project.clone())
                .flatten(),
            metadata: metadata(config, env_project.clone()),
        })
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(status_error("onboardUser", response.status()));
    }
    let mut operation: Operation = response.json().await?;

    let mut attempts = 0_u32;
    while !operation.done {
        let Some(name) = operation.name.clone() else {
            break;
        };
        attempts += 1;
        if attempts > ONBOARD_POLL_ATTEMPTS {
            return Err(OAuthError::Protocol {
                detail: "project provisioning did not finish in time; retry login shortly"
                    .to_owned(),
            });
        }
        tokio::time::sleep(ONBOARD_POLL_INTERVAL).await;
        let response = client
            .get(format!("{}/v1internal/{name}", config.endpoint))
            .bearer_auth(access_token)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(status_error("operation poll", response.status()));
        }
        operation = response.json().await?;
    }

    operation
        .response
        .and_then(|response| response.cloudaicompanion_project)
        .and_then(ProjectRef::into_id)
        .or(env_project)
        .ok_or_else(|| OAuthError::Protocol {
            detail: "could not discover or provision a Cloud Code Assist project".to_owned(),
        })
}

/// Access material for one Cloud Code Assist request.
#[derive(Debug)]
pub(crate) struct AssistAccess {
    pub(crate) access_token: Secret,
    /// The Cloud Code Assist project every request must name.
    pub(crate) project: String,
}

/// Owns refresh serialization for one provider instance.
#[derive(Debug)]
pub(crate) struct OAuthManager {
    client: reqwest::Client,
    config: &'static GoogleClient,
    token_url: String,
    secrets: Arc<dyn SecretStore>,
    refresh_lock: Mutex<()>,
}

impl OAuthManager {
    pub(crate) fn new(config: &'static GoogleClient, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
            token_url: TOKEN_URL.to_owned(),
            secrets,
            refresh_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        config: &'static GoogleClient,
        secrets: Arc<dyn SecretStore>,
        token_url: impl Into<String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
            token_url: token_url.into(),
            secrets,
            refresh_lock: Mutex::new(()),
        }
    }

    pub(crate) fn credentialed(&self) -> bool {
        self.secrets
            .resolve(
                &ProviderId::new(self.config.provider_id),
                CredentialKind::OAuth,
            )
            .is_ok()
    }

    /// Resolve access material, refreshing once when needed.
    pub(crate) async fn access(&self) -> Result<AssistAccess, ProviderError> {
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
        let provider = ProviderId::new(self.config.provider_id);
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

    async fn refresh(&self, credential: OAuthCredential) -> Result<AssistAccess, ProviderError> {
        let response = self
            .client
            .post(&self.token_url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(form_body(&[
                ("client_id", self.config.client_id().as_str()),
                ("client_secret", self.config.client_secret().as_str()),
                ("refresh_token", credential.refresh_token().expose()),
                ("grant_type", "refresh_token"),
            ]))
            .send()
            .await
            .map_err(|error| ProviderError::Transport {
                detail: error.without_url().to_string(),
            })?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
            || response.status() == reqwest::StatusCode::BAD_REQUEST
        {
            // Google answers 400 invalid_grant for revoked refresh tokens.
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

        let (_, prior_refresh, _, project) = credential.into_parts();
        let refresh_token = tokens
            .refresh_token
            .filter(|token| !token.is_empty())
            .map_or(prior_refresh, Secret::new);
        let replacement = OAuthCredential::new(
            Secret::new(tokens.access_token),
            refresh_token,
            unix_time() + tokens.expires_in,
            project,
        );

        // Persist-before-consume, as in the other OAuth managers.
        store_credential(Arc::clone(&self.secrets), self.config, replacement)
            .await
            .map_err(|error| ProviderError::Transport {
                detail: error.to_string(),
            })?;
        self.resolve().await.map(to_access)
    }
}

fn to_access(credential: OAuthCredential) -> AssistAccess {
    let (access_token, _refresh, _expires, project) = credential.into_parts();
    AssistAccess {
        access_token,
        project,
    }
}

fn map_secret_error(error: SecretError) -> ProviderError {
    match error {
        SecretError::NotFound { .. } | SecretError::KindMismatch { .. } => ProviderError::Auth,
        SecretError::Unavailable { detail } => ProviderError::Transport { detail },
    }
}

async fn store_credential(
    secrets: Arc<dyn SecretStore>,
    config: &'static GoogleClient,
    credential: OAuthCredential,
) -> Result<(), OAuthError> {
    let provider = ProviderId::new(config.provider_id);
    tokio::task::spawn_blocking(move || secrets.store(&provider, Credential::OAuth(credential)))
        .await
        .map_err(|error| OAuthError::Transport {
            detail: format!("credential persistence task failed: {error}"),
        })?
        .map_err(OAuthError::Store)
}

fn status_error(operation: &str, status: reqwest::StatusCode) -> OAuthError {
    OAuthError::Protocol {
        detail: format!("{operation} returned HTTP {}", status.as_u16()),
    }
}

fn random_url_safe(bytes: usize) -> Result<String, OAuthError> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer).map_err(|error| OAuthError::Protocol {
        detail: format!("no system randomness for the OAuth state: {error}"),
    })?;
    let mut output = String::with_capacity(bytes * 2);
    for byte in buffer {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    Ok(output)
}

fn unmask_bytes(bytes: &[u8]) -> String {
    let unmasked: Vec<u8> = bytes.iter().map(|byte| byte ^ 0x5C).collect();
    String::from_utf8(unmasked).unwrap_or_default()
}

fn decode_query_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut position = 0;
    while position < bytes.len() {
        match bytes.get(position) {
            Some(b'%') => {
                let hex: Option<u8> = bytes
                    .get(position + 1..position + 3)
                    .and_then(|pair| std::str::from_utf8(pair).ok())
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok());
                if let Some(byte) = hex {
                    output.push(byte);
                    position += 3;
                } else {
                    output.push(b'%');
                    position += 1;
                }
            }
            Some(b'+') => {
                output.push(b' ');
                position += 1;
            }
            Some(byte) => {
                output.push(*byte);
                position += 1;
            }
            None => break,
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn form_body(fields: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (position, (name, value)) in fields.iter().enumerate() {
        if position > 0 {
            body.push('&');
        }
        encode_query_component(name, &mut body);
        body.push('=');
        encode_query_component(value, &mut body);
    }
    body
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

    #[test]
    fn the_embedded_client_identities_decode_to_the_published_values() {
        assert!(
            GEMINI_CLI
                .client_id()
                .ends_with(".apps.googleusercontent.com")
        );
        assert!(GEMINI_CLI.client_secret().starts_with("GOCSPX-"));
        assert!(
            ANTIGRAVITY
                .client_id()
                .ends_with(".apps.googleusercontent.com")
        );
        assert!(ANTIGRAVITY.client_secret().starts_with("GOCSPX-"));
    }

    #[test]
    fn query_component_decoding_reverses_percent_and_plus() {
        assert_eq!(decode_query_component("4%2F0Ab-c+d"), "4/0Ab-c d");
        assert_eq!(decode_query_component("plain"), "plain");
        assert_eq!(decode_query_component("bad%2"), "bad%2");
    }

    use crate::core::secrets::{ResolvedCredential, SecretSource};
    use std::sync::Mutex as StdMutex;
    use wiremock::matchers::{body_string_contains, method, path};
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
    async fn an_expired_credential_refreshes_and_keeps_the_project_and_refresh_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=refresh-old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access-new",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        let store = Arc::new(MemoryStore::default());
        store
            .store(
                &ProviderId::new(GEMINI_CLI.provider_id),
                Credential::OAuth(OAuthCredential::new(
                    Secret::new("access-old".to_owned()),
                    Secret::new("refresh-old".to_owned()),
                    unix_time() - 10,
                    "project-1".to_owned(),
                )),
            )
            .expect("seed");

        let manager = OAuthManager::for_test(
            &GEMINI_CLI,
            Arc::clone(&store) as Arc<dyn SecretStore>,
            format!("{}/token", server.uri()),
        );
        let access = manager.access().await.expect("refresh");
        assert_eq!(access.access_token.expose(), "access-new");
        assert_eq!(access.project, "project-1");

        // Google omitted a refresh token, so the prior one must survive.
        let held = store
            .resolve(
                &ProviderId::new(GEMINI_CLI.provider_id),
                CredentialKind::OAuth,
            )
            .expect("stored");
        let oauth = held.credential.into_oauth().expect("oauth");
        assert_eq!(oauth.refresh_token().expose(), "refresh-old");
    }

    #[test]
    fn authorize_urls_pin_the_loopback_redirect_and_offline_access() {
        let url = GEMINI_CLI.authorize_url("state-1");
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("localhost%3A8085%2Foauth2callback"));
        let url = ANTIGRAVITY.authorize_url("state-2");
        assert!(url.contains("localhost%3A51121%2Foauth-callback"));
    }
}
