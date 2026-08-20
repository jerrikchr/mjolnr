//! Subprocess stdio transport with JSON-RPC 2.0 framing and scrubbed environment (ADR-0016).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use crate::core::error::{ReasonCode, ToolError};
use crate::core::process::sanitized_environment;
use crate::plugins::jsonrpc::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

type ResponseMap = Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<JsonRpcResponse, String>>>>>;

/// Transport managing the stdio pipes and request-response matching for a plugin process.
#[derive(Debug)]
pub struct PluginTransport {
    child: Option<Child>,
    stdin: BufWriter<ChildStdin>,
    next_id: AtomicU64,
    pending: ResponseMap,
    reader_handle: tokio::task::JoinHandle<()>,
    cancel: CancellationToken,
}

impl PluginTransport {
    /// Spawn the plugin subprocess with a scrubbed environment and explicit credential injection.
    ///
    /// # Security (ADR-0016 §4, AGENTS.md §3)
    /// - `env_clear()` + `sanitized_environment()` removes all provider API keys.
    /// - Only explicitly granted credentials from `granted_credentials` are injected.
    pub fn spawn(
        program: &str,
        arguments: &[String],
        cwd: &Path,
        granted_credentials: &BTreeMap<String, String>,
        cancel: CancellationToken,
    ) -> Result<Self, ToolError> {
        let mut cmd = Command::new(program);
        cmd.args(arguments)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env_clear()
            .envs(sanitized_environment());

        for (key, value) in granted_credentials {
            cmd.env(key, value);
        }

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|error| ToolError::Execution {
            detail: format!("cannot start plugin process `{program}`: {error}"),
        })?;

        let stdin = child.stdin.take().ok_or_else(|| ToolError::Execution {
            detail: "cannot capture plugin stdin".to_owned(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::Execution {
            detail: "cannot capture plugin stdout".to_owned(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| ToolError::Execution {
            detail: "cannot capture plugin stderr".to_owned(),
        })?;

        // Stderr must be drained, not merely piped: an unread pipe fills its
        // OS buffer (~64 KiB) and then blocks the plugin's next write to
        // stderr forever, which surfaces as a hung plugin. Drained line-wise
        // and discarded — plugin diagnostics have no route into mjolnr's own
        // output (stdout is the alternate screen) and no tracing subscriber
        // exists to file them yet.
        tokio::spawn(async move {
            let mut stderr_lines = BufReader::new(stderr).lines();
            while let Ok(Some(_line)) = stderr_lines.next_line().await {
                // Intentionally discarded; see the comment above.
            }
        });

        let pending: ResponseMap = Arc::new(Mutex::new(BTreeMap::new()));
        let pending_clone = Arc::clone(&pending);

        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    let mut lock = pending_clone.lock().await;
                    if let Some(tx) = lock.remove(&response.id) {
                        let _ = tx.send(Ok(response));
                    }
                }
            }
        });

        Ok(Self {
            child: Some(child),
            stdin: BufWriter::new(stdin),
            next_id: AtomicU64::new(1),
            pending,
            reader_handle,
            cancel,
        })
    }

    /// Send a JSON-RPC request and wait for the response with a timeout.
    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, ToolError> {
        self.call_with_cancel(method, params, timeout, CancellationToken::new())
            .await
    }

    /// Send a JSON-RPC request and wait for the response with a timeout and per-call cancellation token.
    pub async fn call_with_cancel(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);
        let mut line = serde_json::to_string(&request).map_err(|error| ToolError::Execution {
            detail: format!("cannot serialize request: {error}"),
        })?;
        line.push('\n');

        let (tx, rx) = oneshot::channel();
        {
            let mut lock = self.pending.lock().await;
            lock.insert(id, tx);
        }

        if let Err(error) = self.stdin.write_all(line.as_bytes()).await {
            let mut lock = self.pending.lock().await;
            lock.remove(&id);
            return Err(ToolError::Execution {
                detail: format!("cannot write to plugin stdin: {error}"),
            });
        }
        if let Err(error) = self.stdin.flush().await {
            let mut lock = self.pending.lock().await;
            lock.remove(&id);
            return Err(ToolError::Execution {
                detail: format!("cannot flush plugin stdin: {error}"),
            });
        }

        tokio::select! {
            () = self.cancel.cancelled() => {
                let mut lock = self.pending.lock().await;
                lock.remove(&id);
                Err(ToolError::Refused {
                    code: ReasonCode::Cancelled,
                    detail: "plugin call was cancelled".to_owned(),
                })
            }
            () = cancel.cancelled() => {
                let mut lock = self.pending.lock().await;
                lock.remove(&id);
                Err(ToolError::Refused {
                    code: ReasonCode::Cancelled,
                    detail: "plugin call was cancelled".to_owned(),
                })
            }
            () = tokio::time::sleep(timeout) => {
                let mut lock = self.pending.lock().await;
                lock.remove(&id);
                Err(ToolError::Refused {
                    code: ReasonCode::CommandTimeout,
                    detail: format!("plugin call `{method}` timed out after {timeout:?}"),
                })
            }
            response = rx => {
                let response = response.map_err(|_| ToolError::Execution {
                    detail: "plugin process closed stdout before responding".to_owned(),
                })?.map_err(|detail| ToolError::Execution { detail })?;

                if let Some(err) = response.error {
                    return Err(ToolError::Execution {
                        detail: format!("plugin returned error {}: {}", err.code, err.message),
                    });
                }
                Ok(response.result.unwrap_or(serde_json::Value::Null))
            }
        }
    }

    /// Send a fire-and-forget JSON-RPC notification.
    pub async fn notify(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), ToolError> {
        let notification = JsonRpcNotification::new(method, params);
        let mut line =
            serde_json::to_string(&notification).map_err(|error| ToolError::Execution {
                detail: format!("cannot serialize notification: {error}"),
            })?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| ToolError::Execution {
                detail: format!("cannot write notification to plugin stdin: {error}"),
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|error| ToolError::Execution {
                detail: format!("cannot flush plugin stdin: {error}"),
            })?;
        Ok(())
    }

    /// Cleanly shut down the plugin subprocess.
    pub async fn shutdown(&mut self) -> Result<(), ToolError> {
        let _ = self.notify("shutdown", serde_json::json!({})).await;
        self.reader_handle.abort();
        if let Some(mut child) = self.child.take() {
            let _ = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
            let _ = child.kill().await;
        }
        Ok(())
    }
}

impl Drop for PluginTransport {
    fn drop(&mut self) {
        self.reader_handle.abort();
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}
