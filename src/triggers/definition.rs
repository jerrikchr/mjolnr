//! Trigger definitions as diffable per-project files.
//!
//! One YAML file per trigger under `.mjolnr/triggers/`, the file's stem is the
//! trigger's name. This mirrors the loading convention `context::skills` uses
//! for `.mjolnr/skills/`: plain files under version control, read once at
//! startup, a bad file produces a diagnostic rather than aborting every other
//! trigger's load.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::core::policy::PolicyMode;
use crate::core::trigger::{OverlapPolicy, TriggerSourceKind};
use crate::runtime::budget::BudgetLimits;

/// A file this build could not load as a trigger, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerLoadDiagnostic {
    pub path: PathBuf,
    pub detail: String,
}

/// Where a trigger fires from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerSource {
    /// A five-field cron expression (`minute hour day-of-month month
    /// day-of-week`), interpreted in UTC.
    Schedule { cron: String },
    /// A local-only HTTP listener. `path` defaults to `/`.
    Webhook { port: u16, path: String },
}

impl TriggerSource {
    #[must_use]
    pub const fn kind(&self) -> TriggerSourceKind {
        match self {
            Self::Schedule { .. } => TriggerSourceKind::Schedule,
            Self::Webhook { .. } => TriggerSourceKind::Webhook,
        }
    }
}

/// A loaded, validated trigger — everything the scheduler needs to fire it.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerDefinition {
    pub name: String,
    pub source: TriggerSource,
    /// Sent as the firing's directive. For a webhook trigger, the payload is
    /// appended as canonical input rather than replacing this text — the
    /// checklist requires the payload to travel as canonical input, and an
    /// operator-authored directive still frames what the payload means.
    pub directive: String,
    /// The ceiling this trigger's firings may never exceed. Used exactly as
    /// configured —  forbids "widening policy to make automation
    /// convenient" the way Phase 13 forbids a child exceeding its parent's
    /// ceiling.
    pub policy_ceiling: PolicyMode,
    pub budgets: BudgetLimits,
    pub provider: String,
    pub model: String,
    /// A named route this trigger's firings open on , instead
    /// of the fixed `provider`/`model` above. `provider`/`model` remain
    /// required regardless — a route that fails to resolve at firing time
    /// (missing file, unknown name) must still leave a firing able to open on
    /// something, rather than refusing the whole trigger.
    pub route: Option<String>,
    /// A role this trigger's firings request. Resolved
    /// through the project's route tags; an unmapped role falls back to
    /// `route` above, then to `provider`/`model`, so a role that no route
    /// claims can never leave a firing with nowhere to open.
    pub role: Option<String>,
    /// Where a human would be told about an outcome. mjolnr has no outbound
    /// notification channel yet, so this travels as inert, displayed metadata
    /// — never silently dropped, never silently acted on.
    pub notify: Option<String>,
    pub overlap: OverlapPolicy,
    /// Consecutive `Failed` outcomes before the trigger disables itself.
    pub max_consecutive_failures: u32,
}

/// The on-disk shape. Kept separate from [`TriggerDefinition`] so a YAML
/// field rename is a mechanical parse-layer edit, not a change to the type
/// the scheduler reasons about — the same split `store::wire` makes for the
/// database.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDefinition {
    schedule: Option<String>,
    webhook_port: Option<u16>,
    webhook_path: Option<String>,
    directive: String,
    policy: Option<String>,
    provider: String,
    model: String,
    route: Option<String>,
    role: Option<String>,
    notify: Option<String>,
    overlap: Option<String>,
    max_consecutive_failures: Option<u32>,
    max_provider_turns: Option<u32>,
    max_tool_calls: Option<u32>,
    max_wall_time_seconds: Option<u64>,
    command_timeout_seconds: Option<u64>,
}

const DEFAULT_MAX_CONSECUTIVE_FAILURES: u32 = 3;
const MAX_TRIGGER_FILE_BYTES: u64 = 64 * 1024;
const MAX_TRIGGER_FILES: usize = 256;

