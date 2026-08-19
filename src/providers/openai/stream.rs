//! Shared OpenAI Responses SSE state machine.
//!
//! Both API-key OpenAI and `ChatGPT` subscription traffic use this documented
//! event dialect. Authentication, endpoints, request envelopes, and HTTP error
//! semantics remain owned by their distinct provider adapters.

use std::collections::HashMap;

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::event::{FinishReason, ProviderEvent};
use crate::core::model::Usage;
use crate::core::provider::ProviderCompletion;

use super::wire;

/// Map a `response.failed` payload to a typed error.
///
/// **This is the mid-stream half of the rate-limit story.** The HTTP status was
/// 200; only `error.code` reveals a rate limit. Mapping this to a generic
/// protocol error would send the user retrying into the same wall.
pub(super) fn map_response_error(
    response: &wire::Response,
    dialect: ResponseDialect,
) -> ProviderError {
    let code = response
        .error
        .as_ref()
        .and_then(|error| error.code.as_deref())
        .unwrap_or("unknown");

    match code {
        "rate_limit_exceeded" => ProviderError::RateLimit {
            retry_after_seconds: None,
        },
        code if dialect == ResponseDialect::Subscription && is_plan_quota_code(code) => {
            ProviderError::PlanQuota {
                reset_at_unix: response
                    .error
                    .as_ref()
                    .and_then(|error| error.reset_at_unix),
            }
        }
        "invalid_api_key" => ProviderError::Auth,
        other => ProviderError::Protocol {
            detail: format!("response.failed ({other})"),
        },
    }
}

pub(crate) fn is_plan_quota_code(code: &str) -> bool {
    matches!(
        code,
        "usage_limit_reached"
            | "plan_quota_exceeded"
            | "rate_limit_reached"
            | "workspace_owner_credits_depleted"
            | "workspace_member_credits_depleted"
            | "workspace_owner_usage_limit_reached"
            | "workspace_member_usage_limit_reached"
    )
}

pub(super) fn to_usage(usage: &wire::ResponseUsage) -> Usage {
    // `input_tokens` already includes cached tokens and `output_tokens` already
    // includes reasoning tokens: the `_details` are breakdowns, not addenda.
    Usage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
    }
}

/// Consume the SSE stream and emit normalised events.
///
/// Owns *transport* concerns only: frame iteration, the `[DONE]` sentinel, the
/// bare `error` frame, and undecodable frames. Event semantics live in
/// [`handle_event`]. The split is not cosmetic — mixing them produced a function
/// clippy flagged for cognitive complexity, which was a fair signal that two
/// different jobs were sharing one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseDialect {
    Api,
    Subscription,
}

struct DecodeState {
    usage: Option<Usage>,
    tool_call_ids: HashMap<String, String>,
    saw_tool_call: bool,
    dialect: ResponseDialect,
}

pub(crate) async fn decode_stream(
    response: reqwest::Response,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    dialect: ResponseDialect,
) -> Result<ProviderCompletion, ProviderError> {
    let mut stream = response.bytes_stream().eventsource();

    if !emit(events, cancel, ProviderEvent::Started).await {
        return Err(ProviderError::Cancelled);
    }

    // Argument events carry the output item id (`fc_…`), while smed and the
    // eventual function-call output must use `call_id` (`call_…`). Preserve the
    // relationship announced by `response.output_item.added` for the stream.
    let mut state = DecodeState {
        usage: None,
        tool_call_ids: HashMap::new(),
        saw_tool_call: false,
        dialect,
    };

    loop {
        let next = tokio::select! {
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            next = stream.next() => next,
        };

        let Some(frame) = next else {
            // The stream ended without a terminal event. Output may already have
            // been produced, so this is reported, never retried.
            return Err(ProviderError::Protocol {
                detail: "stream ended without a terminal event".to_owned(),
            });
        };

        let frame = frame.map_err(|error| ProviderError::Transport {
            detail: error.to_string(),
        })?;

        // Confirmed from the spec: `event: done` / `data: [DONE]`. A transport
        // sentinel, not a provider event.
        if frame.data.trim() == "[DONE]" {
            continue;
        }

        // A bare, unwrapped `Error` — unlike the HTTP body, which wraps it.
        if frame.event == "error" {
            return Err(error_frame(&frame.data, events, cancel, dialect).await);
        }

        let Ok(event) = serde_json::from_str::<wire::StreamEvent>(&frame.data) else {
            // A frame we cannot even shape-match. Retained, not fatal.
            emit(
                events,
                cancel,
                ProviderEvent::UnknownUpstream {
                    kind: frame.event.clone(),
                },
            )
            .await;
            continue;
        };

        if let Some(outcome) = handle_event(event, &frame.event, events, cancel, &mut state).await {
            return outcome;
        }
    }
}

