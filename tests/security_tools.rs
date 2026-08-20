//! Negative tests for the Phase 3 filesystem guards.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mjolnr::core::error::ReasonCode;
use mjolnr::core::message::ToolOutcome;
use mjolnr::core::tool::{ReadSet, ToolContext};
use mjolnr::policy::paths;
use mjolnr::tools::ToolRegistry;
use proptest::prelude::*;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn context(root: &Path) -> ToolContext {
    ToolContext {
        workspace_root: std::fs::canonicalize(root).expect("canonical workspace"),
        read_set: Arc::new(ReadSet::default()),
        max_output_bytes: 64 * 1024,
        command_timeout: Duration::from_secs(5),
    }
}

fn assert_refused(result: &mjolnr::core::message::ToolResult, expected: ReasonCode) {
    assert_eq!(
        result.outcome,
        ToolOutcome::Refused(expected),
        "guard returned the wrong outcome: {result:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_and_parent_escapes_are_rejected() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let outside = temp.path().join("outside.txt");
    std::fs::write(&outside, "outside").expect("outside fixture");
    symlink(&outside, workspace.join("escape.txt")).expect("symlink");

    let registry = ToolRegistry::builtins();
    let read = registry.get("read_file").expect("read tool");
    let escaped = read
        .execute(
            serde_json::json!({ "path": "escape.txt" }),
            context(&workspace),
            CancellationToken::new(),
        )
        .await
        .expect("structured result");
    assert_refused(&escaped, ReasonCode::PathSymlinkEscape);

    let parent = read
        .execute(
            serde_json::json!({ "path": "../outside.txt" }),
            context(&workspace),
            CancellationToken::new(),
        )
        .await
        .expect("structured result");
    assert_refused(&parent, ReasonCode::PathOutsideWorkspace);
}

#[tokio::test]
async fn edit_without_read_is_rejected() {
    let temp = TempDir::new().expect("tempdir");
    std::fs::write(temp.path().join("file.txt"), "before\n").expect("fixture");
    let edit = ToolRegistry::builtins()
        .get("edit_file")
        .expect("edit tool");

    let result = edit
        .execute(
            serde_json::json!({ "path": "file.txt", "old": "before\n", "new": "after\n" }),
            context(temp.path()),
            CancellationToken::new(),
        )
        .await
        .expect("structured result");

    assert_refused(&result, ReasonCode::FileNotObserved);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("file.txt")).expect("read fixture"),
        "before\n"
    );
}

#[tokio::test]
async fn write_preview_preserves_path_refusal_code() {
    let temp = TempDir::new().expect("tempdir");
    let write = ToolRegistry::builtins()
        .get("write_file")
        .expect("write tool");

    let error = write
        .preview(
            &serde_json::json!({ "path": "../escape.txt", "content": "nope" }),
            &context(temp.path()),
        )
        .await
        .expect_err("preview must refuse an escaped path");

    assert_eq!(error.reason_code(), ReasonCode::PathOutsideWorkspace);
}

#[tokio::test]
async fn search_preserves_path_refusal_code() {
    let temp = TempDir::new().expect("tempdir");
    let search = ToolRegistry::builtins()
        .get("search_text")
        .expect("search tool");

    let result = search
        .execute(
            serde_json::json!({ "query": "anything", "path": ".." }),
            context(temp.path()),
            CancellationToken::new(),
        )
        .await
        .expect("structured result");

    assert_refused(&result, ReasonCode::PathOutsideWorkspace);
}

#[tokio::test]
async fn external_change_between_read_and_edit_is_rejected() {
    let temp = TempDir::new().expect("tempdir");
    let file = temp.path().join("file.txt");
    std::fs::write(&file, "before\n").expect("fixture");
    let registry = ToolRegistry::builtins();
    let context = context(temp.path());

    let read_result = registry
        .get("read_file")
        .expect("read tool")
        .execute(
            serde_json::json!({ "path": "file.txt" }),
            context.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("read");
    assert!(
        read_result.outcome.is_ok(),
        "read must succeed: {read_result:?}"
    );
    std::fs::write(&file, "changed elsewhere\n").expect("external edit");

    let result = registry
        .get("edit_file")
        .expect("edit tool")
        .execute(
            serde_json::json!({ "path": "file.txt", "old": "before\n", "new": "after\n" }),
            context,
            CancellationToken::new(),
        )
        .await
        .expect("structured result");

    assert_refused(&result, ReasonCode::StaleFileVersion);
    assert_eq!(
        std::fs::read_to_string(file).expect("read fixture"),
        "changed elsewhere\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_output_is_bounded_and_disclosed() {
    let temp = TempDir::new().expect("tempdir");
    let mut bounded = context(temp.path());
    bounded.max_output_bytes = 8;
    let result = ToolRegistry::builtins()
        .get("run_command")
        .expect("command tool")
        .execute(
            serde_json::json!({
                "program": "/bin/sh",
                "arguments": ["-c", "printf 12345678901234567890"]
            }),
            bounded,
            CancellationToken::new(),
        )
        .await
        .expect("command result");

    assert!(result.outcome.is_ok());
    assert!(result.truncated, "truncation must be explicit metadata");
    assert!(!result.content.contains("12345678901234567890"));
}

#[cfg(unix)]
#[tokio::test]
async fn nonzero_command_is_not_reported_as_success() {
    let temp = TempDir::new().expect("tempdir");
    let result = ToolRegistry::builtins()
        .get("run_command")
        .expect("command tool")
        .execute(
            serde_json::json!({
                "program": "/bin/sh",
                "arguments": ["-c", "exit 7"]
            }),
            context(temp.path()),
            CancellationToken::new(),
        )
        .await
        .expect("command result");

    assert_eq!(
        result.outcome,
        ToolOutcome::Failed(ReasonCode::ToolExecution)
    );
    assert!(result.content.contains("exit_code: 7"));
}

proptest! {
    #[test]
    fn arbitrary_parent_paths_fail_closed(name in "[a-zA-Z0-9._-]{1,32}") {
        let temp = TempDir::new().expect("tempdir");
        let root = std::fs::canonicalize(temp.path()).expect("canonical root");
        let requested = format!("../{name}");
        let refusal = paths::for_write(&root, Path::new(&requested)).expect_err("must refuse");
        prop_assert_eq!(refusal.code, ReasonCode::PathOutsideWorkspace);
    }
}
