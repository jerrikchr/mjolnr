//! Tier 1: explicit workspace rules and user profile
//! (master implementation plan, §2.1).
//!
//! One responsibility: read the diffable Markdown a human maintains under
//! `.mjolnr/rules/*.md` and `.mjolnr/USER.md` into a **frozen snapshot**, once,
//! for injection at session start.
//!
//! Three properties are load-bearing:
//!
//! 1. **Frozen means frozen.** The snapshot is loaded once and never
//!    re-read mid-session. Rules changing under a running session would
//!    churn the prompt cache — the exact cost this tier exists to avoid —
//!    and would let a mid-session write silently become instruction. A rule
//!    edit takes effect at the next session start, via the ordinary `Write`
//!    gate like any other file.
//! 2. **Limits are refusals, not truncation.** A file over its limit is
//!    reported and skipped, never silently clipped: a truncated rule says
//!    something its author did not say. The limit is the consolidation
//!    mechanism.
//! 3. **Containment is rechecked immediately before every read** via
//!    [`policy::paths`], the codebase's one answer to that question — the
//!    same deliberate exception `workspace_files` carries.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::memory::error::MemoryError;
use crate::policy::paths;

/// Longest single rule file, in characters.
pub const MAX_RULE_FILE_CHARS: usize = 16_384;

/// Longest user profile, in characters.
pub const MAX_USER_PROFILE_CHARS: usize = 8_192;

/// Most rule files. Bounded so discovery cannot walk an unbounded directory.
pub const MAX_RULE_FILES: usize = 32;

/// Most bytes read from any one file before the character count.
///
/// A size probe before reading keeps a multi-gigabyte file at `.mjolnr/rules/`
/// from being slurped to be counted. The probe refuses; the limit refuses
/// again after decode if the file grew between the two.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// One frozen Markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleDocument {
    /// The file stem (`coding-standards`), or `USER` for the profile.
    pub name: String,
    /// SHA-256 of the exact bytes read, so a session can state which version
    /// of a rule it ran under.
    pub sha256: String,
    /// Character count, already checked against the limit.
    pub chars: usize,
    /// The content, verbatim.
    pub content: String,
}

/// The Tier 1 snapshot: everything the session will ever see of the rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesSnapshot {
    pub user_profile: Option<RuleDocument>,
    /// Ordered by name for reproducibility: two loads of the same directory
    /// must produce the same snapshot, or "which rules was this session
    /// under" stops being answerable by diffing.
    pub rules: Vec<RuleDocument>,
}

impl RulesSnapshot {
    /// Load the snapshot from a workspace root.
    ///
    /// Missing `.mjolnr/rules/` and missing `.mjolnr/USER.md` are the normal
    /// state of a workspace that has opted into nothing — the result is an
    /// empty snapshot, not an error.
    pub fn load(workspace_root: &Path) -> Result<Self, MemoryError> {
        let canonical_root =
            paths::canonical_root(workspace_root).map_err(|refusal| MemoryError::Unavailable {
                detail: format!("workspace root: {}", refusal.detail),
            })?;

        let user_profile = read_profile(&canonical_root)?;
        let rules = read_rules_dir(&canonical_root)?;

        Ok(Self {
            user_profile,
            rules,
        })
    }

    /// Total characters across all documents, for the caller's budget.
    #[must_use]
    pub fn total_chars(&self) -> usize {
        self.user_profile
            .as_ref()
            .map_or(0, |document| document.chars)
            + self
                .rules
                .iter()
                .map(|document| document.chars)
                .sum::<usize>()
    }

    /// Formats the Tier 1 snapshot into a system prompt section, or `None` if empty.
    #[must_use]
    pub fn prompt_section(&self) -> Option<String> {
        use std::fmt::Write as _;

        if self.user_profile.is_none() && self.rules.is_empty() {
            return None;
        }
        let mut section = String::from("## Workspace Rules & User Profile (Frozen Snapshot)\n");
        if let Some(profile) = &self.user_profile {
            section.push_str("\n### User Profile (`.mjolnr/USER.md`)\n");
            section.push_str(&profile.content);
            section.push('\n');
        }
        for rule in &self.rules {
            let hash_prefix = if rule.sha256.len() >= 8 {
                &rule.sha256[..8]
            } else {
                &rule.sha256
            };
            let _ = write!(
                section,
                "\n### Rule: {} (`sha256:{}`)\n",
                rule.name, hash_prefix
            );
            section.push_str(&rule.content);
            section.push('\n');
        }
        Some(section)
    }
}

/// `.mjolnr/USER.md`, or `None` when the workspace declares no profile.
fn read_profile(root: &Path) -> Result<Option<RuleDocument>, MemoryError> {
    let config_dir = crate::core::paths::resolve_workspace_config_dir(root);
    let path = config_dir.join("USER.md");
    if !path.is_file() {
        return Ok(None);
    }
    let contained = paths::existing(root, &path).map_err(|_| MemoryError::PathEscape {
        path: path.display().to_string(),
    })?;
    read_document(&contained, "USER", MAX_USER_PROFILE_CHARS).map(Some)
}

