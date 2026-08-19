//! Personas: a role-bound voice overlaid on the Soul.
//!
//! A persona is a markdown file discovered from the same trust-gated locations
//! skills and templates use. A route names a persona (`persona: <name>`), and
//! because roles alias routes, the persona is what a model *wears* while filling
//! that role — the same model sounds different as `plan` than as `smol`. A
//! persona is **inert prose**, exactly like the Soul: it overlays voice and
//! preference into the system prompt and grants nothing. Every action it
//! inspires still crosses the normal policy gate.
//!
//! Unlike a prompt template, a persona's frontmatter is optional: a persona may
//! be pure prose, because its file stem already names it and nothing here needs
//! a declared description to function. When present, `description` is carried
//! for a future `/persona` listing.

use std::path::{Path, PathBuf};

use crate::core::context::{ContextDiagnostic, SkillScope};
use crate::core::error::ReasonCode;

const MAX_PERSONA_BYTES: u64 = 64 * 1024;
const MAX_PERSONAS: usize = 256;

/// The optional frontmatter a persona file may carry.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPersona {
    #[serde(default)]
    description: Option<String>,
}

/// A discovered persona, ready to overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Persona {
    pub name: String,
    pub description: Option<String>,
    pub body: String,
    pub scope: SkillScope,
    pub path: PathBuf,
}

/// The discovery roots for personas, project before user so a project may
/// deliberately override a user persona of the same name.
pub(super) fn roots(project_root: Option<&Path>, user_config: &Path) -> Vec<(PathBuf, SkillScope)> {
    let mut roots = Vec::new();
    if let Some(project) = project_root {
        let config_dir = crate::core::paths::resolve_workspace_config_dir(project);
        roots.push((config_dir.join("personas"), SkillScope::Project));
    }
    roots.push((user_config.join("personas"), SkillScope::User));
    roots
}

/// Parse one persona file: optional frontmatter, then a non-empty body.
///
/// # Errors
/// A description of what made the file unusable, for a load diagnostic. The
/// name is the file stem; a stem that could not be a route's `persona:` value
/// is refused so `persona: foo` can never silently miss a `foo bar.md`.
pub(super) fn parse(name: String, contents: &str) -> Result<Persona, String> {
    if !crate::core::routing::is_valid_role_name(&name) {
        return Err(format!(
            "`{name}` is not a usable persona name; use 1-64 characters of letters, digits, '-', or '_'"
        ));
    }
    // Frontmatter is optional: only attempt the split when the file opens with
    // a fence, so a pure-prose persona is not rejected for lacking one.
    let (description, body) = if contents.starts_with("---\n") || contents.starts_with("---\r\n") {
        let (frontmatter, body) = super::frontmatter::split(contents, "a persona")?;
        let fields = serde_yaml_ng::from_str::<RawPersona>(&frontmatter)
            .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;
        (
            fields
                .description
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty()),
            body,
        )
    } else {
        (None, contents.to_owned())
    };

    let body = body.trim();
    if body.is_empty() {
        return Err("a persona needs a non-empty body".to_owned());
    }

    Ok(Persona {
        name,
        description,
        body: body.to_owned(),
        scope: SkillScope::Project,
        path: PathBuf::new(),
    })
}

/// Every discovered persona, indexed by name.
#[derive(Debug, Clone, Default)]
pub(super) struct PersonaCatalog {
    personas: Vec<Persona>,
}

impl PersonaCatalog {
    /// Discover personas from `roots`, earlier roots winning name collisions —
    /// the same posture the prompt catalog takes, so a project override of a
    /// user persona is deliberate and the shadowed one is reported.
    pub(super) fn discover(
        roots: Vec<(PathBuf, SkillScope)>,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) -> Self {
        let mut personas: Vec<Persona> = Vec::new();
        for (root, scope) in roots {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
                })
                .collect();
            paths.sort();

