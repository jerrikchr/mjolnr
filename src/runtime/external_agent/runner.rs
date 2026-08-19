use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::process::Child;
use tokio_util::sync::CancellationToken;

use crate::core::client::external_agent::{ExternalAgentStatus, ExternalAgentView, TrustClass};

const MAX_SCROLLBACK_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ExternalAgentRecord {
    pub id: String,
    pub profile_name: String,
    pub executable: String,
    pub branch: String,
    pub worktree: String,
    pub trust: TrustClass,
    pub status: ExternalAgentStatus,
    pub started_at: String,
    pub scrollback: Arc<Mutex<Vec<u8>>>,
    pub scrollback_truncated: bool,
    child: Arc<tokio::sync::Mutex<Option<Child>>>,
    cancel: CancellationToken,
}

impl ExternalAgentRecord {
    #[must_use]
    pub fn to_view(&self) -> ExternalAgentView {
        ExternalAgentView {
            id: self.id.clone(),
            profile_name: self.profile_name.clone(),
            executable: self.executable.clone(),
            branch: self.branch.clone(),
            trust: TrustClass::ExternalUnverified,
            status: self.status.clone(),
            started_at: self.started_at.clone(),
        }
    }
}

#[derive(Debug)]
pub struct ExternalAgentRegistry {
    agents: BTreeMap<String, ExternalAgentRecord>,
}

impl ExternalAgentRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, record: ExternalAgentRecord) {
        let _ = self.agents.insert(record.id.clone(), record);
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ExternalAgentRecord> {
        self.agents.get(id)
    }

    #[must_use]
    pub fn get_mut(&mut self, id: &str) -> Option<&mut ExternalAgentRecord> {
        self.agents.get_mut(id)
    }

    #[allow(clippy::redundant_closure, reason = "stable method reference")]
    #[must_use]
    pub fn views(&self) -> Vec<ExternalAgentView> {
        let mut views: Vec<_> = self
            .agents
            .values()
            .map(ExternalAgentRecord::to_view)
            .collect();
        views.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        views
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.agents.len()
    }
}

impl Default for ExternalAgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn spawn_external_agent(
    profile: &crate::context::external_agent::ExternalAgentProfile,
    resolved_exe: &str,
    worktree: &str,
    ext_id: String,
    branch: String,
) -> Result<ExternalAgentRecord, String> {
    let args: Vec<String> = profile
        .args
        .iter()
        .map(|a| {
            if a == "{worktree}" {
                worktree.to_owned()
            } else {
                a.clone()
            }
        })
        .collect();

    let mut cmd = tokio::process::Command::new(resolved_exe);
    cmd.args(&args);
    cmd.current_dir(worktree);
    cmd.env_clear();
    for (k, v) in allowed_env() {
        cmd.env(k, v);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let scrollback: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sb_clone = Arc::clone(&scrollback);
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; 4096];
            let mut reader = stdout;
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut guard) = sb_clone.lock()
                            && guard.len() < MAX_SCROLLBACK_BYTES
                        {
                            let take = MAX_SCROLLBACK_BYTES.saturating_sub(guard.len()).min(n);
                            if let Some(slice) = buf.get(..take) {
                                guard.extend_from_slice(slice);
                            }
                        }
                    }
                }
            }
        });
    }
    let cancel = CancellationToken::new();
    let started_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    Ok(ExternalAgentRecord {
        id: ext_id,
        profile_name: profile.name.clone(),
        executable: resolved_exe.to_owned(),
        branch,
        worktree: worktree.to_owned(),
        trust: TrustClass::ExternalUnverified,
        status: ExternalAgentStatus::Running,
        started_at,
        scrollback,
        scrollback_truncated: false,
        child: Arc::new(tokio::sync::Mutex::new(Some(child))),
        cancel,
    })
}

fn allowed_env() -> Vec<(String, String)> {
    let allow = ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "TERM"];
    std::env::vars()
        .filter(|(k, _)| allow.contains(&k.as_str()))
        .collect()
}

pub async fn stop_agent(record: &mut ExternalAgentRecord) -> Result<(), String> {
    record.cancel.cancel();
    if let Some(mut child) = record.child.lock().await.take() {
        let _ = child.kill().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        record.status = ExternalAgentStatus::Stopped { exit_code: None };
    }
    Ok(())
}