/// `.mjolnr/rules/*.md`, ordered by name, bounded by [`MAX_RULE_FILES`].
fn read_rules_dir(root: &Path) -> Result<Vec<RuleDocument>, MemoryError> {
    let config_dir = crate::core::paths::resolve_workspace_config_dir(root);
    let dir = config_dir.join("rules");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|error| MemoryError::Execution {
            detail: format!("read {}: {error}", dir.display()),
        })?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        })
        .collect();
    candidates.sort();

    let mut documents = Vec::new();
    for path in candidates {
        if documents.len() == MAX_RULE_FILES {
            return Err(MemoryError::RuleLimitExceeded {
                path: dir.display().to_string(),
                actual: MAX_RULE_FILES + 1,
                limit: MAX_RULE_FILES,
            });
        }
        // Containment is rechecked here, immediately before the read, per file
        // — the gap between one check and its use is the vulnerability.
        let contained = paths::existing(root, &path).map_err(|_| MemoryError::PathEscape {
            path: path.display().to_string(),
        })?;
        let name = contained
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default()
            .to_owned();
        documents.push(read_document(&contained, &name, MAX_RULE_FILE_CHARS)?);
    }
    Ok(documents)
}

/// Read one contained file into a [`RuleDocument`], refusing over-limit files.
fn read_document(path: &Path, name: &str, limit: usize) -> Result<RuleDocument, MemoryError> {
    let metadata = std::fs::metadata(path).map_err(|error| MemoryError::Execution {
        detail: format!("stat {}: {error}", path.display()),
    })?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(MemoryError::RuleLimitExceeded {
            path: path.display().to_string(),
            actual: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            limit,
        });
    }

    let bytes = std::fs::read(path).map_err(|error| MemoryError::Execution {
        detail: format!("read {}: {error}", path.display()),
    })?;
    let content = String::from_utf8(bytes).map_err(|error| MemoryError::Execution {
        detail: format!("decode {}: {error}", path.display()),
    })?;

    let chars = content.chars().count();
    if chars > limit {
        return Err(MemoryError::RuleLimitExceeded {
            path: path.display().to_string(),
            actual: chars,
            limit,
        });
    }

    let digest = Sha256::digest(content.as_bytes());
    let sha256 = digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    Ok(RuleDocument {
        name: name.to_owned(),
        sha256,
        chars,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::ReasonCode;

    fn workspace_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (relative, content) in files {
            let path = dir.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    #[test]
    fn a_workspace_with_nothing_declares_an_empty_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = RulesSnapshot::load(dir.path()).unwrap();
        assert!(snapshot.user_profile.is_none());
        assert!(snapshot.rules.is_empty());
        assert_eq!(snapshot.total_chars(), 0);
    }

    #[test]
    fn rules_and_profile_load_verbatim_with_hashes() {
        let dir = workspace_with(&[
            (".mjolnr/rules/coding-standards.md", "Use Result<T, E>.\n"),
            (".mjolnr/rules/a-conventions.md", "Run cargo test.\n"),
            (".smd-typo", "ignored"),
            (".mjolnr/rules/notes.txt", "not markdown"),
            (".mjolnr/USER.md", "Terse. Rust first.\n"),
        ]);
        let snapshot = RulesSnapshot::load(dir.path()).unwrap();

        // Ordered by name, not discovery order.
        let names: Vec<&str> = snapshot
            .rules
            .iter()
            .map(|rule| rule.name.as_str())
            .collect();
        assert_eq!(names, ["a-conventions", "coding-standards"]);
        let coding_standards = snapshot
            .rules
            .get(1)
            .expect("the second rule by name order");
        assert_eq!(coding_standards.content, "Use Result<T, E>.\n");
        assert_eq!(coding_standards.chars, 18);
        assert_eq!(coding_standards.sha256.len(), 64);

        let profile = snapshot.user_profile.expect("profile loaded");
        assert_eq!(profile.name, "USER");
        assert_eq!(profile.content, "Terse. Rust first.\n");
    }

    #[test]
    fn an_over_limit_rule_is_refused_not_truncated() {
        let oversized = "x".repeat(MAX_RULE_FILE_CHARS + 1);
        let dir = workspace_with(&[(".mjolnr/rules/big.md", oversized.as_str())]);
        let error = RulesSnapshot::load(dir.path()).unwrap_err();
        assert_eq!(
            error.reason_code(),
            ReasonCode::OutputTruncated,
            "the limit is the consolidation mechanism: refuse, never clip"
        );
    }

    #[test]
    fn an_over_limit_profile_is_refused() {
        let oversized = "x".repeat(MAX_USER_PROFILE_CHARS + 1);
        let dir = workspace_with(&[(".mjolnr/USER.md", oversized.as_str())]);
        assert!(RulesSnapshot::load(dir.path()).is_err());
    }

    #[test]
    fn a_symlink_escape_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("evil.md"), "injected").unwrap();
        let dir = workspace_with(&[(
            ".mjolnr/rules",
            "", // placeholder so create_dir_all makes the dir; real files next
        )]);
        std::fs::remove_file(dir.path().join(".mjolnr/rules")).unwrap();
        std::fs::create_dir_all(dir.path().join(".mjolnr/rules")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("evil.md"),
            dir.path().join(".mjolnr/rules/evil.md"),
        )
        .unwrap();

        let error = RulesSnapshot::load(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), ReasonCode::PathOutsideWorkspace);
    }

    #[test]
    fn loading_twice_produces_identical_snapshots() {
        let dir = workspace_with(&[
            (".mjolnr/rules/a.md", "one"),
            (".mjolnr/rules/b.md", "two"),
            (".mjolnr/USER.md", "profile"),
        ]);
        let first = RulesSnapshot::load(dir.path()).unwrap();
        let second = RulesSnapshot::load(dir.path()).unwrap();
        assert_eq!(
            first, second,
            "snapshot order and content must be reproducible"
        );
    }
}