            for path in paths {
                if personas.len() >= MAX_PERSONAS {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!(
                            "persona budget of {MAX_PERSONAS} reached; remaining files ignored"
                        ),
                    });
                    break;
                }
                let Some(name) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
                    continue;
                };
                let name = name.to_owned();

                match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.len() > MAX_PERSONA_BYTES => {
                        diagnostics.push(ContextDiagnostic {
                            code: ReasonCode::SchemaInvalid,
                            detail: format!(
                                "file exceeds the {MAX_PERSONA_BYTES}-byte persona budget: {}",
                                path.display()
                            ),
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        diagnostics.push(ContextDiagnostic {
                            code: ReasonCode::SchemaInvalid,
                            detail: format!("could not inspect {}: {error}", path.display()),
                        });
                        continue;
                    }
                }

                let Ok(contents) = std::fs::read_to_string(&path) else {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("{} is not readable UTF-8", path.display()),
                    });
                    continue;
                };

                match parse(name, &contents) {
                    Ok(mut persona) => {
                        if let Some(existing) =
                            personas.iter().find(|other| other.name == persona.name)
                        {
                            diagnostics.push(ContextDiagnostic {
                                code: ReasonCode::SchemaInvalid,
                                detail: format!(
                                    "persona `{}` is already defined by {}; this one is ignored",
                                    persona.name,
                                    existing.path.display()
                                ),
                            });
                            continue;
                        }
                        persona.scope = scope;
                        persona.path = path;
                        personas.push(persona);
                    }
                    Err(detail) => diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("{}: {detail}", path.display()),
                    }),
                }
            }
        }
        Self { personas }
    }

    /// The discovered personas as a client renders them — name, description,
    /// scope; never the body, which the runtime overlays by name.
    pub(super) fn summaries(&self) -> Vec<crate::core::context::PersonaSummary> {
        self.personas
            .iter()
            .map(|persona| crate::core::context::PersonaSummary {
                name: persona.name.clone(),
                description: persona.description.clone(),
                scope: persona.scope,
            })
            .collect()
    }

    /// Name-and-body pairs for change detection across a `/reload`
    /// . The body is included so an *in-place* edit — the shape
    /// self-evolution takes when smed refines a persona it already wrote — is
    /// detected, not just an add or remove.
    pub(super) fn digest(&self) -> Vec<(String, String)> {
        self.personas
            .iter()
            .map(|persona| (persona.name.clone(), persona.body.clone()))
            .collect()
    }

    /// The persona body a route's `persona:` name resolves to, if it exists.
    ///
    /// `None` means the named persona is not on disk — the caller overlays
    /// nothing rather than inventing a voice, and a route naming a persona that
    /// does not exist runs the bare Soul.
    pub(super) fn overlay(&self, name: &str) -> Option<&Persona> {
        self.personas.iter().find(|persona| persona.name == name)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "AGENTS.md §7: tests may panic freely")]
mod tests {
    use super::*;

    #[test]
    fn a_pure_prose_persona_parses_without_frontmatter() {
        let persona = parse("plan".to_owned(), "You are a rigorous architect.").unwrap();
        assert_eq!(persona.name, "plan");
        assert!(persona.description.is_none());
        assert_eq!(persona.body, "You are a rigorous architect.");
    }

    #[test]
    fn frontmatter_description_is_carried_when_present() {
        let persona = parse(
            "smol".to_owned(),
            "---\ndescription: Terse helper\n---\nBe brief.\n",
        )
        .unwrap();
        assert_eq!(persona.description.as_deref(), Some("Terse helper"));
        assert_eq!(persona.body, "Be brief.");
    }

    #[test]
    fn an_empty_persona_is_refused() {
        let error = parse("plan".to_owned(), "   \n").unwrap_err();
        assert!(error.contains("non-empty body"));
    }

    #[test]
    fn a_bad_name_is_refused_before_parsing() {
        let error = parse("not a persona".to_owned(), "prose").unwrap_err();
        assert!(error.contains("usable persona name"));
    }

    #[test]
    fn discovery_reads_a_directory_and_the_overlay_resolves_by_name() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".mjolnr").join("personas");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plan.md"), "Think first.").unwrap();

        let mut diagnostics = Vec::new();
        let catalog =
            PersonaCatalog::discover(roots(Some(temp.path()), temp.path()), &mut diagnostics);
        assert!(diagnostics.is_empty());
        assert_eq!(
            catalog.overlay("plan").map(|p| p.body.as_str()),
            Some("Think first.")
        );
        assert!(catalog.overlay("absent").is_none());
    }
}
