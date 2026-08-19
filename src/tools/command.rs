use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::message::{ToolEffect, ToolOutcome, ToolResult};
use crate::core::tool::{CommandSpec, Tool, ToolContext, ToolTier};

#[derive(Debug)]
pub struct RunCommand;

#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
    pub truncated: bool,
    pub timed_out: bool,
    pub cancelled: bool,
}

#[async_trait]
impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "Run one exact argv command at the workspace root with timeout and bounded output"
    }

    fn tier(&self) -> ToolTier {
        ToolTier::Execute
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "program": { "type": "string", "minLength": 1 },
                "arguments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": 256
                }
            },
            "required": ["program", "arguments"],
            "additionalProperties": false
        })
    }

    async fn preview(
        &self,
        arguments: &serde_json::Value,
        _context: &ToolContext,
    ) -> Result<String, ToolError> {
        Ok(parse_spec(arguments)?.display())
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let spec = parse_spec(&arguments)?;
        let workspace_root = revalidate_workspace(&context.workspace_root).await?;
        let result = run_process(
            Path::new(&spec.program),
            &spec.arguments,
            &workspace_root,
            context.command_timeout,
            context.max_output_bytes,
            cancel,
        )
        .await?;

        result_from_process(&result)
    }
}

/// Turn a finished process into the `ToolResult` a command-shaped tool returns.
///
/// Shared by [`RunCommand`] and the scripted extension tool (
/// ADR 0002) so a loaded extension's result is byte-for-byte what the same argv
/// would produce through `run_command` — the whole point of the scripted shim
/// is one execution surface, not two that can drift.
pub(crate) fn result_from_process(result: &ProcessOutput) -> Result<ToolResult, ToolError> {
    if result.cancelled {
        return Err(ToolError::Cancelled);
    }

    let exit = result
        .exit_code
        .map_or_else(|| "signal".to_owned(), |code| code.to_string());
    let mut text = format!("exit_code: {exit}\n");
    if !result.stdout.is_empty() {
        text.push_str("stdout:\n");
        text.push_str(&result.stdout);
    }
    if !result.stderr.is_empty() {
        text.push('\n');
        text.push_str("stderr:\n");
        text.push_str(&result.stderr);
    }
    if result.stdout.is_empty() && result.stderr.is_empty() {
        text.push_str("(no output)");
    }
    let success = result.exit_code == Some(0) && !result.timed_out;
    let effect = ToolEffect::Command {
        exit_code: result.exit_code,
        success,
        duration_ms: u64::try_from(result.duration.as_millis()).unwrap_or(u64::MAX),
    };

    if result.timed_out {
        return Ok(ToolResult {
            outcome: ToolOutcome::Failed(ReasonCode::CommandTimeout),
            content: text,
            truncated: result.truncated,
            effect,
            evidence_event_id: None,
        });
    }

    let outcome = if success {
        ToolOutcome::Ok
    } else {
        ToolOutcome::Failed(ReasonCode::ToolExecution)
    };
    Ok(ToolResult {
        outcome,
        content: text,
        truncated: result.truncated,
        effect,
        evidence_event_id: None,
    })
}

/// Re-resolve the command working directory immediately before spawning.
///
/// A resumed session validates its stored root, but the directory can still be
/// replaced by a symlink between resume and a later command. Commands are the
/// only built-in tool that hands the root itself to the operating system, so
/// they repeat the identity check at the effect boundary.
pub(crate) async fn revalidate_workspace(root: &Path) -> Result<PathBuf, ToolError> {
    let expected = root.to_path_buf();
    super::files::blocking(move || {
        let canonical = crate::policy::paths::canonical_root(&expected).map_err(|refusal| {
            ToolError::Refused {
                code: refusal.code,
                detail: refusal.detail,
            }
        })?;
        if canonical != expected {
            return Err(ToolError::Refused {
                code: ReasonCode::PathSymlinkEscape,
                detail: format!(
                    "workspace {} now resolves to {}; refusing to change command identity",
                    expected.display(),
                    canonical.display()
                ),
            });
        }
        Ok(canonical)
    })
    .await
}

pub(crate) fn parse_spec(arguments: &serde_json::Value) -> Result<CommandSpec, ToolError> {
    Ok(CommandSpec {
        program: super::arguments::required_string(arguments, "program")?,
        arguments: super::arguments::string_array(arguments, "arguments")?,
    })
}

