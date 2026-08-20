//! Where mjolnr's database lives.
//!
//! : "Use one SQLite database in the platform-appropriate user data
//! directory."
//!
//! # Why not `$HOME/.mjolnr`
//!
//! Because it is wrong on both platforms mjolnr targets, and the rules are not
//! guessable: macOS wants `~/Library/Application Support`, Linux wants
//! `$XDG_DATA_HOME` with a `~/.local/share` fallback, and the fallback applies
//! only when the variable is unset *or* not absolute. Hand-rolling that is a
//! pile of `cfg!` branches nobody tests on the platform they are not using.
//!
//! # Why `etcetera` rather than 's `directories`
//!
//! `directories 6` depends on `option-ext`, which is **MPL-2.0**. `deny.toml`
//! allows permissive licences only, and `THIRD_PARTY.md` says why: mjolnr's own
//! licence is an open owner decision , and a copyleft dependency
//! appearing in the graph "fails CI on purpose" so that decision stays open.
//!
//! Adding MPL-2.0 to the allowlist would be making that decision on Jerrik's
//! behalf. `etcetera` does the same job under MIT OR Apache-2.0 with two
//! permissive dependencies, so the shortlist deviation is the cheap side of the
//! trade. Recorded in the Phase 4 report and `THIRD_PARTY.md`.
//!
//! `choose_native_strategy` — not `choose_app_strategy` — because the latter
//! uses XDG *on macOS too*. That is a defensible CLI convention, but
//! asks for platform-appropriate, and it is what `directories` would have given:
//!
//! | Platform | Data directory |
//! |---|---|
//! | macOS | `~/Library/Application Support/mjolnr` |
//! | Linux | `$XDG_DATA_HOME/mjolnr`, else `~/.local/share/mjolnr` |

use std::path::{Path, PathBuf};

use etcetera::app_strategy::{AppStrategy, AppStrategyArgs, choose_native_strategy};

/// The database file name inside the data directory.
const DATABASE_FILE: &str = "mjolnr.sqlite3";

/// The credentials sub-directory inside the data directory.
const CREDENTIALS_DIR: &str = "credentials";

/// Reverse-DNS components for the app strategy.
///
/// Both are deliberately empty: mjolnr has no domain or organisation yet, and
/// inventing one would put the data directory somewhere that has to be migrated
/// the moment a real one exists. `AppStrategyArgs::bundle_id` drops empty parts,
/// so the identifier is exactly `mjolnr`.
const TOP_LEVEL_DOMAIN: &str = "";
const AUTHOR: &str = "";
const APPLICATION: &str = "mjolnr";

/// Why a data location could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("no home directory could be resolved for this user: {detail}")]
    NoDataDirectory { detail: String },

    #[error("could not create the data directory {path}: {detail}")]
    NotCreatable { path: PathBuf, detail: String },
}

/// The default database path, creating the data directory if needed.
///
/// # Errors
/// When the platform has no home directory, or the directory cannot be created.
pub fn default_database_path() -> Result<PathBuf, PathError> {
    let strategy = choose_native_strategy(AppStrategyArgs {
        top_level_domain: TOP_LEVEL_DOMAIN.to_owned(),
        author: AUTHOR.to_owned(),
        app_name: APPLICATION.to_owned(),
    })
    .map_err(|error| PathError::NoDataDirectory {
        detail: error.to_string(),
    })?;

    database_path_in(&strategy.data_dir())
}

/// The directory holding one credential file per provider.
///
/// A sibling of the database rather than a table inside it: the database is a
/// replayable event log that gets copied, inspected, and shipped around in bug
/// reports, and credentials must never ride along.
///
/// # Errors
/// When the platform has no home directory.
pub fn default_credentials_dir() -> Result<PathBuf, PathError> {
    let strategy = choose_native_strategy(AppStrategyArgs {
        top_level_domain: TOP_LEVEL_DOMAIN.to_owned(),
        author: AUTHOR.to_owned(),
        app_name: APPLICATION.to_owned(),
    })
    .map_err(|error| PathError::NoDataDirectory {
        detail: error.to_string(),
    })?;

    Ok(strategy.data_dir().join(CREDENTIALS_DIR))
}

