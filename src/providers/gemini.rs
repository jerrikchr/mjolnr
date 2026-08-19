//! Gemini `generateContent` adapter (Phase 7).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::{FinishReason, ProviderEvent};
use crate::core::message::{ContentBlock, Role, ToolCall, ToolOutcome};
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId, Usage};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};
use crate::core::secrets::{CredentialKind, SecretError, SecretStore};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const PROVIDER_ID: &str = "gemini";
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";
const MODELS: &[(&str, &str, u32, u32)] = &[
    ("gemini-2.5-flash", "Gemini 2.5 Flash", 1_048_576, 8_192),
    ("gemini-2.5-pro", "Gemini 2.5 Pro", 1_048_576, 8_192),
];

#[derive(Debug)]
pub struct GeminiProvider {
    client: reqwest::Client,
    base_url: String,
    secrets: Arc<dyn SecretStore>,
}

impl GeminiProvider {
    #[must_use]
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            secrets,
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct RequestBody {
    contents: Vec<Content>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolGroup>,
}

#[derive(Debug, Serialize)]
struct Content {
    #[serde(skip_serializing_if = "String::is_empty")]
    role: String,
    parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Part {
    Text {
        text: String,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: FunctionCall,
        /// Omitted rather than null when absent: a `thoughtSignature: null` is
        /// itself rejected, and non-thinking Gemini models never issue one.
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: FunctionResponse,
    },
    /// Confirmed against current documentation 2026-07-25
    /// (`provider-contract.md` §5.5). camelCase to match `functionCall` and
    /// `thoughtSignature` above: smed targets the camelCase JSON surface, and
    /// mixing spellings in one `parts` array is a request that half-decodes.
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Serialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct FunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct FunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ToolGroup {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Serialize)]
struct FunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "usageMetadata")]
    usage: Option<UsageWire>,
    #[serde(rename = "promptFeedback")]
    prompt_feedback: Option<PromptFeedback>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: Option<ContentWire>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentWire {
    #[serde(default)]
    parts: Vec<PartWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartWire {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<FunctionCallWire>,
    /// Sibling of `functionCall`, not a field inside it. Gemini 3 thinking
    /// models require it echoed back on the replayed call.
    #[serde(rename = "thoughtSignature")]
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FunctionCallWire {
    name: String,
    #[serde(default)]
    args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UsageWire {
    #[serde(rename = "promptTokenCount", default)]
    input: u64,
    #[serde(rename = "candidatesTokenCount", default)]
    output: u64,
}

#[derive(Debug, Deserialize)]
struct PromptFeedback {
    block_reason: Option<String>,
}

/// JSON Schema keywords Google's proto-typed `functionDeclarations` reject
/// with HTTP 400 "Unknown name" (list verified against oh-my-pi's Google
/// normalizer, 2026-07-21).
const UNSUPPORTED_SCHEMA_FIELDS: &[&str] = &[
    "$schema",
    "$ref",
    "$defs",
    "$dynamicRef",
    "$dynamicAnchor",
    "examples",
    "prefixItems",
    "unevaluatedProperties",
    "unevaluatedItems",
    "patternProperties",
    "additionalProperties",
    "propertyNames",
    "minItems",
    "maxItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "pattern",
    "format",
];

/// Collapse a JSON Schema union `type` into the single scalar Google's proto
/// `Schema` accepts. `["string", "null"]` becomes `"string"` plus
/// `nullable: true`; the first non-null entry wins for wider unions.
fn collapse_type(entry: &serde_json::Value) -> Option<(serde_json::Value, bool)> {
    let variants = entry.as_array()?;
    let nullable = variants.iter().any(|variant| variant == "null");
    let concrete = variants
        .iter()
        .find(|variant| *variant != "null")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("string".to_string()));
    Some((concrete, nullable))
}

/// Strip keywords Google rejects, recursively. Keys under `properties` are
/// property *names*, not keywords, so they are preserved verbatim even when
/// they collide with the list (a tool may legitimately take a `format`
/// argument).
fn sanitize_schema(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut cleaned_map = serde_json::Map::new();
            let mut nullable = false;
            for (key, entry) in map {
                if UNSUPPORTED_SCHEMA_FIELDS.contains(&key.as_str()) {
                    continue;
                }
                let cleaned = if key == "properties" {
                    match entry {
                        serde_json::Value::Object(properties) => serde_json::Value::Object(
                            properties
                                .iter()
                                .map(|(name, schema)| (name.clone(), sanitize_schema(schema)))
                                .collect(),
                        ),
                        other => other.clone(),
                    }
                } else if key == "type" {
                    match collapse_type(entry) {
                        Some((concrete, is_nullable)) => {
                            nullable |= is_nullable;
                            concrete
                        }
                        None => entry.clone(),
                    }
                } else {
                    sanitize_schema(entry)
                };
                cleaned_map.insert(key.clone(), cleaned);
            }
            if nullable {
                cleaned_map.insert("nullable".to_string(), serde_json::Value::Bool(true));
            }
            serde_json::Value::Object(cleaned_map)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sanitize_schema).collect())
        }
        other => other.clone(),
    }
}

