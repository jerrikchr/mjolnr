//! Deterministic model and provider routing.
//!
//! mjolnr selects a turn's, a subagent spawn's, or a scheduled run's
//! provider/model from a diffable per-project routing table, and advances
//! along an ordered fallback chain on typed conditions — quota reserve
//! breached, typed provider failure, circuit breaker open — with every
//! selection and advance recorded as an event naming the rule that fired.
//!
//! Provenance: a 2026-07-20 review of comparable routers. The chain-
//! plus-conditions shape follows 9router, the breaker vocabulary follows
//! wayland-core, and both are explicitly *not* learned routing — see
//! [`crate::core::routing`] for the anti-pattern this rejects.
//!
//! # Module map
//!
//! - [`definition`] loads `.mjolnr/routes/*.yaml` (one named chain per file)
//!   and `.mjolnr/routing.yaml` (task-class mapping, child default, breaker
//!   overrides) into one [`crate::core::routing::RouteTable`] — the diffable
//!   configuration, exactly the posture `crate::triggers::definition` takes.
//! - [`pricing`] loads `.mjolnr/pricing.yaml` overrides onto the bundled
//!   per-Mtok pricing table in [`crate::core::pricing`].
//!
//! The runtime-side mechanism — attaching a route to a session, gating a
//! provider turn on a breaker, and advancing on a typed condition — lives in
//! `crate::runtime::routing`, the same split `crate::runtime::subagent` makes
//! from `crate::core::routing`'s own value types.

pub mod definition;
pub mod edit;
pub mod pricing;
pub mod scaffold;

pub use definition::{RoutingLoadDiagnostic, load_dir};