fn default_policy() -> PolicyMode {
    PolicyMode::ReadOnly
}

fn parse_policy(raw: &str) -> Result<PolicyMode, String> {
    match raw {
        "read-only" => Ok(PolicyMode::ReadOnly),
        "workspace-write" => Ok(PolicyMode::WorkspaceWrite),
        "full-auto" => Ok(PolicyMode::FullAuto),
        // `ask` needs a human, which a scheduled firing never has (plan
        // §Phase 12's headless rule applies unchanged): refused, not silently
        // downgraded.
        other => Err(format!(
            "policy `{other}` is not valid for a trigger; use read-only, workspace-write, or full-auto"
        )),
    }
}

/// Parse one trigger file's content. Exposed separately from [`load_dir`] so
/// tests can exercise parsing without a filesystem.
pub fn parse(name: String, content: &str) -> Result<TriggerDefinition, String> {
    let raw: RawDefinition =
        serde_yaml_ng::from_str(content).map_err(|error| format!("invalid YAML: {error}"))?;

    let source = match (raw.schedule, raw.webhook_port) {
        (Some(cron), None) => {
            super::schedule::CronSchedule::parse(&cron)
                .map_err(|error| format!("invalid schedule `{cron}`: {error}"))?;
            TriggerSource::Schedule { cron }
        }
        (None, Some(port)) => TriggerSource::Webhook {
            port,
            path: raw.webhook_path.unwrap_or_else(|| "/".to_owned()),
        },
        (Some(_), Some(_)) => {
            return Err(
                "a trigger must have exactly one of `schedule` or `webhook_port`, not both"
                    .to_owned(),
            );
        }
        (None, None) => {
            return Err(
                "a trigger needs `schedule` (cron) or `webhook_port` (local webhook)".to_owned(),
            );
        }
    };

    let policy_ceiling = raw
        .policy
        .as_deref()
        .map_or(Ok(default_policy()), parse_policy)?;

    let overlap = raw
        .overlap
        .as_deref()
        .map_or(Some(OverlapPolicy::Skip), OverlapPolicy::parse)
        .ok_or_else(|| {
            format!(
                "overlap `{}` is not valid; use skip, queue, or replace",
                raw.overlap.as_deref().unwrap_or_default()
            )
        })?;

    if raw.directive.trim().is_empty() {
        return Err("a trigger needs a non-empty `directive`".to_owned());
    }

    let default_limits = BudgetLimits::default();
    let budgets = BudgetLimits {
        max_provider_turns: raw
            .max_provider_turns
            .unwrap_or(default_limits.max_provider_turns),
        max_tool_calls: raw.max_tool_calls.unwrap_or(default_limits.max_tool_calls),
        max_wall_time: raw
            .max_wall_time_seconds
            .map_or(default_limits.max_wall_time, std::time::Duration::from_secs),
        command_timeout: raw.command_timeout_seconds.map_or(
            default_limits.command_timeout,
            std::time::Duration::from_secs,
        ),
        ..default_limits
    };

    Ok(TriggerDefinition {
        name,
        source,
        directive: raw.directive,
        policy_ceiling,
        budgets,
        provider: raw.provider,
        model: raw.model,
        route: raw.route,
        role: raw.role,
        notify: raw.notify,
        overlap,
        max_consecutive_failures: raw
            .max_consecutive_failures
            .unwrap_or(DEFAULT_MAX_CONSECUTIVE_FAILURES)
            .max(1),
    })
}

