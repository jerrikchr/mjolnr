//! mjolnr: a local-first terminal AI coding harness.
//!
//! Engineering standards: `AGENTS.md`.
//!
//! # Architecture
//!
//! ```text
//! tui → runtime → core (traits + types)
//!                  ↑        ↑
//!             providers   tools / store / policy / context
//! ```
//!
//! The direction is one-way and mechanically enforced by `tests/architecture.rs`.
//! In particular `tui` is a *client*: it renders snapshots and emits commands,
//! and it may never call a provider, execute a tool, or hold the authoritative
//! transcript. There is no process boundary making that true — only the test.
//!
//! `core` depends on nothing else here. Everything in it is either a contract or
//! a value that crosses one.

pub mod cli;
pub mod context;
pub mod core;
pub mod discovery;
pub mod governance;
pub mod graph;
pub mod headless;
pub mod integrations;
pub mod mcp;
pub mod memory;
pub mod plugins;
pub mod policy;
pub mod providers;
pub mod repository;
pub mod routing;
pub mod runtime;
pub mod store;
pub mod tools;
pub mod triggers;
pub mod tui;
pub mod workspace_files;
