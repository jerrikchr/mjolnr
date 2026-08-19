//! Anthropic Messages API wire types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(super) struct EventEnvelope {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub(super) struct CreateMessage {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<MessageParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    pub tools: Vec<ToolDefinition>,
    pub stream: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(super) struct SystemBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

impl SystemBlock {
    pub(super) fn text(text: impl Into<String>) -> Self {
        Self {
            kind: "text",
            text: text.into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct MessageParam {
    pub role: String,
    pub content: Vec<InputBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum InputBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Confirmed against current documentation 2026-07-25
    /// (`provider-contract.md` §5.5).
    Image {
        source: ImageSource,
    },
}

/// A base64 image payload. Only the `base64` source type: smed holds the bytes
/// and never hands a provider a URL to fetch, which would be an outbound request
/// nothing in the gate reviewed.
#[derive(Debug, Serialize)]
pub(super) struct ImageSource {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StreamEvent {
    MessageStart {
        message: MessageStart,
    },
    ContentBlockStart {
        index: u32,
        content_block: StartBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDelta,
        usage: Usage,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiError,
    },
}

impl StreamEvent {
    pub(super) fn recognizes(kind: &str) -> bool {
        matches!(
            kind,
            "message_start"
                | "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "message_delta"
                | "message_stop"
                | "ping"
                | "error"
        )
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageStart {
    pub usage: Usage,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StartBlock {
    Text,
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum BlockDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageDelta {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiError {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ErrorResponse {
    pub error: ApiError,
}
