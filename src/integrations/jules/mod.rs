//! Google Jules cloud-agent transport.
//!
//! This module owns only the documented Jules REST wire contract. It does not
//! approve plans, apply patches, persist remote state, or run a cloud task on
//! behalf of a model. Callers must place those operations behind mjolnr's
//! runtime governance boundary.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::core::secrets::Secret;

use super::{IntegrationError, IntegrationId};

const DEFAULT_BASE_URL: &str = "https://jules.googleapis.com/v1alpha";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn integration_id() -> IntegrationId {
    IntegrationId::new("jules")
}

/// A configured Jules REST client. The API key is never exposed through this
/// type's debug representation or serialized wire models.
pub struct JulesClient {
    api_key: Secret,
    client: reqwest::Client,
    base_url: String,
}

impl JulesClient {
    #[must_use]
    pub fn new(api_key: Secret) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Override the endpoint for a local HTTP test server.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        base_url
            .into()
            .trim_end_matches('/')
            .clone_into(&mut self.base_url);
        self
    }

    #[must_use]
    pub fn has_credential(&self) -> bool {
        !self.api_key.is_blank()
    }

    pub async fn list_sources(&self) -> Result<Vec<JulesSource>, IntegrationError> {
        let response = self
            .request(reqwest::Method::GET, "/sources", Option::<&()>::None)
            .await?;
        let page: SourcePage = decode_json(response).await?;
        Ok(page.sources)
    }

    pub async fn get_source(
        &self,
        owner: &str,
        repository: &str,
    ) -> Result<JulesSource, IntegrationError> {
        let owner = path_segment(owner, "source owner")?;
        let repository = path_segment(repository, "source repository")?;
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/sources/github/{owner}/{repository}"),
                Option::<&()>::None,
            )
            .await?;
        decode_json(response).await
    }

    pub async fn create_session(
        &self,
        request: &CreateSessionRequest,
    ) -> Result<JulesSession, IntegrationError> {
        let response = self
            .request(reqwest::Method::POST, "/sessions", Some(request))
            .await?;
        decode_json(response).await
    }

    pub async fn get_session(&self, session_id: &str) -> Result<JulesSession, IntegrationError> {
        let session_id = path_segment(session_id, "session id")?;
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/sessions/{session_id}"),
                Option::<&()>::None,
            )
            .await?;
        decode_json(response).await
    }

    pub async fn list_activities(
        &self,
        session_id: &str,
        since: Option<&str>,
    ) -> Result<Vec<JulesActivity>, IntegrationError> {
        let session_id = path_segment(session_id, "session id")?;
        let mut path = format!("/sessions/{session_id}/activities");
        if let Some(since) = since {
            if since.contains(['&', '?', '#', '\n', '\r']) {
                return Err(invalid_id("activity timestamp", since));
            }
            path.push_str("?filter=create_time%3E%22");
            path.push_str(since);
            path.push_str("%22");
        }
        let response = self
            .request(reqwest::Method::GET, &path, Option::<&()>::None)
            .await?;
        let page: ActivityPage = decode_json(response).await?;
        Ok(page.activities)
    }

    pub async fn approve_plan(&self, session_id: &str) -> Result<(), IntegrationError> {
        self.action(session_id, ":approvePlan", &()).await
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<(), IntegrationError> {
        if prompt.trim().is_empty() {
            return Err(IntegrationError::InvalidTaskId {
                integration: integration_id(),
                task_id: session_id.to_owned(),
                detail: "message must not be blank".to_owned(),
            });
        }
        self.action(session_id, ":sendMessage", &SendMessageRequest { prompt })
            .await
    }

    pub async fn archive_session(
        &self,
        session_id: &str,
        archived: bool,
    ) -> Result<(), IntegrationError> {
        self.action(session_id, ":archive", &ArchiveSessionRequest { archived })
            .await
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<(), IntegrationError> {
        let session_id = path_segment(session_id, "session id")?;
        let response = self
            .request(
                reqwest::Method::DELETE,
                &format!("/sessions/{session_id}"),
                Option::<&()>::None,
            )
            .await?;
        consume_success(response).await
    }

    async fn action<T: Serialize>(
        &self,
        session_id: &str,
        action: &str,
        body: &T,
    ) -> Result<(), IntegrationError> {
        let session_id = path_segment(session_id, "session id")?;
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/sessions/{session_id}{action}"),
                Some(body),
            )
            .await?;
        consume_success(response).await
    }

    async fn request<T: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&T>,
    ) -> Result<reqwest::Response, IntegrationError> {
        let url = format!("{}{path}", self.base_url);
        let mut request = self
            .client
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, "mjolnr")
            .header("X-Goog-Api-Key", self.api_key.expose())
            .timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| IntegrationError::Transport {
                integration: integration_id(),
                detail: error.to_string(),
            })?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let detail = bounded_error_detail(response).await;
        Err(if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            IntegrationError::RateLimited {
                integration: integration_id(),
            }
        } else if status == reqwest::StatusCode::NOT_FOUND {
            IntegrationError::NotFound {
                integration: integration_id(),
                task_id: path.to_owned(),
            }
        } else if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            IntegrationError::CredentialRejected {
                integration: integration_id(),
            }
        } else {
            IntegrationError::Transport {
                integration: integration_id(),
                detail: format!("Jules returned HTTP {status}: {detail}"),
            }
        })
    }
}

