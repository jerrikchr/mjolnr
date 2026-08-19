//! A starter routing scaffold for `smed init` (Pillar 2).
//!
//! The inverse of [`definition`](super::definition): that module *reads*
//! `.mjolnr/routes/*.yaml` and `.mjolnr/routing.yaml`; this one *writes* a first
//! honest version of them from the providers a credential resolves for. It
//! produces file contents as values and nothing more — previewing them,
//! checking what already exists, and writing are the CLI's job, so the
//! generation itself stays pure and testable.
//!
//! It does not invent model rankings. `ModelDescriptor` carries no "flagship"
//! or "fast" signal, so a scaffold that claimed one route was the cheap one and
//! another the strong one would be asserting a fact it cannot know (§1.3). The
//! primary provider's route is tagged `default`; every other route is left
//! untagged with a comment showing how to tag it. Which model is worth `plan`
//! and which `smol` is a judgement the person makes, in a diffable file.

use std::path::PathBuf;

use crate::core::model::{ModelId, ProviderId};

/// One authenticated provider and the model its starter route opens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSeed {
    pub provider: ProviderId,
    pub model: ModelId,
}

/// A route the guided onboarding flow will write, with the roles a person has
/// *confirmed* for it ( step 3). Unlike [`ProviderSeed`], which
/// carries no ranking because `smed init` refuses to invent one, this carries
/// the roles a human accepted or edited during onboarding — the fact the flow
/// exists to capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeededRoute {
    pub provider: ProviderId,
    pub model: ModelId,
    /// Roles this route answers to (`/role <name>` and subagent routing). The
    /// first route additionally always carries `default` so a session resolves,
    /// added by [`generate_with_roles`] if it is not already present.
    pub roles: Vec<String>,
}

/// A file `smed init` would create: a path relative to the project root and
/// its full contents. Emitting the whole intended set as values is what keeps
/// [`generate`] pure — the CLI decides which of these to actually write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    pub relative_path: PathBuf,
    pub contents: String,
}

/// Build a starter scaffold from the authenticated providers, primary first.
///
/// The first seed is primary: its route carries the `default` role and the
/// task-class and child-spawn defaults point at it, so `/role default` and a
/// subagent spawn both resolve the moment the files land. Every other provider
/// gets a plain named route, reachable by `/route <provider>` and ready to be
/// tagged. Returns empty for no seeds — nothing authenticated is nothing to
/// route, and an empty scaffold writes nothing.
#[must_use]
pub fn generate(seeds: &[ProviderSeed]) -> Vec<ScaffoldFile> {
    // `smed init` invents no ranking: the primary route carries only `default`,
    // every other route is left untagged. That is exactly `generate_with_roles`
    // with empty role lists, so the two share one code path.
    let routes: Vec<SeededRoute> = seeds
        .iter()
        .map(|seed| SeededRoute {
            provider: seed.provider.clone(),
            model: seed.model.clone(),
            roles: Vec::new(),
        })
        .collect();
    generate_with_roles(&routes)
}

/// Build a scaffold from routes whose roles a person has confirmed (
/// 22). The first route is primary: it always carries `default` (added here if
/// absent) so a session resolves the moment the files land, and the routing
/// config points its task-class and child-spawn defaults at it. Every route's
/// confirmed roles are written verbatim, so `/role plan` reaches whichever route
/// the human tagged. Returns empty for no routes.
#[must_use]
pub fn generate_with_roles(routes: &[SeededRoute]) -> Vec<ScaffoldFile> {
    let Some(primary) = routes.first() else {
        return Vec::new();
    };
    let primary_name = route_name(&primary.provider);
    let mut files = Vec::with_capacity(routes.len() + 1);
    files.push(ScaffoldFile {
        relative_path: routing_config_path(),
        contents: routing_config(&primary_name),
    });
    for (index, route) in routes.iter().enumerate() {
        let mut roles = route.roles.clone();
        // The primary route must answer to `default`, always — that is what the
        // routing config points at. Add it if the person did not already.
        if index == 0 && !roles.iter().any(|role| role == "default") {
            roles.insert(0, "default".to_owned());
        }
        files.push(ScaffoldFile {
            relative_path: route_path(&route_name(&route.provider)),
            contents: route_file(&route.provider, &route.model, &roles, index == 0),
        });
    }
    files
}

fn route_name(provider: &ProviderId) -> String {
    provider.as_str().to_owned()
}

fn routing_config_path() -> PathBuf {
    PathBuf::from(".mjolnr").join("routing.yaml")
}

fn route_path(name: &str) -> PathBuf {
    PathBuf::from(".mjolnr")
        .join("routes")
        .join(format!("{name}.yaml"))
}

fn routing_config(primary: &str) -> String {
    format!(
        "# Generated by `smed init`. Maps task classes and the subagent spawn\n\
         # default to routes under .mjolnr/routes/. Plain and diffable — edit freely.\n\
         task_classes:\n  default: {primary}\nchild_default: {primary}\n"
    )
}

fn route_file(provider: &ProviderId, model: &ModelId, roles: &[String], primary: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("# Generated by `smed init`. A route is an ordered fallback chain;\n");
    out.push_str("# add hops below the first to fall through on quota or typed failure.\n");
    out.push_str("hops:\n");
    // Writing to a String is infallible; the result is discarded deliberately.
    let _ = write!(
        out,
        "  - provider: \"{}\"\n    model: \"{}\"\n",
        provider.as_str(),
        model.as_str()
    );
    if primary {
        out.push_str("# Roles let `/role <name>` and subagents pick a route. Well-known roles:\n");
        out.push_str("# default, smol, slow, plan. Tag a stronger model's route [plan],\n");
        out.push_str("# a cheaper one [smol]; which model earns which is your call.\n");
    } else {
        out.push_str("# Tag this route to reach it by role, e.g. roles: [\"smol\"].\n");
    }
    out.push_str(&roles_line(roles));
    out
}