/// Load every `.yaml`/`.yml` file directly under `<project>/.mjolnr/triggers/`.
///
/// A missing directory is not an error — most projects have no triggers. A
/// bad file is not fatal to the others: it is reported as a diagnostic and
/// skipped, the same posture `context::skills` takes toward a bad skill file.
#[must_use]
pub fn load_dir(project_root: &Path) -> (Vec<TriggerDefinition>, Vec<TriggerLoadDiagnostic>) {
    let config_dir = crate::core::paths::resolve_workspace_config_dir(project_root);
    let directory = config_dir.join("triggers");
    let mut definitions = Vec::new();
    let mut diagnostics = Vec::new();

    let Ok(entries) = std::fs::read_dir(&directory) else {
        return (definitions, diagnostics);
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
                })
        })
        .collect();
    paths.sort();
    paths.truncate(MAX_TRIGGER_FILES);

    for path in paths {
        let Some(name) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        let name = name.to_owned();

        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                diagnostics.push(TriggerLoadDiagnostic {
                    path,
                    detail: format!("could not inspect file: {error}"),
                });
                continue;
            }
        };
        if metadata.len() > MAX_TRIGGER_FILE_BYTES {
            diagnostics.push(TriggerLoadDiagnostic {
                path,
                detail: format!(
                    "file exceeds the {MAX_TRIGGER_FILE_BYTES}-byte trigger definition budget"
                ),
            });
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                diagnostics.push(TriggerLoadDiagnostic {
                    path,
                    detail: format!("not readable UTF-8: {error}"),
                });
                continue;
            }
        };

        match parse(name, &content) {
            Ok(definition) => definitions.push(definition),
            Err(detail) => diagnostics.push(TriggerLoadDiagnostic { path, detail }),
        }
    }

    (definitions, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_schedule_trigger_parses_with_defaults() {
        let definition = parse(
            "nightly".to_owned(),
            "schedule: \"0 3 * * *\"\ndirective: run the nightly audit\nprovider: fake\nmodel: fake-1\n",
        )
        .expect("parse");
        assert_eq!(definition.name, "nightly");
        assert_eq!(definition.policy_ceiling, PolicyMode::ReadOnly);
        assert_eq!(definition.overlap, OverlapPolicy::Skip);
        assert_eq!(definition.max_consecutive_failures, 3);
        assert!(matches!(definition.source, TriggerSource::Schedule { .. }));
    }

    #[test]
    fn a_webhook_trigger_parses() {
        let definition = parse(
            "incoming".to_owned(),
            "webhook_port: 8877\ndirective: handle the payload\nprovider: fake\nmodel: fake-1\noverlap: replace\n",
        )
        .expect("parse");
        assert_eq!(definition.overlap, OverlapPolicy::Replace);
        assert!(matches!(
            definition.source,
            TriggerSource::Webhook { port: 8877, .. }
        ));
    }

    #[test]
    fn ask_policy_is_refused_a_trigger_has_no_human() {
        let error = parse(
            "x".to_owned(),
            "schedule: \"* * * * *\"\ndirective: d\nprovider: fake\nmodel: fake-1\npolicy: ask\n",
        )
        .expect_err("ask must be refused");
        assert!(error.contains("ask"));
    }

    #[test]
    fn schedule_and_webhook_together_are_refused() {
        let error = parse(
            "x".to_owned(),
            "schedule: \"* * * * *\"\nwebhook_port: 1\ndirective: d\nprovider: fake\nmodel: fake-1\n",
        )
        .expect_err("must be refused");
        assert!(error.contains("exactly one"));
    }

    #[test]
    fn neither_schedule_nor_webhook_is_refused() {
        let error = parse(
            "x".to_owned(),
            "directive: d\nprovider: fake\nmodel: fake-1\n",
        )
        .expect_err("must be refused");
        assert!(error.contains("schedule"));
    }

    #[test]
    fn a_missing_triggers_directory_yields_no_diagnostics() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (definitions, diagnostics) = load_dir(temp.path());
        assert!(definitions.is_empty());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn one_bad_file_does_not_block_the_others() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("triggers");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("good.yaml"),
            "schedule: \"0 3 * * *\"\ndirective: run\nprovider: fake\nmodel: fake-1\n",
        )
        .expect("write good");
        std::fs::write(directory.join("bad.yaml"), "not: [valid\n").expect("write bad");

        let (definitions, diagnostics) = load_dir(temp.path());
        assert_eq!(definitions.len(), 1);
        assert!(
            definitions
                .iter()
                .any(|definition| definition.name == "good")
        );
        assert_eq!(diagnostics.len(), 1);
    }
}