/// Decode a mid-stream `error` frame into a typed error.
async fn error_frame(
    data: &str,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    dialect: ResponseDialect,
) -> ProviderError {
    let body = serde_json::from_str::<wire::ErrorFrameBody>(data).ok();
    let code = body
        .as_ref()
        .and_then(|body| body.code.clone())
        .or_else(|| {
            body.as_ref()
                .and_then(|body| body.error.as_ref())
                .and_then(|error| error.code.clone().or_else(|| error.kind.clone()))
        })
        .or_else(|| {
            body.as_ref()
                .and_then(|body| body.kind.clone())
                .filter(|kind| kind != "error")
        })
        .unwrap_or_else(|| "error".to_owned());
    let error = match code.as_str() {
        "rate_limit_exceeded" => ProviderError::RateLimit {
            retry_after_seconds: None,
        },
        code if dialect == ResponseDialect::Subscription && is_plan_quota_code(code) => {
            ProviderError::PlanQuota {
                reset_at_unix: body.as_ref().and_then(|body| {
                    body.reset_at_unix
                        .or_else(|| body.error.as_ref().and_then(|error| error.reset_at_unix))
                }),
            }
        }
        "invalid_api_key" => ProviderError::Auth,
        _ => ProviderError::Protocol { detail: code },
    };

    emit(
        events,
        cancel,
        ProviderEvent::Failed {
            detail: error.to_string(),
        },
    )
    .await;

    error
}

/// Apply one decoded event.
///
/// Returns `Some` when the stream reached a terminal state, `None` to keep
/// reading. Terminal states are the three the spec defines — completed, failed,
/// incomplete — and each maps to a distinct outcome rather than being collapsed.
/// Continue reading on success, surface the error as a terminal result.
///
/// The per-event handlers return `Result<(), ProviderError>`; the decode loop
/// speaks `Option<Result<ProviderCompletion, ProviderError>>`, where `None`
/// means "keep reading". This is the one translation between them.
fn step(result: Result<(), ProviderError>) -> Option<Result<ProviderCompletion, ProviderError>> {
    match result {
        Ok(()) => None,
        Err(error) => Some(Err(error)),
    }
}

async fn handle_event(
    event: wire::StreamEvent,
    frame_event: &str,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    state: &mut DecodeState,
) -> Option<Result<ProviderCompletion, ProviderError>> {
    match event {
        wire::StreamEvent::OutputTextDelta { delta } => {
            return display_delta(ProviderEvent::TextDelta { text: delta }, events, cancel).await;
        }

        wire::StreamEvent::ReasoningSummaryTextDelta { delta }
        | wire::StreamEvent::ReasoningTextDelta { delta } => {
            return display_delta(
                ProviderEvent::ReasoningDelta { text: delta },
                events,
                cancel,
            )
            .await;
        }

        wire::StreamEvent::OutputItemAdded { item } => {
            if matches!(&item, wire::OutputItem::FunctionCall { .. }) {
                state.saw_tool_call = true;
            }
            return step(
                handle_output_item_added(item, &mut state.tool_call_ids, events, cancel).await,
            );
        }

        wire::StreamEvent::FunctionCallArgumentsDelta { item_id, delta } => {
            return step(
                handle_tool_arguments_delta(item_id, delta, &state.tool_call_ids, events, cancel)
                    .await,
            );
        }

        wire::StreamEvent::FunctionCallArgumentsDone {
            item_id,
            call_id: reported_call_id,
            name,
            arguments,
        } => {
            return step(
                handle_completed_tool_arguments(
                    item_id,
                    reported_call_id,
                    name,
                    &arguments,
                    &mut state.tool_call_ids,
                    events,
                    cancel,
                )
                .await,
            );
        }

        wire::StreamEvent::Completed { response } => {
            let reason = if state.saw_tool_call {
                FinishReason::ToolCalls
            } else {
                FinishReason::Stop
            };
            return terminal_result(reason, &response, events, cancel, state).await;
        }

        wire::StreamEvent::Incomplete { response } => {
            // Neither success nor failure: the model stopped early. Reporting it
            // as either would misreport state (AGENTS.md §1.3).
            return terminal_result(FinishReason::Incomplete, &response, events, cancel, state)
                .await;
        }

        wire::StreamEvent::Failed { response } => {
            let error = map_response_error(&response, state.dialect);
            emit(
                events,
                cancel,
                ProviderEvent::Failed {
                    detail: error.to_string(),
                },
            )
            .await;
            return Some(Err(error));
        }

        // A fallback close for a function call the argument-done path never
        // settled; see [`handle_output_item_done`].
        wire::StreamEvent::OutputItemDone { item } => {
            return step(
                handle_output_item_done(item, &mut state.tool_call_ids, events, cancel).await,
            );
        }

        wire::StreamEvent::Unknown => {
            emit(
                events,
                cancel,
                ProviderEvent::UnknownUpstream {
                    kind: frame_event.to_owned(),
                },
            )
            .await;
        }

        // Structural events smed does not act on. Named rather than wildcarded
        // so a new variant forces a decision here.
        wire::StreamEvent::Created
        | wire::StreamEvent::InProgress
        | wire::StreamEvent::OutputTextDone => {}
    }

    None
}

async fn terminal_result(
    reason: FinishReason,
    response: &wire::Response,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    state: &mut DecodeState,
) -> Option<Result<ProviderCompletion, ProviderError>> {
    Some(Ok(terminal(
        reason,
        response,
        events,
        cancel,
        &mut state.usage,
    )
    .await))
}

