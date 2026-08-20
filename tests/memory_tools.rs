//! Integration tests for memory tools and runtime actor query answering (master implementation plan §2.1–2.3).

use std::sync::Arc;

use tempfile::tempdir;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use mjolnr::core::tool::{ReadSet, Tool, ToolContext, ToolTier};
use mjolnr::memory::store::MemoryStore;
use mjolnr::memory::{RuleDocument, RulesSnapshot};
use mjolnr::tools::ToolRegistry;
use mjolnr::tools::memory::{MemoryExpand, MemorySearch, MemoryTimeline};

#[test]
fn memory_tools_are_built_in_and_have_valid_schemas() {
    let registry = ToolRegistry::builtins();
    for tool_name in ["memory_search", "memory_timeline", "memory_expand"] {
        let tool = registry
            .get(tool_name)
            .unwrap_or_else(|| panic!("{tool_name} must be registered"));
        assert_eq!(tool.tier(), ToolTier::Read);
        let schema = tool.schema();
        assert!(
            jsonschema::meta::is_valid(&schema),
            "{tool_name} schema must be valid JSON schema"
        );
        assert!(
            !schema.to_string().contains("$ref"),
            "{tool_name} must not contain external $ref"
        );
    }
}

#[test]
fn memory_tools_enforce_tool_tier_read() {
    let search = MemorySearch;
    let timeline = MemoryTimeline;
    let expand = MemoryExpand;

    assert_eq!(search.tier(), ToolTier::Read);
    assert_eq!(timeline.tier(), ToolTier::Read);
    assert_eq!(expand.tier(), ToolTier::Read);
}

#[tokio::test]
async fn marker_tools_refuse_to_execute_on_their_own() {
    // The negative test for the marker pattern (AGENTS.md §7): the tool's
    // `execute` must refuse, proving a memory tool invoked outside actor
    // mediation cannot act — even though its tier is Read and mediation is
    // the only real path.
    let context = ToolContext {
        workspace_root: std::path::PathBuf::new(),
        read_set: Arc::new(ReadSet::default()),
        max_output_bytes: 4096,
        command_timeout: std::time::Duration::from_secs(1),
    };
    let tools: [(&str, Arc<dyn Tool>); 3] = [
        ("memory_search", Arc::new(MemorySearch)),
        ("memory_timeline", Arc::new(MemoryTimeline)),
        ("memory_expand", Arc::new(MemoryExpand)),
    ];
    for (name, tool) in tools {
        let result = tool
            .execute(
                serde_json::json!({}),
                context.clone(),
                CancellationToken::new(),
            )
            .await
            .unwrap_or_else(|_| panic!("{name} execute must return, not error"));
        assert!(
            result.outcome.reason_code().is_some(),
            "{name} must refuse when executed outside the actor"
        );
    }
}

#[test]
fn rules_snapshot_prompt_section_formats_cleanly() {
    let snapshot = RulesSnapshot {
        user_profile: Some(RuleDocument {
            name: "USER".to_owned(),
            sha256: "abcdef0123456789".to_owned(),
            chars: 20,
            content: "Prefer concise Rust.".to_owned(),
        }),
        rules: vec![RuleDocument {
            name: "style".to_owned(),
            sha256: "123456789abcdef0".to_owned(),
            chars: 18,
            content: "No unwrap in lib.".to_owned(),
        }],
    };

    let prompt = snapshot.prompt_section().expect("non-empty prompt section");
    assert!(prompt.contains("## Workspace Rules & User Profile (Frozen Snapshot)"));
    assert!(prompt.contains("### User Profile (`.mjolnr/USER.md`)"));
    assert!(prompt.contains("Prefer concise Rust."));
    assert!(prompt.contains("### Rule: style (`sha256:12345678`)"));
    assert!(prompt.contains("No unwrap in lib."));
}

#[test]
fn empty_rules_snapshot_produces_no_prompt_section() {
    let snapshot = RulesSnapshot::default();
    assert_eq!(snapshot.prompt_section(), None);
}

#[tokio::test]
async fn memory_tools_answer_from_workspace_store() {
    let workspace = tempdir().expect("tempdir");
    let mjolnr_dir = workspace.path().join(".mjolnr").join("data");
    std::fs::create_dir_all(&mjolnr_dir).expect("create mjolnr dir");
    let db_path = mjolnr_dir.join("memory.db");

    // Populate facts directly in the projection database
    let store = MemoryStore::open(&db_path).await.expect("open store");
    let now = OffsetDateTime::now_utc();
    let id1 = store
        .record_fact("Auth", "uses", "Lucia", "session-1", now)
        .await
        .expect("record fact 1");
    let id2 = store
        .record_fact("Auth", "uses", "BetterAuth", "session-2", now)
        .await
        .expect("record fact 2");
    let _id3 = store
        .record_fact("Database", "engine", "PostgreSQL 16", "session-2", now)
        .await
        .expect("record fact 3");

    // Search
    let hits = store.search("BetterAuth", None).await.expect("search");
    assert_eq!(hits.len(), 1);
    let first_hit = hits.first().expect("hit 0");
    assert_eq!(first_hit.id, id2);
    assert_eq!(first_hit.subject, "Auth");

    // Timeline for Auth
    let timeline = store.timeline("Auth").await.expect("timeline");
    assert_eq!(timeline.len(), 2);
    let first_entry = timeline.first().expect("timeline 0");
    let second_entry = timeline.get(1).expect("timeline 1");
    assert_eq!(first_entry.id, id1);
    assert!(first_entry.valid_until.is_some(), "id1 is superseded");
    assert_eq!(second_entry.id, id2);
    assert!(second_entry.valid_until.is_none(), "id2 is current");

    // Expand
    let expanded = store.expand(&[id1, id2]).await.expect("expand");
    assert_eq!(expanded.len(), 2);
}
