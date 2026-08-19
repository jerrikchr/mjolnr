//! The one place a `git` process is created (Phase D5).
//!
//! Every invocation is an explicit argument vector with a scrubbed environment.
//! No caller may build a command string, so there is no path by which UI or
//! model text becomes shell syntax (AGENTS.md §3).

use std::path::Path;
use std::process::{Command, Stdio};

use super::error::RepositoryError;

/// Largest stdout smed keeps from one invocation. `git status` on a very large
/// tree is the realistic producer; the cap turns that into bounded truncation
/// rather than unbounded memory (AGENTS.md §5).
pub(super) const MAX_GIT_STDOUT_BYTES: usize = 1 << 20;

/// Largest stderr smed keeps. Failure text is carried to the human verbatim,
/// so it is bounded too.
const MAX_GIT_STDERR_BYTES: usize = 8 << 10;

/// A completed invocation. Deliberately not `Result`: a non-zero exit is
/// ordinary information here, and classifying it is the caller's job because
/// only the caller knows what the operation was meant to do.
pub(super) struct GitOutput {
    pub(super) success: bool,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

/// Run `git` with exact argv in `work_dir`.
///
/// Blocking on purpose: the async boundary is one `spawn_blocking` in the
/// runtime, not a per-call `tokio::process` (AGENTS.md §4). Keeping the module
/// synchronous is also what lets it be tested without a reactor.
pub(super) fn run(
    work_dir: &Path,
    operation: &'static str,
    arguments: &[&str],
) -> Result<GitOutput, RepositoryError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A git subprocess has no business inheriting provider credentials,
        // and `GIT_*` variables from smed's own environment would silently
        // redirect the operation. Clear, then add back only what is sanitized.
        .env_clear()
        .envs(crate::core::process::sanitized_environment())
        .output()
        .map_err(|error| RepositoryError::CommandFailed {
            operation,
            detail: format!("cannot run git: {error}"),
        })?;

    Ok(GitOutput {
        success: output.status.success(),
        stdout: bounded(&output.stdout, MAX_GIT_STDOUT_BYTES),
        stderr: bounded(&output.stderr, MAX_GIT_STDERR_BYTES),
    })
}

/// A completed invocation whose stdout is kept as bytes.
///
/// Separate from [`GitOutput`] because one caller must not have its stdout
/// decoded for it: the Phase D3 change producer has to know that a file's diff
/// was not valid UTF-8, and `from_utf8_lossy` destroys that fact by answering
/// with U+FFFD. Decoding is that caller's decision, made per file.
pub(super) struct RawGitOutput {
    pub(super) success: bool,
    pub(super) stdout: Vec<u8>,
    /// True when stdout hit [`MAX_GIT_STDOUT_BYTES`], so what follows is part
    /// of git's answer rather than all of it.
    pub(super) truncated: bool,
    pub(super) stderr: String,
}

/// Run `git` with exact argv, keeping stdout as bytes.
///
/// Same process contract as [`run`] — argv only, scrubbed environment — and it
/// truncates at the same bound. It cuts on a byte boundary rather than a
/// character boundary, which is sound here only because the sole caller splits
/// the output into per-file sections and decodes each one independently: a cut
/// mid-character makes that final section undecodable, which is exactly what
/// the caller is built to report.
pub(super) fn run_raw(
    work_dir: &Path,
    operation: &'static str,
    arguments: &[&str],
) -> Result<RawGitOutput, RepositoryError> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(crate::core::process::sanitized_environment())
        .output()
        .map_err(|error| RepositoryError::CommandFailed {
            operation,
            detail: format!("cannot run git: {error}"),
        })?;

    let truncated = output.stdout.len() > MAX_GIT_STDOUT_BYTES;
    let mut stdout = output.stdout;
    stdout.truncate(MAX_GIT_STDOUT_BYTES);

    Ok(RawGitOutput {
        success: output.status.success(),
        stdout,
        truncated,
        stderr: bounded(&output.stderr, MAX_GIT_STDERR_BYTES),
    })
}

/// Run `git` and require success, mapping a failure to `CommandFailed`. Use
/// only where no richer classification is possible or useful.
pub(super) fn run_checked(
    work_dir: &Path,
    operation: &'static str,
    arguments: &[&str],
) -> Result<String, RepositoryError> {
    let output = run(work_dir, operation, arguments)?;
    if output.success {
        Ok(output.stdout)
    } else {
        Err(RepositoryError::CommandFailed {
            operation,
            detail: output.stderr,
        })
    }
}

fn bounded(raw: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(raw);
    if text.len() <= limit {
        return text.into_owned();
    }
    // Truncate on a character boundary and say so, rather than silently
    // shortening (AGENTS.md §5).
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = text[..end].to_owned();
    truncated.push_str("\n[truncated by smed: git output exceeded its bound]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_output_is_truncated_with_an_explicit_marker() {
        let raw = "x".repeat(MAX_GIT_STDOUT_BYTES + 10).into_bytes();
        let bounded = bounded(&raw, MAX_GIT_STDOUT_BYTES);
        assert!(bounded.contains("[truncated by smed"));
        assert!(bounded.len() < MAX_GIT_STDOUT_BYTES + 100);
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // A limit landing mid-character must round down, not produce U+FFFD.
        let raw = "é".repeat(10).into_bytes();
        let bounded = bounded(&raw, 5);
        assert!(!bounded.contains('\u{fffd}'));
        assert!(bounded.starts_with("éé"));
    }

    #[test]
    fn output_within_the_bound_is_returned_verbatim() {
        assert_eq!(
            bounded(b"on branch main", MAX_GIT_STDOUT_BYTES),
            "on branch main"
        );
    }
}