impl std::fmt::Debug for JulesClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JulesClient")
            .field("api_key", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesSource {
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub prompt: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starting_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_plan_approval: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesSession {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JulesActivity {
    pub name: String,
    #[serde(default)]
    pub create_time: Option<String>,
    #[serde(default)]
    pub plan_generated: Option<PlanGenerated>,
    #[serde(default)]
    pub agent_messaged: Option<AgentMessaged>,
    #[serde(default)]
    pub progress_updated: Option<ProgressUpdated>,
    #[serde(default)]
    pub session_completed: Option<SessionCompleted>,
    #[serde(default)]
    pub session_failed: Option<SessionFailed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGenerated {
    #[serde(default)]
    pub plan: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessaged {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdated {
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompleted {
    #[serde(default)]
    pub pull_request: Option<PullRequestArtifact>,
    #[serde(default)]
    pub change_set: Option<ChangeSetArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFailed {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestArtifact {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSetArtifact {
    #[serde(default)]
    pub unidiff_patch: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcePage {
    #[serde(default)]
    sources: Vec<JulesSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityPage {
    #[serde(default)]
    activities: Vec<JulesActivity>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageRequest<'a> {
    prompt: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveSessionRequest {
    archived: bool,
}

async fn decode_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, IntegrationError> {
    let bytes = bounded_body(response).await?;
    serde_json::from_slice(&bytes).map_err(|error| IntegrationError::Transport {
        integration: integration_id(),
        detail: format!("Jules returned invalid JSON: {error}"),
    })
}

async fn consume_success(response: reqwest::Response) -> Result<(), IntegrationError> {
    let _ = bounded_body(response).await?;
    Ok(())
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, IntegrationError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(IntegrationError::Transport {
            integration: integration_id(),
            detail: "Jules response exceeded the 4 MiB limit".to_owned(),
        });
    }
    let mut body = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|error| IntegrationError::Transport {
            integration: integration_id(),
            detail: error.to_string(),
        })?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(IntegrationError::Transport {
                integration: integration_id(),
                detail: "Jules response exceeded the 4 MiB limit".to_owned(),
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn bounded_error_detail(response: reqwest::Response) -> String {
    bounded_body(response)
        .await
        .ok()
        .and_then(|body| String::from_utf8(body).ok())
        .map(|detail| detail.chars().take(512).collect())
        .unwrap_or_else(|| "response body unavailable".to_owned())
}

fn path_segment<'a>(value: &'a str, label: &str) -> Result<&'a str, IntegrationError> {
    if value.is_empty() || value.contains(['/', '?', '#', '&', '\n', '\r']) {
        return Err(invalid_id(label, value));
    }
    Ok(value)
}

fn invalid_id(label: &str, value: &str) -> IntegrationError {
    IntegrationError::InvalidTaskId {
        integration: integration_id(),
        task_id: value.to_owned(),
        detail: format!("invalid {label}"),
    }
}
