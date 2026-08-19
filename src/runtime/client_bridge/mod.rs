//! The bridge between the runtime and a hosted frontend (Phase A0).

pub mod board;
pub mod bridge;
pub mod command;
pub mod convert;
pub mod graph;
pub mod pump;
pub mod workspace;

#[cfg(test)]
mod streaming_profile;

#[cfg(test)]
mod tests;

pub use bridge::{ClientBridge, ClientBridgeError};
pub use convert::{session_summary_to_client, snapshot_to_client};
