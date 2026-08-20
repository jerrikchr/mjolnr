//! Provider and model identity, and capability metadata.
//!
//! Capabilities are declared, not assumed. The rule is explicit:
//! "all models" does not mean pretending every model is identical. A route that
//! needs a capability must refuse *before* sending a request rather than
//! discovering the gap in a stream half-way through.

use std::fmt;
use std::sync::Arc;

use time::OffsetDateTime;

/// Identifies a provider adapter, e.g. `openai`, `anthropic`, `ollama`.
///
/// `Arc<str>` rather than `String`: these are cloned onto every message and
/// every event, and they never change after construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderId(Arc<str>);

impl ProviderId {
    pub fn new(id: impl AsRef<str>) -> Self {
        Self(Arc::from(id.as_ref()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Identifies a model within a provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(Arc<str>);

impl ModelId {
    pub fn new(id: impl AsRef<str>) -> Self {
        Self(Arc::from(id.as_ref()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a model can actually do.
///
/// Defaults are deliberately pessimistic: an unknown capability is absent, not
/// present. That is the fail-closed rule (AGENTS.md §1.2) applied to metadata —
/// a model wrongly believed to support tools fails confusingly mid-stream, while
/// one wrongly believed not to fails immediately and visibly.
/// The lint targets bools used as an implicit state machine. This is a
/// capability record: the fields are independent facts about a model, and the
/// set is fixed by an explicit whitelist rather than open-ended. Enums here
/// would be `Streaming::Yes | Streaming::No`, which is worse.
///
/// The real cost is that adding a capability touches four places (this struct,
/// [`Capability`], [`ModelDescriptor::supports`], and [`Capability::as_str`]).
/// If that set starts churning, a bitflag over [`Capability`] removes the
/// duplication — but paying that complexity for five stable flags is premature.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a fixed capability record, not a state machine"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelCapabilities {
    pub streaming: bool,
    pub tools: bool,
    pub structured_output: bool,
    pub images_in: bool,
    pub reasoning_controls: bool,
}

impl ModelCapabilities {
    /// The common baseline: streams text, calls tools.
    #[must_use]
    pub const fn text_and_tools() -> Self {
        Self {
            streaming: true,
            tools: true,
            structured_output: false,
            images_in: false,
            reasoning_controls: false,
        }
    }

    /// The baseline plus image input.
    ///
    /// Declared per adapter rather than defaulted on, because the capability is
    /// a promise about a wire format: `provider-contract.md` §5.5 records which
    /// shapes are confirmed against documentation and which are inferred, and a
    /// model that claims this and cannot deliver it fails after the tokens are
    /// spent.
    #[must_use]
    pub const fn text_tools_and_images() -> Self {
        Self {
            images_in: true,
            ..Self::text_and_tools()
        }
    }
}

/// A capability a route may require.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Streaming,
    Tools,
    StructuredOutput,
    ImagesIn,
    ReasoningControls,
}

impl Capability {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::Tools => "tools",
            Self::StructuredOutput => "structured_output",
            Self::ImagesIn => "images_in",
            Self::ReasoningControls => "reasoning_controls",
        }
    }
}

/// Curated model tier hint for guided onboarding and route assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Flagship,
    Fast,
    Cheap,
}

impl ModelTier {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Flagship => "flagship",
            Self::Fast => "fast",
            Self::Cheap => "cheap",
        }
    }

    /// The well-known route role the onboarding flow *suggests* for a model of
    /// this tier ( step 3). It is a suggestion, never a fact: the
    /// person confirms it, and a model with no curated tier gets no suggestion.
    #[must_use]
    pub const fn suggested_role(self) -> &'static str {
        match self {
            // The scaffold's well-known roles: a strong model earns `plan`, the
            // everyday driver is `default`, a cheap one is `smol`.
            Self::Flagship => "plan",
            Self::Fast => "default",
            Self::Cheap => "smol",
        }
    }

    /// mjolnr's own curated tier suggestion for a known `(provider, model)`, or
    /// `None` when mjolnr holds no opinion — in which case the onboarding role
    /// step prompts with no suggestion rather than fabricating a ranking (plan
    /// §Phase 22: "Absent a hint, the step asks with no suggestion").
    ///
    /// Curation lives in this one place so it reads as a single, reviewable
    /// judgement rather than a claim smuggled into each provider's model table.
    /// Only models mjolnr has an actual opinion about appear; everything else is
    /// deliberately absent.
    #[must_use]
    pub fn curated(provider: &ProviderId, model: &ModelId) -> Option<Self> {
        // Nested by provider so each provider's ranking reads as its own list;
        // an unlisted provider or model falls through to no opinion.
        let tier = match provider.as_str() {
            "anthropic" => match model.as_str() {
                "claude-opus-4-8" => Self::Flagship,
                "claude-sonnet-5" => Self::Fast,
                "claude-haiku-4-5-20251001" => Self::Cheap,
                _ => return None,
            },
            "openai" => match model.as_str() {
                "gpt-4.1" => Self::Flagship,
                "gpt-4o" => Self::Fast,
                "gpt-4o-mini" | "gpt-4.1-mini" => Self::Cheap,
                _ => return None,
            },
            "openai-codex" => match model.as_str() {
                "gpt-5.4" => Self::Flagship,
                "gpt-5.4-mini" => Self::Fast,
                "gpt-5.3-codex-spark" => Self::Cheap,
                _ => return None,
            },
            "gemini" => match model.as_str() {
                "gemini-2.5-pro" => Self::Flagship,
                "gemini-2.5-flash" => Self::Fast,
                _ => return None,
            },
            "forge" if model.as_str() == "claude-sonnet-5" => Self::Flagship,
            _ => return None,
        };
        Some(tier)
    }
}

