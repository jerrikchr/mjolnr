//! Canonical-history translation into Anthropic message blocks.

use crate::core::message::{ContentBlock, Role, ToolOutcome};
use crate::core::provider::ProviderRequest;

use super::wire::{ImageSource, InputBlock, MessageParam, ToolDefinition};

pub(super) fn messages(request: &ProviderRequest) -> Vec<MessageParam> {
    request
        .messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                Role::Assistant => "assistant",
                Role::User | Role::System | Role::Tool => "user",
            };
            let content = message
                .blocks
                .iter()
                .filter_map(|item| self::block(item, &request.images))
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| MessageParam {
                role: role.to_owned(),
                content,
            })
        })
        .collect()
}

fn block(block: &ContentBlock, images: &crate::core::image::ImageSidecar) -> Option<InputBlock> {
    match block {
        ContentBlock::Text { text } => Some(InputBlock::Text { text: text.clone() }),
        ContentBlock::ToolCall(call) => Some(InputBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.arguments.clone(),
        }),
        ContentBlock::ToolResult {
            call_id, result, ..
        } => {
            let status = match result.outcome {
                ToolOutcome::Ok => "ok".to_owned(),
                ToolOutcome::Refused(code) => format!("refused:{}", code.as_str()),
                ToolOutcome::Failed(code) => format!("failed:{}", code.as_str()),
            };
            Some(InputBlock::ToolResult {
                tool_use_id: call_id.clone(),
                content: serde_json::json!({
                    "status": status,
                    "content": result.content,
                    "truncated": result.truncated,
                    "evidence_event_id": result.evidence_event_id,
                })
                .to_string(),
                is_error: !result.outcome.is_ok(),
            })
        }
        // A block whose bytes are absent is dropped rather than sent empty. The
        // runtime only assembles a sidecar it has already filled, so reaching
        // here means the gate let through a model that cannot take images — in
        // which case the block was already projected into a text placeholder and
        // this arm is unreachable defence.
        ContentBlock::ImageRef { source, .. } => {
            images.get(source).map(|bytes| InputBlock::Image {
                source: ImageSource {
                    kind: "base64",
                    media_type: bytes.media_type.clone(),
                    data: bytes.base64(),
                },
            })
        }
    }
}

pub(super) fn tools(request: &ProviderRequest) -> Vec<ToolDefinition> {
    request
        .tools
        .iter()
        .map(|tool| {
            let mut schema = tool.schema.clone();
            if let Some(object) = schema.as_object_mut() {
                object.remove("$schema");
            }
            ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: schema,
            }
        })
        .collect()
}
