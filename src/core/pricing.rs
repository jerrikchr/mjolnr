//! Labelled, overridable cost estimates.
//!
//! Every number this module produces is an estimate. 's own
//! standing principle ("estimates are labelled") applies directly: a spend
//! figure derived from a bundled per-Mtok table must never be rendered
//! alongside a provider-reported usage count as if the two were the same kind
//! of fact. [`CostEstimate`] exists specifically so a caller cannot format one
//! without the label travelling with it.

use crate::core::model::{ModelId, ProviderId, Usage};

/// Per-Mtok input/output rates for one provider/model pair, in US dollars.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPrice {
    pub provider: ProviderId,
    pub model: ModelId,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// A bundled, per-project-overridable pricing table.
#[derive(Debug, Clone, Default)]
pub struct PricingTable {
    prices: Vec<ModelPrice>,
}

impl PricingTable {
    #[must_use]
    pub fn new(prices: Vec<ModelPrice>) -> Self {
        Self { prices }
    }

    /// Bundled defaults for a handful of well-known models. Approximate,
    /// published list prices as of this phase — deliberately overridable via
    /// `.mjolnr/pricing.yaml` rather than treated as ground truth.
    #[must_use]
    pub fn bundled_defaults() -> Self {
        Self::new(vec![
            ModelPrice {
                provider: ProviderId::new("anthropic"),
                model: ModelId::new("claude-sonnet-4-5"),
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
            },
            ModelPrice {
                provider: ProviderId::new("anthropic"),
                model: ModelId::new("claude-haiku-4-5"),
                input_per_mtok: 1.0,
                output_per_mtok: 5.0,
            },
            ModelPrice {
                provider: ProviderId::new("openai"),
                model: ModelId::new("gpt-5"),
                input_per_mtok: 5.0,
                output_per_mtok: 15.0,
            },
            ModelPrice {
                provider: ProviderId::new("openai"),
                model: ModelId::new("gpt-5-mini"),
                input_per_mtok: 0.25,
                output_per_mtok: 2.0,
            },
        ])
    }

    /// Replace or add entries from `overrides`, keyed by provider/model. Later
    /// entries win, so a project's override file always beats the bundle.
    #[must_use]
    pub fn merged_with(mut self, overrides: Vec<ModelPrice>) -> Self {
        for price in overrides {
            if let Some(existing) = self
                .prices
                .iter_mut()
                .find(|entry| entry.provider == price.provider && entry.model == price.model)
            {
                *existing = price;
            } else {
                self.prices.push(price);
            }
        }
        self
    }

    #[must_use]
    pub fn rate(&self, provider: &ProviderId, model: &ModelId) -> Option<&ModelPrice> {
        self.prices
            .iter()
            .find(|price| &price.provider == provider && &price.model == model)
    }
}

/// A labelled cost estimate. The `estimate` field exists so nothing can format
/// this value without the label — : "always marked as
/// estimates; provider-reported numbers remain the only facts."
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostEstimate {
    pub usd: f64,
    pub estimate: bool,
}

/// Estimate the dollar cost of `usage` against `price`. Always
/// [`CostEstimate::estimate`] `true` — there is no code path that produces one
/// of these and calls it a fact.
#[must_use]
pub fn estimate_cost(usage: &Usage, price: &ModelPrice) -> CostEstimate {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a dollar estimate only needs display precision, not lossless integer recovery"
    )]
    let input = usage.input_tokens as f64 / 1_000_000.0 * price.input_per_mtok;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a dollar estimate only needs display precision, not lossless integer recovery"
    )]
    let output = usage.output_tokens as f64 / 1_000_000.0 * price.output_per_mtok;
    CostEstimate {
        usd: input + output,
        estimate: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_defaults_cover_the_common_models() {
        let table = PricingTable::bundled_defaults();
        assert!(
            table
                .rate(
                    &ProviderId::new("anthropic"),
                    &ModelId::new("claude-sonnet-4-5")
                )
                .is_some()
        );
        assert!(
            table
                .rate(&ProviderId::new("unknown"), &ModelId::new("unknown"))
                .is_none()
        );
    }

    #[test]
    fn an_override_replaces_the_bundled_rate_rather_than_duplicating_it() {
        let table = PricingTable::bundled_defaults().merged_with(vec![ModelPrice {
            provider: ProviderId::new("anthropic"),
            model: ModelId::new("claude-sonnet-4-5"),
            input_per_mtok: 1.0,
            output_per_mtok: 1.0,
        }]);
        let price = table
            .rate(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-sonnet-4-5"),
            )
            .expect("overridden rate");
        assert!((price.input_per_mtok - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_cost_estimate_is_always_labelled_as_an_estimate() {
        let price = ModelPrice {
            provider: ProviderId::new("p"),
            model: ModelId::new("m"),
            input_per_mtok: 2.0,
            output_per_mtok: 10.0,
        };
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
        };
        let estimate = estimate_cost(&usage, &price);
        assert!(
            estimate.estimate,
            "a cost figure must self-identify as an estimate"
        );
        assert!((estimate.usd - 7.0).abs() < f64::EPSILON);
    }
}