/// A model a provider offers, with what it can do and how big its context is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: ModelId,
    pub provider: ProviderId,
    pub display_name: String,
    pub capabilities: ModelCapabilities,
    pub context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub tier: Option<ModelTier>,
}

impl ModelDescriptor {
    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::Streaming => self.capabilities.streaming,
            Capability::Tools => self.capabilities.tools,
            Capability::StructuredOutput => self.capabilities.structured_output,
            Capability::ImagesIn => self.capabilities.images_in,
            Capability::ReasoningControls => self.capabilities.reasoning_controls,
        }
    }

    /// The first required capability this model lacks, if any.
    ///
    /// Callers use this to refuse *before* a request is sent (/// "unsupported model capabilities fail before sending a request").
    #[must_use]
    pub fn missing_capability(&self, required: &[Capability]) -> Option<Capability> {
        required
            .iter()
            .copied()
            .find(|capability| !self.supports(*capability))
    }
}

/// Token accounting for one provider exchange.
///
/// Normalisation is per-provider arithmetic and it is not uniform: Anthropic's
/// counts are cumulative, OpenRouter and Ollama report on a final chunk, and
/// Gemini's prompt count already includes cached tokens
/// (`docs/provider-contract.md` §6.6). Adapters normalise to *totals for this
/// exchange* so callers never have to know which dialect produced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// One provider-reported quota window.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    pub label: String,
    pub used_fraction: f32,
    pub resets_at: Option<OffsetDateTime>,
}

/// Quota facts a provider actually reported. Absence means unknown; mjolnr
/// never guesses a fraction to populate this type. Most providers report
/// this passively on the response of a request already made for useful
/// work. Google reports nothing there (E0 spike) — its `gemini_cli`
/// producer is the one deliberate exception that pays for a small side
/// request instead of leaving the type empty, documented at the call site.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSnapshot {
    pub provider: ProviderId,
    pub windows: Vec<QuotaWindow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(capabilities: ModelCapabilities) -> ModelDescriptor {
        ModelDescriptor {
            id: ModelId::new("m"),
            provider: ProviderId::new("p"),
            display_name: "m".to_owned(),
            capabilities,
            context_tokens: None,
            max_output_tokens: None,
            tier: None,
        }
    }

    #[test]
    fn unknown_capabilities_default_to_absent() {
        // Fail closed: the default must not claim anything.
        let capabilities = ModelCapabilities::default();
        assert!(!capabilities.tools);
        assert!(!capabilities.streaming);
        assert!(!capabilities.structured_output);
        assert!(!capabilities.images_in);
        assert!(!capabilities.reasoning_controls);
    }

    #[test]
    fn missing_capability_is_reported_before_a_request_would_be_sent() {
        let model = descriptor(ModelCapabilities::text_and_tools());

        assert_eq!(
            model.missing_capability(&[Capability::Streaming, Capability::Tools]),
            None
        );
        assert_eq!(
            model.missing_capability(&[Capability::Tools, Capability::ImagesIn]),
            Some(Capability::ImagesIn)
        );
    }

    #[test]
    fn usage_totals_do_not_double_count() {
        let usage = Usage {
            input_tokens: 7,
            output_tokens: 2,
        };
        assert_eq!(usage.total(), 9);
    }

    #[test]
    fn curated_tier_is_a_suggestion_only_for_models_mjolnr_has_an_opinion_about() {
        // A curated model surfaces mjolnr's ranking as a suggestion.
        assert_eq!(
            ModelTier::curated(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-opus-4-8")
            ),
            Some(ModelTier::Flagship)
        );
        assert_eq!(
            ModelTier::curated(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-haiku-4-5-20251001")
            ),
            Some(ModelTier::Cheap)
        );
        // An uncurated model gets no suggestion — never a fabricated ranking.
        assert_eq!(
            ModelTier::curated(&ProviderId::new("anthropic"), &ModelId::new("made-up")),
            None
        );
        assert_eq!(
            ModelTier::curated(&ProviderId::new("who"), &ModelId::new("gpt-4o")),
            None
        );
    }

    #[test]
    fn each_tier_maps_to_a_well_known_scaffold_role() {
        assert_eq!(ModelTier::Flagship.suggested_role(), "plan");
        assert_eq!(ModelTier::Fast.suggested_role(), "default");
        assert_eq!(ModelTier::Cheap.suggested_role(), "smol");
    }
}
