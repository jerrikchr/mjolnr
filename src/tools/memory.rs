//! Tools for querying workspace memory (master implementation plan, §2.3).
//!
//! Three tools expose the Tier 3 progressive recall capabilities:
//! - [`MemorySearch`]: hybrid-scored search over current facts, returning one-line summaries with ids.
//! - [`MemoryTimeline`]: chronological history for one subject (past and current facts).
//! - [`MemoryExpand`]: targeted full-detail retrieval for named fact ids.
//!
//! All three are marker tools in [`ToolTier::Read`]. The runtime actor intercepts them
//! and answers from `.mjolnr/data/memory.db`.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::ToolResult;
use crate::core::tool::{Tool, ToolContext, ToolTier};
use crate::memory::store::{
    DEFAULT_SEARCH_LIMIT, MAX_EXPAND_IDS, MAX_SEARCH_LIMIT, MIN_QUERY_CHARS,
};

/// Search current workspace memory facts.
#[derive(Debug)]
pub struct MemorySearch;

impl MemorySearch {
    pub const NAME: &'static str = "memory_search";
}

#[async_trait]
impl Tool for MemorySearch {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Search recorded workspace knowledge and temporal facts. Returns one-line summaries with ids; use memory_expand for full detail on interesting hits."
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": MIN_QUERY_CHARS,
                    "maxLength": 256,
                    "description": "Text to match across knowledge facts. Phrase-matched."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_SEARCH_LIMIT,
                    "default": DEFAULT_SEARCH_LIMIT,
                    "description": "How many hits to return, highest-scored first."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let query = arguments
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let limit = arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_SEARCH_LIMIT as u64);
        Ok(format!(
            "search workspace memory for \"{query}\" (up to {limit} hits)"
        ))
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::failed(
            ReasonCode::ToolExecution,
            "memory_search is answered by the runtime actor",
        ))
    }
}

/// Chronological history for one subject.
#[derive(Debug)]
pub struct MemoryTimeline;

impl MemoryTimeline {
    pub const NAME: &'static str = "memory_timeline";
}

#[async_trait]
impl Tool for MemoryTimeline {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Inspect the full chronological history for one subject — past and current facts, oldest first."
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 2048,
                    "description": "The subject to read the timeline for."
                }
            },
            "required": ["subject"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let subject = arguments
            .get("subject")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Ok(format!("inspect memory timeline for subject \"{subject}\""))
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::failed(
            ReasonCode::ToolExecution,
            "memory_timeline is answered by the runtime actor",
        ))
    }
}

/// Targeted full-detail fetch for named fact ids.
#[derive(Debug)]
pub struct MemoryExpand;

impl MemoryExpand {
    pub const NAME: &'static str = "memory_expand";
}

#[async_trait]
impl Tool for MemoryExpand {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Fetch full detail for specific memory facts by id (e.g. from a prior memory_search). Up to 10 ids."
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Read
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "items": {
                        "type": "integer",
                        "minimum": 1
                    },
                    "minItems": 1,
                    "maxItems": MAX_EXPAND_IDS,
                    "description": "Fact ids to fetch in full detail."
                }
            },
            "required": ["ids"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        let count = arguments
            .get("ids")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        Ok(format!("expand {count} memory fact(s) in full detail"))
    }

    async fn execute(
        &self,
        _arguments: serde_json::Value,
        _context: ToolContext,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::failed(
            ReasonCode::ToolExecution,
            "memory_expand is answered by the runtime actor",
        ))
    }
}
