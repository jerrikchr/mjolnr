//! Ollama `/api/chat` adapter (Phase 7).

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::{FinishReason, ProviderEvent};
use crate::core::message::{ContentBlock, Role, ToolCall, ToolOutcome};
use crate::core::model::{ModelCapabilities, ModelDescriptor, ModelId, ProviderId, Usage};
use crate::core::provider::{Provider, ProviderCompletion, ProviderRequest};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";
pub const PROVIDER_ID: &str = "ollama";
pub const DEFAULT_MODEL: &str = "llama3.2";

#[derive(Debug)]
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize)]
struct RequestBody {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool>,
    stream: bool,
}
#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ToolCallWire>,
}
#[derive(Debug, Serialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: FunctionDefinition,
}
#[derive(Debug, Serialize)]
struct FunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}
#[derive(Debug, Serialize, Deserialize)]
struct ToolCallWire {
    function: FunctionCallWire,
}
#[derive(Debug, Serialize, Deserialize)]
struct FunctionCallWire {
    name: String,
    arguments: serde_json::Value,
}
#[derive(Debug, Deserialize)]
struct Chunk {
    done: bool,
    message: Option<ChunkMessage>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    done_reason: Option<String>,
}
#[derive(Debug, Deserialize)]
struct ChunkMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallWire>,
}

#[derive(Debug, Deserialize)]
struct ModelList {
    #[serde(default)]
    models: Vec<InstalledModel>,
}

#[derive(Debug, Deserialize)]
struct InstalledModel {
    model: String,
}

#[derive(Debug, Serialize)]
struct ShowModelRequest<'a> {
    model: &'a str,
}

#[derive(Debug, Deserialize)]
struct ShowModelResponse {
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    model_info: HashMap<String, serde_json::Value>,
}

fn translate(request: &ProviderRequest) -> RequestBody {
    let mut messages = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System => "system",
            Role::User => "user",
        };
        let mut content = String::new();
        let mut calls = Vec::new();
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => content.push_str(text),
                ContentBlock::ToolCall(call) => calls.push(ToolCallWire {
                    function: FunctionCallWire {
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    },
                }),
                ContentBlock::ToolResult { name, result, .. } => {
                    let status = match result.outcome {
                        ToolOutcome::Ok => "ok",
                        ToolOutcome::Refused(_) => "refused",
                        ToolOutcome::Failed(_) => "failed",
                    };
                    content.push_str(&serde_json::json!({"tool": name, "status": status, "content": result.content}).to_string());
                }
                // Unreachable: Ollama declares no `images_in`, so the runtime
                // projects any image into a text placeholder before a request is
                // built. Left as a drop rather than a panic
                // because a translator is the wrong place to discover a gate bug.
                ContentBlock::ImageRef { .. } => {}
            }
        }
        if !content.is_empty() || !calls.is_empty() {
            messages.push(Message {
                role: role.to_owned(),
                content,
                tool_calls: calls,
            });
        }
    }
    let tools = request
        .tools
        .iter()
        .map(|tool| Tool {
            kind: "function",
            function: FunctionDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.schema.clone(),
            },
        })
        .collect();
    if let Some(system) = &request.system {
        messages.insert(
            0,
            Message {
                role: "system".to_owned(),
                content: system.clone(),
                tool_calls: Vec::new(),
            },
        );
    }
    RequestBody {
        model: request.model.to_string(),
        messages,
        tools,
        stream: true,
    }
}

