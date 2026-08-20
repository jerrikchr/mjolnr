//! Provider-neutral conversation history.
//!
//! This is the canonical form every provider is translated into and out of. It
//! is deliberately *not* any provider's wire model: forcing Anthropic or Gemini
//! through a fake OpenAI shape before normalising is a plan anti-pattern
//! (§Phase 6), because the lossy step then happens twice.
//!
//! What is canonical: user-visible text, tool calls, tool results, and task
//! state. What is **not** canonical: provider-private reasoning state and cache
//! handles. Those do not survive a model switch, and mjolnr discloses that rather
//! than pretending they migrate.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::error::ReasonCode;
use crate::core::model::{ModelId, ProviderId};

/// Who produced a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool invocation proposed by a model.
///
/// `id` is the provider's correlation handle. Providers disagree about what
/// that is — OpenAI carries both an item `id` (`fc_…`) and a `call_id`
/// (`call_…`) and results must reference `call_id`; Anthropic keys by content
/// block index. The adapter resolves that and stores whatever the *result* must
/// quote back (`docs/provider-contract.md` §0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Parsed arguments. Never a raw fragment: providers stream tool arguments
    /// as partial JSON strings, and this field only exists once the provider's
    /// completion boundary has been reached and the accumulation parsed.
    pub arguments: serde_json::Value,
    /// An opaque token the producing adapter must replay verbatim when this
    /// call reappears in history, or `None` where the provider needs nothing.
    ///
    /// Gemini 3 thinking models return a `thoughtSignature` beside every
    /// `functionCall` and reject the next turn with HTTP 400 if the replayed
    /// call omits it. It lives here rather than in a cache inside the adapter
    /// because requests are rebuilt by replaying persisted messages: an
    /// in-memory map would evaporate on resume and fail the turn after.
    ///
    /// Deliberately provider-*opaque* — mjolnr never interprets it, and
    /// OpenAI's reasoning-item ids are the same shape of problem.
    pub provider_signature: Option<String>,
}

/// How a tool invocation ended.
///
/// A refusal is a normal outcome, not an error (AGENTS.md §6). It carries a
/// stable reason code back to the model so the loop can correct itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    Ok,
    Refused(ReasonCode),
    Failed(ReasonCode),
}

/// Structured facts the runtime uses for stale-read and completion guards.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolEffect {
    #[default]
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
    /// A skill's instructions entered canonical context.
    ///
    /// This is inert: it grants no script or tool authority. Persisting the
    /// effect lets recovery rebuild activation and trust from the event log.
    SkillActivated {
        name: String,
        project: bool,
    },
}

impl ToolOutcome {
    /// The stable code, if this outcome carries one. Tests assert on these;
    /// they never assert on human-readable text (AGENTS.md §6).
    #[must_use]
    pub fn reason_code(&self) -> Option<ReasonCode> {
        match self {
            Self::Ok => None,
            Self::Refused(code) | Self::Failed(code) => Some(*code),
        }
    }

    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// The result of a tool invocation, as the model will see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub outcome: ToolOutcome,
    /// Bounded content. Every tool output is truncated rather than allowed to
    /// grow without limit (AGENTS.md §5).
    pub content: String,
    /// Set when `content` was cut. Truncation is always disclosed — silently
    /// shortened output is a lie about state (AGENTS.md §1.3).
    pub truncated: bool,
    pub effect: ToolEffect,
    /// The durable `ToolCompleted` event that can be cited by `finish_task`.
    /// Assigned by the runtime after the event store accepts the result.
    pub evidence_event_id: Option<String>,
}

impl ToolResult {
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            outcome: ToolOutcome::Ok,
            content: content.into(),
            truncated: false,
            effect: ToolEffect::None,
            evidence_event_id: None,
        }
    }

    #[must_use]
    pub fn refused(code: ReasonCode, content: impl Into<String>) -> Self {
        Self {
            outcome: ToolOutcome::Refused(code),
            content: content.into(),
            truncated: false,
            effect: ToolEffect::None,
            evidence_event_id: None,
        }
    }

    #[must_use]
    pub fn failed(code: ReasonCode, content: impl Into<String>) -> Self {
        Self {
            outcome: ToolOutcome::Failed(code),
            content: content.into(),
            truncated: false,
            effect: ToolEffect::None,
            evidence_event_id: None,
        }
    }

    #[must_use]
    pub fn with_effect(mut self, effect: ToolEffect) -> Self {
        self.effect = effect;
        self
    }
}

/// One unit of canonical content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ImageRef {
        media_type: String,
        source: String,
    },
    ToolCall(ToolCall),
    ToolResult {
        call_id: String,
        name: String,
        result: ToolResult,
    },
}

/// A message in canonical history.
///
/// `provider` and `model` record what produced this message, not what should
/// replay it. After a model switch the history is mixed by construction, and
/// that provenance is what lets the UI be honest about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalMessage {
    pub id: Uuid,
    pub role: Role,
    pub blocks: Vec<ContentBlock>,
    pub provider: Option<ProviderId>,
    pub model: Option<ModelId>,
    pub created_at: OffsetDateTime,
}

