//! Per-model governance floors ( part A).
//!
//! `PolicyMode` is a property of the *session*. Until this module, the model on
//! the other end of the socket contributed nothing to what was permitted, so a
//! `/model` switch changed who was acting without changing what they may do.
//! That is wrong in one direction that matters: models differ in how much
//! supervision the same directive costs, and the ones that need the most are
//! exactly the ones cheap enough to leave running.
//!
//! # Why a tier is declared and never learned
//!
//! The obvious implementation — watch which models trip refusals and tighten
//! the ones that trip more often — is forbidden by `AGENTS.md` §11 law 2 as a
//! success-rate score, and it should stay forbidden for two independent
//! reasons. A governance level that moves with last week's traffic is not a
//! rule, it is a mood. And a level the *model* can move is a level the model
//! can farm: a policy that widens after a clean streak turns compliance into a
//! strategy for acquiring authority, which is precisely the property smed
//! exists to deny. Research, maybe. Not a mechanism to ship.
//!
//! So a tier is the owner's standing judgement, read from
//! `.mjolnr/governance.yaml`, diffable and revertible (law 7). Nothing in this
//! module writes one.
//!
//! # Why it can only ever narrow
//!
//! Every ceiling here is applied with [`PolicyMode::narrower_of`], so the
//! effective policy is the *narrower* of what the human set and what the model
//! is trusted with. [`GovernanceTier::Trusted`] grants nothing — an `ask`
//! session on the most trusted model in the catalogue is still `ask`. That is
//! law 4 ("nothing in flight is widened") applied to a new axis, and it is what
//! keeps this from becoming a back door into authority nobody set.
//!
//! This is the same posture [`ModelCapabilities`](crate::core::model::ModelCapabilities)
//! already takes toward wire formats: declared per model, pessimistic by
//! default, refusing before the request rather than discovering the gap
//! mid-stream.

use crate::core::model::{ModelId, ProviderId};
use crate::core::policy::PolicyMode;

/// Children one `spawn_subagent` call may request without an envelope, per
/// tier.
///
/// Mirrors `tools::subagent::MAX_CHILDREN`, which `core` may not import
/// (`AGENTS.md` §2.1). A test in `tools::subagent` asserts the two agree, so
/// the duplication cannot drift silently — the same arrangement
/// [`envelope::DEFAULT_TURNS_PER_CHILD`](crate::core::envelope::DEFAULT_TURNS_PER_CHILD)
/// already uses.
const DEFAULT_FAN_OUT: u32 = 4;

/// How much supervision a model needs, as declared by the owner.
///
/// Ordered by trust so that a table can be reasoned about, but note that
/// ordering is *not* used to widen anything: see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum GovernanceTier {
    /// The fail-closed default, and where every unmatched model lands.
    ///
    /// Not a judgement about an unknown model — an admission that smed has
    /// none. An unknown capability is absent rather than present
    /// (`core::model`), and an unknown model is supervised rather than trusted,
    /// for the same reason: the wrong guess in this direction is visible and
    /// cheap, and in the other direction is neither.
    #[default]
    Supervised,
    Standard,
    Trusted,
}