async fn display_delta(
    event: ProviderEvent,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Option<Result<ProviderCompletion, ProviderError>> {
    (!emit(events, cancel, event).await).then_some(Err(ProviderError::Cancelled))
}

/// Close a function call the argument-done event never closed.
///
/// Some Responses backends stream a function call as `output_item.added` plus a
/// terminal `output_item.done` carrying the fully-populated arguments, with no
/// intervening `function_call_arguments.done`. This resolves the item back to
/// its callable id and completes the call from the item's own arguments. A call
/// already settled through the argument path is absent from `tool_call_ids` and
/// is left untouched.
async fn handle_output_item_done(
    item: wire::OutputItem,
    tool_call_ids: &mut HashMap<String, String>,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let wire::OutputItem::FunctionCall {
        id: item_id,
        name,
        arguments,
        ..
    } = item
    else {
        return Ok(());
    };

    // Already completed by `function_call_arguments.done`; nothing owed.
    let Some(call_id) = tool_call_ids.remove(&item_id) else {
        return Ok(());
    };

    complete_tool_call(call_id, name, &arguments, events, cancel).await
}

async fn handle_output_item_added(
    item: wire::OutputItem,
    tool_call_ids: &mut HashMap<String, String>,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let wire::OutputItem::FunctionCall {
        id: item_id,
        call_id,
        name,
        ..
    } = item
    else {
        return Ok(());
    };

    if tool_call_ids
        .insert(item_id.clone(), call_id.clone())
        .is_some()
    {
        return Err(ProviderError::Protocol {
            detail: format!("duplicate function call item id {item_id}"),
        });
    }

    if emit(
        events,
        cancel,
        ProviderEvent::ToolCallStarted { id: call_id, name },
    )
    .await
    {
        Ok(())
    } else {
        Err(ProviderError::Cancelled)
    }
}

async fn handle_tool_arguments_delta(
    item_id: String,
    delta: String,
    tool_call_ids: &HashMap<String, String>,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let Some(call_id) = tool_call_ids.get(&item_id).cloned() else {
        return Err(ProviderError::Protocol {
            detail: format!("arguments arrived for unknown function call item {item_id}"),
        });
    };

    if emit(
        events,
        cancel,
        ProviderEvent::ToolArgumentsDelta {
            id: call_id,
            fragment: delta,
        },
    )
    .await
    {
        Ok(())
    } else {
        Err(ProviderError::Cancelled)
    }
}

/// Resolve the output item back to its callable id and close the tool call.
async fn handle_completed_tool_arguments(
    item_id: String,
    reported_call_id: Option<String>,
    name: String,
    arguments: &str,
    tool_call_ids: &mut HashMap<String, String>,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let Some(call_id) = tool_call_ids.remove(&item_id) else {
        return Err(ProviderError::Protocol {
            detail: format!("completed arguments arrived for unknown function call item {item_id}"),
        });
    };

    if reported_call_id
        .as_deref()
        .is_some_and(|reported| reported != call_id)
    {
        return Err(ProviderError::Protocol {
            detail: format!("function call id changed for item {item_id}"),
        });
    }

    complete_tool_call(call_id, name, arguments, events, cancel).await
}

/// Parse accumulated tool arguments at the provider's completion boundary.
///
/// `response.function_call_arguments.done` is the **only** point at which the
/// accumulation is valid JSON — every prefix is a syntax error by construction.
/// A failure here means the provider's own completion boundary produced
/// something unparseable, which is a protocol error, not a recoverable one.
async fn complete_tool_call(
    call_id: String,
    name: String,
    arguments: &str,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    // A no-argument call carries an empty string here (the wire field defaults
    // to `""`), which is not valid JSON but means the empty object.
    let parsed = if arguments.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(arguments).map_err(|error| ProviderError::Protocol {
            detail: format!(
                "tool arguments were not valid JSON at the completion boundary: {error}"
            ),
        })?
    };

    let call = crate::core::message::ToolCall {
        id: call_id,
        name,
        arguments: parsed,
        provider_signature: None,
    };

    if emit(events, cancel, ProviderEvent::ToolCallCompleted { call }).await {
        Ok(())
    } else {
        Err(ProviderError::Cancelled)
    }
}

/// Report usage and finish, for the two non-error terminal states.
async fn terminal(
    reason: FinishReason,
    response: &wire::Response,
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    usage: &mut Option<Usage>,
) -> ProviderCompletion {
    if let Some(reported) = response.usage.as_ref().map(to_usage) {
        *usage = Some(reported);
        emit(events, cancel, ProviderEvent::Usage { usage: reported }).await;
    }

    emit(events, cancel, ProviderEvent::Finished { reason }).await;

    ProviderCompletion {
        reason,
        usage: *usage,
    }
}

/// Send an event, honouring cancellation and backpressure.
async fn emit(
    events: &mpsc::Sender<ProviderEvent>,
    cancel: &CancellationToken,
    event: ProviderEvent,
) -> bool {
    tokio::select! {
        () = cancel.cancelled() => false,
        result = events.send(event) => result.is_ok(),
    }
}
