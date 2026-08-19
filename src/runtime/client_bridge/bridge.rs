//! `ClientBridge` handle and dispatch implementation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::core::client::{ClientCommand, ClientSnapshot, ClientUpdate};
use crate::core::error::ReasonCode;
use crate::core::runtime::SmedRuntime;

use super::command::command_to_smed;
use super::convert::snapshot_to_client;
use super::pump::pump_updates;

const CLIENT_UPDATE_CAPACITY: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum ClientBridgeError {
    #[error("{detail}")]
    InvalidInput {
        code: ReasonCode,
        field: &'static str,
        detail: String,
    },
    #[error("the runtime is closed")]
    RuntimeClosed,
    #[error("{detail}")]
    RuntimeRefused {
        code: Option<ReasonCode>,
        detail: String,
    },
    #[error("no client is listening")]
    ClientGone,
}

impl ClientBridgeError {
    #[must_use]
    pub const fn reason_code(&self) -> Option<ReasonCode> {
        match self {
            Self::InvalidInput { code, .. } => Some(*code),
            Self::RuntimeRefused { code, .. } => *code,
            Self::RuntimeClosed | Self::ClientGone => None,
        }
    }
}

#[derive(Debug)]
pub struct ClientBridge {
    runtime: Arc<dyn SmedRuntime>,
    sequence: Arc<AtomicU64>,
    updates: mpsc::Sender<ClientUpdate>,
    receiver: Mutex<Option<mpsc::Receiver<ClientUpdate>>>,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl ClientBridge {
    #[must_use]
    pub fn start(runtime: Arc<dyn SmedRuntime>) -> Self {
        Self::start_with_capacity(runtime, CLIENT_UPDATE_CAPACITY)
    }

    #[must_use]
    pub fn start_with_capacity(runtime: Arc<dyn SmedRuntime>, capacity: usize) -> Self {
        let (updates, receiver) = mpsc::channel(capacity.max(1));
        let sequence = Arc::new(AtomicU64::new(0));
        let pump = tokio::spawn(pump_updates(
            Arc::clone(&runtime),
            updates.clone(),
            Arc::clone(&sequence),
        ));
        Self {
            runtime,
            sequence,
            updates,
            receiver: Mutex::new(Some(receiver)),
            pump: Mutex::new(Some(pump)),
        }
    }

    pub async fn dispatch(&self, command: ClientCommand) -> Result<(), ClientBridgeError> {
        match command_to_smed(&command)? {
            Some(smed_command) => {
                self.runtime
                    .dispatch(smed_command)
                    .await
                    .map_err(|error| match error {
                        crate::core::error::SmedError::RuntimeClosed => {
                            ClientBridgeError::RuntimeClosed
                        }
                        other => ClientBridgeError::RuntimeRefused {
                            code: other.reason_code(),
                            detail: other.to_string(),
                        },
                    })
            }
            None => self.emit_snapshot().await,
        }
    }

    /// One page of a deterministic workspace search (Phase D4 client half).
    ///
    /// Speaks `ClientWorkspaceSearch*` on both sides, so the internal
    /// `core::store` types never reach a frontend. It is the last hop before
    /// TypeScript and therefore the place the wire bounds are applied: the
    /// filter's limit is clamped to `MAX_SEARCH_RESULTS_PER_PAGE` and every
    /// snippet to `MAX_SEARCH_SNIPPET_BYTES`, both again, because the store's
    /// own bounds are separate constants it cannot share (see
    /// `search_page_to_client`).
    ///
    /// A filter this bridge cannot type is refused with `SCHEMA_INVALID` before
    /// the store is asked. An unparseable `project_id` dropped instead of
    /// refused would widen the query past the scope the caller named.
    pub async fn search_workspace(
        &self,
        filter: crate::core::client::types::ClientWorkspaceSearchFilter,
    ) -> Result<crate::core::client::types::ClientWorkspaceSearchPage, ClientBridgeError> {
        let filter = super::workspace::search_filter_from_client(&filter).map_err(|refusal| {
            ClientBridgeError::InvalidInput {
                code: ReasonCode::SchemaInvalid,
                field: "filter",
                detail: refusal.message,
            }
        })?;

        let page = self
            .runtime
            .search_workspace(filter)
            .await
            .map_err(|error| ClientBridgeError::RuntimeRefused {
                code: error.reason_code(),
                detail: error.to_string(),
            })?;

        Ok(super::workspace::search_page_to_client(page))
    }

    /// One page of one directory of the open project (Phase D7 client half).
    ///
    /// Speaks the wire DTOs on the way out, so `core::workspace_files` never
    /// reaches a frontend, and applies the wire bounds on the way in: a path
    /// longer than `MAX_WORKSPACE_FILE_PATH_BYTES` is refused here rather than
    /// inside a syscall, and the page size is smed's, not the caller's, so a
    /// client cannot ask for a page larger than the projection will carry.
    pub async fn list_directory(
        &self,
        path: &str,
        page: u32,
    ) -> Result<crate::core::client::workspace::DirectoryPage, ClientBridgeError> {
        let path = validate_file_path(path)?;
        let answer = self
            .runtime
            .read_workspace_files(
                crate::core::workspace_files::WorkspaceFileRequest::Directory {
                    path,
                    page,
                    page_size: crate::core::client::workspace::MAX_DIRECTORY_ENTRIES_PER_PAGE,
                },
            )
            .await
            .map_err(refused)?;

        match answer {
            crate::core::workspace_files::WorkspaceFileAnswer::Directory(listing) => {
                Ok(super::workspace::project_directory_page(&listing))
            }
            // The runtime answered a different question than the one asked.
            // A typed refusal rather than a panic: this is a routing bug
            // between two halves that ship together, and a crash in a dispatch
            // path is fail-open (AGENTS.md §3).
            crate::core::workspace_files::WorkspaceFileAnswer::File(_) => {
                Err(ClientBridgeError::RuntimeRefused {
                    code: Some(ReasonCode::SchemaInvalid),
                    detail: "the runtime answered a directory request with a file".to_owned(),
                })
            }
        }
    }

    /// One file of the open project, and whether an editor may have it.
    pub async fn open_file(
        &self,
        path: &str,
    ) -> Result<crate::core::client::workspace::FileOpenView, ClientBridgeError> {
        let path = validate_file_path(path)?;
        let answer = self
            .runtime
            .read_workspace_files(crate::core::workspace_files::WorkspaceFileRequest::File { path })
            .await
            .map_err(refused)?;

        match answer {
            crate::core::workspace_files::WorkspaceFileAnswer::File(read) => {
                Ok(super::workspace::project_file_open(&read))
            }
            crate::core::workspace_files::WorkspaceFileAnswer::Directory(_) => {
                Err(ClientBridgeError::RuntimeRefused {
                    code: Some(ReasonCode::SchemaInvalid),
                    detail: "the runtime answered a file request with a directory".to_owned(),
                })
            }
        }
    }

    /// Build one bounded, read-only view of the deterministic code graph.
    ///
    /// The graph is rebuilt on a blocking thread from the runtime-owned
    /// workspace root. The client supplies only a relative focus path and
    /// traversal shape; it cannot provide a filesystem root or an authority
    /// claim.
    pub async fn query_graph(
        &self,
        query: crate::core::client::graph::ClientGraphQuery,
    ) -> Result<crate::core::client::graph::ClientGraphPage, ClientBridgeError> {
        let root =
            self.runtime
                .snapshot()
                .workspace_root
                .ok_or(ClientBridgeError::RuntimeRefused {
                    code: Some(ReasonCode::WorkspaceCapabilityUnavailable),
                    detail: "the code graph needs an open workspace".to_owned(),
                })?;
        let query = validate_graph_query(query)?;
        let answer = tokio::task::spawn_blocking(move || super::graph::build_page(&root, query))
            .await
            .map_err(|error| ClientBridgeError::RuntimeRefused {
                code: None,
                detail: format!("code graph worker failed: {error}"),
            })?
            .map_err(|error| ClientBridgeError::RuntimeRefused {
                code: Some(ReasonCode::WorkspaceSearchRefused),
                detail: error.to_string(),
            })?;
        Ok(answer)
    }

    /// Read the board: what can be decided right now, and why the rest is
    /// fogged (Phase E5, step 3).
    ///
    /// A pure query — the runtime computes the cross-session projection and
    /// this bridge maps it to wire shape, applying the wire bounds after the
    /// last transformation. The client cannot influence which sessions or
    /// records participate.
    pub async fn query_board(
        &self,
    ) -> Result<crate::core::client::board::ClientBoardOverview, ClientBridgeError> {
        let overview = self.runtime.query_board().await.map_err(refused)?;
        super::board::board_overview_to_client(&overview)
    }

    /// Read a bounded newest-first repository history. This is a direct query,
    /// not snapshot state: opening a history drawer must not make every
    /// snapshot carry an unbounded list that most surfaces never render.
    pub async fn query_repository_history(
        &self,
        limit: u32,
    ) -> Result<crate::core::client::workspace::RepositoryHistory, ClientBridgeError> {
        if !(1..=crate::core::repository::MAX_HISTORY_ENTRIES).contains(&limit) {
            return Err(ClientBridgeError::InvalidInput {
                code: ReasonCode::SchemaInvalid,
                field: "limit",
                detail: format!(
                    "history limit must be between 1 and {}",
                    crate::core::repository::MAX_HISTORY_ENTRIES
                ),
            });
        }
        let history = self
            .runtime
            .query_repository_history(limit)
            .await
            .map_err(refused)?;
        Ok(super::workspace::project_repository_history(
            &history, limit,
        ))
    }

    pub fn take_updates(&self) -> Option<mpsc::Receiver<ClientUpdate>> {
        self.receiver.lock().ok().and_then(|mut slot| slot.take())
    }

    #[must_use]
    pub fn snapshot(&self) -> ClientSnapshot {
        snapshot_to_client(
            self.sequence.fetch_add(1, Ordering::Relaxed),
            &self.runtime.snapshot(),
        )
    }

    pub async fn close(&self) -> Result<(), crate::core::error::SmedError> {
        self.runtime.close().await
    }

    async fn emit_snapshot(&self) -> Result<(), ClientBridgeError> {
        self.updates
            .send(ClientUpdate::Snapshot {
                snapshot: self.snapshot(),
            })
            .await
            .map_err(|_| ClientBridgeError::ClientGone)
    }
}

/// Bound and normalise a path before it crosses into the runtime (Phase D7).
///
/// The empty string is legal and means the project root, which is why this
/// cannot simply reject empty input the way `OpenProject` does. What it refuses
/// is a path too long to be a path and a path carrying a NUL byte — the second
/// because a NUL truncates a C string, so a value that reads as `src/a.rs\0/..`
/// on the wire could reach a syscall as `src/a.rs` and defeat the containment
/// check applied to the whole string.
fn validate_file_path(path: &str) -> Result<String, ClientBridgeError> {
    use crate::core::client::workspace::MAX_WORKSPACE_FILE_PATH_BYTES;

    if path.len() > MAX_WORKSPACE_FILE_PATH_BYTES {
        return Err(ClientBridgeError::InvalidInput {
            code: ReasonCode::SchemaInvalid,
            field: "path",
            detail: format!(
                "a workspace path may be at most {MAX_WORKSPACE_FILE_PATH_BYTES} bytes"
            ),
        });
    }
    if path.contains('\0') {
        return Err(ClientBridgeError::InvalidInput {
            code: ReasonCode::SchemaInvalid,
            field: "path",
            detail: "a workspace path may not contain a NUL byte".to_owned(),
        });
    }
    Ok(path.to_owned())
}

fn validate_graph_query(
    mut query: crate::core::client::graph::ClientGraphQuery,
) -> Result<crate::core::client::graph::ClientGraphQuery, ClientBridgeError> {
    if let Some(path) = query.path.as_mut() {
        *path = validate_file_path(path)?;
        if path.starts_with('/') || path.contains("..") {
            return Err(ClientBridgeError::InvalidInput {
                code: ReasonCode::PathOutsideWorkspace,
                field: "path",
                detail: "a graph focus path must be workspace-relative".to_owned(),
            });
        }
    }
    query.depth = query.depth.min(crate::graph::MAX_DEPTH);
    Ok(query)
}

fn refused(error: crate::core::error::SmedError) -> ClientBridgeError {
    match error {
        crate::core::error::SmedError::RuntimeClosed => ClientBridgeError::RuntimeClosed,
        other => ClientBridgeError::RuntimeRefused {
            code: other.reason_code(),
            detail: other.to_string(),
        },
    }
}

impl Drop for ClientBridge {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.pump.lock()
            && let Some(pump) = slot.take()
        {
            pump.abort();
        }
    }
}