impl GovernanceTier {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Supervised => "supervised",
            Self::Standard => "standard",
            Self::Trusted => "trusted",
        }
    }

    /// Parse the spelling used in `.mjolnr/governance.yaml`.
    ///
    /// An unknown spelling is `None` rather than a silent default: a typo that
    /// quietly meant `supervised` would be survivable, and one that quietly
    /// meant `trusted` would not, so neither is allowed to happen.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "supervised" => Some(Self::Supervised),
            "standard" => Some(Self::Standard),
            "trusted" => Some(Self::Trusted),
            _ => None,
        }
    }

    /// The widest policy a session may run at on a model in this tier.
    ///
    /// Only `Supervised` withholds anything, and what it withholds is exactly
    /// unattended autonomy. A supervised model may still write and still run
    /// commands — with a human answering each one.
    #[must_use]
    pub const fn policy_ceiling(self) -> PolicyMode {
        match self {
            Self::Supervised => PolicyMode::WorkspaceWrite,
            Self::Standard | Self::Trusted => PolicyMode::FullAuto,
        }
    }

    /// Apply the ceiling to a requested policy.
    ///
    /// Narrowing only, by construction: this is `min`, never `max`.
    #[must_use]
    pub const fn clamp(self, requested: PolicyMode) -> PolicyMode {
        requested.narrower_of(self.policy_ceiling())
    }

    /// Children one `spawn_subagent` call may draw against an envelope armed
    /// for `armed` children.
    ///
    /// `None` means this tier may not draw against an envelope at all. An
    /// envelope's whole purpose is to make a *shape* approvable once so
    /// individual spawns need not be read; a model that needs supervision is
    /// the one case where that trade should not be available, because the
    /// preview is the supervision.
    #[must_use]
    pub const fn enveloped_fan_out(self, armed: u32) -> Option<u32> {
        match self {
            Self::Supervised => None,
            // Half, rounded down, and never below one: a standard model may use
            // an envelope, but a human who armed 32 did so reasoning about the
            // models they trust most.
            Self::Standard => Some(if armed / 2 == 0 { 1 } else { armed / 2 }),
            Self::Trusted => Some(armed),
        }
    }

    /// Children one `spawn_subagent` call may draw with no envelope armed.
    #[must_use]
    pub const fn fan_out(self) -> u32 {
        match self {
            Self::Supervised => 1,
            Self::Standard | Self::Trusted => DEFAULT_FAN_OUT,
        }
    }

    /// Whether `a` — approve this exact command for the session — is offered.
    ///
    /// A grant that outlives the approval is a bet on the model proposing the
    /// same command for the same reason next time. Withheld where that bet is
    /// worst; the command itself is still approvable with `y`, every time.
    #[must_use]
    pub const fn permits_exact_command_grant(self) -> bool {
        !matches!(self, Self::Supervised)
    }
}

/// How a rule's `model` field matches.
///
/// A literal, or one trailing `*`. Not a regex, and not a glob with more than
/// that: the same reasoning the code graph's import resolution records — an
/// expressive matcher that silently matches too much is worse than a dumb one
/// that misses visibly, and here "matches too much" means a model quietly
/// running at a tier the owner did not mean to give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPattern {
    Exact(String),
    Prefix(String),
}

impl ModelPattern {
    /// Build a pattern from the file's spelling.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        text.strip_suffix('*').map_or_else(
            || Self::Exact(text.to_ascii_lowercase()),
            |prefix| Self::Prefix(prefix.to_ascii_lowercase()),
        )
    }

    #[must_use]
    pub fn matches(&self, model: &str) -> bool {
        let model = model.to_ascii_lowercase();
        match self {
            Self::Exact(exact) => *exact == model,
            Self::Prefix(prefix) => model.starts_with(prefix.as_str()),
        }
    }
}

/// One declared row: which models, and what tier they run at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceRule {
    pub provider: String,
    pub model: ModelPattern,
    pub tier: GovernanceTier,
}

impl GovernanceRule {
    #[must_use]
    pub fn matches(&self, provider: &ProviderId, model: &ModelId) -> bool {
        self.provider.eq_ignore_ascii_case(provider.as_str()) && self.model.matches(model.as_str())
    }
}

/// The declared table, in file order.
///
/// First match wins and there is no scoring, no specificity ranking, and no
/// most-recently-used. A reader resolving a model by hand and smed resolving
/// it in code must reach the same row by the same argument, or the file has
/// stopped being the record of what was decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceTable {
    /// Where an unmatched model lands. Defaults to `Supervised` and a file may
    /// widen it — that is the owner writing a judgement down, which is the one
    /// mechanism this module accepts.
    pub default_tier: GovernanceTier,
    pub rules: Vec<GovernanceRule>,
}

impl Default for GovernanceTable {
    /// No file means no declared judgement.
    ///
    /// Deliberately *not* `Supervised` for everything: absent config must
    /// restore present-day behaviour exactly, as `routing::load_dir` records
    /// for routes. A project that has never heard of this feature would
    /// otherwise find full-auto gone after an upgrade, which is a breaking
    /// change wearing a safety argument.
    fn default() -> Self {
        Self {
            default_tier: GovernanceTier::Trusted,
            rules: Vec::new(),
        }
    }
}

impl GovernanceTable {
    /// Every model supervised, whatever it is.
    ///
    /// What a *present but unreadable* declaration resolves to. Distinct from
    /// [`Default`], which is the absence of one: someone wrote a file saying
    /// models differ, and smed cannot read what they said. Resolving that to
    /// the permissive table would let a typo restore exactly the authority the
    /// file was written to withhold.
    #[must_use]
    pub const fn narrowest() -> Self {
        Self {
            default_tier: GovernanceTier::Supervised,
            rules: Vec::new(),
        }
    }