impl CanonicalMessage {
    /// An inert system-authored context message, used for compact continuation.
    #[must_use]
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            role: Role::System,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            provider: None,
            model: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// A message from the user. Never carries provider attribution: the user is
    /// not a model.
    #[must_use]
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::now_v7(),
            role: Role::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            provider: None,
            model: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[must_use]
    pub fn assistant(blocks: Vec<ContentBlock>, provider: ProviderId, model: ModelId) -> Self {
        Self {
            id: Uuid::now_v7(),
            role: Role::Assistant,
            blocks,
            provider: Some(provider),
            model: Some(model),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[must_use]
    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        result: ToolResult,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            role: Role::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: call_id.into(),
                name: name.into(),
                result,
            }],
            provider: None,
            model: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    /// Concatenated text of every text block. Non-text blocks are skipped, so
    /// this is a display convenience and never a substitute for the blocks.
    #[must_use]
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.blocks.iter().filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call),
            _ => None,
        })
    }
}

/// A message together with the durable event that introduced it (
/// 16.5, Pillar 1).
///
/// The transcript needs two different things from a message. Providers need the
/// message; branching needs a name for the point in history the message sits
/// at, so a client can say "branch from *here*" without inventing its own
/// numbering. Carrying both in one entry is what keeps them from disagreeing:
/// the alternative — a parallel `Vec<u64>` beside the messages — is a second
/// index that can drift out of step with the thing it indexes.
///
/// `sequence` is the store sequence of the event that *introduced* the message:
/// `MessageAppended` for anything said, `ToolCompleted`/`ToolFailed` for a tool
/// result. It is `None` only where no such event exists — messages seeded from
/// a compaction checkpoint, which describe history rather than being history.
/// A `None` entry is not a branch point, and the UI must not offer it as one.
///
/// [`Deref`](std::ops::Deref) to the message is deliberate: every render and
/// translation site wants the message and nothing else, and making each of them
/// write `.message` would add noise without adding a decision. The sequence is
/// reached explicitly, because reaching for it *is* a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    /// The store sequence of the event that introduced this message, when one
    /// exists. See the type docs for why this may be absent.
    pub sequence: Option<u64>,
    pub message: CanonicalMessage,
}

impl TranscriptEntry {
    /// An entry anchored to the event that introduced it.
    #[must_use]
    pub fn anchored(sequence: u64, message: CanonicalMessage) -> Self {
        Self {
            sequence: Some(sequence),
            message,
        }
    }

    /// An entry with no durable event behind it, and so no branch point.
    #[must_use]
    pub fn unanchored(message: CanonicalMessage) -> Self {
        Self {
            sequence: None,
            message,
        }
    }
}

impl std::ops::Deref for TranscriptEntry {
    type Target = CanonicalMessage;

    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unanchored_entry_offers_no_branch_point() {
        // The UI decides what is selectable in `/tree` from this field. A
        // checkpoint-seeded message describes history it was not part of, so
        // rewinding "to" it would name a point the store never recorded.
        let entry = TranscriptEntry::unanchored(CanonicalMessage::user("seeded"));
        assert!(entry.sequence.is_none());
        assert_eq!(
            entry.text(),
            "seeded",
            "the message must still read through"
        );
    }

    #[test]
    fn user_messages_carry_no_provider_attribution() {
        let message = CanonicalMessage::user("hello");
        assert_eq!(message.role, Role::User);
        assert!(message.provider.is_none());
        assert!(message.model.is_none());
    }

    #[test]
    fn text_concatenates_only_text_blocks() {
        let message = CanonicalMessage::assistant(
            vec![
                ContentBlock::Text {
                    text: "Hel".to_owned(),
                },
                ContentBlock::ToolCall(ToolCall {
                    id: "call_1".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({}),
                    provider_signature: None,
                }),
                ContentBlock::Text {
                    text: "lo".to_owned(),
                },
            ],
            ProviderId::new("fake"),
            ModelId::new("fake-1"),
        );

        assert_eq!(message.text(), "Hello");
        assert_eq!(message.tool_calls().count(), 1);
    }

    #[test]
    fn ids_are_time_sortable() {
        // v7 UUIDs sort by creation time, which keeps event ordering stable
        // without a separate sequence column in memory.
        let first = CanonicalMessage::user("a");
        let second = CanonicalMessage::user("b");
        assert!(first.id < second.id, "v7 UUIDs must sort by creation time");
    }

    #[test]
    fn refusal_carries_a_stable_code() {
        let result = ToolResult::refused(ReasonCode::PathOutsideWorkspace, "nope");
        assert_eq!(
            result.outcome.reason_code(),
            Some(ReasonCode::PathOutsideWorkspace)
        );
        assert!(!result.outcome.is_ok());
    }
}