pub(crate) async fn run_process(
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
    cancel: CancellationToken,
) -> Result<ProcessOutput, ToolError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear()
        .envs(sanitized_environment());
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(|error| ToolError::Execution {
        detail: format!("cannot start {}: {error}", program.display()),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| ToolError::Execution {
        detail: "command stdout was not captured".to_owned(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| ToolError::Execution {
        detail: "command stderr was not captured".to_owned(),
    })?;
    let budget = Arc::new(Mutex::new(CaptureBudget::new(output_limit)));
    let stdout_task = tokio::spawn(capture(stdout, Arc::clone(&budget)));
    let stderr_task = tokio::spawn(capture(stderr, Arc::clone(&budget)));
    let started = Instant::now();

    let mut timeout_sleep = Box::pin(tokio::time::sleep(timeout));
    let (status, timed_out, cancelled) = tokio::select! {
        status = child.wait() => (Some(status.map_err(|error| ToolError::Execution {
            detail: format!("cannot wait for {}: {error}", program.display()),
        })?), false, false),
        () = cancel.cancelled() => {
            let status = terminate_group(&mut child).await?;
            (status, false, true)
        }
        () = &mut timeout_sleep => {
            let status = terminate_group(&mut child).await?;
            (status, true, false)
        }
    };

    let stdout = join_capture(stdout_task).await?;
    let stderr = join_capture(stderr_task).await?;
    let capture = budget.lock().await;
    let truncated = capture.truncated;
    drop(capture);

    Ok(ProcessOutput {
        exit_code: status.and_then(|status| status.code()),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        duration: started.elapsed(),
        truncated,
        timed_out,
        cancelled,
    })
}

#[derive(Debug)]
struct CaptureBudget {
    remaining: usize,
    truncated: bool,
}

impl CaptureBudget {
    const fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            truncated: false,
        }
    }
}

async fn capture(
    mut stream: impl AsyncRead + Unpin,
    budget: Arc<Mutex<CaptureBudget>>,
) -> Result<Vec<u8>, std::io::Error> {
    let mut captured = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(captured);
        }
        let mut limit = budget.lock().await;
        let take = read.min(limit.remaining);
        if let Some(bytes) = chunk.get(..take) {
            captured.extend_from_slice(bytes);
        }
        limit.remaining = limit.remaining.saturating_sub(take);
        if take < read || limit.remaining == 0 {
            limit.truncated = true;
        }
    }
}

async fn join_capture(
    task: tokio::task::JoinHandle<Result<Vec<u8>, std::io::Error>>,
) -> Result<Vec<u8>, ToolError> {
    task.await
        .map_err(|error| ToolError::Execution {
            detail: format!("output capture task did not complete: {error}"),
        })?
        .map_err(|error| ToolError::Execution {
            detail: format!("cannot capture command output: {error}"),
        })
}

async fn terminate_group(
    child: &mut tokio::process::Child,
) -> Result<Option<std::process::ExitStatus>, ToolError> {
    let Some(pid) = child.id() else {
        return child
            .wait()
            .await
            .map(Some)
            .map_err(|error| ToolError::Execution {
                detail: format!("cannot wait for command: {error}"),
            });
    };

    #[cfg(unix)]
    {
        signal_group(pid, "-TERM").await;
        match tokio::time::timeout(Duration::from_millis(250), child.wait()).await {
            Ok(status) => {
                return status.map(Some).map_err(|error| ToolError::Execution {
                    detail: format!("cannot wait for terminated command: {error}"),
                });
            }
            Err(_) => signal_group(pid, "-KILL").await,
        }
    }

    #[cfg(not(unix))]
    let _ = pid;

    #[cfg(not(unix))]
    child.start_kill().map_err(|error| ToolError::Execution {
        detail: format!("cannot terminate command: {error}"),
    })?;

    child
        .wait()
        .await
        .map(Some)
        .map_err(|error| ToolError::Execution {
            detail: format!("cannot wait for killed command: {error}"),
        })
}

#[cfg(unix)]
async fn signal_group(pid: u32, signal: &str) {
    let mut kill = tokio::process::Command::new("/bin/kill");
    // The `--` is load-bearing: procps-ng kill parses a bare `-<pgid>` as an
    // option bundle and exits 0 without signalling anyone.
    kill.arg(signal)
        .arg("--")
        .arg(format!("-{pid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear()
        .kill_on_drop(true);
    let _ = kill.status().await;
}

/// Re-exported from [`crate::core::process`], which owns the rule. Three
/// modules spawn children — this one, `runtime::subagent`, and `repository` —
/// and a security rule with three copies has three chances to drift.
pub(crate) use crate::core::process::sanitized_environment;

pub(crate) fn find_program(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use crate::core::error::ReasonCode;

    use super::revalidate_workspace;

    #[test]
    fn process_output_uses_the_shared_utf8_truncator_for_display_contract() {
        let (text, truncated) = crate::tools::output::truncate("hello".to_owned(), 2);
        assert!(truncated);
        assert!(text.contains("truncated"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_rejects_a_workspace_replaced_after_context_creation() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("temporary parent");
        let root = parent.path().join("workspace");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&root).expect("workspace");
        std::fs::create_dir(&outside).expect("outside");
        let expected = std::fs::canonicalize(&root).expect("canonical workspace");

        std::fs::remove_dir(&root).expect("remove workspace");
        symlink(&outside, &root).expect("replace workspace with symlink");

        let error = revalidate_workspace(&expected)
            .await
            .expect_err("changed workspace identity must be refused");
        assert_eq!(error.reason_code(), ReasonCode::PathSymlinkEscape);
    }
}
