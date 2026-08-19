//! Traits and types. Depends on nothing else in smed.
//!
//! `core` is the base of the dependency direction in `AGENTS.md` §2.1:
//!
//! ```text
//! tui → runtime → core
//!                  ↑
//!        providers / tools / store
//! ```
//!
//! Everything here is either a contract (`Provider`, `Tool`, `EventStore`,
//! `SmedRuntime`) or a value that crosses one. If something in `core` needs to
//! import an implementation, the boundary has been drawn in the wrong place.

pub mod board;
pub mod change_capture;
pub mod changes;
pub mod checkpoint;
pub mod client;
pub mod command;
pub mod context;
pub mod continuation;
pub mod council;
pub mod directive;
pub mod discovery;
pub mod envelope;
pub mod error;
pub mod event;
pub mod extension;
pub mod fleet;
pub mod frontier;
pub mod governance;
pub mod image;
pub mod imported;
pub mod mcp;
pub mod memory;
pub mod message;
pub mod model;
pub mod paths;
pub mod plan;
pub mod plugin;
pub mod policy;
pub mod preview;
pub mod pricing;
pub mod process;
pub mod provider;
pub mod recovery;
pub mod repository;
pub mod review;
pub mod routing;
pub mod runtime;
pub mod secrets;
pub mod store;
pub mod tool;
pub mod trigger;
pub mod workspace_files;
