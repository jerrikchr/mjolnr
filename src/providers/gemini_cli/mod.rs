//! Google Cloud Code Assist adapter (Phase 16): Gemini via the user's own
//! Google subscription login (Gemini CLI), plus the Antigravity client.
//!
//! Same wire as the public Gemini API, one wrapper deeper: requests POST to
//! `{endpoint}/v1internal:streamGenerateContent?alt=sse` as
//! `{"project", "model", "request": {…standard body…}}` and each SSE frame
//! wraps the standard chunk as `{"response": {…}}`. Translation and stream
//! decoding are therefore borrowed from [`super::gemini`].
//!
//! Antigravity adds an envelope on top (verified against oh-my-pi's capture
//! of the real `antigravity/hub` client): the `daily-` inference host, a
//! structured `requestId`, a per-conversation `sessionId`, telemetry labels,
//! a fixed per-model output cap, and its own user-agent. Without these the
//! backend answers HTTP 400.

mod oauth;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::ProviderEvent;
use crate::core::model::{
    ModelCapabilities, ModelDescriptor, ModelId, ProviderId, QuotaSnapshot, QuotaWindow,
};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::SecretStore;

pub use oauth::{ANTIGRAVITY, BrowserPrompt, GEMINI_CLI, GoogleClient, OAuthError, browser_login};

pub const GEMINI_CLI_PROVIDER_ID: &str = "gemini-cli";
pub const ANTIGRAVITY_PROVIDER_ID: &str = "antigravity";
pub const DEFAULT_MODEL: &str = "gemini-2.5-pro";

