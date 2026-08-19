//! Anthropic SSE decoding and indexed content-block assembly.

use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::{FinishReason, ProviderEvent};
use crate::core::message::ToolCall;
use crate::core::model::Usage;
use crate::core::provider::ProviderCompletion;

use super::wire::{BlockDelta, EventEnvelope, StartBlock, StreamEvent};

#[derive(Debug)]
struct PendingTool {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
struct StreamState {
    tools: HashMap<u32, PendingTool>,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: Option<String>,
}

pub(super) async fn decode(
    response: reqwest::Response,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    let mut stream = response.bytes_stream().eventsource();
    let mut state = StreamState::default();
    emit(events, cancel, ProviderEvent::Started).await?;

    loop {
        let next = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            next = stream.next() => next,
        };
        let Some(frame) = next else {
            return Err(ProviderError::Protocol {
                detail: "Anthropic stream ended without message_stop".to_owned(),
            });
        };
        let frame = frame.map_err(|error| ProviderError::Transport {
            detail: error.to_string(),
        })?;
        let envelope = serde_json::from_str::<EventEnvelope>(&frame.data).map_err(|error| {
            ProviderError::Protocol {
                detail: format!("malformed Anthropic event envelope: {error}"),
            }
        })?;
        if !StreamEvent::recognizes(&envelope.kind) {
            emit(
                events,
                cancel,
                ProviderEvent::UnknownUpstream {
                    kind: envelope.kind,
                },
            )
            .await?;
            continue;
        }
        let event = serde_json::from_str::<StreamEvent>(&frame.data).map_err(|error| {
            ProviderError::Protocol {
                detail: format!("malformed Anthropic {} event: {error}", envelope.kind),
            }
        })?;
        if let Some(completion) = handle(event, &mut state, events, cancel).await? {
            return Ok(completion);
        }
    }
}

async fn handle(
    event: StreamEvent,
    state: &mut StreamState,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<Option<ProviderCompletion>, ProviderError> {
    match event {
        StreamEvent::MessageStart { message } => {
            state.input_tokens = message.usage.input_tokens;
            state.output_tokens = message.usage.output_tokens;
        }
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        } => start_block(index, content_block, state, events, cancel).await?,
        StreamEvent::ContentBlockDelta { index, delta } => {
            delta_block(index, delta, state, events, cancel).await?;
        }
        StreamEvent::ContentBlockStop { index } => {
            stop_block(index, state, events, cancel).await?;
        }
        StreamEvent::MessageDelta { delta, usage } => {
            state.stop_reason = delta.stop_reason;
            state.output_tokens = usage.output_tokens;
            if usage.input_tokens > 0 {
                state.input_tokens = usage.input_tokens;
            }
        }
        StreamEvent::MessageStop => return terminal(state, events, cancel).await.map(Some),
        StreamEvent::Ping => {}
        StreamEvent::Error { error } => return Err(map_stream_error(&error.kind)),
    }
    Ok(None)
}

async fn start_block(
    index: u32,
    block: StartBlock,
    state: &mut StreamState,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let StartBlock::ToolUse { id, name, input } = block else {
        return Ok(());
    };
    if state.tools.contains_key(&index) {
        return Err(ProviderError::Protocol {
            detail: format!("duplicate Anthropic content block index {index}"),
        });
    }
    let arguments = if input.as_object().is_some_and(serde_json::Map::is_empty) {
        String::new()
    } else {
        serde_json::to_string(&input).map_err(|error| ProviderError::Protocol {
            detail: format!("Anthropic tool input was not serializable: {error}"),
        })?
    };
    emit(
        events,
        cancel,
        ProviderEvent::ToolCallStarted {
            id: id.clone(),
            name: name.clone(),
        },
    )
    .await?;
    if !arguments.is_empty() {
        emit(
            events,
            cancel,
            ProviderEvent::ToolArgumentsDelta {
                id: id.clone(),
                fragment: arguments.clone(),
            },
        )
        .await?;
    }
    state.tools.insert(
        index,
        PendingTool {
            id,
            name,
            arguments,
        },
    );
    Ok(())
}

