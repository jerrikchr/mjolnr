//! Triggers and scheduled runs.
//!
//! smed fires headless runs from cron-style schedules and local webhook
//! triggers, defined in diffable per-project files, with the same budgets,
//! policy ceilings, and evidence as interactive work. This module is the
//! scheduler *process* — `main.rs` wires `smed triggers run` to
//! [`scheduler::run`] exactly as it wires `smed exec` to
//! [`crate::headless::run`]. Everything else (the CLI's `list`/`rearm`
//! surfaces, and the TUI's `/triggers` overlay) reads the same durable state
//! through [`status::collect`] rather than duplicating the read model.
//!
//! # Module map
//!
//! - [`definition`] loads trigger files (schedule/webhook, directive, policy
//!   ceiling, budgets, provider/model, notify, overlap) — the diffable
//!   configuration.
//! - [`schedule`] is a minimal five-field cron parser and its "next firing"
//!   arithmetic.
//! - [`overlap`] is the pure skip/queue/replace decision.
//! - [`control`] is a trigger's control session: deterministic identity,
//!   creation, and event-sourced replay of its lifecycle.
//! - [`status`] is the one read model behind `list`, `rearm`'s precondition,
//!   and the TUI overlay.
//! - [`scheduler`] drives it all: one background task per trigger, reusing
//!   the headless host for every firing.
//! - [`webhook`] is the minimal local HTTP listener a webhook trigger binds.

pub mod control;
pub mod definition;
pub mod overlap;
pub mod schedule;
pub mod scheduler;
pub mod status;
pub mod webhook;

pub use definition::{TriggerDefinition, TriggerLoadDiagnostic, TriggerSource};
pub use scheduler::SchedulerDeps;
