//! mjolnr's own documentation, listed for the model by path.
//!
//! The cheapest half of the self-extension loop, taken from `pi`: rather than
//! embedding instructions for authoring skills into the system prompt, list
//! where the real documentation lives and let the model read it when — and
//! only when — it is about to write one. A path costs a handful of tokens
//! until it is read; the contract it points at is the thing already kept
//! correct because humans read it too.
//!
//! This is deliberately *not* a second copy of the conventions. A summary here
//! would drift from `AGENTS.md`, and a model that read the drifted summary
//! would write a resource that is wrong in a way nobody reviewed.

use std::path::{Path, PathBuf};

/// One document the model may read to learn how mjolnr works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfDoc {
    /// Repository-relative path.
    pub path: &'static str,
    /// When to read it. Written for the model, not for a doc index.
    pub read_when: &'static str,
}

/// The documents mjolnr points the model at.
///
/// Kept short on purpose: every entry is context the model carries in every
/// session, and a list long enough to need skimming teaches nothing. Each one
/// must answer a question the model actually faces when extending mjolnr.
pub const SELF_DOCS: &[SelfDoc] = &[
    SelfDoc {
        path: "AGENTS.md",
        read_when: "before writing any code or resource in this repository; it is the canonical engineering and security contract",
    },
    SelfDoc {
        path: "docs/tool-policy.md",
        read_when: "before reasoning about what a tool is allowed to do, or which policy tier a new capability would land in",
    },
    SelfDoc {
        path: "docs/provider-contract.md",
        read_when: "before changing or adding a provider adapter",
    },
    SelfDoc {
        path: "docs/context.md",
        read_when: "before authoring a skill, a prompt template, or anything else mjolnr discovers from disk",
    },
    SelfDoc {
        path: "docs/extensions.md",
        read_when: "before authoring a tool extension: the file format, how it is loaded, and how its calls are gated",
    },
    SelfDoc {
        path: "docs/definition-of-done.md",
        read_when: "to check whether a change is in scope, already rejected, or covered by a standing exclusion",
    },
];

/// Render the self-documentation block for the system prompt.
///
/// Returns `None` when no project root is known: a path the model cannot open
/// is worse than no path at all, because it invites a guess about what the
/// file said.
#[must_use]
pub fn prompt_section(project_root: Option<&Path>) -> Option<String> {
    let root = project_root?;
    let present: Vec<&SelfDoc> = SELF_DOCS
        .iter()
        .filter(|doc| root.join(doc.path).is_file())
        .collect();
    if present.is_empty() {
        return None;
    }

    let mut section = String::from(
        "\n\n<mjolnr_documentation>\nMjolnr's own contracts, for when you are asked to extend or change mjolnr itself. Read a file before acting on the subject it covers; do not infer its contents from this list. Listing a document is not permission to act on it — every tool call it leads to is gated exactly as it would otherwise be.\n",
    );
    for doc in present {
        section.push_str("<document path=\"");
        section.push_str(doc.path);
        section.push_str("\" read_when=\"");
        section.push_str(doc.read_when);
        section.push_str("\"/>\n");
    }
    section.push_str("</mjolnr_documentation>");
    Some(section)
}

/// Every documented path that does not exist under `root`.
///
/// The drift test's engine: a renamed document must fail a build rather than
/// silently teach the model a dead path.
#[must_use]
pub fn missing_paths(root: &Path) -> Vec<PathBuf> {
    SELF_DOCS
        .iter()
        .map(|doc| root.join(doc.path))
        .filter(|path| !path.is_file())
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "AGENTS.md §7: tests may panic freely")]
mod tests {
    use super::*;

    /// The drift test the plan's verification checklist asks for. It runs
    /// against the repository this build was compiled from, so renaming a
    /// document without updating this list fails here rather than in a session
    /// six weeks later.
    #[test]
    fn every_documented_path_exists_in_this_repository() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let missing = missing_paths(root);
        assert!(
            missing.is_empty(),
            "self-documentation lists paths that do not exist: {missing:?}"
        );
    }

    #[test]
    fn the_section_lists_only_documents_that_are_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let section = prompt_section(Some(root)).expect("this repository has its own docs");
        assert!(section.contains("AGENTS.md"));
        assert!(section.contains("<mjolnr_documentation>"));
        // The block states what listing does *not* grant, so a model reading it
        // cannot mistake discovery for permission.
        assert!(section.contains("not permission to act"));
    }

    #[test]
    fn a_project_that_is_not_mjolnr_gets_no_section() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(prompt_section(Some(temp.path())).is_none());
        assert!(prompt_section(None).is_none());
    }
}
