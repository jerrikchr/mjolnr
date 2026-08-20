//! `mjolnr triggers list` and `mjolnr triggers rearm` — the store-only trigger
//! surfaces.
//!
//! `mjolnr triggers run` (the scheduler process) is deliberately absent here:
//! it drives a `Runtime`, and this module answers to the same rule
//! `tests/architecture.rs` enforces for `cli` — it must never become a second
//! client of the agent loop. `main.rs` wires `run` directly, exactly as it
//! wires `mjolnr exec`.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommands run instead of the TUI, so stdout is not the alternate screen"
)]

use clap::Subcommand;

use crate::core::event::MjolnrEvent;
use crate::core::store::{EventStore, StoreError};
use crate::triggers::{control, definition, status};

pub type Store = crate::store::sqlite::SqliteEventStore;

#[derive(Debug, Subcommand)]
pub enum TriggersCommand {
    /// Run the scheduler: fire every configured trigger until interrupted.
    ///
    /// Handled by `main.rs`, not this module — it drives a `Runtime`, which
    /// `cli` may never do (`tests/architecture.rs`). Present here only so
    /// `clap` parses it; [`run`] declines it exactly as it declines `Exec`.
    Run,

    /// List triggers configured under `.mjolnr/triggers/` in the current
    /// project, their overlap policy, and their last-known outcome.
    List,

    /// Re-arm a trigger that disabled itself after repeated failures.
    Rearm {
        /// The trigger's name (its file's stem under `.mjolnr/triggers/`).
        name: String,
    },
}

/// Run a triggers subcommand against `workspace_root` — the current working
/// directory's project for a real invocation
/// (see [`super::run_with_store`](crate::cli::run_with_store)), an isolated
/// fixture directory in a test. Returns the process exit code. `Run` is
/// never passed here — see [`TriggersCommand::Run`].
pub async fn run(
    command: TriggersCommand,
    store: &Store,
    workspace_root: &std::path::Path,
) -> Result<i32, StoreError> {
    match command {
        TriggersCommand::Run => Ok(2),
        TriggersCommand::List => list(store, workspace_root).await,
        TriggersCommand::Rearm { name } => rearm(store, workspace_root, &name).await,
    }
}

async fn list(store: &Store, workspace_root: &std::path::Path) -> Result<i32, StoreError> {
    let Ok(root_realpath) = control::root_realpath(workspace_root) else {
        println!("no project here — `.mjolnr/triggers/` is read relative to the current directory");
        return Ok(0);
    };
    let (statuses, diagnostics) = status::collect(store, workspace_root, &root_realpath).await?;

    if statuses.is_empty() && diagnostics.is_empty() {
        println!("no triggers configured — add a file under .mjolnr/triggers/");
        return Ok(0);
    }

    println!(
        "{:<20} {:<10} {:<8} {:<10} {:<12} LAST OUTCOME",
        "TRIGGER", "SOURCE", "OVERLAP", "STATE", "FAILURES"
    );
    for trigger in &statuses {
        let state = if trigger.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let failures = format!(
            "{}/{}",
            trigger.consecutive_failures, trigger.max_consecutive_failures
        );
        let outcome = trigger
            .last_outcome
            .map_or("never fired", |outcome| outcome.label());
        println!(
            "{:<20} {:<10} {:<8} {:<10} {:<12} {}",
            trigger.name,
            trigger.source.label(),
            trigger.overlap.label(),
            state,
            failures,
            outcome
        );
        if let Some(code) = trigger.disabled_reason {
            println!("  disabled: {code} // {}", code.sentence());
        }
        if let Some(next) = trigger.next_fire_at {
            println!("  next firing: {next}");
        }
    }

    for diagnostic in &diagnostics {
        eprintln!(
            "mjolnr: {} could not be loaded as a trigger: {}",
            diagnostic.path.display(),
            diagnostic.detail
        );
    }

    Ok(0)
}

async fn rearm(
    store: &Store,
    workspace_root: &std::path::Path,
    name: &str,
) -> Result<i32, StoreError> {
    let (definitions, _) = definition::load_dir(workspace_root);
    if !definitions.iter().any(|definition| definition.name == name) {
        eprintln!("no trigger named `{name}` under .mjolnr/triggers/");
        return Ok(1);
    }
    let Ok(root_realpath) = control::root_realpath(workspace_root) else {
        eprintln!("mjolnr: could not resolve the project root");
        return Ok(1);
    };
    let control_session = control::control_session_id(&root_realpath, name);
    let history = control::history(store, control_session).await?;
    let state = control::replay(&history, name);
    if state.disabled_reason.is_none() {
        println!("trigger `{name}` is not disabled");
        return Ok(0);
    }

    store
        .append(MjolnrEvent::TriggerRearmed {
            session: control_session,
            trigger: name.to_owned(),
        })
        .await?;
    println!("re-armed trigger `{name}`");
    Ok(0)
}
