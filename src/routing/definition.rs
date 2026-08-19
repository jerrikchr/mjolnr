//! Route definitions as diffable per-project files.
//!
//! Same diffable posture as Phase 14's trigger files
//! (`crate::triggers::definition`): one YAML file per route under
//! `.mjolnr/routes/`, the file's stem is the route's name, a bad file produces
//! a diagnostic rather than aborting every other route's load. A second file,
//! `.mjolnr/routing.yaml`, holds the project-wide task-class mapping, the
//! child-spawn default, and per-provider breaker configuration — the parts
//! that are not "one named chain" and so do not belong inside a route file.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::core::model::{ModelId, ProviderId};
use crate::core::routing::{BreakerConfig, RouteDefinition, RouteHop, RouteTable};

/// A file this build could not load as routing config, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingLoadDiagnostic {
    pub path: PathBuf,
    pub detail: String,
}

const MAX_ROUTE_FILE_BYTES: u64 = 64 * 1024;
const MAX_ROUTE_FILES: usize = 256;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHop {
    provider: String,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRoute {
    hops: Vec<RawHop>,
    /// Role tags, declared on the route they point at.
    #[serde(default)]
    roles: Vec<String>,
    /// The persona this route wears , by name.
    #[serde(default)]
    persona: Option<String>,
}

/// Parse one route file's content.
///
/// # Errors
/// A description of what made the file unusable: invalid YAML, or a chain
/// with fewer than one hop (a route with no provider to run on is not a
/// route, and a route with exactly one hop is a legitimate "no fallback"
/// declaration —  does not require more than one position).
pub fn parse_route(name: String, content: &str) -> Result<RouteDefinition, String> {
    let raw: RawRoute =
        serde_yaml_ng::from_str(content).map_err(|error| format!("invalid YAML: {error}"))?;
    if raw.hops.is_empty() {
        return Err("a route needs at least one hop".to_owned());
    }
    Ok(RouteDefinition {
        name,
        hops: raw
            .hops
            .into_iter()
            .map(|hop| RouteHop {
                provider: ProviderId::new(hop.provider),
                model: ModelId::new(hop.model),
            })
            .collect(),
        roles: raw.roles,
        persona: raw.persona,
    })
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawBreaker {
    failure_threshold: Option<u32>,
    recovery_timeout_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRoutingConfig {
    #[serde(default)]
    task_classes: std::collections::BTreeMap<String, String>,
    child_default: Option<String>,
    #[serde(default)]
    breakers: std::collections::BTreeMap<String, RawBreaker>,
}

/// Parse `.mjolnr/routing.yaml`'s task-class mapping, child default, and
/// breaker overrides into `table`. Route names are not validated against
/// `table.routes` here — a mapping to a route that failed to load, or does
/// not exist, resolves to nothing at lookup time rather than being rejected
/// at parse time, the same posture [`crate::triggers::definition`] takes
/// toward one bad file among many.
fn parse_routing_config(content: &str, table: &mut RouteTable) -> Result<(), String> {
    let raw: RawRoutingConfig =
        serde_yaml_ng::from_str(content).map_err(|error| format!("invalid YAML: {error}"))?;
    table.task_classes = raw.task_classes;
    table.child_default = raw.child_default;
    let default = BreakerConfig::default();
    table.breakers = raw
        .breakers
        .into_iter()
        .map(|(provider, raw)| {
            (
                provider,
                BreakerConfig {
                    failure_threshold: raw.failure_threshold.unwrap_or(default.failure_threshold),
                    recovery_timeout: raw
                        .recovery_timeout_seconds
                        .map_or(default.recovery_timeout, std::time::Duration::from_secs),
                },
            )
        })
        .collect();
    Ok(())
}

/// Load `.mjolnr/routes/*.yaml` and `.mjolnr/routing.yaml` into one
/// [`RouteTable`]. A missing directory or file is not an error — most
/// projects have no routing config, and that absence is exactly what restores
/// present-day behaviour ( checklist).
#[must_use]
pub fn load_dir(project_root: &Path) -> (RouteTable, Vec<RoutingLoadDiagnostic>) {
    let mut table = RouteTable::default();
    let mut diagnostics = Vec::new();

    let config_dir = crate::core::paths::resolve_workspace_config_dir(project_root);
    let routes_dir = config_dir.join("routes");
    if let Ok(entries) = std::fs::read_dir(&routes_dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("yaml")
                            || extension.eq_ignore_ascii_case("yml")
                    })
            })
            .collect();
        paths.sort();
        paths.truncate(MAX_ROUTE_FILES);

        for path in paths {
            let Some(name) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
                continue;
            };
            let name = name.to_owned();

            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    diagnostics.push(RoutingLoadDiagnostic {
                        path,
                        detail: format!("could not inspect file: {error}"),
                    });
                    continue;
                }
            };
            if metadata.len() > MAX_ROUTE_FILE_BYTES {
                diagnostics.push(RoutingLoadDiagnostic {
                    path,
                    detail: format!(
                        "file exceeds the {MAX_ROUTE_FILE_BYTES}-byte route definition budget"
                    ),
                });
                continue;
            }

            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    diagnostics.push(RoutingLoadDiagnostic {
                        path,
                        detail: format!("not readable UTF-8: {error}"),
                    });
                    continue;
                }
            };

            match parse_route(name.clone(), &content) {
                Ok(route) => {
                    table.routes.insert(name, route);
                }
                Err(detail) => diagnostics.push(RoutingLoadDiagnostic { path, detail }),
            }
        }
    }

    let config_path = config_dir.join("routing.yaml");
    if let Ok(content) = std::fs::read_to_string(&config_path)
        && let Err(detail) = parse_routing_config(&content, &mut table)
    {
        diagnostics.push(RoutingLoadDiagnostic {
            path: config_path,
            detail,
        });
    }

    // Build the role index from the routes that actually loaded, so a role
    // tagged on a route that failed to parse simply does not exist rather
    // than pointing at nothing.
    for (route_name, role, detail) in table.reindex_roles() {
        diagnostics.push(RoutingLoadDiagnostic {
            path: routes_dir.join(format!("{route_name}.yaml")),
            detail: format!("role '{role}' ignored: {detail}"),
        });
    }

    (table, diagnostics)
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    #[test]
    fn a_route_file_with_one_hop_parses() {
        let route = parse_route(
            "cheap".to_owned(),
            "hops:\n  - provider: openai\n    model: gpt-5-mini\n",
        )
        .expect("parse");
        assert_eq!(route.hops.len(), 1);
        assert_eq!(route.hops[0].provider, ProviderId::new("openai"));
    }

    #[test]
    fn a_route_file_with_no_hops_is_refused() {
        let error = parse_route("empty".to_owned(), "hops: []\n").expect_err("must be refused");
        assert!(error.contains("at least one hop"));
    }

    #[test]
    fn routing_config_parses_task_classes_and_breakers() {
        let mut table = RouteTable::default();
        parse_routing_config(
            "task_classes:\n  default: main\nchild_default: cheap\nbreakers:\n  openai:\n    failure_threshold: 2\n    recovery_timeout_seconds: 10\n",
            &mut table,
        )
        .expect("parse");
        assert_eq!(table.task_classes.get("default"), Some(&"main".to_owned()));
        assert_eq!(table.child_default.as_deref(), Some("cheap"));
        let breaker = table.breaker_config(&ProviderId::new("openai"));
        assert_eq!(breaker.failure_threshold, 2);
        assert_eq!(breaker.recovery_timeout, std::time::Duration::from_secs(10));
    }

    #[test]
    fn a_missing_routing_directory_yields_an_empty_table_and_no_diagnostics() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (table, diagnostics) = load_dir(temp.path());
        assert!(table.is_empty());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn one_bad_route_file_does_not_block_the_others() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("routes");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("main.yaml"),
            "hops:\n  - provider: anthropic\n    model: claude-sonnet-4-5\n",
        )
        .expect("write good");
        std::fs::write(directory.join("bad.yaml"), "not: [valid\n").expect("write bad");

        let (table, diagnostics) = load_dir(temp.path());
        assert_eq!(table.routes.len(), 1);
        assert!(table.routes.contains_key("main"));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn a_full_project_config_resolves_end_to_end() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("routes");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("main.yaml"),
            "hops:\n  - provider: anthropic\n    model: a1\n  - provider: openai\n    model: o1\n",
        )
        .expect("write route");
        std::fs::write(
            temp.path().join(".mjolnr").join("routing.yaml"),
            "task_classes:\n  default: main\nchild_default: main\n",
        )
        .expect("write routing config");

        let (table, diagnostics) = load_dir(temp.path());
        assert!(diagnostics.is_empty());
        let (resolved, _) = table.resolve(None, None, "default").expect("resolved");
        assert_eq!(resolved.name, "main");
        assert_eq!(resolved.hops.len(), 2);
    }

    #[test]
    fn a_route_file_declares_its_roles() {
        let route = parse_route(
            "cheap".to_owned(),
            "hops:\n  - provider: openai\n    model: gpt-5-mini\nroles: [smol, plan]\n",
        )
        .expect("parse");
        assert_eq!(route.roles, vec!["smol".to_owned(), "plan".to_owned()]);
    }

    #[test]
    fn a_route_file_declares_its_persona() {
        let route = parse_route(
            "plan".to_owned(),
            "hops:\n  - provider: anthropic\n    model: a1\nroles: [plan]\npersona: architect\n",
        )
        .expect("parse");
        assert_eq!(route.persona.as_deref(), Some("architect"));
    }

    #[test]
    fn a_route_file_without_a_persona_carries_none() {
        let route = parse_route(
            "main".to_owned(),
            "hops:\n  - provider: openai\n    model: gpt-5\n",
        )
        .expect("parse");
        assert!(route.persona.is_none());
    }

    #[test]
    fn a_route_file_without_roles_carries_none() {
        let route = parse_route(
            "main".to_owned(),
            "hops:\n  - provider: openai\n    model: gpt-5\n",
        )
        .expect("parse");
        assert!(route.roles.is_empty());
    }

    #[test]
    fn roles_are_indexed_across_route_files_at_load() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("routes");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("main.yaml"),
            "hops:\n  - provider: anthropic\n    model: a1\nroles: [default]\n",
        )
        .expect("write main");
        std::fs::write(
            directory.join("cheap.yaml"),
            "hops:\n  - provider: openai\n    model: o1\nroles: [smol]\n",
        )
        .expect("write cheap");

        let (table, diagnostics) = load_dir(temp.path());
        assert!(diagnostics.is_empty());
        assert_eq!(table.roles.get("smol"), Some(&"cheap".to_owned()));
        let (resolved, _) = table
            .resolve(None, Some("smol"), "default")
            .expect("resolved");
        assert_eq!(resolved.name, "cheap");
    }

    #[test]
    fn a_duplicate_role_across_files_produces_a_diagnostic_not_a_silent_win() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("routes");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(
            directory.join("alpha.yaml"),
            "hops:\n  - provider: a\n    model: m\nroles: [smol]\n",
        )
        .expect("write alpha");
        std::fs::write(
            directory.join("beta.yaml"),
            "hops:\n  - provider: b\n    model: m\nroles: [smol]\n",
        )
        .expect("write beta");

        let (table, diagnostics) = load_dir(temp.path());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].detail.contains("smol"));
        assert_eq!(table.roles.get("smol"), Some(&"alpha".to_owned()));
    }

    #[test]
    fn a_role_tagged_on_an_unparseable_route_does_not_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr").join("routes");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(directory.join("broken.yaml"), "hops: []\nroles: [smol]\n")
            .expect("write broken");

        let (table, diagnostics) = load_dir(temp.path());
        assert_eq!(diagnostics.len(), 1);
        assert!(table.roles.is_empty());
        assert!(table.route_for_role("smol").is_none());
    }
}