/// The database path inside an explicit data directory, creating it if needed.
///
/// The seam that makes every persistence test run against a temporary directory
/// rather than the developer's real database (`AGENTS.md` §7: the default test
/// run touches nothing real), and what `mjolnr --data-dir` drives.
///
/// # Errors
/// When the directory cannot be created.
pub fn database_path_in(data_directory: &Path) -> Result<PathBuf, PathError> {
    std::fs::create_dir_all(data_directory).map_err(|error| PathError::NotCreatable {
        path: data_directory.to_path_buf(),
        detail: error.to_string(),
    })?;
    Ok(data_directory.join(DATABASE_FILE))
}

pub use crate::core::paths::{
    LEGACY_WORKSPACE_CONFIG_DIR, WORKSPACE_CONFIG_DIR, migrate_config_on_write,
    resolve_workspace_config_dir,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_directory_is_created_on_demand() {
        let temporary = tempfile::tempdir().expect("temp dir");
        let nested = temporary.path().join("deep").join("nested");
        assert!(!nested.exists());

        let path = database_path_in(&nested).expect("path");

        assert!(nested.is_dir(), "the data directory must be created");
        assert_eq!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some(DATABASE_FILE)
        );
        assert!(
            !path.exists(),
            "resolving a path must not create the database; opening it does"
        );
    }

    #[test]
    fn the_default_path_is_platform_appropriate() {
        // Not asserting an exact string: the whole point of the strategy is that
        // the answer differs per platform. Asserting the properties that must
        // hold everywhere instead.
        let path = default_database_path().expect("a platform data directory");

        assert!(path.is_absolute(), "the database path must be absolute");
        assert_eq!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some(DATABASE_FILE)
        );
        assert!(
            path.parent().is_some_and(Path::is_dir),
            "the parent data directory must exist after resolution"
        );

        // The regression this guards: a dotfile dumped straight in $HOME.
        let parent_path = path.parent().unwrap_or(Path::new(""));
        let parent = parent_path.to_string_lossy();
        assert!(
            parent_path
                .components()
                .any(|component| component.as_os_str() == std::ffi::OsStr::new("mjolnr")),
            "the data directory must contain a mjolnr namespace: {parent}"
        );
        assert!(
            !parent.contains("/.mjolnr") && !parent.contains("\\.mjolnr"),
            "a bare dotfile in $HOME is not a platform data directory: {parent}"
        );
    }

    #[test]
    fn the_platform_convention_is_the_native_one() {
        // `choose_app_strategy` would put macOS data in `~/.local/share`. That is
        // a CLI convention, not the platform's, and  asks for the
        // platform's. This pins which one mjolnr actually uses.
        let path = default_database_path().expect("path");
        let rendered = path.to_string_lossy();

        if cfg!(target_os = "macos") {
            assert!(
                rendered.contains("Library/Application Support/mjolnr"),
                "macOS data belongs in Application Support: {rendered}"
            );
        } else if cfg!(target_os = "linux") {
            assert!(
                rendered.contains("/mjolnr"),
                "Linux data belongs under XDG_DATA_HOME or ~/.local/share: {rendered}"
            );
        }
    }

    #[test]
    fn an_unwritable_data_directory_reports_the_path_it_tried() {
        // An error that does not name the path leaves the user guessing which
        // directory to fix (AGENTS.md §6: errors carry context).
        let temporary = tempfile::tempdir().expect("temp dir");
        let file = temporary.path().join("not-a-directory");
        std::fs::write(&file, b"x").expect("write");

        let error = database_path_in(&file.join("child")).expect_err("must refuse");
        assert!(matches!(error, PathError::NotCreatable { .. }));
        assert!(error.to_string().contains("not-a-directory"));
    }
}