pub(crate) fn translate(request: &ProviderRequest) -> RequestBody {
    let mut contents = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::Assistant => "model",
            _ => "user",
        };
        let mut parts = Vec::new();
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => parts.push(Part::Text { text: text.clone() }),
                ContentBlock::ToolCall(call) => parts.push(Part::FunctionCall {
                    function_call: FunctionCall {
                        name: call.name.clone(),
                        args: call.arguments.clone(),
                    },
                    thought_signature: call.provider_signature.clone(),
                }),
                ContentBlock::ToolResult { name, result, .. } => {
                    let status = match result.outcome {
                        ToolOutcome::Ok => "ok",
                        ToolOutcome::Refused(_) => "refused",
                        ToolOutcome::Failed(_) => "failed",
                    };
                    parts.push(Part::FunctionResponse { function_response: FunctionResponse { name: name.clone(), response: serde_json::json!({"status": status, "content": result.content, "truncated": result.truncated, "evidence_event_id": result.evidence_event_id}) } });
                }
                ContentBlock::ImageRef { source, .. } => {
                    if let Some(bytes) = request.images.get(source) {
                        parts.push(Part::InlineData {
                            inline_data: InlineData {
                                mime_type: bytes.media_type.clone(),
                                data: bytes.base64(),
                            },
                        });
                    }
                }
            }
        }
        if !parts.is_empty() {
            contents.push(Content {
                role: role.to_owned(),
                parts,
            });
        }
    }
    let tools = if request.tools.is_empty() {
        Vec::new()
    } else {
        vec![ToolGroup {
            function_declarations: request
                .tools
                .iter()
                .map(|tool| FunctionDeclaration {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: sanitize_schema(&tool.schema),
                })
                .collect(),
        }]
    };
    RequestBody {
        contents,
        system_instruction: request.system.clone().map(|text| Content {
            role: String::new(),
            parts: vec![Part::Text { text }],
        }),
        tools,
    }
}

async fn emit(
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    event: ProviderEvent,
) -> Result<(), ProviderError> {
    tokio::select! { () = cancel.cancelled() => Err(ProviderError::Cancelled), result = events.send(event) => result.map_err(|_| ProviderError::Cancelled) }
}

fn map_secret(error: SecretError) -> ProviderError {
    match error {
        SecretError::NotFound { .. } | SecretError::KindMismatch { .. } => ProviderError::Auth,
        SecretError::Unavailable { detail } => ProviderError::Transport { detail },
    }
}

