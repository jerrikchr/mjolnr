//! Persisted mirrors of canonical messages and tool values.
//!
//! One reason to change: the stored shape of conversation content.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::message::{
    CanonicalMessage, ContentBlock, ToolCall, ToolEffect, ToolOutcome, ToolResult,
};
use crate::core::model::{ModelId, ProviderId};
use crate::store::wire::enums::{ReasonCodeWire, RoleWire};

/// A tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct ToolCallWire {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Absent for every provider that needs no replay token, which keeps
    /// already-stored events byte-identical and readable — `default` covers
    /// events written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_signature: Option<String>,
}

impl From<ToolCall> for ToolCallWire {
    fn from(call: ToolCall) -> Self {
        Self {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
            provider_signature: call.provider_signature,
        }
    }
}

impl From<ToolCallWire> for ToolCall {
    fn from(call: ToolCallWire) -> Self {
        Self {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
            provider_signature: call.provider_signature,
        }
    }
}

/// How a tool invocation ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(in crate::store) enum ToolOutcomeWire {
    Ok,
    Refused { code: ReasonCodeWire },
    Failed { code: ReasonCodeWire },
}

impl From<ToolOutcome> for ToolOutcomeWire {
    fn from(outcome: ToolOutcome) -> Self {
        match outcome {
            ToolOutcome::Ok => Self::Ok,
            ToolOutcome::Refused(code) => Self::Refused {
                code: ReasonCodeWire(code),
            },
            ToolOutcome::Failed(code) => Self::Failed {
                code: ReasonCodeWire(code),
            },
        }
    }
}

impl From<ToolOutcomeWire> for ToolOutcome {
    fn from(outcome: ToolOutcomeWire) -> Self {
        match outcome {
            ToolOutcomeWire::Ok => Self::Ok,
            ToolOutcomeWire::Refused { code } => Self::Refused(code.0),
            ToolOutcomeWire::Failed { code } => Self::Failed(code.0),
        }
    }
}

/// Structured facts about what a tool did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub(in crate::store) enum ToolEffectWire {
    None,
    Read {
        path: String,
        sha256: String,
    },
    Mutation {
        path: String,
        sha256: String,
    },
    Command {
        exit_code: Option<i32>,
        success: bool,
        duration_ms: u64,
    },
    Completion {
        outcome: String,
    },
    SkillActivated {
        name: String,
        project: bool,
    },
}

impl From<ToolEffect> for ToolEffectWire {
    fn from(effect: ToolEffect) -> Self {
        match effect {
            ToolEffect::None => Self::None,
            ToolEffect::Read { path, sha256 } => Self::Read { path, sha256 },
            ToolEffect::Mutation { path, sha256 } => Self::Mutation { path, sha256 },
            ToolEffect::Command {
                exit_code,
                success,
                duration_ms,
            } => Self::Command {
                exit_code,
                success,
                duration_ms,
            },
            ToolEffect::Completion { outcome } => Self::Completion { outcome },
            ToolEffect::SkillActivated { name, project } => Self::SkillActivated { name, project },
        }
    }
}

impl From<ToolEffectWire> for ToolEffect {
    fn from(effect: ToolEffectWire) -> Self {
        match effect {
            ToolEffectWire::None => Self::None,
            ToolEffectWire::Read { path, sha256 } => Self::Read { path, sha256 },
            ToolEffectWire::Mutation { path, sha256 } => Self::Mutation { path, sha256 },
            ToolEffectWire::Command {
                exit_code,
                success,
                duration_ms,
            } => Self::Command {
                exit_code,
                success,
                duration_ms,
            },
            ToolEffectWire::Completion { outcome } => Self::Completion { outcome },
            ToolEffectWire::SkillActivated { name, project } => {
                Self::SkillActivated { name, project }
            }
        }
    }
}

/// The result of a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct ToolResultWire {
    pub outcome: ToolOutcomeWire,
    pub content: String,
    pub truncated: bool,
    pub effect: ToolEffectWire,
    pub evidence_event_id: Option<String>,
}

impl From<ToolResult> for ToolResultWire {
    fn from(result: ToolResult) -> Self {
        Self {
            outcome: result.outcome.into(),
            content: result.content,
            truncated: result.truncated,
            effect: result.effect.into(),
            evidence_event_id: result.evidence_event_id,
        }
    }
}

impl From<ToolResultWire> for ToolResult {
    fn from(result: ToolResultWire) -> Self {
        Self {
            outcome: result.outcome.into(),
            content: result.content,
            truncated: result.truncated,
            effect: result.effect.into(),
            evidence_event_id: result.evidence_event_id,
        }
    }
}

/// One unit of canonical content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "block", rename_all = "snake_case")]
pub(in crate::store) enum ContentBlockWire {
    Text {
        text: String,
    },
    ImageRef {
        media_type: String,
        source: String,
    },
    ToolCall {
        call: ToolCallWire,
    },
    ToolResult {
        call_id: String,
        name: String,
        result: ToolResultWire,
    },
}

impl From<ContentBlock> for ContentBlockWire {
    fn from(block: ContentBlock) -> Self {
        match block {
            ContentBlock::Text { text } => Self::Text { text },
            ContentBlock::ImageRef { media_type, source } => Self::ImageRef { media_type, source },
            ContentBlock::ToolCall(call) => Self::ToolCall { call: call.into() },
            ContentBlock::ToolResult {
                call_id,
                name,
                result,
            } => Self::ToolResult {
                call_id,
                name,
                result: result.into(),
            },
        }
    }
}

