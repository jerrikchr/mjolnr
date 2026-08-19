//! The frontend-safe client contract (Phase A0, `docs/tauri-path-and-phases.md`).

pub mod board;
pub mod command;
pub mod event;
pub mod external_agent;
pub mod graph;
pub mod terminal;
pub mod types;
pub mod workspace;

#[cfg(test)]
mod tests;

pub use board::*;
pub use command::*;
pub use event::*;
pub use external_agent::*;
pub use graph::*;
pub use terminal::*;
pub use types::*;