fn map_http(status: reqwest::StatusCode) -> ProviderError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => ProviderError::Auth,
        reqwest::StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit {
            retry_after_seconds: None,
        },
        _ => ProviderError::Protocol {
            detail: format!("http {}", status.as_u16()),
        },
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    fn credentialed(&self) -> bool {
        self.secrets
            .resolve(
                &ProviderId::new(PROVIDER_ID),
                crate::core::secrets::CredentialKind::ApiKey,
            )
            .is_ok()
    }

    fn models(&self) -> Vec<ModelDescriptor> {
        MODELS
            .iter()
            .map(|(id, name, context, max_output)| {
                let id = ModelId::new(*id);
                let provider = self.id();
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

    #[allow(
        clippy::too_many_lines,
        reason = "Gemini transport and candidate decoder are one provider-specific state machine"
    )]
    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let secrets = Arc::clone(&self.secrets);
        let provider = self.id();
        let resolved =
            tokio::task::spawn_blocking(move || secrets.resolve(&provider, CredentialKind::ApiKey));
        let resolved = tokio::select! { () = cancel.cancelled() => return Err(ProviderError::Cancelled), result = resolved => result.map_err(|error| ProviderError::Transport { detail: error.to_string() })? }.map_err(map_secret)?;
        let Some(secret) = resolved.credential.api_key() else {
            return Err(ProviderError::Auth);
        };
        if secret.is_blank() {
            return Err(ProviderError::Auth);
        }
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base_url, request.model
        );
        let response = tokio::select! { () = cancel.cancelled() => return Err(ProviderError::Cancelled), result = self.client.post(url).header("x-goog-api-key", secret.expose()).json(&translate(&request)).send() => result.map_err(|error| ProviderError::Transport { detail: error.without_url().to_string() })? };
        crate::providers::quota::emit_from_headers(
            ProviderId::new(PROVIDER_ID),
            response.headers(),
            &events,
            &cancel,
        )
        .await?;
        if !response.status().is_success() {
            return Err(map_http(response.status()));
        }
        drive_sse(response, &events, &cancel, FrameShape::Plain).await
    }
}

/// How each SSE frame's JSON carries the `GenerateContentResponse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameShape {
    /// The public Gemini API: the frame *is* the response.
    Plain,
    /// The Cloud Code Assist API (`v1internal:streamGenerateContent`): the
    /// frame wraps it as `{"response": {…}}`.
    CloudCodeWrapped,
}

#[derive(Debug, Deserialize)]
struct WrappedChunk {
    response: Option<Response>,
}

