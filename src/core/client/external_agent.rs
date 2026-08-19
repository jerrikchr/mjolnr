//! Client-safe projections for external-agent compatibility (Phase D9).
//!
//! The frontend sees an isolated worktree, a resolved absolute executable, and
//! a trust label it cannot promote. It never receives a PTY handle, raw
//! environment, or a claim that the work was smed-governed.

use serde::{Deserialize, Serialize};

pub use super::workspace::TrustClass;

pub const MAX_EXTERNAL_AGENTS: usize = 20;
pub const MAX_EXTERNAL_AGENT_NAME_BYTES: usize = 64;
pub const MAX_EXTERNAL_AGENT_EXECUTABLE_BYTES: usize = 1_024;
pub const MAX_EXTERNAL_AGENT_BRANCH_BYTES: usize = 200;
pub const MAX_EXTERNAL_AGENT_FAILURE_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum ExternalAgentStatus {
    Running,
    #[serde(rename_all = "camelCase")]
    Stopped {
        exit_code: Option<i32>,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub struct ExternalAgentView {
    pub id: String,
    pub profile_name: String,
    pub executable: String,
    pub branch: String,
    pub trust: TrustClass,
    pub status: ExternalAgentStatus,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalAgentCapability {
    pub available: bool,
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_agent_view_round_trip() {
        let view = ExternalAgentView {
            id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            profile_name: "codex".to_owned(),
            executable: "/usr/local/bin/codex".to_owned(),
            branch: "smed/ext-abc123".to_owned(),
            trust: TrustClass::ExternalUnverified,
            status: ExternalAgentStatus::Running,
            started_at: "2026-08-18T00:00:00Z".to_owned(),
        };
        let json = serde_json::to_string(&view).unwrap();
        let parsed: ExternalAgentView = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, view);
    }

    #[test]
    fn external_agent_status_variants_round_trip() {
        for status in [
            ExternalAgentStatus::Running,
            ExternalAgentStatus::Stopped { exit_code: Some(0) },
            ExternalAgentStatus::Stopped { exit_code: None },
            ExternalAgentStatus::Failed {
                reason: "executable not found".to_owned(),
            },
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: ExternalAgentStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn unknown_trust_class_becomes_external_unverified_in_view() {
        let json = r#"{
            "id":"x","profileName":"p","executable":"/bin/true",
            "branch":"smed/ext-x","trust":"adminOverride",
            "status":{"type":"running"},"startedAt":"2026-08-18T00:00:00Z"
        }"#;
        let parsed: ExternalAgentView = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.trust, TrustClass::ExternalUnverified);
    }

    #[test]
    fn external_agent_view_rejects_unknown_fields() {
        let json = r#"{
            "id":"x","profileName":"p","executable":"/bin/true",
            "branch":"smed/ext-x","trust":"externalUnverified",
            "status":{"type":"running"},"startedAt":"now","extra":true
        }"#;
        assert!(serde_json::from_str::<ExternalAgentView>(json).is_err());
    }

    #[test]
    fn external_agent_capability_round_trip() {
        let cap = ExternalAgentCapability {
            available: false,
            reason: Some("unknown profile".to_owned()),
        };
        let json = serde_json::to_string(&cap).unwrap();
        let parsed: ExternalAgentCapability = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, cap);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn all_limits_are_positive() {
        assert!(MAX_EXTERNAL_AGENTS > 0);
        assert!(MAX_EXTERNAL_AGENT_NAME_BYTES > 0);
        assert!(MAX_EXTERNAL_AGENT_EXECUTABLE_BYTES > 0);
        assert!(MAX_EXTERNAL_AGENT_BRANCH_BYTES > 0);
        assert!(MAX_EXTERNAL_AGENT_FAILURE_BYTES > 0);
    }
}