/// `v1internal:fetchAvailableModels` response shape (E0 spike). Only the
/// quota fields are read; the rest of each model entry (display name,
/// token caps, experiment payloads) is not this provider's job to surface.
#[derive(Debug, serde::Deserialize)]
struct FetchAvailableModelsResponse {
    #[serde(default)]
    models: std::collections::BTreeMap<String, ModelQuotaEntry>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelQuotaEntry {
    quota_info: Option<ModelQuotaInfo>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelQuotaInfo {
    remaining_fraction: f32,
    reset_time: Option<String>,
}

/// Google reports quota per model, not per account — dozens of models
/// sharing one pool (E0 spike: a five-hour Gemini pool and a separate
/// weekly-scale pool for the Claude/GPT models Antigravity fronts).
/// Grouping by identical `(remainingFraction, resetTime)` collapses that
/// into the small number of windows it actually represents, rather than
/// showing the owner the same fraction twenty times under twenty model
/// names. Models reporting `remainingFraction: 1` with no reset time are
/// unmetered in this response and are not a window to show.
fn quota_windows_by_pool(
    models: std::collections::BTreeMap<String, ModelQuotaEntry>,
) -> Vec<QuotaWindow> {
    let mut pools: std::collections::BTreeMap<(u32, Option<String>), Vec<String>> =
        std::collections::BTreeMap::new();
    for (name, entry) in models {
        let Some(info) = entry.quota_info else {
            continue;
        };
        if (info.remaining_fraction - 1.0).abs() < f32::EPSILON && info.reset_time.is_none() {
            continue;
        }
        pools
            .entry((info.remaining_fraction.to_bits(), info.reset_time))
            .or_default()
            .push(name);
    }
    pools
        .into_iter()
        .map(|((fraction_bits, reset_time), mut members)| {
            members.sort();
            QuotaWindow {
                label: pool_label(&members),
                used_fraction: (1.0 - f32::from_bits(fraction_bits)).clamp(0.0, 1.0),
                resets_at: reset_time
                    .as_deref()
                    .and_then(crate::providers::quota::parse_reset),
            }
        })
        .collect()
}

/// A pool's label names what it covers, since Google does not name the
/// window itself the way Anthropic and Codex do.
fn pool_label(members: &[String]) -> String {
    let has = |needle: &str| members.iter().any(|name| name.contains(needle));
    if has("claude") || has("gpt") {
        "claude/gpt".to_owned()
    } else if has("gemini") {
        "gemini".to_owned()
    } else {
        members
            .first()
            .cloned()
            .unwrap_or_else(|| "google".to_owned())
    }
}

/// First systemInstruction part the Antigravity backend expects for Claude
/// and Gemini-3 wire ids (captured from the real client).
const ANTIGRAVITY_SYSTEM_INSTRUCTION: &str = "You are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.You are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.**Absolute paths only****Proactiveness**";

/// Per-wire-id constants from the real client. `model_enum` is a telemetry
/// label; the output cap is enforced by the backend (Claude 400s above it).
const ANTIGRAVITY_WIRE_PROFILES: &[(&str, Option<&str>, u32)] = &[
    (
        "gemini-3.5-flash-extra-low",
        Some("MODEL_PLACEHOLDER_M187"),
        65_536,
    ),
    (
        "gemini-3.5-flash-low",
        Some("MODEL_PLACEHOLDER_M20"),
        65_536,
    ),
    (
        "gemini-3-flash-agent",
        Some("MODEL_PLACEHOLDER_M132"),
        65_536,
    ),
    ("gemini-3.1-pro-low", Some("MODEL_PLACEHOLDER_M36"), 65_535),
    ("gemini-pro-agent", Some("MODEL_PLACEHOLDER_M16"), 65_535),
    ("claude-sonnet-4-6", None, 64_000),
    ("claude-opus-4-6-thinking", None, 64_000),
];

/// Stable per-process conversation identity for the Antigravity envelope.
/// The real client threads agent/trajectory/session ids across steps; steps
/// advance per request.
#[derive(Debug)]
struct AntigravitySession {
    agent_id: String,
    trajectory_id: String,
    session_id: String,
    step: AtomicU64,
}

impl AntigravitySession {
    fn new() -> Self {
        Self {
            agent_id: uuid::Uuid::now_v7().to_string(),
            trajectory_id: uuid::Uuid::now_v7().to_string(),
            session_id: signed_decimal_session_id(),
            step: AtomicU64::new(2),
        }
    }
}

/// The real client's sessionId shape: `-<random int63 decimal>`.
fn signed_decimal_session_id() -> String {
    const BOUND: u64 = 9_000_000_000_000_000_000;
    let mut bytes = [0_u8; 8];
    loop {
        if getrandom::fill(&mut bytes).is_err() {
            // Randomness only differentiates telemetry sessions; a
            // time-derived fallback is acceptable and never blocks a request.
            return format!("-{}", uuid::Uuid::now_v7().as_u64_pair().1 >> 1);
        }
        let value = u64::from_be_bytes(bytes) & (u64::MAX >> 1);
        if value < BOUND {
            return format!("-{value}");
        }
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

/// `process.platform` / `process.arch` vocabulary the Google clients use.
fn node_platform() -> (&'static str, &'static str) {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    (platform, arch)
}

fn antigravity_user_agent() -> String {
    let (platform, arch) = node_platform();
    let os = if platform == "win32" {
        "windows"
    } else {
        platform
    };
    let arch = if arch == "x64" { "amd64" } else { arch };
    format!("antigravity/hub/2.1.4 {os}/{arch}")
}

fn gemini_cli_user_agent(model: &str) -> String {
    let (platform, arch) = node_platform();
    format!("GeminiCLI/0.46.0/{model} ({platform}; {arch}; terminal)")
}

/// How long a `fetchAvailableModels` quota answer is trusted before the next
/// `stream()` call fetches a fresh one.
const QUOTA_CACHE_TTL: std::time::Duration = std::time::Duration::from_mins(5);

#[derive(Debug)]
pub struct GeminiCliProvider {
    client: reqwest::Client,
    config: &'static GoogleClient,
    base_url: String,
    oauth: oauth::OAuthManager,
    session: Option<AntigravitySession>,
    quota_cache: tokio::sync::Mutex<Option<(std::time::Instant, QuotaSnapshot)>>,
}

impl GeminiCliProvider {
    #[must_use]
    pub fn new(config: &'static GoogleClient, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: config.inference_endpoint.to_owned(),
            oauth: oauth::OAuthManager::new(config, secrets),
            session: config.antigravity.then(AntigravitySession::new),
            quota_cache: tokio::sync::Mutex::new(None),
            config,
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The inference response carries no quota signal at all (E0 spike), so
    /// unlike every other provider's passive header read, this is smed's
    /// one active quota probe: a side request to `fetchAvailableModels`,
    /// found via reverse-engineering docs and confirmed live — it 403s
    /// without the `antigravity` user-agent header (not a scope issue; the
    /// scopes already requested match the reference client's), and 200s
    /// with it, returning per-model `quotaInfo{remainingFraction,
    /// resetTime}`. Cached for `QUOTA_CACHE_TTL` and refreshed opportunistically
    /// off the next `stream()` call once stale, never on a background timer —
    /// a session nobody drives never fires the extra request.
    async fn refresh_quota(&self, access: &oauth::AssistAccess) -> Option<QuotaSnapshot> {
        {
            let cache = self.quota_cache.lock().await;
            if let Some((fetched_at, snapshot)) = cache.as_ref()
                && fetched_at.elapsed() < QUOTA_CACHE_TTL
            {
                return Some(snapshot.clone());
            }
        }
        let response = self
            .client
            .post(format!(
                "{}/v1internal:fetchAvailableModels",
                self.config.endpoint
            ))
            .bearer_auth(access.access_token.expose())
            .header(reqwest::header::USER_AGENT, antigravity_user_agent())
            .json(&serde_json::json!({ "project": access.project }))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body: FetchAvailableModelsResponse = response.json().await.ok()?;
        let windows = quota_windows_by_pool(body.models);
        if windows.is_empty() {
            return None;
        }
        let snapshot = QuotaSnapshot {
            provider: self.id(),
            windows,
        };
        *self.quota_cache.lock().await = Some((std::time::Instant::now(), snapshot.clone()));
        Some(snapshot)
    }

    /// Wrap the translated Gemini body in the Cloud Code Assist envelope.
    fn assemble_body(
        &self,
        model: &str,
        request: &ProviderRequest,
        project: &str,
    ) -> serde_json::Value {
        let mut inner = serde_json::to_value(crate::providers::gemini::translate(request))
            .unwrap_or_else(|_| serde_json::json!({}));

        let Some(session) = &self.session else {
            return serde_json::json!({
                "project": project,
                "model": model,
                "request": inner,
            });
        };

        // Antigravity envelope, mirrored from the real client.
        let step = session.step.fetch_add(1, Ordering::Relaxed);
        let profile = ANTIGRAVITY_WIRE_PROFILES
            .iter()
            .find(|(id, _, _)| *id == model);
        let is_claude = model.contains("claude");
        let instruction = (is_claude || model.contains("gemini-3")).then(|| {
            let existing = inner
                .get("systemInstruction")
                .and_then(|instruction| instruction.get("parts"))
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut parts = vec![serde_json::json!({"text": ANTIGRAVITY_SYSTEM_INSTRUCTION})];
            parts.extend(existing);
            serde_json::json!({"role": "user", "parts": parts})
        });

        let mut labels = serde_json::Map::new();
        labels.insert(
            "last_step_index".to_owned(),
            serde_json::Value::String((step - 1).to_string()),
        );
        if let Some((_, Some(model_enum), _)) = profile {
            labels.insert(
                "model_enum".to_owned(),
                serde_json::Value::String((*model_enum).to_owned()),
            );
        }
        labels.insert(
            "trajectory_id".to_owned(),
            serde_json::Value::String(session.trajectory_id.clone()),
        );
        labels.insert(
            "used_claude".to_owned(),
            serde_json::Value::String(is_claude.to_string()),
        );
        labels.insert(
            "used_claude_conservative".to_owned(),
            serde_json::Value::String(is_claude.to_string()),
        );

        if let Some(body) = inner.as_object_mut() {
            if let Some(instruction) = instruction {
                body.insert("systemInstruction".to_owned(), instruction);
            }
            body.insert(
                "sessionId".to_owned(),
                serde_json::Value::String(session.session_id.clone()),
            );
            body.insert("labels".to_owned(), serde_json::Value::Object(labels));
            if let Some((_, _, max_output)) = profile {
                body.insert(
                    "generationConfig".to_owned(),
                    serde_json::json!({"maxOutputTokens": max_output}),
                );
            }
        }

        serde_json::json!({
            "project": project,
            "requestId": format!(
                "agent/{}/{}/{}/{step}",
                session.agent_id,
                unix_millis(),
                session.trajectory_id
            ),
            "request": inner,
            "model": model,
            "userAgent": "antigravity",
            "requestType": "agent",
        })
    }
}

#[async_trait]
impl Provider for GeminiCliProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(self.config.provider_id)
    }

    fn credentialed(&self) -> bool {
        self.oauth.credentialed()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        self.config
            .models
            .iter()
            .map(|(id, name, context)| ModelDescriptor {
                id: ModelId::new(*id),
                provider: self.id(),
                display_name: (*name).to_owned(),
                capabilities: ModelCapabilities::text_and_tools(),
                context_tokens: Some(*context),
                max_output_tokens: Some(65_536),
                tier: None,
            })
            .collect()
    }

    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        // Refresh is not cancellable mid-flight (rotating credential chain).
        let access = self.oauth.access().await?;
        if let Some(snapshot) = self.refresh_quota(&access).await {
            tokio::select! {
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                result = events.send(ProviderEvent::Quota { snapshot }) => {
                    result.map_err(|_| ProviderError::Cancelled)?;
                }
            }
        }
        let model = request.model.to_string();
        let body = self.assemble_body(&model, &request, &access.project);

        let mut builder = self
            .client
            .post(format!(
                "{}/v1internal:streamGenerateContent?alt=sse",
                self.base_url
            ))
            .bearer_auth(access.access_token.expose())
            .json(&body);
        if self.config.antigravity {
            builder = builder.header(reqwest::header::USER_AGENT, antigravity_user_agent());
        } else {
            // Identify as the official CLI; the backend rate-limits unknown
            // agents differently and expects the client metadata header.
            builder = builder
                .header(reqwest::header::USER_AGENT, gemini_cli_user_agent(&model))
                .header(
                    "Client-Metadata",
                    "ideType=IDE_UNSPECIFIED,platform=PLATFORM_UNSPECIFIED,pluginType=GEMINI",
                );
        }

        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = builder.send() => result.map_err(|error| ProviderError::Transport {
                detail: error.without_url().to_string(),
            })?,
        };
        crate::providers::quota::emit_from_headers(self.id(), response.headers(), &events, &cancel)
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                    ProviderError::Auth
                }
                reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit {
                    retry_after_seconds: None,
                },
                status => {
                    // The body names the rejected field; without it a 400 is
                    // undebuggable from the TUI.
                    let body = response.text().await.unwrap_or_default();
                    let mut detail = format!("http {}", status.as_u16());
                    let trimmed = body.trim();
                    if !trimmed.is_empty() {
                        detail.push_str(" // ");
                        detail.extend(trimmed.chars().take(300));
                    }
                    ProviderError::Protocol { detail }
                }
            });
        }
        crate::providers::gemini::drive_sse(
            response,
            &events,
            &cancel,
            crate::providers::gemini::FrameShape::CloudCodeWrapped,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antigravity_models_all_have_wire_profiles() {
        for (id, _, _) in ANTIGRAVITY.models {
            assert!(
                ANTIGRAVITY_WIRE_PROFILES
                    .iter()
                    .any(|(wire, _, _)| wire == id),
                "{id} would ship without its output cap and telemetry label"
            );
        }
    }

    #[test]
    fn session_ids_are_negative_int63_decimals() {
        let id = signed_decimal_session_id();
        assert!(id.starts_with('-'));
        let digits = id.trim_start_matches('-');
        assert!(digits.parse::<u64>().expect("decimal") < 9_000_000_000_000_000_000);
    }

    #[test]
    fn user_agents_match_the_official_client_shapes() {
        let antigravity = antigravity_user_agent();
        assert!(
            antigravity.starts_with("antigravity/hub/2.1.4 "),
            "{antigravity}"
        );
        let cli = gemini_cli_user_agent("gemini-2.5-pro");
        assert!(
            cli.starts_with("GeminiCLI/0.46.0/gemini-2.5-pro ("),
            "{cli}"
        );
    }

    #[test]
    fn models_sharing_a_quota_pool_collapse_into_one_window() {
        let mut models = std::collections::BTreeMap::new();
        models.insert(
            "gemini-2.5-pro".to_owned(),
            ModelQuotaEntry {
                quota_info: Some(ModelQuotaInfo {
                    remaining_fraction: 0.9899,
                    reset_time: Some("2026-07-31T11:12:40Z".to_owned()),
                }),
            },
        );
        models.insert(
            "gemini-3-flash".to_owned(),
            ModelQuotaEntry {
                quota_info: Some(ModelQuotaInfo {
                    remaining_fraction: 0.9899,
                    reset_time: Some("2026-07-31T11:12:40Z".to_owned()),
                }),
            },
        );
        models.insert(
            "claude-sonnet-4-6".to_owned(),
            ModelQuotaEntry {
                quota_info: Some(ModelQuotaInfo {
                    remaining_fraction: 0.327_876,
                    reset_time: Some("2026-08-04T07:00:38Z".to_owned()),
                }),
            },
        );
        // Unmetered: no reset, full fraction — not a window.
        models.insert(
            "tab_flash_lite_preview".to_owned(),
            ModelQuotaEntry {
                quota_info: Some(ModelQuotaInfo {
                    remaining_fraction: 1.0,
                    reset_time: None,
                }),
            },
        );
        // No quotaInfo at all.
        models.insert(
            "chat_20706".to_owned(),
            ModelQuotaEntry { quota_info: None },
        );

        let windows = quota_windows_by_pool(models);
        assert_eq!(windows.len(), 2);
        let gemini = windows
            .iter()
            .find(|window| window.label == "gemini")
            .expect("gemini pool");
        assert!((gemini.used_fraction - (1.0 - 0.9899)).abs() < f32::EPSILON);
        assert!(gemini.resets_at.is_some());
        let premium = windows
            .iter()
            .find(|window| window.label == "claude/gpt")
            .expect("claude/gpt pool");
        assert!((premium.used_fraction - (1.0 - 0.327_876)).abs() < f32::EPSILON);
    }

    #[test]
    fn pool_label_names_what_it_covers() {
        assert_eq!(
            pool_label(&["claude-sonnet-4-6".to_owned(), "gpt-oss-120b".to_owned()]),
            "claude/gpt"
        );
        assert_eq!(
            pool_label(&["gemini-2.5-pro".to_owned(), "gemini-3-flash".to_owned()]),
            "gemini"
        );
        assert_eq!(pool_label(&["chat_20706".to_owned()]), "chat_20706");
    }
}
