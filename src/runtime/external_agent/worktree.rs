use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::core::error::ReasonCode;
use crate::core::event::SessionId;

const WORKTREES_DIR: &str = "smed-worktrees";

fn worktree_path(ext_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join(WORKTREES_DIR)
        .join(format!("ext-{ext_id}"))
}

fn branch_name(ext_id: &str) -> String {
    format!("smed/ext-{ext_id}")
}

async fn git(workspace: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(workspace);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env_clear();
    for (k, v) in sanitized_env() {
        cmd.env(k, v);
    }
    cmd.output().await.map_err(|e| e.to_string())
}

fn sanitized_env() -> Vec<(String, String)> {
    let allow = ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "TERM"];
    std::env::vars()
        .filter(|(k, _)| allow.contains(&k.as_str()))
        .collect()
}

pub async fn create(
    workspace: &Path,
    ext_id: &str,
) -> Result<(PathBuf, String), crate::core::error::SmedError> {
    let branch = branch_name(ext_id);
    let path = worktree_path(ext_id);
    let output = git(
        workspace,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &path.display().to_string(),
        ],
    )
    .await
    .map_err(|detail| {
        crate::core::error::SmedError::workspace_refused(ReasonCode::WorktreeUnavailable, detail)
    })?;
    if !output.status.success() {
        return Err(crate::core::error::SmedError::workspace_refused(
            ReasonCode::WorktreeUnavailable,
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok((path, branch))
}

pub async fn remove(
    workspace: &Path,
    worktree: &Path,
    branch: &str,
    force: bool,
) -> Result<(), String> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let wt = worktree.display().to_string();
    args.push(&wt);
    let _ = git(workspace, &args).await;
    let _ = git(workspace, &["branch", "-D", branch]).await;
    Ok(())
}

#[allow(dead_code)]
#[must_use]
pub fn ext_id_from_session(session: SessionId) -> String {
    let s = session.to_string();
    s.chars().take(8).collect()
}
