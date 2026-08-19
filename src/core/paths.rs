//! Workspace configuration directory resolution and migration helpers.
//!
//! Compat shim (ADR-0018): on read, check for `.mjolnr/` first and fall back to
//! `.smed/`. On first write, migrate content from `.smed/` to `.mjolnr/` but
//! never delete `.smed/`.

use std::path::{Path, PathBuf};

/// The canonical workspace config directory name for new projects.
pub const WORKSPACE_CONFIG_DIR: &str = ".mjolnr";

/// Legacy workspace config directory name for the backward-compatibility window.
pub const LEGACY_WORKSPACE_CONFIG_DIR: &str = ".smed";

/// Resolve the workspace config directory, preferring `.mjolnr/` with `.smed/` fallback.
///
/// Returns the canonical `.mjolnr/` path when `.mjolnr/` exists or `.smed/` does
/// not exist. If `.smed/` exists and `.mjolnr/` does not, returns `.smed/`.
#[must_use]
pub fn resolve_workspace_config_dir(project_root: &Path) -> PathBuf {
    let mjolnr = project_root.join(WORKSPACE_CONFIG_DIR);
    let smed = project_root.join(LEGACY_WORKSPACE_CONFIG_DIR);
    if mjolnr.exists() || !smed.exists() {
        mjolnr
    } else {
        smed
    }
}

/// Migrate config from `.smed/` to `.mjolnr/` on first write.
///
/// Creates `.mjolnr/` and copies all contents from `.smed/`. Never deletes
/// `.smed/` — the compat window must be explicit and advertised.
///
/// # Errors
/// When the directory cannot be created or files cannot be copied.
pub fn migrate_config_on_write(project_root: &Path) -> Result<(), std::io::Error> {
    let mjolnr = project_root.join(WORKSPACE_CONFIG_DIR);
    let smed = project_root.join(LEGACY_WORKSPACE_CONFIG_DIR);
    if mjolnr.exists() || !smed.exists() {
        return Ok(());
    }
    copy_dir_all(&smed, &mjolnr)
}

/// The canonical user-scoped application namespace for the platform config and
/// data directories.
pub const APPLICATION: &str = "mjolnr";

/// Legacy user-scoped application namespace for the ADR-0018 compat window.
pub const LEGACY_APPLICATION: &str = "smed";

/// Resolve the user config directory, preferring `mjolnr` with a `smed` fallback.
///
/// The same rule the workspace shim uses, for the same reason: an existing
/// install keeps reading and writing where its files already are, and a fresh
/// install gets the canonical namespace. Resolving once — rather than reading
/// from one namespace and writing to the other — is the whole point. A theme
/// written to `smed` and read back from `mjolnr` is silently no theme at all.
///
/// Returns `None` when the platform has no resolvable home directory, which is
/// the caller's cue to skip a best-effort preference rather than fail a launch.
#[must_use]
pub fn resolve_user_config_dir() -> Option<PathBuf> {
    let canonical = app_config_dir(APPLICATION)?;
    let legacy = app_config_dir(LEGACY_APPLICATION);
    Some(prefer_canonical_unless_only_legacy_exists(
        canonical,
        legacy.as_deref(),
    ))
}

/// The selection rule, split out from the platform lookup so it is testable
/// without reaching for the real home directory or mutating process-wide
/// environment variables from a parallel test run.
fn prefer_canonical_unless_only_legacy_exists(
    canonical: PathBuf,
    legacy: Option<&Path>,
) -> PathBuf {
    match legacy {
        Some(legacy) if !canonical.exists() && legacy.exists() => legacy.to_path_buf(),
        _ => canonical,
    }
}

fn app_config_dir(app_name: &str) -> Option<PathBuf> {
    use etcetera::app_strategy::{AppStrategy, AppStrategyArgs, choose_native_strategy};
    choose_native_strategy(AppStrategyArgs {
        top_level_domain: String::new(),
        author: String::new(),
        app_name: app_name.to_owned(),
    })
    .ok()
    .map(|strategy| strategy.config_dir())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_falls_back_to_legacy_only_when_canonical_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().join(APPLICATION);
        let legacy = temp.path().join(LEGACY_APPLICATION);
        std::fs::create_dir_all(&legacy).unwrap();

        assert_eq!(
            prefer_canonical_unless_only_legacy_exists(canonical.clone(), Some(&legacy)),
            legacy,
            "an existing install keeps its own directory"
        );

        std::fs::create_dir_all(&canonical).unwrap();
        assert_eq!(
            prefer_canonical_unless_only_legacy_exists(canonical.clone(), Some(&legacy)),
            canonical,
            "once the canonical directory exists it wins"
        );
    }

    #[test]
    fn user_config_is_canonical_when_neither_directory_exists() {
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().join(APPLICATION);
        let legacy = temp.path().join(LEGACY_APPLICATION);
        assert_eq!(
            prefer_canonical_unless_only_legacy_exists(canonical.clone(), Some(&legacy)),
            canonical
        );
    }

    #[test]
    fn the_theme_write_and_read_resolve_to_one_directory() {
        // The regression this guards: `persist_preference` writing under one
        // namespace while the startup read looked under the other, which made a
        // chosen theme silently vanish on the next launch.
        let first = resolve_user_config_dir().expect("a platform config directory");
        let second = resolve_user_config_dir().expect("a platform config directory");
        assert_eq!(first, second);
        assert!(
            first.ends_with(APPLICATION) || first.ends_with(LEGACY_APPLICATION),
            "the config directory must be namespaced: {}",
            first.display()
        );
    }

    #[test]
    fn fallback_to_legacy_smed_when_mjolnr_absent() {
        let temp = tempfile::tempdir().unwrap();
        let smed_dir = temp.path().join(".smed");
        std::fs::create_dir_all(&smed_dir).unwrap();
        assert_eq!(resolve_workspace_config_dir(temp.path()), smed_dir);
    }

    #[test]
    fn canonical_mjolnr_wins_when_both_exist() {
        let temp = tempfile::tempdir().unwrap();
        let smed_dir = temp.path().join(".smed");
        let mjolnr_dir = temp.path().join(".mjolnr");
        std::fs::create_dir_all(&smed_dir).unwrap();
        std::fs::create_dir_all(&mjolnr_dir).unwrap();
        assert_eq!(resolve_workspace_config_dir(temp.path()), mjolnr_dir);
    }

    #[test]
    fn defaults_to_mjolnr_when_neither_exists() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_workspace_config_dir(temp.path()),
            temp.path().join(".mjolnr")
        );
    }

    #[test]
    fn migration_on_write_copies_without_deleting_smed() {
        let temp = tempfile::tempdir().unwrap();
        let smed_dir = temp.path().join(".smed");
        std::fs::create_dir_all(smed_dir.join("routes")).unwrap();
        std::fs::write(smed_dir.join("routes").join("main.yaml"), "test: 1").unwrap();

        migrate_config_on_write(temp.path()).unwrap();

        let mjolnr_dir = temp.path().join(".mjolnr");
        assert!(mjolnr_dir.join("routes").join("main.yaml").exists());
        assert!(smed_dir.join("routes").join("main.yaml").exists());
    }
}