/// Decode a Gemini SSE body into provider events. Shared by the API-key
/// adapter and the Cloud Code Assist (subscription) adapter.
#[allow(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "the candidate decoder is one provider-specific state machine"
)]
pub(crate) async fn drive_sse(
    response: reqwest::Response,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    shape: FrameShape,
) -> Result<ProviderCompletion, ProviderError> {
    let mut stream = response.bytes_stream().eventsource();
    emit(events, cancel, ProviderEvent::Started).await?;
    let mut usage = None;
    let mut saw_tool = HashSet::new();
    loop {
        let Some(frame) = (tokio::select! { () = cancel.cancelled() => return Err(ProviderError::Cancelled), frame = stream.next() => frame })
        else {
            return Err(ProviderError::Protocol {
                detail: "Gemini stream ended without a terminal candidate".to_owned(),
            });
        };
        let frame = frame.map_err(|error| ProviderError::Transport {
            detail: error.to_string(),
        })?;
        let response: Response = match shape {
            FrameShape::Plain => {
                serde_json::from_str(&frame.data).map_err(|error| ProviderError::Protocol {
                    detail: format!("malformed Gemini response: {error}"),
                })?
            }
            FrameShape::CloudCodeWrapped => {
                let wrapped: WrappedChunk =
                    serde_json::from_str(&frame.data).map_err(|error| ProviderError::Protocol {
                        detail: format!("malformed Cloud Code response: {error}"),
                    })?;
                match wrapped.response {
                    Some(response) => response,
                    None => continue,
                }
            }
        };
        {
            if let Some(wire_usage) = response.usage {
                usage = Some(Usage {
                    input_tokens: wire_usage.input,
                    output_tokens: wire_usage.output,
                });
            }
            if let Some(feedback) = response.prompt_feedback
                && let Some(reason) = feedback.block_reason
            {
                emit(
                    events,
                    cancel,
                    ProviderEvent::Failed {
                        detail: format!("Gemini prompt blocked ({reason})"),
                    },
                )
                .await?;
                return Err(ProviderError::Protocol {
                    detail: format!("Gemini prompt blocked ({reason})"),
                });
            }
            for candidate in response.candidates {
                if let Some(content) = candidate.content {
                    for part in content.parts {
                        if let Some(text) = part.text {
                            emit(events, cancel, ProviderEvent::TextDelta { text }).await?;
                        }
                        if let Some(call) = part.function_call {
                            let id = format!("gemini-{}", saw_tool.len());
                            if saw_tool.insert(id.clone()) {
                                emit(
                                    events,
                                    cancel,
                                    ProviderEvent::ToolCallStarted {
                                        id: id.clone(),
                                        name: call.name.clone(),
                                    },
                                )
                                .await?;
                                emit(
                                    events,
                                    cancel,
                                    ProviderEvent::ToolArgumentsDelta {
                                        id: id.clone(),
                                        fragment: call.args.to_string(),
                                    },
                                )
                                .await?;
                                emit(
                                    events,
                                    cancel,
                                    ProviderEvent::ToolCallCompleted {
                                        call: ToolCall {
                                            id,
                                            name: call.name,
                                            arguments: call.args,
                                            provider_signature: part.thought_signature,
                                        },
                                    },
                                )
                                .await?;
                            }
                        }
                    }
                }
                if let Some(reason) = candidate.finish_reason {
                    let finish = if saw_tool.is_empty() && reason == "STOP" {
                        FinishReason::Stop
                    } else if !saw_tool.is_empty() && reason == "STOP" {
                        FinishReason::ToolCalls
                    } else {
                        FinishReason::Incomplete
                    };
                    if let Some(usage) = usage {
                        emit(events, cancel, ProviderEvent::Usage { usage }).await?;
                    }
                    emit(events, cancel, ProviderEvent::Finished { reason: finish }).await?;
                    return Ok(ProviderCompletion {
                        reason: finish,
                        usage,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    /// A one-message history replaying a single tool call.
    fn request_replaying(call: crate::core::message::ToolCall) -> ProviderRequest {
        ProviderRequest {
            model: ModelId::new("gemini-3.5-flash"),
            messages: vec![crate::core::message::CanonicalMessage {
                id: uuid::Uuid::now_v7(),
                role: Role::Assistant,
                blocks: vec![ContentBlock::ToolCall(call)],
                provider: Some(ProviderId::new(PROVIDER_ID)),
                model: Some(ModelId::new("gemini-3.5-flash")),
                created_at: time::OffsetDateTime::now_utc(),
            }],
            system: None,
            tools: Vec::new(),
            images: crate::core::image::ImageSidecar::new(),
        }
    }

    #[test]
    fn an_image_encodes_as_camel_case_inline_data() {
        use crate::core::message::CanonicalMessage;
        // Confirmed against current documentation 2026-07-25
        // (`provider-contract.md` §5.5). camelCase is load-bearing: this adapter
        // already sends `functionCall` and `thoughtSignature`, and a snake_case
        // `inline_data` in the same `parts` array is a request that half-decodes.
        let mut message = CanonicalMessage::user("what is this?");
        message.blocks.push(ContentBlock::ImageRef {
            media_type: "image/png".to_owned(),
            source: "shot.png".to_owned(),
        });
        let mut images = crate::core::image::ImageSidecar::new();
        images.insert(
            "shot.png".to_owned(),
            crate::core::image::ImageBytes {
                media_type: "image/png".to_owned(),
                bytes: std::sync::Arc::from([1_u8, 2, 3].as_slice()),
            },
        );
        let request = ProviderRequest {
            model: ModelId::new("gemini-3.5-flash"),
            messages: vec![message],
            system: None,
            tools: Vec::new(),
            images,
        };

        let encoded = serde_json::to_value(translate(&request)).expect("encode");
        let part = encoded
            .pointer("/contents/0/parts/1")
            .expect("the image part travels beside the text");
        assert_eq!(part["inlineData"]["mimeType"], "image/png");
        assert_eq!(part["inlineData"]["data"], "AQID");
        assert!(
            part.get("inline_data").is_none(),
            "snake_case would not decode beside functionCall: {part}"
        );
    }

    #[test]
    fn an_image_without_bytes_contributes_no_part() {
        use crate::core::message::CanonicalMessage;
        let mut message = CanonicalMessage::user("what is this?");
        message.blocks.push(ContentBlock::ImageRef {
            media_type: "image/png".to_owned(),
            source: "missing.png".to_owned(),
        });
        let request = ProviderRequest {
            model: ModelId::new("gemini-3.5-flash"),
            messages: vec![message],
            system: None,
            tools: Vec::new(),
            images: crate::core::image::ImageSidecar::new(),
        };

        let encoded = serde_json::to_value(translate(&request)).expect("encode");
        assert!(
            encoded.pointer("/contents/0/parts/1").is_none(),
            "an absent image must not become an empty part: {encoded}"
        );
    }

    #[test]
    fn a_replayed_tool_call_carries_its_thought_signature_back() {
        // Gemini 3 thinking models 400 the next turn without this.
        let body = request_replaying(crate::core::message::ToolCall {
            id: "gemini-0".to_owned(),
            name: "list_files".to_owned(),
            arguments: serde_json::json!({}),
            provider_signature: Some("sig-abc123".to_owned()),
        });
        let encoded = serde_json::to_value(translate(&body)).expect("encode");
        let part = encoded
            .pointer("/contents/0/parts/0")
            .expect("the replayed call is the first part");

        assert_eq!(
            part.pointer("/functionCall/name")
                .and_then(serde_json::Value::as_str),
            Some("list_files")
        );
        assert_eq!(part["thoughtSignature"], "sig-abc123");
    }

    #[test]
    fn a_call_without_a_signature_omits_the_key_rather_than_sending_null() {
        // An explicit `thoughtSignature: null` is itself rejected, so absent has
        // to mean absent — this is the non-thinking-model path.
        let body = request_replaying(crate::core::message::ToolCall {
            id: "gemini-0".to_owned(),
            name: "list_files".to_owned(),
            arguments: serde_json::json!({}),
            provider_signature: None,
        });
        let encoded = serde_json::to_value(translate(&body)).expect("encode");
        let part = encoded
            .pointer("/contents/0/parts/0")
            .expect("the replayed call is the first part");

        assert_eq!(
            part.pointer("/functionCall/name")
                .and_then(serde_json::Value::as_str),
            Some("list_files")
        );
        assert!(
            part.get("thoughtSignature").is_none(),
            "an absent signature must be omitted, not null: {part}"
        );
    }

    #[test]
    fn union_types_collapse_to_a_scalar_google_accepts() {
        // Google's proto `Schema.type` is a single enum, so `["string", "null"]`
        // draws HTTP 400 "Proto field is not repeating, cannot start list".
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": ["string", "null"], "minLength": 1 },
                "recursive": { "type": ["boolean", "null"] },
                "name": { "type": "string" }
            }
        });
        let cleaned = sanitize_schema(&schema);
        let properties = cleaned.get("properties").expect("properties survive");

        let path = properties.get("path").expect("path survives");
        assert_eq!(path.get("type"), Some(&serde_json::json!("string")));
        assert_eq!(path.get("nullable"), Some(&serde_json::json!(true)));

        let recursive = properties.get("recursive").expect("recursive survives");
        assert_eq!(recursive.get("type"), Some(&serde_json::json!("boolean")));

        // A scalar type is left exactly as it was, nullable included.
        let name = properties.get("name").expect("name survives");
        assert_eq!(name.get("type"), Some(&serde_json::json!("string")));
        assert!(name.get("nullable").is_none());
    }

    #[test]
    fn schema_sanitizing_strips_keywords_but_keeps_property_names() {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "format": { "type": "string", "pattern": "^a+$" },
                "path": { "type": "string", "minLength": 1 }
            },
            "required": ["path"]
        });
        let cleaned = sanitize_schema(&schema);
        assert!(cleaned.get("$schema").is_none());
        assert!(cleaned.get("additionalProperties").is_none());
        let properties = cleaned.get("properties").expect("properties survive");
        let format = properties
            .get("format")
            .expect("a property named format survives");
        assert!(
            format.get("pattern").is_none(),
            "keywords inside a property schema are stripped"
        );
        let path = properties.get("path").expect("path survives");
        assert!(path.get("minLength").is_none());
        assert_eq!(cleaned.get("required"), schema.get("required"));
    }
}
