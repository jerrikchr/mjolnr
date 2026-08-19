//! Durable storage implementations of the [`EventStore`](crate::core::store::EventStore) port.
//!
//! Phase 1 shipped [`memory::InMemoryEventStore`]; Phase 4 adds [`sqlite`]
//! behind the same port. Neither is visible to the runtime, which holds
//! `Arc<dyn EventStore>`.
//!
//! [`wire`] owns the persisted format — deliberately separate from both, because
//! the format is a contract that outlives any one backend and must not be
//! rewritten by a refactor of the domain types.

pub mod memory;
pub mod paths;
pub mod secrets;
pub mod sqlite;
mod wire;