async fn delta_block(
    index: u32,
    delta: BlockDelta,
    state: &mut StreamState,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    match delta {
        BlockDelta::TextDelta { text } => {
            emit(events, cancel, ProviderEvent::TextDelta { text }).await?;
        }
        BlockDelta::InputJsonDelta { partial_json } => {
            let Some(tool) = state.tools.get_mut(&index) else {
                return Err(ProviderError::Protocol {
                    detail: format!("tool arguments arrived for unknown Anthropic block {index}"),
                });
            };
            tool.arguments.push_str(&partial_json);
            let id = tool.id.clone();
            emit(
                events,
                cancel,
                ProviderEvent::ToolArgumentsDelta {
                    id,
                    fragment: partial_json,
                },
            )
            .await?;
        }
        BlockDelta::ThinkingDelta { thinking } => {
            emit(
                events,
                cancel,
                ProviderEvent::ReasoningDelta { text: thinking },
            )
            .await?;
        }
        // Signatures are verification material, not user-facing reasoning.
        BlockDelta::SignatureDelta | BlockDelta::Unknown => {}
    }
    Ok(())
}

async fn stop_block(
    index: u32,
    state: &mut StreamState,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let Some(tool) = state.tools.remove(&index) else {
        return Ok(());
    };
    let arguments = if tool.arguments.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&tool.arguments).map_err(|error| ProviderError::Protocol {
            detail: format!("Anthropic tool arguments were invalid at content_block_stop: {error}"),
        })?
    };
    emit(
        events,
        cancel,
        ProviderEvent::ToolCallCompleted {
            call: ToolCall {
                id: tool.id,
                name: tool.name,
                arguments,
                provider_signature: None,
            },
        },
    )
    .await
}

async fn terminal(
    state: &StreamState,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<ProviderCompletion, ProviderError> {
    if !state.tools.is_empty() {
        return Err(ProviderError::Protocol {
            detail: "Anthropic message_stop left a tool block open".to_owned(),
        });
    }
    let reason = match state.stop_reason.as_deref() {
        Some("tool_use") => FinishReason::ToolCalls,
        Some("end_turn" | "stop_sequence") => FinishReason::Stop,
        Some("max_tokens" | "refusal" | "pause_turn") => FinishReason::Incomplete,
        Some(other) => {
            return Err(ProviderError::Protocol {
                detail: format!("unknown Anthropic stop reason {other}"),
            });
        }
        None => {
            return Err(ProviderError::Protocol {
                detail: "Anthropic message_stop had no stop reason".to_owned(),
            });
        }
    };
    let usage = Usage {
        input_tokens: state.input_tokens,
        output_tokens: state.output_tokens,
    };
    emit(events, cancel, ProviderEvent::Usage { usage }).await?;
    emit(events, cancel, ProviderEvent::Finished { reason }).await?;
    Ok(ProviderCompletion {
        reason,
        usage: Some(usage),
    })
}

fn map_stream_error(kind: &str) -> ProviderError {
    match kind {
        "authentication_error" | "permission_error" => ProviderError::Auth,
        "rate_limit_error" => ProviderError::RateLimit {
            retry_after_seconds: None,
        },
        "overloaded_error" => ProviderError::Overloaded {
            retry_after_seconds: None,
        },
        other => ProviderError::Protocol {
            detail: format!("Anthropic stream error ({other})"),
        },
    }
}

async fn emit(
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    event: ProviderEvent,
) -> Result<(), ProviderError> {
    tokio::select! {
        () = cancel.cancelled() => Err(ProviderError::Cancelled),
        result = events.send(event) => result.map_err(|_| ProviderError::Cancelled),
    }
}