    /// The declared tier for a model.
    #[must_use]
    pub fn tier_for(&self, provider: &ProviderId, model: &ModelId) -> GovernanceTier {
        self.rules
            .iter()
            .find(|rule| rule.matches(provider, model))
            .map_or(self.default_tier, |rule| rule.tier)
    }

    /// The policy a session may actually run at, given what a human set.
    #[must_use]
    pub fn clamp(
        &self,
        provider: &ProviderId,
        model: &ModelId,
        requested: PolicyMode,
    ) -> PolicyMode {
        self.tier_for(provider, model).clamp(requested)
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    const MODES: [PolicyMode; 4] = [
        PolicyMode::ReadOnly,
        PolicyMode::Ask,
        PolicyMode::WorkspaceWrite,
        PolicyMode::FullAuto,
    ];

    const TIERS: [GovernanceTier; 3] = [
        GovernanceTier::Supervised,
        GovernanceTier::Standard,
        GovernanceTier::Trusted,
    ];

    fn table() -> GovernanceTable {
        GovernanceTable {
            default_tier: GovernanceTier::Supervised,
            rules: vec![
                GovernanceRule {
                    provider: "anthropic".to_owned(),
                    model: ModelPattern::parse("claude-opus-5"),
                    tier: GovernanceTier::Trusted,
                },
                GovernanceRule {
                    provider: "openrouter".to_owned(),
                    model: ModelPattern::parse("moonshot/kimi-k3*"),
                    tier: GovernanceTier::Standard,
                },
                GovernanceRule {
                    provider: "gemini".to_owned(),
                    model: ModelPattern::parse("gemini-3.*"),
                    tier: GovernanceTier::Supervised,
                },
            ],
        }
    }

    #[test]
    fn a_tier_never_widens_a_policy() {
        // The load-bearing property. Stated over the whole cross product rather
        // than the interesting corner, because the failure this guards against
        // is a tier that grants — and a grant would show up in exactly one
        // cell nobody thought to write a case for.
        for tier in TIERS {
            for requested in MODES {
                let clamped = tier.clamp(requested);
                assert!(
                    clamped.width() <= requested.width(),
                    "{} clamped {requested:?} to {clamped:?}, which is wider",
                    tier.label(),
                );
            }
        }
    }

    #[test]
    fn the_most_trusted_tier_grants_nothing() {
        // Trusted is the absence of a ceiling, not the presence of a licence.
        for requested in MODES {
            assert_eq!(GovernanceTier::Trusted.clamp(requested), requested);
        }
    }

    #[test]
    fn a_supervised_model_cannot_run_unattended() {
        assert_eq!(
            GovernanceTier::Supervised.clamp(PolicyMode::FullAuto),
            PolicyMode::WorkspaceWrite,
            "the one authority a supervised model is refused"
        );
        // But it is not otherwise crippled: everything narrower is untouched,
        // and in particular `ask` survives as `ask` rather than collapsing to
        // workspace-write the way a *child* policy clamp collapses it. A
        // session has a human attached; that is the whole difference.
        assert_eq!(
            GovernanceTier::Supervised.clamp(PolicyMode::Ask),
            PolicyMode::Ask
        );
        assert_eq!(
            GovernanceTier::Supervised.clamp(PolicyMode::WorkspaceWrite),
            PolicyMode::WorkspaceWrite
        );
        assert_eq!(
            GovernanceTier::Supervised.clamp(PolicyMode::ReadOnly),
            PolicyMode::ReadOnly
        );
    }

    #[test]
    fn an_unknown_tier_spelling_is_refused_rather_than_defaulted() {
        assert_eq!(
            GovernanceTier::parse("trusted"),
            Some(GovernanceTier::Trusted)
        );
        assert_eq!(
            GovernanceTier::parse("  TRUSTED "),
            Some(GovernanceTier::Trusted)
        );
        assert_eq!(GovernanceTier::parse("trustd"), None);
        assert_eq!(GovernanceTier::parse(""), None);
        // The one that matters: a near-miss must not resolve to the widest tier.
        assert_eq!(GovernanceTier::parse("trust"), None);
    }

    #[test]
    fn the_default_tier_is_the_narrowest_one() {
        assert_eq!(GovernanceTier::default(), GovernanceTier::Supervised);
    }

    #[test]
    fn a_prefix_pattern_matches_only_at_the_front() {
        let pattern = ModelPattern::parse("gemini-3.*");
        assert!(pattern.matches("gemini-3.6-flash"));
        assert!(pattern.matches("gemini-3.5-flash"));
        assert!(pattern.matches("GEMINI-3.6-FLASH"), "case is not a bypass");
        assert!(!pattern.matches("gemini-2.5-pro"));
        assert!(
            !pattern.matches("not-gemini-3.6-flash"),
            "a prefix pattern must not match in the middle, or a hostile-looking \
             model name could inherit a tier it was never given"
        );
    }

    #[test]
    fn an_exact_pattern_does_not_match_a_longer_name() {
        let pattern = ModelPattern::parse("claude-opus-5");
        assert!(pattern.matches("claude-opus-5"));
        assert!(
            !pattern.matches("claude-opus-5-cheap-knockoff"),
            "an exact row is exact; extending the name must not extend the trust"
        );
    }

    #[test]
    fn first_match_wins_in_file_order() {
        let mut table = table();
        table.rules.insert(
            0,
            GovernanceRule {
                provider: "anthropic".to_owned(),
                model: ModelPattern::parse("claude-*"),
                tier: GovernanceTier::Standard,
            },
        );
        assert_eq!(
            table.tier_for(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-opus-5")
            ),
            GovernanceTier::Standard,
            "the earlier row wins; specificity does not rank, because a reader \
             resolving by hand reads top to bottom"
        );
    }

    #[test]
    fn an_unmatched_model_takes_the_declared_default() {
        let table = table();
        assert_eq!(
            table.tier_for(&ProviderId::new("openai"), &ModelId::new("gpt-5.6")),
            GovernanceTier::Supervised
        );
        assert_eq!(
            table.tier_for(
                &ProviderId::new("gemini"),
                &ModelId::new("gemini-3.6-flash")
            ),
            GovernanceTier::Supervised
        );
        assert_eq!(
            table.tier_for(
                &ProviderId::new("openrouter"),
                &ModelId::new("moonshot/kimi-k3-preview")
            ),
            GovernanceTier::Standard
        );
    }

    #[test]
    fn a_rule_does_not_match_across_providers() {
        let table = table();
        assert_eq!(
            table.tier_for(
                &ProviderId::new("openrouter"),
                &ModelId::new("claude-opus-5")
            ),
            GovernanceTier::Supervised,
            "the same model name behind a different provider is a different \
             grant, and must not inherit the tier"
        );
    }

    #[test]
    fn no_config_restores_present_day_behaviour() {
        // An absent file must not silently remove full-auto from a project that
        // has never heard of this feature.
        let table = GovernanceTable::default();
        for requested in MODES {
            assert_eq!(
                table.clamp(
                    &ProviderId::new("anything"),
                    &ModelId::new("whatever"),
                    requested
                ),
                requested
            );
        }
    }

    #[test]
    fn a_supervised_model_may_not_draw_against_an_envelope() {
        assert_eq!(GovernanceTier::Supervised.enveloped_fan_out(32), None);
        assert_eq!(GovernanceTier::Standard.enveloped_fan_out(32), Some(16));
        assert_eq!(GovernanceTier::Trusted.enveloped_fan_out(32), Some(32));
        // Halving must not silently reach zero, which would be a refusal
        // spelled as an allowance.
        assert_eq!(GovernanceTier::Standard.enveloped_fan_out(1), Some(1));
    }

    #[test]
    fn enveloped_fan_out_is_never_wider_than_what_was_armed() {
        for tier in TIERS {
            for armed in 1..=64_u32 {
                if let Some(allowed) = tier.enveloped_fan_out(armed) {
                    assert!(
                        allowed <= armed,
                        "{} drew {allowed} against an envelope armed for {armed}",
                        tier.label()
                    );
                }
            }
        }
    }

    #[test]
    fn fan_out_without_an_envelope_never_exceeds_the_standing_cap() {
        for tier in TIERS {
            assert!(tier.fan_out() <= DEFAULT_FAN_OUT);
        }
        assert_eq!(GovernanceTier::Supervised.fan_out(), 1);
    }

    #[test]
    fn only_the_supervised_tier_withholds_the_exact_command_grant() {
        assert!(!GovernanceTier::Supervised.permits_exact_command_grant());
        assert!(GovernanceTier::Standard.permits_exact_command_grant());
        assert!(GovernanceTier::Trusted.permits_exact_command_grant());
    }
}
