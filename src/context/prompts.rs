//! Prompt templates: reusable prompts that expand from a slash command
//! (Pillar 3).
//!
//! A template is a markdown file with frontmatter, discovered from the same
//! trust-gated locations skills use. Invoking one expands its body into the
//! *user message* — never the system prompt, and never anything executable.
//! A template is text a human could have typed; every tool it leads the model
//! toward is gated exactly as it would have been if they had.
//!
//! Provenance: `pi`'s prompt templates (`docs/prompt-templates.md`, reviewed
//! 2026-07-22), read for shape. Argument syntax follows the same positional
//! form so a template written for one harness reads correctly in the other.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::core::context::{ContextDiagnostic, SkillScope};
use crate::core::error::ReasonCode;

/// The on-disk frontmatter of a prompt template.
///
/// `name` is deliberately absent: a template's name is its file stem. Skills
/// carry a name because the standard they implement requires one; a template
/// answers to the thing a user types, and letting a file disagree with its own
/// command name is a way to make `/foo` run `bar.md`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    description: String,
    #[serde(default, rename = "argument-hint")]
    argument_hint: Option<String>,
}

/// A discovered, validated prompt template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    /// Free text shown next to the command, e.g. `<file> [message]`.
    pub argument_hint: Option<String>,
    pub body: String,
    pub scope: SkillScope,
    pub path: PathBuf,
}

const MAX_TEMPLATE_BYTES: u64 = 64 * 1024;
const MAX_TEMPLATES: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 1_024;
const MAX_ARGUMENT_HINT_CHARS: usize = 200;

/// Whether a file stem is usable as a template's command name.
///
/// Matches the shape of the built-in commands it shares a namespace with:
/// lowercase, hyphenated, no surprises. A name that needs quoting is a name
/// that cannot be typed after a slash.
#[must_use]
pub fn is_valid_template_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// Parse one template file's contents.
///
/// # Errors
/// A description of what made the file unusable, for a load diagnostic.
pub fn parse(name: String, contents: &str) -> Result<PromptTemplate, String> {
    if !is_valid_template_name(&name) {
        return Err(format!(
            "`{name}` is not a usable template name; use lowercase letters, digits, and hyphens"
        ));
    }
    let (frontmatter, body) = super::frontmatter::split(contents, "a prompt template")?;
    let fields = serde_yaml_ng::from_str::<RawTemplate>(&frontmatter)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;

    let description = fields.description.trim();
    let description_length = description.chars().count();
    if description_length == 0 || description_length > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "`description` must contain 1-{MAX_DESCRIPTION_CHARS} characters; found {description_length}"
        ));
    }
    if let Some(hint) = fields.argument_hint.as_deref() {
        let hint_length = hint.trim().chars().count();
        if hint_length > MAX_ARGUMENT_HINT_CHARS {
            return Err(format!(
                "`argument-hint` must contain at most {MAX_ARGUMENT_HINT_CHARS} characters; found {hint_length}"
            ));
        }
    }

    let body = body.trim();
    if body.is_empty() {
        return Err("a prompt template needs a non-empty body".to_owned());
    }

    Ok(PromptTemplate {
        name,
        description: description.to_owned(),
        argument_hint: fields
            .argument_hint
            .map(|hint| hint.trim().to_owned())
            .filter(|hint| !hint.is_empty()),
        body: body.to_owned(),
        scope: SkillScope::Project,
        path: PathBuf::new(),
    })
}

/// Expand a template body against positional arguments.
///
/// Supported forms, matching pi's so a template reads the same in both:
/// - `$1`..`$9` — one positional argument
/// - `$@` — every argument, space-joined
/// - `${1:-default}` — a positional argument, or `default` when absent
///
/// A `$N` whose argument was not supplied is left **verbatim**, not replaced
/// with nothing. Silently deleting it would turn a template containing `costs
/// $5.00` into `costs .00` for anyone who passed fewer than five arguments —
/// a corruption the author cannot see. Leaving the token visible is a mistake
/// they can; `${N:-}` says "empty when absent" explicitly.
///
/// Everything else, `$` included, is left exactly as written: a template is
/// text, and a substitution engine that guesses is a template that cannot
/// contain a shell snippet.
#[must_use]
pub fn expand(body: &str, arguments: &[String]) -> String {
    let characters: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut index = 0;
    while index < characters.len() {
        let Some(&character) = characters.get(index) else {
            break;
        };
        if character != '$' {
            out.push(character);
            index += 1;
            continue;
        }
        match characters.get(index + 1) {
            Some('@') => {
                out.push_str(&arguments.join(" "));
                index += 2;
            }
            Some(digit) if digit.is_ascii_digit() && *digit != '0' => {
                let position = (*digit as usize) - ('0' as usize);
                if let Some(argument) = arguments.get(position - 1) {
                    out.push_str(argument);
                    index += 2;
                } else {
                    out.push(character);
                    index += 1;
                }
            }
            Some('{') => {
                if let Some((text, next)) = expand_braced(&characters, index, arguments) {
                    out.push_str(&text);
                    index = next;
                } else {
                    out.push(character);
                    index += 1;
                }
            }
            _ => {
                out.push(character);
                index += 1;
            }
        }
    }
    out
}

