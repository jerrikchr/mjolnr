//! Client-safe projections for the operator terminal surface (Phase D8).
//!
//! The frontend sees a bounded screen projection and typed lifecycle state. It
//! never receives a PTY handle, process object, raw environment, or durable
//! transcript claim.

use serde::{Deserialize, Serialize};

pub const MAX_TERMINAL_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_TERMINAL_SCREEN_BYTES: usize = 256 * 1024;
pub const MAX_TERMINAL_QUERY_BYTES: usize = 256;
pub const MAX_TERMINAL_SEARCH_MATCHES: usize = 32;
pub const MAX_TERMINAL_CWD_BYTES: usize = 4 * 1024;
pub const MAX_TERMINAL_ROWS: u16 = 200;
pub const MAX_TERMINAL_COLS: u16 = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientTerminalStatus {
    Running,
    Stopping,
    Exited,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalSnapshot {
    pub id: String,
    pub status: ClientTerminalStatus,
    pub cwd: String,
    pub screen: String,
    pub rows: u16,
    pub cols: u16,
    pub scrollback_rows: usize,
    pub scrollback_offset: usize,
    pub screen_truncated: bool,
    pub exit_code: Option<u32>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalScroll {
    pub id: String,
    pub rows: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalSearch {
    pub id: String,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalSearchMatch {
    pub scrollback_offset: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalSearchResult {
    pub matches: Vec<ClientTerminalSearchMatch>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalLayout {
    pub primary_cwd: String,
    pub split_direction: Option<ClientTerminalSplitDirection>,
    pub secondary_cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientTerminalSplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalInput {
    pub id: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTerminalResize {
    pub id: String,
    pub rows: u16,
    pub cols: u16,
}
