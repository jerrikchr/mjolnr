//! Loading `.mjolnr/governance.yaml` ( part A).
//!
//! A leaf module: it reads a file and builds a [`GovernanceTable`]. The rules
//! themselves — what a tier means, and the guarantee that it can only narrow —
//! live in [`crate::core::governance`], which has no idea a file exists.
//!
//! Deliberately its own module rather than a corner of `routing`. Routing
//! decides *which* model answers; this decides what that model is permitted to
//! do once it does. Filing them together would invite exactly one clever
//! future change: a route that carries its own tier, which is a route granting
//! authority.

use std::path::Path;

use serde::Deserialize;

use crate::core::governance::{GovernanceRule, GovernanceTable, GovernanceTier, ModelPattern};

/// Bounds, matching `routing::definition`'s posture. A governance file is a
/// short list a human maintains; anything approaching these numbers is a
/// generated file, and a generated governance file is not a judgement.
const MAX_FILE_BYTES: u64 = 64 * 1024;
const MAX_RULES: usize = 512;

/// Something wrong with the file, surfaced rather than swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceLoadDiagnostic {
    pub path: std::path::PathBuf,
    pub detail: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMatch {
    provider: String,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(rename = "match")]
    matcher: RawMatch,
    tier: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawGovernance {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    models: Vec<RawRule>,
}

/// Parse the file's contents into a table.
///
/// # Errors
///
/// Returns the human-readable reason the file could not be used. There is no
/// partial success: see [`load_dir`] for why a half-read governance file is
/// worse than none.
pub fn parse(content: &str) -> Result<GovernanceTable, String> {
    let raw: RawGovernance =
        serde_yaml_ng::from_str(content).map_err(|error| format!("invalid YAML: {error}"))?;

    if raw.models.len() > MAX_RULES {
        return Err(format!(
            "{} rules exceeds the {MAX_RULES}-rule budget",
            raw.models.len()
        ));
    }

    let default_tier = match raw.default {
        Some(text) => {
            GovernanceTier::parse(&text).ok_or_else(|| format!("unknown default tier '{text}'"))?
        }
        // A file that lists rules but names no default means the narrowest
        // one. Writing this file at all is saying models differ; the models
        // the author did not think about are the ones to be careful with.
        None => GovernanceTier::Supervised,
    };

    let mut rules = Vec::with_capacity(raw.models.len());
    for (index, raw_rule) in raw.models.into_iter().enumerate() {
        let tier = GovernanceTier::parse(&raw_rule.tier).ok_or_else(|| {
            format!(
                "rule {}: unknown tier '{}' (expected supervised, standard, or trusted)",
                index + 1,
                raw_rule.tier
            )
        })?;
        if raw_rule.matcher.provider.trim().is_empty() || raw_rule.matcher.model.trim().is_empty() {
            return Err(format!(
                "rule {}: match needs both a provider and a model",
                index + 1
            ));
        }
        rules.push(GovernanceRule {
            provider: raw_rule.matcher.provider.trim().to_owned(),
            model: ModelPattern::parse(raw_rule.matcher.model.trim()),
            tier,
        });
    }

    Ok(GovernanceTable {
        default_tier,
        rules,
    })
}

