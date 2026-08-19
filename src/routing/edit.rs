//! In-place edits to a single route file's diffable content.
//!
//! The `/config` surface is a *lens*, not a store: changing a setting writes the
//! same `.mjolnr/routes/<name>.yaml` a hand-editor would. This module owns the
//! one edit that surface needs — binding (or clearing) a route's persona — as a
//! pure string transform so the write is testable without a filesystem and the
//! result is guaranteed to round-trip through [`super::definition::parse_route`].
//!
//! The transform is deliberately line-based rather than a serde re-serialise: a
//! route file carries comments the scaffold wrote, and re-serialising would
//! discard them. A hand-editor would touch only the `persona:` line, so that is
//! all this touches.

/// Return `contents` with its top-level `persona:` binding set to `persona`
/// (or removed when `None`), preserving hops, roles, comments, and trailing
/// newline. Any existing top-level `persona:` line is replaced, so the result
/// never carries two.
#[must_use]
pub fn set_route_persona(contents: &str, persona: Option<&str>) -> String {
    let mut lines: Vec<&str> = contents
        .lines()
        .filter(|line| !is_top_level_persona(line))
        .collect();
    // Drop trailing blank lines so an appended key sits against the content,
    // exactly where a person would type it, not after a gap.
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let mut owned: Vec<String> = lines.into_iter().map(str::to_owned).collect();
    if let Some(name) = persona {
        owned.push(format!("persona: \"{name}\""));
    }
    let mut out = owned.join("\n");
    if contents.ends_with('\n') || persona.is_some() {
        out.push('\n');
    }
    out
}

/// A top-level `persona:` key — column zero, not a comment. An indented
/// `persona:` (none exist today, but a nested key could) or a `# persona:`
/// comment is left untouched.
fn is_top_level_persona(line: &str) -> bool {
    line.starts_with("persona:") && !line.trim_start().starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::definition::parse_route;

    const ROUTE: &str = "# a comment the scaffold wrote\n\
        hops:\n  - provider: \"anthropic\"\n    model: \"claude-opus-4-8\"\n\
        roles: [\"default\"]\n";

    #[test]
    fn binding_a_persona_round_trips_through_the_loader() {
        let edited = set_route_persona(ROUTE, Some("mentor"));
        let route = parse_route("r".to_owned(), &edited).expect("loads without diagnostics");
        assert_eq!(route.persona.as_deref(), Some("mentor"));
        // The hand edit changed nothing else.
        assert_eq!(route.roles, vec!["default".to_owned()]);
        assert_eq!(route.hops.len(), 1);
        assert!(edited.contains("# a comment the scaffold wrote"));
    }

    #[test]
    fn clearing_a_persona_removes_the_line_and_round_trips() {
        let bound = set_route_persona(ROUTE, Some("mentor"));
        let cleared = set_route_persona(&bound, None);
        let route = parse_route("r".to_owned(), &cleared).expect("loads");
        assert_eq!(route.persona, None);
        assert!(!cleared.contains("persona:"));
    }

    #[test]
    fn rebinding_replaces_rather_than_duplicates() {
        let once = set_route_persona(ROUTE, Some("mentor"));
        let twice = set_route_persona(&once, Some("critic"));
        assert_eq!(twice.matches("persona:").count(), 1);
        let route = parse_route("r".to_owned(), &twice).expect("loads");
        assert_eq!(route.persona.as_deref(), Some("critic"));
    }

    #[test]
    fn a_comment_mentioning_persona_is_not_mistaken_for_the_key() {
        let with_comment = format!("# persona: none yet\n{ROUTE}");
        let edited = set_route_persona(&with_comment, Some("mentor"));
        assert!(edited.contains("# persona: none yet"));
        assert_eq!(edited.matches("persona: \"mentor\"").count(), 1);
    }
}
