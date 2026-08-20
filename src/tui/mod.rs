//! The Ratatui client.
//!
//! **This module is a leaf.** Nothing else in mjolnr may import it, and it may
//! not import `providers` or `store`. It talks to the runtime through the
//! [`MjolnrRuntime`](crate::core::runtime::MjolnrRuntime) trait and nothing else.
//!
//! There is no process boundary enforcing that — mjolnr is one binary by design
//! . `tests/architecture.rs` enforces it instead, which is why the rule
//! survives contact with a deadline.

pub mod app;
mod auth;
pub mod auxiliary_panel;
pub mod changes_surface;
mod chrome;
pub mod commands;
mod discovery;
mod empty;
mod external_agents;
mod help;
mod highlight;
mod image;
pub mod jump_palette;
pub(crate) mod keymap;
pub mod launcher;
pub mod layout;
mod markdown;
mod mcp;
mod memory;
mod models;
pub mod plan_surface;
mod plugins;
pub mod reducer;
mod render_cache;
pub mod shell;
mod skills;
pub mod theme;
mod timeline;
mod triggers;
pub mod usage;
pub mod verify_surface;
pub mod viewport;
pub mod workspace_types;