/// Render a route's roles as a YAML flow sequence, each name quoted so a role
/// that collides with a YAML keyword still parses as a string. An empty list is
/// `roles: []`, exactly what the loader reads as "no roles".
fn roles_line(roles: &[String]) -> String {
    if roles.is_empty() {
        return "roles: []\n".to_owned();
    }
    let quoted: Vec<String> = roles.iter().map(|role| format!("\"{role}\"")).collect();
    format!("roles: [{}]\n", quoted.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::definition::parse_route;

    fn seed(provider: &str, model: &str) -> ProviderSeed {
        ProviderSeed {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        }
    }

    #[test]
    fn no_seeds_scaffold_nothing() {
        assert!(generate(&[]).is_empty());
    }

    /// The route file for `name`, found by its path rather than a positional
    /// index, so a reordering of the scaffold cannot make a test lie.
    fn route_contents<'a>(files: &'a [ScaffoldFile], name: &str) -> &'a str {
        let wanted = PathBuf::from(".mjolnr")
            .join("routes")
            .join(format!("{name}.yaml"));
        files
            .iter()
            .find(|file| file.relative_path == wanted)
            .map(|file| file.contents.as_str())
            .expect("the scaffold must contain a route for this provider")
    }

    #[test]
    fn one_provider_yields_a_config_and_its_route() {
        let files = generate(&[seed("openai", "gpt-5.4")]);
        assert_eq!(files.len(), 2);
        let paths: Vec<PathBuf> = files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect();
        assert!(paths.contains(&PathBuf::from(".mjolnr/routing.yaml")));
        assert!(paths.contains(&PathBuf::from(".mjolnr/routes/openai.yaml")));
    }

    #[test]
    fn the_generated_route_parses_and_carries_the_default_role() {
        let files = generate(&[seed("openai", "gpt-5.4")]);
        let route = parse_route("openai".to_owned(), route_contents(&files, "openai"))
            .expect("the scaffold must produce a parseable route");
        assert_eq!(route.roles, vec!["default".to_owned()]);
        assert_eq!(route.hops.len(), 1);
        let hop = route.hops.first().expect("one hop");
        assert_eq!(hop.provider.as_str(), "openai");
        assert_eq!(hop.model.as_str(), "gpt-5.4");
    }

    #[test]
    fn only_the_primary_route_carries_the_default_role() {
        let files = generate(&[
            seed("anthropic", "claude-opus-x"),
            seed("openai", "gpt-5.4"),
        ]);
        // routing.yaml + two route files.
        assert_eq!(files.len(), 3);
        let primary = parse_route("anthropic".to_owned(), route_contents(&files, "anthropic"))
            .expect("primary");
        let secondary =
            parse_route("openai".to_owned(), route_contents(&files, "openai")).expect("secondary");
        assert_eq!(primary.roles, vec!["default".to_owned()]);
        assert!(
            secondary.roles.is_empty(),
            "a non-primary route must not claim the default role"
        );
    }

    fn route(provider: &str, model: &str, roles: &[&str]) -> SeededRoute {
        SeededRoute {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
        }
    }

    #[test]
    fn confirmed_roles_are_written_and_parse_back() {
        // A person tagged the flagship route [plan] during onboarding; the
        // scaffold writes exactly that, and the primary still answers `default`.
        let files = generate_with_roles(&[
            route("anthropic", "claude-opus-4-8", &["plan"]),
            route("openai", "gpt-4o-mini", &["smol"]),
        ]);
        let primary = parse_route("anthropic".to_owned(), route_contents(&files, "anthropic"))
            .expect("primary parses");
        assert!(primary.roles.contains(&"default".to_owned()));
        assert!(primary.roles.contains(&"plan".to_owned()));
        let secondary =
            parse_route("openai".to_owned(), route_contents(&files, "openai")).expect("secondary");
        assert_eq!(secondary.roles, vec!["smol".to_owned()]);
    }

    #[test]
    fn the_primary_gets_default_even_when_no_role_was_confirmed() {
        let files = generate_with_roles(&[route("openai", "gpt-4o", &[])]);
        let primary =
            parse_route("openai".to_owned(), route_contents(&files, "openai")).expect("primary");
        assert_eq!(primary.roles, vec!["default".to_owned()]);
    }

    #[test]
    fn generate_is_generate_with_roles_over_empty_role_lists() {
        // The two entry points must not drift: `smed init` is exactly the
        // guided flow with no confirmed roles.
        let seeds = [
            ProviderSeed {
                provider: ProviderId::new("anthropic"),
                model: ModelId::new("claude-opus-x"),
            },
            ProviderSeed {
                provider: ProviderId::new("openai"),
                model: ModelId::new("gpt-5.4"),
            },
        ];
        let via_generate = generate(&seeds);
        let via_roles = generate_with_roles(&[
            route("anthropic", "claude-opus-x", &[]),
            route("openai", "gpt-5.4", &[]),
        ]);
        assert_eq!(via_generate, via_roles);
    }

    #[test]
    fn a_model_id_with_a_dot_survives_the_round_trip() {
        // Quoting the scalar is what keeps `gpt-5.4` a string rather than a
        // YAML parse hazard; prove it rather than trust it.
        let files = generate(&[seed("openai", "gpt-5.4")]);
        let route =
            parse_route("openai".to_owned(), route_contents(&files, "openai")).expect("parses");
        let hop = route.hops.first().expect("one hop");
        assert_eq!(hop.model.as_str(), "gpt-5.4");
    }
}