/// Expand `${N}` or `${N:-default}` starting at `$`. Returns the replacement
/// and the index just past the closing brace, or `None` when the text is not
/// a form we substitute — in which case the caller leaves it verbatim.
fn expand_braced(
    characters: &[char],
    dollar: usize,
    arguments: &[String],
) -> Option<(String, usize)> {
    let close = characters
        .iter()
        .skip(dollar + 2)
        .position(|character| *character == '}')?
        + dollar
        + 2;
    let inner: String = characters.get(dollar + 2..close)?.iter().collect();
    let (position_text, default) = match inner.split_once(":-") {
        Some((position, default)) => (position, Some(default)),
        None => (inner.as_str(), None),
    };
    let position: usize = position_text.parse().ok()?;
    if position == 0 {
        return None;
    }
    let value = arguments
        .get(position - 1)
        .map(String::as_str)
        .or(default)
        .unwrap_or_default();
    Some((value.to_owned(), close + 1))
}

/// Split a command's argument text the way a shell would for positional use:
/// whitespace-separated, with double quotes grouping.
#[must_use]
pub fn split_arguments(text: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for character in text.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            character if character.is_whitespace() && !quoted => {
                if any {
                    arguments.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            character => {
                current.push(character);
                any = true;
            }
        }
    }
    if any {
        arguments.push(current);
    }
    arguments
}

/// Every discovered template, indexed by name.
#[derive(Debug, Clone, Default)]
pub struct PromptCatalog {
    templates: Vec<PromptTemplate>,
}

