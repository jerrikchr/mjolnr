//! Agent-authored tool extensions: the declarative half.
//!
//! An extension is data, not code (ADR 0002): a name, a description, string
//! parameters, and one exact-argv command template with `${name}` placeholders.
//! This module parses and validates that data and nothing else. Discovery from
//! disk is [`context::extensions`](crate::context), and execution is
//! [`tools::extension`](crate::tools). The definition lives here, below both,
//! so the discoverer and the executor share one notion of what a valid
//! extension is — the same reason [`ToolDefinition`](crate::core::tool) lives
//! in `core`.
//!
//! The safety properties are structural, not enforced by a downstream gate:
//!
//! - The **program is fixed**. A placeholder is never allowed in `program`, so
//!   a loaded extension always runs the same executable; only its arguments
//!   vary per call. A preview can therefore always name what will run.
//! - Substitution is **whole-value into a single argv element**. `${path}`
//!   expands inside one argument and never splits into more, because the argv
//!   reaches `execvp` as written — there is no shell to re-split it (ADR 0002).
//! - Every parameter is **required and referenced**. A declared parameter that
//!   is never used, or a placeholder that names no parameter, is a definition
//!   error rather than a silent surprise at call time.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// The most parameters an extension may declare. Generous for an argv template;
/// a bound at all is what keeps a malformed file from generating an unbounded
/// schema.
const MAX_PARAMETERS: usize = 64;

/// The most argv elements an extension's command may have. Matches the
/// `run_command` tool's own `maxItems`, since the same argv reaches the same
/// spawn.
const MAX_ARGUMENTS: usize = 256;

/// One declared parameter of an extension. Always a string, because the argv it
/// substitutes into is strings (ADR 0002 "Consequences").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub name: String,
    pub description: String,
}

