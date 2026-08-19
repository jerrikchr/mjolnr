//! Deterministic run and tool budgets.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BudgetLimits {
    pub max_provider_turns: u32,
    pub max_tool_calls: u32,
    pub max_wall_time: Duration,
    pub command_timeout: Duration,
    pub max_tool_output_bytes: usize,
    /// Token reserve fallback for providers that do not report a quota window.
    /// `None` means unknown, never an invented quota percentage.
    pub quota_token_budget: Option<u64>,
    pub quota_soft_fraction: f32,
    pub quota_hard_fraction: f32,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            max_provider_turns: 20,
            max_tool_calls: 40,
            max_wall_time: Duration::from_mins(10),
            command_timeout: Duration::from_mins(2),
            max_tool_output_bytes: 64 * 1024,
            quota_token_budget: None,
            quota_soft_fraction: 0.8,
            quota_hard_fraction: 0.95,
        }
    }
}
