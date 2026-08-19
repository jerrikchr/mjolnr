//! Loads `.mjolnr/pricing.yaml` overrides onto the bundled pricing table
//! ("a bundled, overridable per-Mtok pricing table").

use std::path::Path;

use serde::Deserialize;

use crate::core::model::{ModelId, ProviderId};
use crate::core::pricing::{ModelPrice, PricingTable};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPrice {
    provider: String,
    model: String,
    input_per_mtok: f64,
    output_per_mtok: f64,
}

const MAX_PRICING_FILE_BYTES: u64 = 64 * 1024;

/// Load the bundled defaults, then override with `.mjolnr/pricing.yaml` if it
/// exists and parses. A missing or unreadable file is not an error — the
/// bundle stands on its own — and a malformed file leaves the bundle
/// untouched rather than aborting startup, the same fail-soft posture
/// `crate::triggers::definition` and `crate::routing::definition` take
/// toward one bad config file.
#[must_use]
pub fn load(project_root: &Path) -> PricingTable {
    let config_dir = crate::core::paths::resolve_workspace_config_dir(project_root);
    let path = config_dir.join("pricing.yaml");
    let bundled = PricingTable::bundled_defaults();
    let Ok(metadata) = std::fs::metadata(&path) else {
        return bundled;
    };
    if metadata.len() > MAX_PRICING_FILE_BYTES {
        return bundled;
    }
    let Ok(content) = std::fs::read_to_string(&path) else {
        return bundled;
    };
    let Ok(raw) = serde_yaml_ng::from_str::<Vec<RawPrice>>(&content) else {
        return bundled;
    };
    bundled.merged_with(
        raw.into_iter()
            .map(|price| ModelPrice {
                provider: ProviderId::new(price.provider),
                model: ModelId::new(price.model),
                input_per_mtok: price.input_per_mtok,
                output_per_mtok: price.output_per_mtok,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_pricing_file_yields_the_bundled_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let table = load(temp.path());
        assert!(
            table
                .rate(
                    &ProviderId::new("anthropic"),
                    &ModelId::new("claude-sonnet-4-5")
                )
                .is_some()
        );
    }

    #[test]
    fn a_project_override_replaces_the_bundled_rate() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".mjolnr")).expect("mkdir");
        std::fs::write(
            temp.path().join(".mjolnr").join("pricing.yaml"),
            "- provider: anthropic\n  model: claude-sonnet-4-5\n  input_per_mtok: 1.0\n  output_per_mtok: 1.0\n",
        )
        .expect("write");
        let table = load(temp.path());
        let price = table
            .rate(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-sonnet-4-5"),
            )
            .expect("overridden rate");
        assert!((price.input_per_mtok - 1.0).abs() < f64::EPSILON);
    }
}
