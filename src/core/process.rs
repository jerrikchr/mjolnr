//! The rules every child process mjolnr spawns must obey.
//!
//! One responsibility: decide what a child process is allowed to inherit.
//! It lives in `core` because more than one implementer needs it — `tools`
//! spawns commands, `runtime::subagent` spawns git, and `repository` spawns
//! git — and a security rule with three copies is a security rule with three
//! chances to drift (AGENTS.md §3).

use std::ffi::{OsStr, OsString};

/// The environment a child process may inherit: this process's environment with
/// every provider credential removed.
///
/// Callers pair this with `Command::env_clear()`. Clearing and re-adding is
/// deliberate rather than removing individual variables: the default becomes
/// "inherit nothing", and each addition is a decision.
#[must_use]
pub fn sanitized_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| !is_provider_secret(name))
        .collect()
}

/// Provider credentials are named by convention, and the convention is the
/// contract: anything ending `_API_KEY` is a credential. A new provider adding
/// a key gets scrubbed without this function changing.
#[must_use]
pub fn is_provider_secret(name: &OsStr) -> bool {
    name.to_string_lossy()
        .to_ascii_uppercase()
        .ends_with("_API_KEY")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_are_removed_from_child_environments() {
        assert!(is_provider_secret(OsStr::new("OPENAI_API_KEY")));
        assert!(is_provider_secret(OsStr::new(
            "SOME_FUTURE_PROVIDER_API_KEY"
        )));
        assert!(is_provider_secret(OsStr::new("anthropic_api_key")));
    }

    #[test]
    fn ordinary_variables_a_child_needs_are_kept() {
        assert!(!is_provider_secret(OsStr::new("PATH")));
        assert!(!is_provider_secret(OsStr::new("HOME")));
        // Near-misses must not be scrubbed: over-scrubbing breaks children
        // silently, which is its own kind of dishonesty.
        assert!(!is_provider_secret(OsStr::new("API_KEY_PATH")));
    }

    #[test]
    fn the_sanitized_environment_never_contains_a_provider_key() {
        // SAFETY-free: `set_var` is safe on this toolchain's edition surface
        // only inside an unsafe block, so the assertion is made against the
        // filter itself rather than by mutating the process environment — a
        // mutation that would race every other test in this binary.
        let sanitized = sanitized_environment();
        assert!(
            sanitized.iter().all(|(name, _)| !is_provider_secret(name)),
            "a provider key survived the scrub"
        );
    }
}
