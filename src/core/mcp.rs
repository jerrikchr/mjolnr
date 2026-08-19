//! Provider-neutral MCP display state.

use crate::core::error::ReasonCode;
use crate::core::tool::ToolTier;

/// Connection state for one explicitly configured MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConnectionState {
    Connected,
    Unavailable,
}

/// What clients may render about an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSummary {
    pub name: String,
    pub state: McpConnectionState,
    pub tool_count: usize,
    pub tier: ToolTier,
    pub reason: Option<ReasonCode>,
}
