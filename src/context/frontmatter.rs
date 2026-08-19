//! Strict, bounded frontmatter parsing for the Agent Skills fields smed uses.

use std::collections::BTreeMap;

use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    #[serde(default)]
    metadata: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Split a document into its YAML frontmatter and body.
///
/// Shared by skills and prompt templates so both agree on exactly what counts
/// as frontmatter; `noun` only shapes the error text.
pub(super) fn split(contents: &str, noun: &str) -> Result<(String, String), String> {
    let (rest, delimiter) = if let Some(rest) = contents.strip_prefix("---\n") {
        (rest, "\n---\n")
    } else if let Some(rest) = contents.strip_prefix("---\r\n") {
        (rest, "\r\n---\r\n")
    } else {
        return Err(format!("{noun} must start with YAML frontmatter (`---`)"));
    };
    let Some((frontmatter, body)) = rest.split_once(delimiter) else {
        return Err(format!("{noun} frontmatter is not closed with `---`"));
    };
    Ok((frontmatter.to_owned(), body.to_owned()))
}

pub(super) fn parse(contents: &str, directory_name: &str) -> Result<ParsedSkill, String> {
    let (frontmatter, body) = split(contents, "SKILL.md")?;
    let frontmatter = frontmatter.as_str();
    let body = body.as_str();
    let fields = serde_yaml_ng::from_str::<Frontmatter>(frontmatter)
        .map_err(|error| format!("invalid YAML frontmatter: {error}"))?;

    let name = fields.name.trim().nfkc().collect::<String>();
    let directory_name = directory_name.nfkc().collect::<String>();
    let description = fields.description.trim();
    validate_name(&name, &directory_name)?;
    validate_text(description, "description", 1_024)?;
    if let Some(compatibility) = fields.compatibility.as_deref() {
        validate_text(compatibility.trim(), "compatibility", 500)?;
    }

    // These standard fields are parsed to validate their YAML types, but they
    // deliberately confer no runtime authority. In particular, allowed-tools
    // never bypasses smed's deterministic tool policy.
    let _ = (fields.license, fields.allowed_tools, fields.metadata);

    Ok(ParsedSkill {
        name,
        description: description.to_owned(),
        body: body.trim().to_owned(),
    })
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

fn validate_name(name: &str, directory_name: &str) -> Result<(), String> {
    validate_text(name, "name", 64)?;
    if name != name.to_lowercase() {
        return Err("skill name must be lowercase".to_owned());
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("skill name cannot start or end with a hyphen".to_owned());
    }
    if name.contains("--") {
        return Err("skill name cannot contain consecutive hyphens".to_owned());
    }
    if !name
        .chars()
        .all(|character| character.is_alphanumeric() || character == '-')
    {
        return Err(
            "skill name may contain only lowercase letters, numbers, and hyphens".to_owned(),
        );
    }
    if name != directory_name {
        return Err(format!(
            "skill name `{name}` does not match directory `{directory_name}`"
        ));
    }
    Ok(())
}
