//! Session listing, lease release, and database diagnostics.
//!
//! One reason to change: what an operator can see and do about stored sessions
//! from outside a running TUI.
//!
//! These paths run **instead of** the TUI, so stdout is theirs (see
//! [`super::auth`] for the same allowance and the same reason).
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommands run instead of the TUI, so stdout is not the alternate screen"
)]

use clap::Subcommand;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::core::event::SessionId;
use crate::core::store::{
    EventStore, IntegrityReport, SessionStatus, SessionSummary, StoreDiagnostics, StoreError,
};
use crate::store::sqlite::SqliteEventStore;

/// The concrete store these commands drive.
///
/// Deliberately not `dyn EventStore`: diagnostics are a SQLite concern
/// ([`StoreDiagnostics`] exists only for the durable store), and a trait object
/// would have to pretend the in-memory store could answer "what is the WAL
/// state" — an invented diagnostic is worse than an absent one.
pub type Store = SqliteEventStore;

#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// List stored sessions, newest first.
    List,

    /// Release a session's write lease.
    ///
    /// A crash leaves the lease behind, and smed will not take it on its own:
    /// it cannot prove the previous process is gone. This is the explicit human
    /// act that says so (`docs/persistence.md` §5).
    Release { session: String },
}

/// Run a session subcommand. Returns the process exit code.
pub async fn run(command: SessionsCommand, store: &Store) -> Result<i32, StoreError> {
    match command {
        SessionsCommand::List => list(store).await,
        SessionsCommand::Release { session } => release(store, &session).await,
    }
}

async fn list(store: &Store) -> Result<i32, StoreError> {
    let sessions = store.sessions().await?;

    if sessions.is_empty() {
        println!("no sessions yet — run `smed` to start one");
        return Ok(0);
    }

    println!(
        "{:<38} {:<8} {:<7} {:<22} PROJECT",
        "SESSION", "STATUS", "EVENTS", "MODEL"
    );
    for summary in &sessions {
        println!(
            "{:<38} {:<8} {:<7} {:<22} {}",
            format!("{}{}", summary.id, lease_marker(summary)),
            summary.status.as_str(),
            summary.event_count,
            summary.model.as_ref().map_or("-", |model| model.as_str()),
            summary.project_root.display()
        );
    }

    if sessions.iter().any(|summary| summary.leased) {
        println!();
        println!(
            "* held by a running smed, or left behind by one that crashed. \
             `smed sessions release <id>` reclaims it."
        );
    }

    Ok(0)
}

/// A held lease is marked rather than described.
///
/// smed cannot tell "open in another terminal" from "crashed an hour ago", and
/// the marker says exactly that much (`AGENTS.md` §1.3).
fn lease_marker(summary: &SessionSummary) -> &'static str {
    if summary.leased { " *" } else { "" }
}

async fn release(store: &Store, raw: &str) -> Result<i32, StoreError> {
    let Some(session) = parse_session(raw) else {
        eprintln!("`{raw}` is not a session id — `smed sessions list` shows them");
        return Ok(1);
    };

    store.break_lease(session).await?;
    println!("released any write lease on {session}");
    Ok(0)
}

/// Report on the database.
///
/// `integrity` gates `PRAGMA integrity_check`, which  forbids running on
/// every launch: it is O(N log N) over the whole file.
pub async fn diagnostics(store: &Store, integrity: bool) -> Result<i32, StoreError> {
    let report = store.report().await?;

    println!("database        {}", report.database_path.display());
    println!(
        "schema          {} (this build supports {})",
        report.schema_version, report.supported_schema_version
    );
    println!("journal mode    {}", report.journal_mode);
    println!(
        "foreign keys    {}",
        if report.foreign_keys {
            "on"
        } else {
            "OFF — every reference in the schema is unenforced"
        }
    );
    println!("busy timeout    {} ms", report.busy_timeout_ms);
    println!(
        "size            {} bytes ({} pages x {})",
        report.page_count.saturating_mul(report.page_size),
        report.page_count,
        report.page_size
    );
    println!("sessions        {}", report.sessions);
    println!("events          {}", report.events);
    println!("checkpoints     {}", report.checkpoints);
    println!("leases held     {}", report.leased_sessions);

    print_session_states(store).await?;

    if !integrity {
        println!();
        println!("integrity       not checked — pass --integrity to run it");
        return Ok(0);
    }

    println!();
    match store.integrity_check().await? {
        IntegrityReport::Ok => {
            println!("integrity       ok");
            Ok(0)
        }
        IntegrityReport::Problems(problems) => {
            // Verbatim: paraphrasing a corruption report loses the only detail
            // that makes it actionable.
            println!("integrity       {} PROBLEM(S)", problems.len());
            for problem in &problems {
                println!("  {problem}");
            }
            Ok(1)
        }
    }
}

async fn print_session_states(store: &Store) -> Result<(), StoreError> {
    let sessions = store.sessions().await?;
    if sessions.is_empty() {
        return Ok(());
    }

    println!();
    println!("session state");
    for summary in &sessions {
        let checkpoint = summary
            .last_checkpoint_sequence
            .map_or_else(|| "none".to_owned(), |sequence| sequence.to_string());
        let updated = summary
            .updated_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown".to_owned());
        println!(
            "  {} {:<7} events={:<5} checkpoint={:<5} lease={:<5} updated={}",
            summary.id,
            summary.status.as_str(),
            summary.event_count,
            checkpoint,
            if summary.leased { "held" } else { "free" },
            updated
        );
    }

    // A session with events but no checkpoint is the shape a crash leaves. Worth
    // naming, because it is the thing an operator is usually looking for.
    let uncheckpointed = sessions
        .iter()
        .filter(|summary| {
            summary.status == SessionStatus::Active
                && summary.event_count > 0
                && summary.last_checkpoint_sequence != Some(summary.event_count)
        })
        .count();
    if uncheckpointed > 0 {
        println!();
        println!(
            "  {uncheckpointed} active session(s) have events after their last checkpoint. \
             That is normal for a session interrupted mid-run; resuming replays them."
        );
    }

    Ok(())
}

fn parse_session(raw: &str) -> Option<SessionId> {
    Uuid::parse_str(raw.trim()).ok().map(SessionId::from_uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_round_trips_through_its_display_form() {
        // `smed sessions list` prints ids that `smed --resume` must accept.
        let session = SessionId::new();
        assert_eq!(parse_session(&session.to_string()), Some(session));
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        // Copy-paste from a terminal picks up a trailing newline more often than
        // not; refusing it would be pedantry, not a guard.
        let session = SessionId::new();
        assert_eq!(parse_session(&format!("  {session}\n")), Some(session));
    }

    #[test]
    fn a_non_id_is_rejected_rather_than_guessed() {
        assert_eq!(parse_session("latest"), None);
        assert_eq!(parse_session(""), None);
    }

    #[test]
    fn a_held_lease_is_marked_without_claiming_the_holder_is_alive() {
        let summary = SessionSummary {
            id: SessionId::new(),
            project_root: std::path::PathBuf::from("/tmp/p"),
            title: "p".to_owned(),
            status: SessionStatus::Active,
            provider: None,
            model: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
            event_count: 3,
            last_checkpoint_sequence: None,
            leased: true,
            parent: None,
        };
        assert_eq!(lease_marker(&summary), " *");

        let free = SessionSummary {
            leased: false,
            ..summary
        };
        assert_eq!(lease_marker(&free), "");
    }
}