async fn emit(
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    event: ProviderEvent,
) -> Result<(), ProviderError> {
    tokio::select! { () = cancel.cancelled() => Err(ProviderError::Cancelled), result = events.send(event) => result.map_err(|_| ProviderError::Cancelled) }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }
    fn models(&self) -> Vec<ModelDescriptor> {
        vec![ModelDescriptor {
            id: ModelId::new(DEFAULT_MODEL),
            provider: self.id(),
            display_name: "Ollama local model".to_owned(),
            capabilities: ModelCapabilities::text_and_tools(),
            context_tokens: None,
            max_output_tokens: None,
            tier: None,
        }]
    }

    async fn discover_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        let response = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            result = self.client.get(format!("{}/api/tags", self.base_url)).send() => {
                result.map_err(|error| ProviderError::Transport {
                    detail: format!("Ollama is unavailable: {}", error.without_url()),
                })?
            }
        };
        if !response.status().is_success() {
            return Err(ProviderError::Protocol {
                detail: format!("Ollama model discovery http {}", response.status()),
            });
        }
        let installed =
            response
                .json::<ModelList>()
                .await
                .map_err(|error| ProviderError::Protocol {
                    detail: format!("invalid Ollama model catalog: {error}"),
                })?;

        let mut models = Vec::new();
        for installed_model in installed.models {
            let response = tokio::select! {
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                result = self.client
                    .post(format!("{}/api/show", self.base_url))
                    .json(&ShowModelRequest { model: &installed_model.model })
                    .send() => result.map_err(|error| ProviderError::Transport {
                        detail: format!("Ollama model inspection failed: {}", error.without_url()),
                    })?
            };
            if !response.status().is_success() {
                continue;
            }
            let Ok(info) = response.json::<ShowModelResponse>().await else {
                continue;
            };
            if !info
                .capabilities
                .iter()
                .any(|capability| capability == "tools")
            {
                continue;
            }
            let context_tokens = info.model_info.iter().find_map(|(key, value)| {
                key.ends_with(".context_length")
                    .then(|| value.as_u64())
                    .flatten()
                    .and_then(|value| u32::try_from(value).ok())
            });
            models.push(ModelDescriptor {
                id: ModelId::new(&installed_model.model),
                provider: self.id(),
                display_name: installed_model.model,
                capabilities: ModelCapabilities {
                    streaming: true,
                    tools: true,
                    structured_output: false,
                    images_in: info
                        .capabilities
                        .iter()
                        .any(|capability| capability == "vision"),
                    reasoning_controls: info
                        .capabilities
                        .iter()
                        .any(|capability| capability == "thinking"),
                },
                context_tokens,
                max_output_tokens: None,
                tier: None,
            });
        }
        Ok(models)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Ollama NDJSON transport and terminal accounting share one decoder state"
    )]
    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError> {
        let response = tokio::select! { () = cancel.cancelled() => return Err(ProviderError::Cancelled), result = self.client.post(format!("{}/api/chat", self.base_url)).json(&translate(&request)).send() => result.map_err(|error| ProviderError::Transport { detail: format!("Ollama is unavailable: {}", error.without_url()) })? };
        if !response.status().is_success() {
            return Err(ProviderError::Protocol {
                detail: format!(
                    "Ollama http {} (is the model installed?)",
                    response.status().as_u16()
                ),
            });
        }
        emit(&events, &cancel, ProviderEvent::Started).await?;
        let mut lines = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut calls = HashMap::new();
        loop {
            let Some(chunk) = (tokio::select! { () = cancel.cancelled() => return Err(ProviderError::Cancelled), chunk = lines.next() => chunk })
            else {
                break;
            };
            buffer.extend_from_slice(&chunk.map_err(|error| ProviderError::Transport {
                detail: error.to_string(),
            })?);
            while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=index).collect();
                let text = String::from_utf8_lossy(&line);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                let item: Chunk =
                    serde_json::from_str(text).map_err(|error| ProviderError::Protocol {
                        detail: format!("malformed Ollama NDJSON: {error}"),
                    })?;
                if let Some(message) = item.message {
                    if let Some(text) = message.content
                        && !text.is_empty()
                    {
                        emit(&events, &cancel, ProviderEvent::TextDelta { text }).await?;
                    }
                    for call in message.tool_calls {
                        let id = format!("ollama-{}", calls.len());
                        calls.insert(id.clone(), call.function.name.clone());
                        emit(
                            &events,
                            &cancel,
                            ProviderEvent::ToolCallStarted {
                                id: id.clone(),
                                name: call.function.name.clone(),
                            },
                        )
                        .await?;
                        emit(
                            &events,
                            &cancel,
                            ProviderEvent::ToolArgumentsDelta {
                                id: id.clone(),
                                fragment: call.function.arguments.to_string(),
                            },
                        )
                        .await?;
                        emit(
                            &events,
                            &cancel,
                            ProviderEvent::ToolCallCompleted {
                                call: ToolCall {
                                    id,
                                    name: call.function.name,
                                    arguments: call.function.arguments,
                                    provider_signature: None,
                                },
                            },
                        )
                        .await?;
                    }
                }
                if item.done {
                    let usage = Usage {
                        input_tokens: item.prompt_eval_count.unwrap_or_default(),
                        output_tokens: item.eval_count.unwrap_or_default(),
                    };
                    let reason = if calls.is_empty() {
                        if item.done_reason.as_deref() == Some("length") {
                            FinishReason::Incomplete
                        } else {
                            FinishReason::Stop
                        }
                    } else {
                        FinishReason::ToolCalls
                    };
                    emit(&events, &cancel, ProviderEvent::Usage { usage }).await?;
                    emit(&events, &cancel, ProviderEvent::Finished { reason }).await?;
                    return Ok(ProviderCompletion {
                        reason,
                        usage: Some(usage),
                    });
                }
            }
        }
        if !buffer.iter().all(u8::is_ascii_whitespace) {
            return Err(ProviderError::Protocol {
                detail: "Ollama stream ended with an unterminated NDJSON line".to_owned(),
            });
        }
        Err(ProviderError::Protocol {
            detail: "Ollama stream ended without done=true".to_owned(),
        })
    }
}