impl From<ContentBlockWire> for ContentBlock {
    fn from(block: ContentBlockWire) -> Self {
        match block {
            ContentBlockWire::Text { text } => Self::Text { text },
            ContentBlockWire::ImageRef { media_type, source } => {
                Self::ImageRef { media_type, source }
            }
            ContentBlockWire::ToolCall { call } => Self::ToolCall(call.into()),
            ContentBlockWire::ToolResult {
                call_id,
                name,
                result,
            } => Self::ToolResult {
                call_id,
                name,
                result: result.into(),
            },
        }
    }
}

/// A message in canonical history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct MessageWire {
    pub id: Uuid,
    pub role: RoleWire,
    pub blocks: Vec<ContentBlockWire>,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl From<CanonicalMessage> for MessageWire {
    fn from(message: CanonicalMessage) -> Self {
        Self {
            id: message.id,
            role: message.role.into(),
            blocks: message.blocks.into_iter().map(Into::into).collect(),
            provider: message.provider.map(|id| id.as_str().to_owned()),
            model: message.model.map(|id| id.as_str().to_owned()),
            created_at: message.created_at,
        }
    }
}

impl From<MessageWire> for CanonicalMessage {
    fn from(message: MessageWire) -> Self {
        Self {
            id: message.id,
            role: message.role.into(),
            blocks: message.blocks.into_iter().map(Into::into).collect(),
            provider: message.provider.map(ProviderId::new),
            model: message.model.map(ModelId::new),
            created_at: message.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ReasonCode;
    use crate::core::message::Role;

    #[test]
    fn a_tool_result_survives_a_round_trip_with_its_effect_and_evidence() {
        let result = ToolResult {
            outcome: ToolOutcome::Refused(ReasonCode::StaleFileVersion),
            content: "stale".to_owned(),
            truncated: true,
            effect: ToolEffect::Mutation {
                path: "src/a.rs".to_owned(),
                sha256: "abc".to_owned(),
            },
            evidence_event_id: Some("event-1".to_owned()),
        };

        let wire = ToolResultWire::from(result.clone());
        let json = serde_json::to_string(&wire).expect("serialize");
        let decoded: ToolResultWire = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(ToolResult::from(decoded), result);
    }

    #[test]
    fn command_evidence_survives_a_round_trip() {
        // The `finish_task` evidence chain depends on this effect surviving a
        // restart; a lost exit code turns proven work back into a claim.
        let result = ToolResult::ok("done").with_effect(ToolEffect::Command {
            exit_code: Some(0),
            success: true,
            duration_ms: 12,
        });

        let json = serde_json::to_string(&ToolResultWire::from(result.clone())).expect("serialize");
        let decoded: ToolResultWire = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ToolResult::from(decoded), result);
    }

    #[test]
    fn a_provider_signature_survives_a_round_trip() {
        // Gemini 3 rejects a replayed functionCall whose thoughtSignature is
        // missing, and requests are rebuilt from persisted messages — so losing
        // this in the store breaks the turn *after* a resume, not the one that
        // wrote it.
        let call = ToolCall {
            id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: serde_json::json!({}),
            provider_signature: Some("sig-abc123".to_owned()),
        };
        let encoded = serde_json::to_string(&ToolCallWire::from(call)).expect("encode");
        let decoded: ToolCall = serde_json::from_str::<ToolCallWire>(&encoded)
            .expect("decode")
            .into();
        assert_eq!(decoded.provider_signature.as_deref(), Some("sig-abc123"));
    }

    #[test]
    fn a_tool_call_stored_before_signatures_existed_still_loads() {
        // `deny_unknown_fields` makes the reverse direction strict, so the
        // absent-field case is the one that has to keep working: every tool call
        // already in a user's database was written without this key.
        let stored = r#"{"id":"call_1","name":"read_file","arguments":{}}"#;
        let decoded: ToolCall = serde_json::from_str::<ToolCallWire>(stored)
            .expect("an event written before the field existed must still load")
            .into();
        assert_eq!(decoded.provider_signature, None);

        // And a call carrying nothing must not start emitting the key, or every
        // non-Gemini event changes shape for no reason.
        let encoded = serde_json::to_string(&ToolCallWire::from(decoded)).expect("encode");
        assert!(
            !encoded.contains("provider_signature"),
            "an absent signature must stay absent on the wire: {encoded}"
        );
    }

    #[test]
    fn a_message_survives_a_round_trip_with_every_block_kind() {
        let message = CanonicalMessage {
            id: Uuid::now_v7(),
            role: Role::Assistant,
            blocks: vec![
                ContentBlock::Text {
                    text: "hello".to_owned(),
                },
                ContentBlock::ImageRef {
                    media_type: "image/png".to_owned(),
                    source: "file://a.png".to_owned(),
                },
                ContentBlock::ToolCall(ToolCall {
                    id: "call_1".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({ "path": "a.rs" }),
                    provider_signature: None,
                }),
                ContentBlock::ToolResult {
                    call_id: "call_1".to_owned(),
                    name: "read_file".to_owned(),
                    result: ToolResult::ok("contents"),
                },
            ],
            provider: Some(ProviderId::new("fake")),
            model: Some(ModelId::new("fake-1")),
            created_at: OffsetDateTime::now_utc(),
        };

        let wire = MessageWire::from(message.clone());
        let json = serde_json::to_string(&wire).expect("serialize");
        let decoded: MessageWire = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(CanonicalMessage::from(decoded), message);
    }

    #[test]
    fn timestamps_round_trip_to_the_same_instant() {
        let message = CanonicalMessage::user("hello");
        let json = serde_json::to_string(&MessageWire::from(message.clone())).expect("serialize");
        let decoded: MessageWire = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            CanonicalMessage::from(decoded).created_at,
            message.created_at
        );
    }
}