/// Load `.mjolnr/governance.yaml`, if there is one.
///
/// Two absences that look alike and are not:
///
/// - **No file.** No declared judgement, so nothing is clamped and behaviour is
///   exactly what it was before this feature existed. Removing full-auto from
///   every project that has never heard of governance would be a breaking
///   change wearing a safety argument.
/// - **A file that will not parse.** Someone decided models differ and smed
///   cannot read what they decided. That resolves to `supervised` everywhere —
///   the narrowest table, not the absent one — because the alternative is a
///   typo silently restoring authority the file was written to withhold.
///
/// Both cases carry a diagnostic when there is anything to say. Neither panics
/// and neither halts the session: a session that will not open because of a
/// config file is a worse failure than a session that opens narrow.
#[must_use]
pub fn load_dir(project_root: &Path) -> (GovernanceTable, Vec<GovernanceLoadDiagnostic>) {
    let config_dir = crate::core::paths::resolve_workspace_config_dir(project_root);
    let path = config_dir.join("governance.yaml");
    let mut diagnostics = Vec::new();

    let Ok(metadata) = std::fs::metadata(&path) else {
        return (GovernanceTable::default(), diagnostics);
    };
    if metadata.len() > MAX_FILE_BYTES {
        diagnostics.push(GovernanceLoadDiagnostic {
            path,
            detail: format!("file exceeds the {MAX_FILE_BYTES}-byte governance budget"),
        });
        return (GovernanceTable::narrowest(), diagnostics);
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            diagnostics.push(GovernanceLoadDiagnostic {
                path,
                detail: format!("not readable UTF-8: {error}"),
            });
            return (GovernanceTable::narrowest(), diagnostics);
        }
    };

    match parse(&content) {
        Ok(table) => (table, diagnostics),
        Err(detail) => {
            diagnostics.push(GovernanceLoadDiagnostic { path, detail });
            (GovernanceTable::narrowest(), diagnostics)
        }
    }
}