impl PromptCatalog {
    /// Discover templates from `roots`, earlier roots winning name collisions.
    ///
    /// Project roots are listed before user roots by the caller, so a project
    /// may override a user template deliberately — and the shadowed one is
    /// reported rather than silently dropped.
    pub fn discover(
        roots: Vec<(PathBuf, SkillScope)>,
        diagnostics: &mut Vec<ContextDiagnostic>,
    ) -> Self {
        let mut templates: Vec<PromptTemplate> = Vec::new();
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
                if templates.len() >= MAX_TEMPLATES {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("prompt template budget of {MAX_TEMPLATES} reached; remaining files ignored"
                        ),
                    });
                    break;
                }
                let Some(name) = path.file_stem().and_then(std::ffi::OsStr::to_str) else {
                    continue;
                };
                let name = name.to_owned();

                match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.len() > MAX_TEMPLATE_BYTES => {
                        diagnostics.push(ContextDiagnostic {
                            code: ReasonCode::SchemaInvalid,
                            detail: format!(
                                "file exceeds the {MAX_TEMPLATE_BYTES}-byte prompt template budget"
                            ),
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        diagnostics.push(ContextDiagnostic {
                            code: ReasonCode::SchemaInvalid,
                            detail: format!("could not inspect file: {error}"),
                        });
                        continue;
                    }
                }

                let Ok(contents) = std::fs::read_to_string(&path) else {
                    diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: "not readable UTF-8".to_owned(),
                    });
                    continue;
                };

                match parse(name.clone(), &contents) {
                    Ok(mut template) => {
                        if let Some(existing) =
                            templates.iter().find(|other| other.name == template.name)
                        {
                            diagnostics.push(ContextDiagnostic {
                                code: ReasonCode::SchemaInvalid,
                                detail: format!("prompt template `{}` is already defined by {}; this one is ignored",
                                    template.name,
                                    existing.path.display()
                                ),
                            });
                            continue;
                        }
                        template.scope = scope;
                        template.path = path;
                        templates.push(template);
                    }
                    Err(detail) => diagnostics.push(ContextDiagnostic {
                        code: ReasonCode::SchemaInvalid,
                        detail: format!("{}: {detail}", path.display()),
                    }),
                }
            }
        }
        templates.sort_by(|left, right| left.name.cmp(&right.name));
        Self { templates }
    }

    #[must_use]
    pub fn templates(&self) -> &[PromptTemplate] {
        &self.templates
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.iter().find(|template| template.name == name)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// The discovery roots for prompt templates, project first.
#[must_use]
pub fn roots(project_root: Option<&Path>, user_config: &Path) -> Vec<(PathBuf, SkillScope)> {
    let mut roots = Vec::new();
    if let Some(project) = project_root {
        let config_dir = crate::core::paths::resolve_workspace_config_dir(project);
        roots.push((config_dir.join("prompts"), SkillScope::Project));
    }
    roots.push((user_config.join("prompts"), SkillScope::User));
    roots
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    fn template(body: &str) -> String {
        format!("---\ndescription: A test template\n---\n{body}\n")
    }

    #[test]
    fn a_template_parses_its_frontmatter_and_body() {
        let parsed = parse("review".to_owned(), &template("Review $1 carefully.")).expect("parse");
        assert_eq!(parsed.name, "review");
        assert_eq!(parsed.description, "A test template");
        assert_eq!(parsed.body, "Review $1 carefully.");
        assert!(parsed.argument_hint.is_none());
    }

    #[test]
    fn an_argument_hint_is_carried_through() {
        let parsed = parse(
            "review".to_owned(),
            "---\ndescription: d\nargument-hint: <file>\n---\nbody\n",
        )
        .expect("parse");
        assert_eq!(parsed.argument_hint.as_deref(), Some("<file>"));
    }

    #[test]
    fn a_template_without_frontmatter_is_refused() {
        let error = parse("review".to_owned(), "just a body\n").expect_err("refused");
        assert!(error.contains("frontmatter"));
    }

    #[test]
    fn a_template_with_an_empty_body_is_refused() {
        let error =
            parse("review".to_owned(), "---\ndescription: d\n---\n\n").expect_err("refused");
        assert!(error.contains("non-empty body"));
    }

    #[test]
    fn an_unusable_name_is_refused_before_anything_else() {
        let error = parse("Not Valid".to_owned(), &template("x")).expect_err("refused");
        assert!(error.contains("not a usable template name"));
    }

    #[test]
    fn positional_arguments_expand() {
        let arguments = vec!["alpha".to_owned(), "beta".to_owned()];
        assert_eq!(expand("$1 and $2", &arguments), "alpha and beta");
        assert_eq!(expand("all: $@", &arguments), "all: alpha beta");
    }

    #[test]
    fn a_missing_positional_argument_is_left_verbatim() {
        // Visible beats silent: an author who passed too few arguments sees
        // `$2` in the output rather than a sentence with a hole in it.
        assert_eq!(expand("[$1][$2]", &["only".to_owned()]), "[only][$2]");
        // And the explicit form still means "empty when absent".
        assert_eq!(expand("[${2:-}]", &["only".to_owned()]), "[]");
    }

    #[test]
    fn a_default_applies_only_when_the_argument_is_absent() {
        assert_eq!(expand("${1:-fallback}", &[]), "fallback");
        assert_eq!(expand("${1:-fallback}", &["given".to_owned()]), "given");
    }

    #[test]
    fn text_that_is_not_a_substitution_survives_verbatim() {
        // A template may legitimately contain shell or currency text; only the
        // forms we document are touched.
        assert_eq!(expand("costs $5.00", &[]), "costs $5.00");
        assert_eq!(expand("$ alone", &[]), "$ alone");
        assert_eq!(expand("${notanumber}", &[]), "${notanumber}");
        assert_eq!(expand("$0 is not positional", &[]), "$0 is not positional");
    }

    #[test]
    fn arguments_split_on_whitespace_with_quotes_grouping() {
        assert_eq!(split_arguments("a b"), vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(
            split_arguments("\"two words\" third"),
            vec!["two words".to_owned(), "third".to_owned()]
        );
        assert!(split_arguments("   ").is_empty());
    }

    #[test]
    fn discovery_reads_a_project_directory_and_reports_a_bad_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join(".mjolnr/prompts");
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(directory.join("review.md"), template("Review $1.")).expect("write good");
        std::fs::write(directory.join("broken.md"), "no frontmatter").expect("write bad");

        let mut diagnostics = Vec::new();
        let catalog =
            PromptCatalog::discover(vec![(directory, SkillScope::Project)], &mut diagnostics);
        assert_eq!(catalog.templates().len(), 1);
        assert!(catalog.get("review").is_some());
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn an_earlier_root_wins_a_name_collision_and_the_loser_is_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let user = temp.path().join("user");
        std::fs::create_dir_all(&project).expect("mkdir project");
        std::fs::create_dir_all(&user).expect("mkdir user");
        std::fs::write(project.join("review.md"), template("project version"))
            .expect("write project");
        std::fs::write(user.join("review.md"), template("user version")).expect("write user");

        let mut diagnostics = Vec::new();
        let catalog = PromptCatalog::discover(
            vec![(project, SkillScope::Project), (user, SkillScope::User)],
            &mut diagnostics,
        );
        assert_eq!(catalog.templates().len(), 1);
        assert_eq!(
            catalog.get("review").expect("template").body,
            "project version"
        );
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].detail.contains("already defined"));
    }

    #[test]
    fn a_missing_directory_yields_an_empty_catalog() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut diagnostics = Vec::new();
        let catalog = PromptCatalog::discover(
            vec![(temp.path().join("absent"), SkillScope::Project)],
            &mut diagnostics,
        );
        assert!(catalog.is_empty());
        assert!(diagnostics.is_empty());
    }
}
