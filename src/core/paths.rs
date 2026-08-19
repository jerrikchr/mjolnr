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