/// The starting `governance.yaml`, for `smed init` to preview and write.
///
/// Ships with rows rather than empty, and with `default: supervised` rather
/// than `trusted`, and both choices are deliberate. An empty file teaches
/// nothing about the format; a permissive default would make the file's
/// presence a no-op until someone thought of every model in advance, which is
/// exactly the thinking a fail-closed default exists to not require.
///
/// The rows are the owner's standing judgement and will age — model names
/// change, and a model that needed watching last year may not. That is the
/// argument for it being a file: `smed init` previews it, never overwrites
/// it, and the owner edits or deletes it like any other.
#[must_use]
pub fn starting_file() -> (std::path::PathBuf, String) {
    (
        std::path::PathBuf::from(".mjolnr").join("governance.yaml"),
        "\
# How much supervision each model needs. Your judgement, not a measurement —
# nothing in smed ever edits this file, and no model's tier moves because of
# how it behaved. A level that drifts with last week's traffic is not a rule,
# and a level a model can move is a level a model can farm.
#
#   trusted     no ceiling. The session's own policy is what applies.
#   standard    full-auto allowed; envelope draws halved.
#   supervised  no full-auto, no envelope draws, no `a` (approve this exact
#               command for the session). Writes and commands still work —
#               with you answering each one.
#
# A tier can only ever narrow. It never grants authority you did not set.
# Evidence-gated completion is identical in all three.
#
# `default` applies to any model no row matches. It is `supervised` on purpose:
# an unknown model is one smed has no judgement about, and the wrong guess in
# that direction is cheap and visible.
#
# Matching: exact, or one trailing `*`. First match wins, top to bottom.
# Delete this file entirely and nothing is clamped.

default: supervised

models:
  - match: { provider: anthropic, model: \"claude-opus-5*\" }
    tier: trusted
  - match: { provider: openai, model: \"gpt-5.6*\" }
    tier: trusted

  - match: { provider: openrouter, model: \"moonshot/kimi-k3*\" }
    tier: standard
  - match: { provider: openrouter, model: \"minimax/m3*\" }
    tier: standard
  - match: { provider: openrouter, model: \"zhipu/glm-5.2*\" }
    tier: standard

  - match: { provider: gemini, model: \"gemini-3.5-flash*\" }
    tier: supervised
  - match: { provider: gemini, model: \"gemini-3.6-flash*\" }
    tier: supervised
"
        .to_owned(),
    )
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;
    use crate::core::model::{ModelId, ProviderId};
    use crate::core::policy::PolicyMode;

    const SHIPPED: &str = "\
default: supervised
models:
  - match: { provider: anthropic, model: claude-opus-5 }
    tier: trusted
  - match: { provider: openai, model: gpt-5.6 }
    tier: trusted
  - match: { provider: openrouter, model: \"moonshot/kimi-k3*\" }
    tier: standard
  - match: { provider: gemini, model: \"gemini-3.5-flash*\" }
    tier: supervised
  - match: { provider: gemini, model: \"gemini-3.6-flash*\" }
    tier: supervised
";

    fn tier(table: &GovernanceTable, provider: &str, model: &str) -> GovernanceTier {
        table.tier_for(&ProviderId::new(provider), &ModelId::new(model))
    }

    #[test]
    fn the_file_init_writes_loads_and_every_row_reaches_itself() {
        // The template is shipped text, so nothing but a test stops it from
        // drifting out of the format it documents. Checked with the default
        // flipped wide, so a row that matches nothing shows up as trusted —
        // the direction that hurts — rather than hiding behind a narrow
        // default that would have produced the same answer anyway.
        let (path, contents) = starting_file();
        assert_eq!(path, std::path::Path::new(".mjolnr/governance.yaml"));
        assert_eq!(
            parse(&contents)
                .expect("the shipped template must load")
                .default_tier,
            GovernanceTier::Supervised,
            "an unknown model is one smed has no judgement about"
        );

        let permissive = contents.replace("default: supervised", "default: trusted");
        let table = parse(&permissive).expect("parses");
        for (provider, model, expected) in [
            ("anthropic", "claude-opus-5", GovernanceTier::Trusted),
            ("openai", "gpt-5.6", GovernanceTier::Trusted),
            ("openrouter", "moonshot/kimi-k3", GovernanceTier::Standard),
            ("openrouter", "minimax/m3", GovernanceTier::Standard),
            ("openrouter", "zhipu/glm-5.2", GovernanceTier::Standard),
            ("gemini", "gemini-3.5-flash", GovernanceTier::Supervised),
            ("gemini", "gemini-3.6-flash", GovernanceTier::Supervised),
        ] {
            assert_eq!(
                tier(&table, provider, model),
                expected,
                "{provider}/{model} did not reach its own row"
            );
        }
    }

    #[test]
    fn the_shipped_file_parses_to_what_it_reads_as() {
        let table = parse(SHIPPED).expect("the file smed init writes must load");
        assert_eq!(table.default_tier, GovernanceTier::Supervised);
        assert_eq!(
            tier(&table, "anthropic", "claude-opus-5"),
            GovernanceTier::Trusted
        );
        assert_eq!(tier(&table, "openai", "gpt-5.6"), GovernanceTier::Trusted);
        assert_eq!(
            tier(&table, "openrouter", "moonshot/kimi-k3-preview"),
            GovernanceTier::Standard
        );
        assert_eq!(
            tier(&table, "gemini", "gemini-3.6-flash"),
            GovernanceTier::Supervised
        );
        assert_eq!(
            tier(&table, "ollama", "something-local"),
            GovernanceTier::Supervised,
            "an unlisted model takes the declared default"
        );
    }

    #[test]
    fn every_shipped_rule_matches_something_by_its_own_row() {
        // Written after the first draft shipped `gemini-3.*-flash*`, which
        // matches nothing: `*` is only a trailing wildcard, so the middle one
        // was a literal. The bug was invisible because the file's default was
        // `supervised` too — the model landed in the right tier by the wrong
        // argument, and would have quietly stopped doing so the day the
        // default changed.
        //
        // So the check flips the default to the *widest* tier. Any row that
        // fails to match now shows up as trusted, which is the direction that
        // hurts.
        let permissive = SHIPPED.replace("default: supervised", "default: trusted");
        let table = parse(&permissive).expect("parses");

        for (provider, model, expected) in [
            ("anthropic", "claude-opus-5", GovernanceTier::Trusted),
            ("openai", "gpt-5.6", GovernanceTier::Trusted),
            (
                "openrouter",
                "moonshot/kimi-k3-preview",
                GovernanceTier::Standard,
            ),
            ("gemini", "gemini-3.5-flash", GovernanceTier::Supervised),
            ("gemini", "gemini-3.6-flash", GovernanceTier::Supervised),
            (
                "gemini",
                "gemini-3.6-flash-latest",
                GovernanceTier::Supervised,
            ),
        ] {
            assert_eq!(
                tier(&table, provider, model),
                expected,
                "{provider}/{model} did not reach its own row"
            );
        }
    }

    #[test]
    fn a_wildcard_is_only_ever_a_trailing_one() {
        // The property the bug above violated, stated where it can be read.
        let table = parse(
            "default: trusted\nmodels:\n  - match: { provider: gemini, model: \"a-*-b\" }\n    tier: supervised\n",
        )
        .expect("parses");
        assert_eq!(
            tier(&table, "gemini", "a-anything-b"),
            GovernanceTier::Trusted,
            "a mid-pattern '*' is a literal, and a row that matches nothing \
             must not look like a row that matches everything"
        );
        assert_eq!(tier(&table, "gemini", "a-*-b"), GovernanceTier::Supervised);
    }

    #[test]
    fn a_file_that_will_not_parse_is_narrowest_not_absent() {
        // The failure that would otherwise be silent: a typo restoring the
        // authority the file exists to withhold.
        let directory = tempfile::tempdir().expect("temp dir");
        let smed = directory.path().join(".mjolnr");
        std::fs::create_dir_all(&smed).expect("create .mjolnr");
        std::fs::write(smed.join("governance.yaml"), "default: [not, a, tier]\n")
            .expect("write file");

        let (table, diagnostics) = load_dir(directory.path());
        assert_eq!(diagnostics.len(), 1, "the reason must be surfaced");
        assert_eq!(
            table.clamp(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-opus-5"),
                PolicyMode::FullAuto
            ),
            PolicyMode::WorkspaceWrite,
            "an unreadable judgement is not the absence of one"
        );
    }

    #[test]
    fn an_unknown_tier_spelling_fails_the_whole_file() {
        // Not "drop the bad rule": dropping it would move some model to the
        // default silently, in whichever direction the default happens to sit.
        let error = parse(
            "models:\n  - match: { provider: openai, model: gpt-5.6 }\n    tier: trustworthy\n",
        )
        .expect_err("an unknown tier must not resolve");
        assert!(
            error.contains("trustworthy"),
            "the reason names the typo: {error}"
        );
    }

    #[test]
    fn an_unknown_field_is_refused() {
        // deny_unknown_fields, asserted rather than assumed: a misspelled key
        // that parses is a rule the author believes is in force and is not.
        assert!(
            parse("models:\n  - match: { provider: openai, model: gpt-5.6 }\n    teir: trusted\n")
                .is_err()
        );
        assert!(parse("defualt: trusted\n").is_err());
    }

    #[test]
    fn no_file_changes_nothing() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (table, diagnostics) = load_dir(directory.path());
        assert!(diagnostics.is_empty());
        for requested in [
            PolicyMode::ReadOnly,
            PolicyMode::Ask,
            PolicyMode::WorkspaceWrite,
            PolicyMode::FullAuto,
        ] {
            assert_eq!(
                table.clamp(
                    &ProviderId::new("openai"),
                    &ModelId::new("gpt-5.6"),
                    requested
                ),
                requested,
                "an absent file must not take authority away from a project \
                 that has never heard of this feature"
            );
        }
    }

    #[test]
    fn a_file_with_rules_and_no_default_is_supervised() {
        let table =
            parse("models:\n  - match: { provider: openai, model: gpt-5.6 }\n    tier: trusted\n")
                .expect("parses");
        assert_eq!(table.default_tier, GovernanceTier::Supervised);
        assert_eq!(tier(&table, "openai", "gpt-5.6"), GovernanceTier::Trusted);
    }

    #[test]
    fn an_empty_match_field_is_refused() {
        assert!(
            parse("models:\n  - match: { provider: \"\", model: gpt-5.6 }\n    tier: trusted\n")
                .is_err(),
            "an empty provider would match nothing while looking like a rule"
        );
        assert!(
            parse("models:\n  - match: { provider: openai, model: \"  \" }\n    tier: trusted\n")
                .is_err()
        );
    }

    #[test]
    fn a_rule_budget_bounds_the_file() {
        use std::fmt::Write as _;
        let mut content = String::from("models:\n");
        for index in 0..=MAX_RULES {
            let _ = writeln!(
                content,
                "  - match: {{ provider: openai, model: m{index} }}\n    tier: trusted"
            );
        }
        assert!(parse(&content).is_err());
    }
}
