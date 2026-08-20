//! Git worktree lifecycle for subagent isolation.
//!
//! One reason to change: how a child's isolated checkout is created, preserved,
//! and cleaned up.
//!
//! Every child works in a fresh `git worktree` on its own `mjolnr/sub-*` branch,
//! rooted under the OS temp directory so sibling containment is a path property
//! rather than a convention. Cleanup is idempotent: a settled child's worktree
//! is removed whether or not the run crashed between dispatch and settlement,
//! and a crashed process's leftovers are reclaimed by [`cleanup_orphans`] on
//! the next project open. Merging a branch back is the parent's explicit act —
//! nothing here ever merges.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::core::error::ReasonCode;
use crate::core::event::SessionId;
use crate::core::message::ToolResult;
use crate::runtime::subagent::{ChildRecord, ChildSpec};

/// Directory under the OS temp root that holds every subagent worktree.
const WORKTREES_DIR: &str = "mjolnr-worktrees";

/// The path a child's worktree lives at.
pub(super) fn worktree_path(child: SessionId) -> PathBuf {
    std::env::temp_dir()
        .join(WORKTREES_DIR)
        .join(child.to_string())
}

/// The branch a child works on.
pub(super) fn branch_name(child: SessionId) -> String {
    // UUIDv7's leading bytes are a timestamp. Siblings minted in one fan-out
    // commonly share the first eight characters, so truncating the front can
    // alias two children onto one branch. The full session id is the identity.
    format!("mjolnr/sub-{child}")
}

/// The sidecar file naming a worktree's owning process. A sibling of the
/// worktree, never inside it — a file inside would dirty every child tree.
pub(super) fn marker_path(worktree: &Path) -> PathBuf {
    let mut name = worktree.file_name().map_or_else(
        || "worktree".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    name.push_str(".owner.json");
    worktree.with_file_name(name)
}

/// Subagents dispatch from a clean, committed tree so every child observes one
/// well-defined parent state. `Some` is the refusal; `None` means proceed.
pub(super) async fn preflight(workspace: &Path) -> Option<ToolResult> {
    match git(workspace, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(output) if output.success => {}
        Ok(_) | Err(_) => {
            return Some(ToolResult::refused(
                ReasonCode::WorktreeUnavailable,
                "the workspace is not a git repository; subagents need worktrees",
            ));
        }
    }
    match git(workspace, &["status", "--porcelain"]).await {
        Ok(output) if output.success && output.stdout.trim().is_empty() => None,
        Ok(output) if output.success => Some(ToolResult::refused(
            ReasonCode::WorkspaceDirty,
            "the workspace has uncommitted changes; commit or stash before spawning subagents",
        )),
        Ok(output) => Some(ToolResult::failed(
            ReasonCode::WorktreeUnavailable,
            format!("git status failed: {}", output.stderr.trim()),
        )),
        Err(detail) => Some(ToolResult::failed(ReasonCode::WorktreeUnavailable, detail)),
    }
}

/// The commit children branch from. `Err` is the typed failure to record.
pub(super) async fn head(workspace: &Path) -> Result<String, ToolResult> {
    match git(workspace, &["rev-parse", "HEAD"]).await {
        Ok(output) if output.success => Ok(output.stdout.trim().to_owned()),
        Ok(output) => Err(ToolResult::failed(
            ReasonCode::WorktreeUnavailable,
            format!("git rev-parse HEAD failed: {}", output.stderr.trim()),
        )),
        Err(detail) => Err(ToolResult::failed(ReasonCode::WorktreeUnavailable, detail)),
    }
}

/// Create the child's worktree and branch at `base`.
///
/// The owner marker is written *before* the worktree exists: it is a sibling
/// file, and this ordering means a crash can never leave a registered worktree
/// that [`cleanup_orphans`] cannot attribute to a process.
pub(super) async fn create(
    workspace: &Path,
    child: &ChildSpec,
    base: &str,
) -> Result<(), ChildRecord> {
    if let Some(parent_dir) = child.worktree.parent()
        && let Err(error) = tokio::fs::create_dir_all(parent_dir).await
    {
        return Err(ChildRecord::dispatch_failure(
            child.link.session,
            ReasonCode::WorktreeUnavailable,
            format!("cannot create worktree directory: {error}"),
        ));
    }
    let marker = serde_json::json!({
        "child_session": child.link.session.to_string(),
        "parent_session": child.link.parent.to_string(),
        "pid": std::process::id(),
    });
    let marker_file = marker_path(&child.worktree);
    if let Err(error) = tokio::fs::write(&marker_file, marker.to_string()).await {
        return Err(ChildRecord::dispatch_failure(
            child.link.session,
            ReasonCode::WorktreeUnavailable,
            format!("cannot write the worktree owner marker: {error}"),
        ));
    }
    let worktree = child.worktree.to_string_lossy().into_owned();
    let added = git(
        workspace,
        &["worktree", "add", "-b", &child.branch, &worktree, base],
    )
    .await;
    let failure = match added {
        Ok(output) if output.success => None,
        Ok(output) => Some(format!("git worktree add failed: {}", output.stderr.trim())),
        Err(detail) => Some(detail),
    };
    if let Some(detail) = failure {
        let _ = tokio::fs::remove_file(&marker_file).await;
        return Err(ChildRecord::dispatch_failure(
            child.link.session,
            ReasonCode::WorktreeUnavailable,
            detail,
        ));
    }
    Ok(())
}

/// Preserve uncommitted child work, then remove the worktree. The branch
/// survives; merging it is the parent's explicit act, never a side effect.
#[allow(
    clippy::cognitive_complexity,
    reason = "linear idempotent cleanup keeps preservation and removal ordering visible"
)]
pub(super) async fn finish(workspace: &Path, spec: &ChildSpec, record: &mut ChildRecord) {
    let worktree = &spec.worktree;
    let _ = tokio::fs::remove_file(marker_path(worktree)).await;
    if !worktree.exists() {
        return;
    }

    let dirty = match git(worktree, &["status", "--porcelain"]).await {
        Ok(output) if output.success => !output.stdout.trim().is_empty(),
        _ => false,
    };
    if dirty {
        let staged = git(worktree, &["add", "-A"]).await;
        let committed = match staged {
            Ok(output) if output.success => {
                git(
                    worktree,
                    &[
                        "-c",
                        "user.name=mjolnr",
                        "-c",
                        "user.email=mjolnr@localhost",
                        "commit",
                        "-m",
                        "mjolnr: preserve subagent work at settlement",
                    ],
                )
                .await
            }
            other => other,
        };
        match committed {
            Ok(output) if output.success => {
                if let Ok(head) = git(worktree, &["rev-parse", "HEAD"]).await
                    && head.success
                {
                    record.preserved_commit = Some(head.stdout.trim().to_owned());
                }
            }
            _ => {
                record
                    .notes
                    .push("uncommitted work could not be preserved; worktree kept".to_owned());
                return;
            }
        }
    }

    let removed = git(
        workspace,
        &["worktree", "remove", "--force", &worktree.to_string_lossy()],
    )
    .await;
    if !matches!(&removed, Ok(output) if output.success) {
        let _ = git(workspace, &["worktree", "prune"]).await;
        record
            .notes
            .push("worktree removal needed a prune; directory may remain".to_owned());
    }

    // Capture the paths this child's branch changed, workspace-relative. Diffed
    // against the parent HEAD in the parent workspace (not the removed worktree),
    // because the branch survives worktree removal. This is the write side of
    // cross-child read-set collision detection (Phase 5 Slice 5.2).
    let base = git(workspace, &["rev-parse", "HEAD"]).await;
    let tip = git(workspace, &["rev-parse", &spec.branch]).await;
    let (Ok(base), Ok(tip)) = (base, tip) else {
        return;
    };
    if !base.success || !tip.success {
        return;
    }
    if let Ok(touched) = git(
        workspace,
        &["diff", "--name-only", base.stdout.trim(), tip.stdout.trim()],
    )
    .await
        && touched.success
    {
        record.touched_paths = touched.stdout.lines().map(str::to_owned).collect();
    }
    // A branch identical to the base carries no work; keep the namespace tidy.
    if base.stdout.trim() == tip.stdout.trim() {
        let _ = git(workspace, &["branch", "-D", &spec.branch]).await;
        record.branch = None;
    }
}

