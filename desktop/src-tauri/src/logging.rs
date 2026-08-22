//! File diagnostics for the desktop shell (`AGENTS.md` §4).
//!
//! stdout belongs to the webview and the alternate screen to the TUI, so
//! `tracing` output must land in a file or nowhere. Until this module it
//! landed nowhere: every `error!`/`info!` in the crate compiled against a
//! subscriber that was never installed, which is why a silently dead run had
//! no log to autopsy. The subscriber writes daily-rolling, non-blocking files
//! under the platform data directory; `RUST_LOG` overrides the filter for an
//! owner debugging a live incident.

use std::path::{Path, PathBuf};

use tracing_subscriber::EnvFilter;

/// The directory holding mjolnr-desktop's rolling log files.
///
/// A sibling of the database (`<data>/logs`) so one platform strategy places
/// everything an owner may need to inspect or ship in a report.
#[must_use]
pub(crate) fn logs_dir(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .map(|parent| parent.join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

/// Install the global file subscriber.
///
/// Returns the non-blocking writer's guard: dropping it flushes and stops the
/// background worker, so the caller holds it for the process lifetime. When
/// even the log directory cannot be created, the fallback is a stderr-format
/// subscriber rather than silence — before the webview exists, stderr is the
/// only channel left.
pub(crate) fn init(database_path: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    let directory = logs_dir(database_path);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if std::fs::create_dir_all(&directory).is_err() {
        // Same non-blocking wrapper as the file path so both return a real
        // guard; stderr is the only channel left before the webview exists.
        let (writer, guard) = tracing_appender::non_blocking(std::io::stderr());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(writer)
            .with_ansi(false)
            .try_init();
        return guard;
    }

    let file = tracing_appender::rolling::daily(&directory, "mjolnr-desktop.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
    guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_live_beside_the_database() {
        let data = std::env::temp_dir().join("mjolnr-logging-test");
        let database = data.join("mjolnr-desktop.db");
        assert_eq!(logs_dir(&database), data.join("logs"));
    }
}
