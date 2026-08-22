//! Desktop composition for isolated, selectable project runtimes.
//!
//! The coordinator owns context lifetime and routing only. Policy, persistence,
//! leases, recovery, providers, and filesystem identity remain runtime-owned.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mjolnr::context::{DiscoveryConfig, ProjectContext};
use mjolnr::core::client::workspace::{DirectoryPage, FileOpenView, RepositoryHistory};
use mjolnr::core::client::{ClientCommand, ClientSnapshot, ClientUpdate};
use mjolnr::core::client::{ClientWorkspaceSearchFilter, ClientWorkspaceSearchPage};
use mjolnr::core::error::ReasonCode;
use mjolnr::core::provider::Provider;
use mjolnr::core::store::EventStore;
use mjolnr::runtime::client_bridge::{ClientBridge, ClientBridgeError};
use mjolnr::runtime::Runtime;
use serde::Serialize;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

const UPDATE_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextTaggedUpdate {
    pub context_id: String,
    pub sequence: u64,
    pub update: ClientUpdate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectSummary {
    pub context_id: String,
    pub root: String,
    pub selected: bool,
    pub session: Option<String>,
    pub run_active: bool,
    pub approval_pending: bool,
    pub recovery_required: bool,
}

#[derive(Debug)]
struct ProjectContextHandle {
    id: String,
    bridge: Arc<ClientBridge>,
}

#[derive(Debug)]
pub struct RuntimeCoordinator {
    contexts: Mutex<BTreeMap<String, Arc<ProjectContextHandle>>>,
    selected: RwLock<Option<String>>,
    updates: mpsc::Sender<ContextTaggedUpdate>,
    receiver: Mutex<Option<mpsc::Receiver<ContextTaggedUpdate>>>,
    sequence: Arc<AtomicU64>,
    initial_snapshot: ClientSnapshot,
    forwarders: Mutex<Vec<JoinHandle<()>>>,
    store: Arc<dyn EventStore>,
    providers: Vec<Arc<dyn Provider>>,
}

impl RuntimeCoordinator {
    pub(crate) fn from_initial(
        initial_id: String,
        bridge: Arc<ClientBridge>,
        store: Arc<dyn EventStore>,
        providers: Vec<Arc<dyn Provider>>,
    ) -> Arc<Self> {
        let (updates, receiver) = mpsc::channel(UPDATE_CAPACITY);
        let initial_snapshot = bridge.snapshot();
        let context = Arc::new(ProjectContextHandle {
            id: initial_id.clone(),
            bridge,
        });
        let mut contexts = BTreeMap::new();
        contexts.insert(initial_id.clone(), context.clone());
        let coordinator = Arc::new(Self {
            contexts: Mutex::new(contexts),
            selected: RwLock::new(Some(initial_id.clone())),
            updates,
            receiver: Mutex::new(Some(receiver)),
            sequence: Arc::new(AtomicU64::new(0)),
            initial_snapshot,
            forwarders: Mutex::new(Vec::new()),
            store,
            providers,
        });
        let coordinator_for_forwarding = Arc::clone(&coordinator);
        tokio::spawn(async move {
            coordinator_for_forwarding.attach_context(context).await;
        });
        coordinator
    }

    pub(crate) fn take_tagged_updates(&self) -> Option<mpsc::Receiver<ContextTaggedUpdate>> {
        self.receiver
            .try_lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    pub fn take_updates(&self) -> Option<mpsc::Receiver<ClientUpdate>> {
        let mut tagged = self.take_tagged_updates()?;
        let (updates, receiver) = mpsc::channel(UPDATE_CAPACITY);
        tokio::spawn(async move {
            while let Some(update) = tagged.recv().await {
                if updates.send(update.update).await.is_err() {
                    break;
                }
            }
        });
        Some(receiver)
    }

    pub async fn dispatch(&self, command: ClientCommand) -> Result<(), ClientBridgeError> {
        if let ClientCommand::OpenProject { root } = command {
            self.open_project(root).await.map(|_| ())
        } else {
            self.selected_bridge().await?.dispatch(command).await
        }
    }

    pub(crate) async fn open_project(&self, root: String) -> Result<String, ClientBridgeError> {
        let requested = PathBuf::from(root.trim());
        if requested.as_os_str().is_empty() {
            return Err(refused(
                ReasonCode::PathOutsideWorkspace,
                "a project root is required",
            ));
        }
        let canonical = tokio::task::spawn_blocking(move || std::fs::canonicalize(requested))
            .await
            .map_err(|error| {
                refused(
                    ReasonCode::PathOutsideWorkspace,
                    format!("project canonicalization failed: {error}"),
                )
            })?
            .map_err(|error| {
                refused(
                    ReasonCode::PathOutsideWorkspace,
                    format!("project root is unavailable: {error}"),
                )
            })?;
        if !canonical.is_dir() {
            return Err(refused(
                ReasonCode::PathOutsideWorkspace,
                "project root is not a directory",
            ));
        }
        let id = canonical.to_string_lossy().into_owned();
        let selected_id = self.selected.read().await.clone();
        let selected_context = selected_id.and_then(|selected| {
            self.contexts
                .try_lock()
                .ok()
                .and_then(|contexts| contexts.get(&selected).cloned())
        });
        match selected_context {
            Some(context) if context.bridge.snapshot().workspace_root.is_none() => {
                context
                    .bridge
                    .dispatch(ClientCommand::OpenProject { root: id })
                    .await?;
                return Ok(context.id.clone());
            }
            _ => {}
        }
        match self.contexts.lock().await.get(&id).cloned() {
            Some(existing)
                if existing.bridge.snapshot().workspace_root.as_deref() == Some(id.as_str()) =>
            {
                self.select_project(id.clone()).await?;
                return Ok(id);
            }
            _ => {}
        }

        let discovery = DiscoveryConfig::for_workspace(canonical.clone()).map_err(|error| {
            refused(
                ReasonCode::PathOutsideWorkspace,
                format!("discovery config refused: {error}"),
            )
        })?;
        let project_context =
            tokio::task::spawn_blocking(move || ProjectContext::discover(discovery))
                .await
                .map_err(|error| {
                    refused(
                        ReasonCode::PathOutsideWorkspace,
                        format!("discovery worker failed: {error}"),
                    )
                })?
                .map_err(|error| {
                    refused(
                        ReasonCode::PathOutsideWorkspace,
                        format!("project discovery refused: {error}"),
                    )
                })?;
        let runtime = Runtime::spawn_with_tools_project_context_and_triggers(
            self.providers.clone(),
            Arc::clone(&self.store),
            mjolnr::tools::ToolRegistry::default(),
            project_context,
            Arc::new(Vec::new()),
            Arc::new(Vec::new()),
            Arc::new(mjolnr::core::routing::RouteTable::default()),
        );
        let bridge: Arc<ClientBridge> = Arc::new(ClientBridge::start(Arc::new(runtime)));
        let bridge_for_open = Arc::clone(&bridge);
        self.add_context_handle(Arc::new(ProjectContextHandle {
            id: id.clone(),
            bridge,
        }))
        .await;
        bridge_for_open
            .dispatch(ClientCommand::OpenProject { root: id.clone() })
            .await?;
        self.select_project(id.clone()).await?;
        Ok(id)
    }

    pub(crate) async fn select_project(&self, context_id: String) -> Result<(), ClientBridgeError> {
        if !self.contexts.lock().await.contains_key(&context_id) {
            return Err(refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "project context is not open",
            ));
        }
        *self.selected.write().await = Some(context_id.clone());
        self.emit_snapshot(&context_id).await
    }

    pub(crate) async fn list_projects(&self) -> Vec<ProjectSummary> {
        let selected = self.selected.read().await.clone();
        let contexts = self.contexts.lock().await.clone();
        contexts
            .values()
            .filter_map(|context| {
                let snapshot = context.bridge.snapshot();
                let root = snapshot.workspace_root.clone()?;
                Some(ProjectSummary {
                    context_id: context.id.clone(),
                    root,
                    selected: selected.as_deref() == Some(context.id.as_str()),
                    session: snapshot.session,
                    run_active: snapshot.run_active,
                    approval_pending: snapshot.pending_approval.is_some(),
                    recovery_required: matches!(
                        snapshot.recovery,
                        mjolnr::core::client::types::ClientRecovery::Required { .. }
                    ),
                })
            })
            .collect()
    }

    pub fn snapshot(&self) -> ClientSnapshot {
        self.selected
            .try_read()
            .ok()
            .and_then(|selected| selected.clone())
            .and_then(|id| self.contexts.try_lock().ok()?.get(&id).cloned())
            .map_or_else(
                || self.cached_snapshot(),
                |context| context.bridge.snapshot(),
            )
    }

    pub(crate) async fn search_workspace(
        &self,
        filter: ClientWorkspaceSearchFilter,
    ) -> Result<ClientWorkspaceSearchPage, ClientBridgeError> {
        self.selected_bridge().await?.search_workspace(filter).await
    }

    pub(crate) async fn list_directory(
        &self,
        path: &str,
        page: u32,
    ) -> Result<DirectoryPage, ClientBridgeError> {
        self.selected_bridge()
            .await?
            .list_directory(path, page)
            .await
    }

    pub(crate) async fn open_file(&self, path: &str) -> Result<FileOpenView, ClientBridgeError> {
        self.selected_bridge().await?.open_file(path).await
    }

    pub(crate) async fn query_graph(
        &self,
        query: mjolnr::core::client::graph::ClientGraphQuery,
    ) -> Result<mjolnr::core::client::graph::ClientGraphPage, ClientBridgeError> {
        self.selected_bridge().await?.query_graph(query).await
    }

    pub(crate) async fn query_graph_status(
        &self,
    ) -> Result<mjolnr::core::client::graph::ClientGraphStatus, ClientBridgeError> {
        Ok(self.selected_bridge().await?.graph_status())
    }

    pub(crate) async fn query_board(
        &self,
    ) -> Result<mjolnr::core::client::board::ClientBoardOverview, ClientBridgeError> {
        self.selected_bridge().await?.query_board().await
    }

    pub(crate) async fn query_repository_history(
        &self,
        limit: u32,
    ) -> Result<RepositoryHistory, ClientBridgeError> {
        self.selected_bridge()
            .await?
            .query_repository_history(limit)
            .await
    }

    pub async fn close(&self) -> Result<(), mjolnr::core::error::MjolnrError> {
        let contexts = self
            .contexts
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for context in contexts {
            match context.bridge.close().await {
                Err(error) if first_error.is_none() => first_error = Some(error),
                _ => {}
            }
        }
        let forwarders = self.forwarders.lock().await.drain(..).collect::<Vec<_>>();
        for forwarder in forwarders {
            let _ = forwarder.await;
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn selected_bridge(&self) -> Result<Arc<ClientBridge>, ClientBridgeError> {
        let selected = self.selected.read().await.clone().ok_or_else(|| {
            refused(
                ReasonCode::WorkspaceCapabilityUnavailable,
                "no project context is selected",
            )
        })?;
        self.contexts
            .lock()
            .await
            .get(&selected)
            .map(|context| Arc::clone(&context.bridge))
            .ok_or_else(|| {
                refused(
                    ReasonCode::WorkspaceCapabilityUnavailable,
                    "selected project context is unavailable",
                )
            })
    }

    async fn add_context_handle(&self, context: Arc<ProjectContextHandle>) {
        self.contexts
            .lock()
            .await
            .insert(context.id.clone(), context.clone());
        self.attach_context(context).await;
    }

    async fn attach_context(&self, context: Arc<ProjectContextHandle>) {
        let bridge = Arc::clone(&context.bridge);
        let context_id = context.id.clone();
        let updates = self.updates.clone();
        let sequence = Arc::clone(&self.sequence);
        let forwarder = tokio::spawn(async move {
            let Some(mut receiver) = bridge.take_updates() else {
                return;
            };
            while let Some(update) = receiver.recv().await {
                let tagged = ContextTaggedUpdate {
                    context_id: context_id.clone(),
                    sequence: sequence.fetch_add(1, Ordering::Relaxed),
                    update,
                };
                if updates.send(tagged).await.is_err() {
                    break;
                }
            }
        });
        self.forwarders.lock().await.push(forwarder);
    }

    fn cached_snapshot(&self) -> ClientSnapshot {
        self.initial_snapshot.clone()
    }

    async fn emit_snapshot(&self, context_id: &str) -> Result<(), ClientBridgeError> {
        let context = self
            .contexts
            .lock()
            .await
            .get(context_id)
            .cloned()
            .ok_or_else(|| {
                refused(
                    ReasonCode::WorkspaceCapabilityUnavailable,
                    "project context is unavailable",
                )
            })?;
        self.updates
            .send(ContextTaggedUpdate {
                context_id: context_id.to_owned(),
                sequence: self.sequence.fetch_add(1, Ordering::Relaxed),
                update: ClientUpdate::Snapshot {
                    snapshot: context.bridge.snapshot(),
                },
            })
            .await
            .map_err(|_| ClientBridgeError::ClientGone)
    }
}

fn refused(code: ReasonCode, detail: impl Into<String>) -> ClientBridgeError {
    ClientBridgeError::RuntimeRefused {
        code: Some(code),
        detail: detail.into(),
    }
}