/// Remove orphaned subagent worktrees whose owning mjolnr process is gone.
///
/// Conservative on purpose: a worktree whose marker names a live process is
/// left alone, whatever its session state — mjolnr cannot prove the owner is
/// done with it. Run on project open, detached from the actor.
pub async fn cleanup_orphans(workspace: PathBuf) {
    let Ok(listing) = git(&workspace, &["worktree", "list", "--porcelain"]).await else {
        return;
    };
    if !listing.success {
        return;
    }
    let namespace = std::env::temp_dir().join(WORKTREES_DIR);
    let namespace = tokio::fs::canonicalize(&namespace)
        .await
        .unwrap_or(namespace);
    for line in listing.stdout.lines() {
        let Some(path) = line.strip_prefix("worktree ") else {
            continue;
        };
        let path = PathBuf::from(path.trim());
        let path = tokio::fs::canonicalize(&path).await.unwrap_or(path);
        if !path.starts_with(&namespace) {
            continue;
        }
        let owner_alive = match tokio::fs::read_to_string(marker_path(&path)).await {
            Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|marker| marker.get("pid").and_then(serde_json::Value::as_u64))
                .is_some_and(process_alive),
            // No marker: either already half-cleaned or not this process's
            // bookkeeping; registration under mjolnr's namespace is itself the
            // orphan signal.
            Err(_) => false,
        };
        if owner_alive {
            continue;
        }
        let _ = git(
            &workspace,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        )
        .await;
        let _ = tokio::fs::remove_file(marker_path(&path)).await;
    }
    let _ = git(&workspace, &["worktree", "prune"]).await;
}

/// Whether a process id is alive, best effort.
fn process_alive(pid: u64) -> bool {
    if pid == u64::from(std::process::id()) {
        return true;
    }
    if Path::new("/proc").exists() {
        return Path::new(&format!("/proc/{pid}")).exists();
    }
    // Platforms without /proc (macOS): `kill -0` reports liveness without
    // sending a signal.
    std::process::Command::new("/bin/kill")
        .args(["-0", "--", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

struct GitOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run git with exact argv in `cwd`. Provider credentials are withheld the
/// same way `run_command` withholds them.
async fn git(cwd: &Path, arguments: &[&str]) -> Result<GitOutput, String> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear()
        .envs(crate::tools::sanitized_environment());
    let output = command
        .output()
        .await
        .map_err(|error| format!("cannot run git: {error}"))?;
    Ok(GitOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_are_namespaced_per_child() {
        let child = SessionId::new();
        let branch = branch_name(child);
        assert!(branch.starts_with("mjolnr/sub-"));
        assert_ne!(branch, branch_name(SessionId::new()));
    }

    #[test]
    fn markers_live_beside_the_worktree_not_inside_it() {
        let worktree = PathBuf::from("/tmp/mjolnr-worktrees/abc");
        let marker = marker_path(&worktree);
        assert_eq!(
            marker,
            PathBuf::from("/tmp/mjolnr-worktrees/abc.owner.json")
        );
        assert!(!marker.starts_with(&worktree));
    }
}