/// A parsed, validated extension: a named view onto one exact-argv command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDefinition {
    name: String,
    description: String,
    parameters: Vec<Parameter>,
    program: String,
    arguments: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtension {
    name: String,
    description: String,
    #[serde(default)]
    parameters: Vec<RawParameter>,
    run: RawRun,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParameter {
    name: String,
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRun {
    program: String,
    arguments: Vec<String>,
}

impl ExtensionDefinition {
    /// Parse and validate an extension file.
    ///
    /// `expected_name` is the file stem the definition must match, the same
    /// discipline skills apply between a `name` field and its directory: a file
    /// and the tool it defines cannot disagree about what the tool is called.
    ///
    /// # Errors
    /// A human-readable reason on any malformed or inconsistent field. The
    /// caller turns it into a typed diagnostic; nothing partially registers.
    pub fn parse(contents: &str, expected_name: &str) -> Result<Self, String> {
        let raw = serde_yaml_ng::from_str::<RawExtension>(contents)
            .map_err(|error| format!("invalid extension YAML: {error}"))?;

        let name = raw.name.trim().to_owned();
        validate_name(&name)?;
        if name != expected_name {
            return Err(format!(
                "extension name `{name}` does not match file `{expected_name}`"
            ));
        }

        let description = raw.description.trim().to_owned();
        validate_text(&description, "description", 1_024)?;

        if raw.parameters.len() > MAX_PARAMETERS {
            return Err(format!(
                "an extension may declare at most {MAX_PARAMETERS} parameters"
            ));
        }
        let mut parameters = Vec::with_capacity(raw.parameters.len());
        let mut seen = BTreeSet::new();
        for parameter in raw.parameters {
            let parameter_name = parameter.name.trim().to_owned();
            validate_parameter_name(&parameter_name)?;
            if !seen.insert(parameter_name.clone()) {
                return Err(format!("duplicate parameter `{parameter_name}`"));
            }
            let parameter_description = parameter.description.trim().to_owned();
            validate_text(
                &parameter_description,
                &format!("parameter `{parameter_name}` description"),
                1_024,
            )?;
            parameters.push(Parameter {
                name: parameter_name,
                description: parameter_description,
            });
        }

        let program = raw.run.program.trim().to_owned();
        if program.is_empty() {
            return Err("`run.program` must not be empty".to_owned());
        }
        // The program is fixed by the author; parameterising it would let a call
        // choose what executable runs, and a preview could no longer name it.
        if program.contains("${") {
            return Err("`run.program` may not contain a `${...}` placeholder; only arguments are parameterised".to_owned());
        }

        if raw.run.arguments.len() > MAX_ARGUMENTS {
            return Err(format!(
                "an extension command may have at most {MAX_ARGUMENTS} arguments"
            ));
        }

        // Every placeholder must name a declared parameter, and every declared
        // parameter must be used. Both directions, so a definition cannot carry
        // a dead parameter or a dangling reference.
        let declared: BTreeSet<&str> = parameters.iter().map(|p| p.name.as_str()).collect();
        let mut referenced = BTreeSet::new();
        for argument in &raw.run.arguments {
            for placeholder in scan_placeholders(argument)? {
                if !declared.contains(placeholder.as_str()) {
                    return Err(format!(
                        "argument references undeclared parameter `{placeholder}`"
                    ));
                }
                referenced.insert(placeholder);
            }
        }
        for parameter in &parameters {
            if !referenced.contains(&parameter.name) {
                return Err(format!(
                    "parameter `{}` is declared but never used in the command",
                    parameter.name
                ));
            }
        }

        Ok(Self {
            name,
            description,
            parameters,
            program,
            arguments: raw.run.arguments,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    #[must_use]
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    /// The JSON Schema for this extension's arguments.
    ///
    /// Every parameter is a required string and no others are permitted, so the
    /// arguments validated immediately before a call are exactly the values the
    /// placeholders need — substitution is total by the time it runs.
    #[must_use]
    pub fn schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        for parameter in &self.parameters {
            properties.insert(
                parameter.name.clone(),
                serde_json::json!({
                    "type": "string",
                    "description": parameter.description,
                }),
            );
        }
        let required: Vec<serde_json::Value> = self
            .parameters
            .iter()
            .map(|parameter| serde_json::Value::from(parameter.name.clone()))
            .collect();
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    /// Substitute validated argument values into the argv template.
    ///
    /// # Errors
    /// If a placeholder names a value that is absent. After schema validation
    /// this cannot happen — every parameter is required — but the check is kept
    /// so a caller that skipped validation fails loudly rather than emitting an
    /// argv with a literal `${name}` in it.
    pub fn resolve(&self, values: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
        self.arguments
            .iter()
            .map(|argument| substitute(argument, values))
            .collect()
    }
}

fn substitute(template: &str, values: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unterminated placeholder in `{template}`"))?;
        let name = &after[..end];
        let value = values
            .get(name)
            .ok_or_else(|| format!("no value for parameter `{name}`"))?;
        out.push_str(value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The parameter names referenced by one template string.
fn scan_placeholders(text: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unterminated `${{` in `{text}`"))?;
        let name = &after[..end];
        if name.is_empty() {
            return Err(format!("empty placeholder `${{}}` in `{text}`"));
        }
        names.push(name.to_owned());
        rest = &after[end + 1..];
    }
    Ok(names)
}

fn validate_text(value: &str, field: &str, maximum: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length == 0 || length > maximum {
        return Err(format!(
            "`{field}` must contain 1-{maximum} characters; found {length}"
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    validate_text(name, "name", 64)?;
    if name != name.to_lowercase() {
        return Err("extension name must be lowercase".to_owned());
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("extension name cannot start or end with a hyphen".to_owned());
    }
    if name.contains("--") {
        return Err("extension name cannot contain consecutive hyphens".to_owned());
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err(
            "extension name may contain only lowercase ASCII letters, digits, and hyphens"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_parameter_name(name: &str) -> Result<(), String> {
    validate_text(name, "parameter name", 64)?;
    let mut characters = name.chars();
    if !characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
    {
        return Err(format!(
            "parameter name `{name}` must start with a lowercase ASCII letter"
        ));
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '-'
            || character == '_'
    }) {
        return Err(format!(
            "parameter name `{name}` may contain only lowercase ASCII letters, digits, hyphens, and underscores"
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    const VALID: &str = "name: count-lines
description: Count the lines in a file at the workspace root.
parameters:
  - name: path
    description: File to count, relative to the workspace root.
run:
  program: wc
  arguments: [\"-l\", \"${path}\"]
";

    #[test]
    fn a_well_formed_extension_parses() {
        let definition = ExtensionDefinition::parse(VALID, "count-lines").expect("valid extension");
        assert_eq!(definition.name(), "count-lines");
        assert_eq!(definition.program(), "wc");
        assert_eq!(definition.parameters().len(), 1);
    }

    #[test]
    fn the_name_must_match_the_file_stem() {
        let error = ExtensionDefinition::parse(VALID, "something-else")
            .expect_err("mismatched name must be rejected");
        assert!(error.contains("does not match file"), "{error}");
    }

    #[test]
    fn substitution_fills_the_argv_whole_value() {
        let definition = ExtensionDefinition::parse(VALID, "count-lines").expect("valid");
        let resolved = definition
            .resolve(&values(&[("path", "src/lib.rs")]))
            .expect("resolve");
        assert_eq!(resolved, vec!["-l".to_owned(), "src/lib.rs".to_owned()]);
    }

    #[test]
    fn substitution_stays_within_one_argv_element_even_with_spaces() {
        // A value containing a space does not become two arguments: there is no
        // shell to re-split it (ADR 0002).
        let definition = ExtensionDefinition::parse(VALID, "count-lines").expect("valid");
        let resolved = definition
            .resolve(&values(&[("path", "a file.txt")]))
            .expect("resolve");
        assert_eq!(resolved, vec!["-l".to_owned(), "a file.txt".to_owned()]);
    }

    #[test]
    fn inline_substitution_within_a_token_is_supported() {
        let source = "name: grep-here
description: Search for a pattern.
parameters:
  - name: pattern
    description: Regular expression.
run:
  program: rg
  arguments: [\"--regexp=${pattern}\"]
";
        let definition = ExtensionDefinition::parse(source, "grep-here").expect("valid");
        let resolved = definition
            .resolve(&values(&[("pattern", "foo|bar")]))
            .expect("resolve");
        assert_eq!(resolved, vec!["--regexp=foo|bar".to_owned()]);
    }

    #[test]
    fn a_placeholder_in_the_program_is_refused() {
        let source = "name: dynamic
description: Run whatever.
parameters:
  - name: cmd
    description: The program.
run:
  program: ${cmd}
  arguments: [\"--version\"]
";
        let error = ExtensionDefinition::parse(source, "dynamic")
            .expect_err("a parameterised program must be refused");
        assert!(error.contains("program"), "{error}");
    }

    #[test]
    fn an_undeclared_placeholder_is_refused() {
        let source = "name: broken
description: Uses an undeclared parameter.
parameters:
  - name: path
    description: A path.
run:
  program: cat
  arguments: [\"${path}\", \"${other}\"]
";
        let error = ExtensionDefinition::parse(source, "broken")
            .expect_err("an undeclared placeholder must be refused");
        assert!(error.contains("undeclared parameter `other`"), "{error}");
    }

    #[test]
    fn a_declared_but_unused_parameter_is_refused() {
        let source = "name: waste
description: Declares a parameter it never uses.
parameters:
  - name: path
    description: A path.
  - name: unused
    description: Never referenced.
run:
  program: cat
  arguments: [\"${path}\"]
";
        let error = ExtensionDefinition::parse(source, "waste")
            .expect_err("an unused parameter must be refused");
        assert!(error.contains("never used"), "{error}");
    }

    #[test]
    fn an_unterminated_placeholder_is_refused() {
        let source = "name: unterminated
description: Has an open placeholder.
parameters:
  - name: path
    description: A path.
run:
  program: cat
  arguments: [\"${path\"]
";
        let error = ExtensionDefinition::parse(source, "unterminated")
            .expect_err("an unterminated placeholder must be refused");
        assert!(error.contains("unterminated"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let source = "name: extra
description: Carries an unknown field.
tier: read
parameters: []
run:
  program: true
  arguments: []
";
        let error = ExtensionDefinition::parse(source, "extra")
            .expect_err("an extension may not declare its own tier");
        assert!(
            error.contains("unknown field") || error.contains("tier"),
            "{error}"
        );
    }

    #[test]
    fn an_uppercase_name_is_refused() {
        let source = "name: LoudTool
description: Shouts.
parameters: []
run:
  program: echo
  arguments: [\"hi\"]
";
        let error = ExtensionDefinition::parse(source, "LoudTool")
            .expect_err("an uppercase name must be refused");
        assert!(error.contains("lowercase"), "{error}");
    }

    #[test]
    fn the_generated_schema_is_a_valid_local_schema_requiring_every_parameter() {
        let definition = ExtensionDefinition::parse(VALID, "count-lines").expect("valid");
        let schema = definition.schema();
        assert!(jsonschema::meta::is_valid(&schema));
        assert!(!schema.to_string().contains("$ref"));
        assert_eq!(schema["required"], serde_json::json!(["path"]));
        assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn a_parameterless_extension_is_valid() {
        let source = "name: status
description: Show the git status.
run:
  program: git
  arguments: [\"status\", \"--short\"]
";
        let definition = ExtensionDefinition::parse(source, "status").expect("valid");
        assert!(definition.parameters().is_empty());
        assert_eq!(
            definition.resolve(&BTreeMap::new()).expect("resolve"),
            vec!["status".to_owned(), "--short".to_owned()]
        );
    }
}
