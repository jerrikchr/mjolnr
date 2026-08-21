//! Tauri 2 backend composition glue for mjolnr desktop client (Phase A0).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::doc_markdown)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::io::Write as _;

use serde::{Deserialize, Serialize};
use mjolnr::context::{DiscoveryConfig, ProjectContext};
use mjolnr::core::client::{ClientCommand, ClientSnapshot};
use mjolnr::core::client::workspace::ClientEditorPreferences;
use mjolnr::core::client::terminal::{
    ClientTerminalInput, ClientTerminalLayout, ClientTerminalResize, ClientTerminalScroll,
    ClientTerminalSearch, ClientTerminalSnapshot,
};
use mjolnr::core::model::ProviderId;
use mjolnr::core::provider::Provider;
use mjolnr::core::routing::RouteTable;
use mjolnr::core::runtime::MjolnrRuntime;
use mjolnr::core::secrets::{Credential, Secret, SecretStore};
use mjolnr::core::store::EventStore;
use mjolnr::providers::anthropic::AnthropicProvider;
use mjolnr::providers::openai::OpenAiProvider;
use mjolnr::providers::openai_codex::OpenAiCodexProvider;
use mjolnr::providers::{gemini, ollama, openrouter};
use mjolnr::runtime::Runtime;
use mjolnr::runtime::client_bridge::ClientBridge;
use mjolnr::runtime::terminal::TerminalManager;
use mjolnr::store::secrets::OsSecretStore;
use mjolnr::store::sqlite::SqliteEventStore;
use tauri::{AppHandle, Emitter, State};
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "type", content = "detail", rename_all = "camelCase")]
pub enum DesktopBridgeError {
    #[error("initialization failed: {0}")]
    Initialization(String),
    /// A typed runtime refusal on its way to the frontend.
    ///
    /// This replaced a `Dispatch(String)` variant that flattened the refusal
    /// into prose. The reason code is the stable contract (AGENTS.md §6), and
    /// that flattening is the one transformation the other side cannot undo —
    /// the frontend was left with a sentence it could only print. `code` is
    /// `Option` because `ClientBridgeError::reason_code` is: a closed runtime
    /// has no refusal code, and inventing one would put a contract string on a
    /// condition that never earned it.
    #[error("{message}")]
    Refused {
        code: Option<String>,
        message: String,
    },
    #[error("bridge disconnected")]
    Disconnected,
}

pub struct AppState {
    pub bridge: Arc<ClientBridge>,
    pub terminal: Arc<TerminalManager>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OnboardingDraft {
    pub root: String,
    pub soul: String,
    pub user_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OnboardingFileStatus {
    pub path: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OnboardingPreview {
    pub root: String,
    pub files: Vec<OnboardingFileStatus>,
}

fn onboarding_files(draft: &OnboardingDraft) -> Vec<mjolnr::routing::scaffold::ScaffoldFile> {
    let selections = mjolnr::cli::onboard::Selections {
        routes: Vec::new(),
        soul: Some(draft.soul.clone()),
        user_profile: draft.user_profile.clone(),
        mcp_servers: Vec::new(),
    };
    mjolnr::cli::onboard::plan_files(&selections)
}

fn onboarding_root(draft: &OnboardingDraft) -> Result<PathBuf, DesktopBridgeError> {
    mjolnr::policy::paths::canonical_root(Path::new(draft.root.trim())).map_err(|refusal| {
        DesktopBridgeError::Refused {
            code: Some(refusal.code.as_str().to_owned()),
            message: refusal.detail,
        }
    })
}

fn onboarding_status(
    root: &Path,
    files: &[mjolnr::routing::scaffold::ScaffoldFile],
) -> Result<Vec<OnboardingFileStatus>, DesktopBridgeError> {
    files
        .iter()
        .map(|file| {
            let target = mjolnr::policy::paths::for_write(root, &file.relative_path).map_err(
                |refusal| DesktopBridgeError::Refused {
                    code: Some(refusal.code.as_str().to_owned()),
                    message: refusal.detail,
                },
            )?;
            Ok(OnboardingFileStatus {
                path: file.relative_path.to_string_lossy().into_owned(),
                action: if target.exists() {
                    "preserve".to_owned()
                } else {
                    "write".to_owned()
                },
            })
        })
        .collect()
}

#[tauri::command]
fn onboarding_preview(draft: OnboardingDraft) -> Result<OnboardingPreview, DesktopBridgeError> {
    let root = onboarding_root(&draft)?;
    let files = onboarding_files(&draft);
    Ok(OnboardingPreview {
        root: root.to_string_lossy().into_owned(),
        files: onboarding_status(&root, &files)?,
    })
}

/// Write only missing onboarding files. The UI's final action is the human
/// confirmation; this command still rechecks the canonical root and each
/// target immediately before opening it, and `create_new` makes a concurrent
/// file appear as a refusal to overwrite rather than a race to clobber it.
#[tauri::command]
fn onboarding_write(draft: OnboardingDraft) -> Result<OnboardingPreview, DesktopBridgeError> {
    let root = onboarding_root(&draft)?;
    let files = onboarding_files(&draft);
    for file in &files {
        let target = mjolnr::policy::paths::for_write(&root, &file.relative_path).map_err(
            |refusal| DesktopBridgeError::Refused {
                code: Some(refusal.code.as_str().to_owned()),
                message: refusal.detail,
            },
        )?;
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                DesktopBridgeError::Initialization(format!(
                    "create onboarding directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let target = mjolnr::policy::paths::for_write(&root, &file.relative_path).map_err(
            |refusal| DesktopBridgeError::Refused {
                code: Some(refusal.code.as_str().to_owned()),
                message: refusal.detail,
            },
        )?;
        let mut handle = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(DesktopBridgeError::Refused {
                    code: Some("WORKSPACE_STALE_REVISION".to_owned()),
                    message: format!(
                        "onboarding file {} appeared during confirmation; nothing was overwritten",
                        file.relative_path.display()
                    ),
                });
            }
            Err(error) => {
                return Err(DesktopBridgeError::Initialization(format!(
                    "create onboarding file {}: {error}",
                    file.relative_path.display()
                )));
            }
        };
        handle.write_all(file.contents.as_bytes()).map_err(|error| {
            DesktopBridgeError::Initialization(format!(
                "write onboarding file {}: {error}",
                file.relative_path.display()
            ))
        })?;
    }

    Ok(OnboardingPreview {
        root: root.to_string_lossy().into_owned(),
        files: onboarding_status(&root, &files)?,
    })
}

#[tauri::command]
async fn dispatch_command(
    command: ClientCommand,
    state: State<'_, AppState>,
) -> Result<(), DesktopBridgeError> {
    state
        .bridge
        .dispatch(command)
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error
                .reason_code()
                .map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

#[tauri::command]
async fn get_snapshot(state: State<'_, AppState>) -> Result<ClientSnapshot, DesktopBridgeError> {
    Ok(state.bridge.snapshot())
}

/// One page of deterministic workspace search (Phase D4 client half).
///
/// Both directions speak the `ClientWorkspaceSearch*` DTOs. An earlier version
/// of this command was removed in the D4 split because it handed the store's
/// internal types straight to the frontend; the bridge now owns the conversion,
/// the typing refusals, and the wire bounds, and this is the thin hop it always
/// should have been.
#[tauri::command]
async fn search_workspace(
    filter: mjolnr::core::client::types::ClientWorkspaceSearchFilter,
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::types::ClientWorkspaceSearchPage, DesktopBridgeError> {
    state
        .bridge
        .search_workspace(filter)
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error.reason_code().map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

/// One bounded page of the open project's directory tree (D7/E9).
#[tauri::command]
async fn list_directory(
    path: String,
    page: u32,
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::workspace::DirectoryPage, DesktopBridgeError> {
    state
        .bridge
        .list_directory(&path, page)
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error.reason_code().map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

/// Open one bounded file projection for the editor (D7/E9).
#[tauri::command]
async fn open_file(
    path: String,
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::workspace::FileOpenView, DesktopBridgeError> {
    state
        .bridge
        .open_file(&path)
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error.reason_code().map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

const EDITOR_PREFERENCES_FILE: &str = "editor-preferences.json";

/// Resolve the user-editable preferences file beneath the open workspace.
///
/// Preferences are intentionally a file under `.mjolnr/`, not browser storage
/// or SQLite prose: an owner can inspect, diff, and revert them. The parent
/// and an existing file are canonicalized immediately before either read or
/// write so a symlink cannot turn this convenience setting into an escape.
fn editor_preferences_path(root: &Path) -> Result<PathBuf, DesktopBridgeError> {
    let root = std::fs::canonicalize(root).map_err(|error| {
        DesktopBridgeError::Initialization(format!("canonicalize workspace root: {error}"))
    })?;
    let mjolnr_dir = root.join(".mjolnr");
    if !mjolnr_dir.exists() {
        return Ok(mjolnr_dir.join(EDITOR_PREFERENCES_FILE));
    }
    let mjolnr_dir = std::fs::canonicalize(&mjolnr_dir).map_err(|error| {
        DesktopBridgeError::Refused {
            code: Some("PATH_OUTSIDE_WORKSPACE".to_owned()),
            message: format!("cannot validate the .mjolnr preferences directory: {error}"),
        }
    })?;
    if !mjolnr_dir.starts_with(&root) {
        return Err(DesktopBridgeError::Refused {
            code: Some("PATH_OUTSIDE_WORKSPACE".to_owned()),
            message: "the .mjolnr preferences directory escapes the workspace".to_owned(),
        });
    }
    let path = mjolnr_dir.join(EDITOR_PREFERENCES_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(DesktopBridgeError::Refused {
                code: Some("PATH_OUTSIDE_WORKSPACE".to_owned()),
                message: "the editor preferences file must not be a symlink".to_owned(),
            });
        }
        Ok(_) => {
            let existing = std::fs::canonicalize(&path).map_err(|error| {
                DesktopBridgeError::Refused {
                    code: Some("PATH_OUTSIDE_WORKSPACE".to_owned()),
                    message: format!("cannot validate the editor preferences file: {error}"),
                }
            })?;
            if !existing.starts_with(&root) {
                return Err(DesktopBridgeError::Refused {
                    code: Some("PATH_OUTSIDE_WORKSPACE".to_owned()),
                    message: "the editor preferences file escapes the workspace".to_owned(),
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DesktopBridgeError::Refused {
                code: Some("PATH_OUTSIDE_WORKSPACE".to_owned()),
                message: format!("cannot validate the editor preferences file: {error}"),
            });
        }
    }
    Ok(path)
}

fn load_editor_preferences(
    root: &Path,
) -> Result<ClientEditorPreferences, DesktopBridgeError> {
    let path = editor_preferences_path(root)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ClientEditorPreferences::default());
        }
        Err(error) => {
            return Err(DesktopBridgeError::Initialization(format!(
                "read editor preferences: {error}"
            )));
        }
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        DesktopBridgeError::Refused {
            code: Some("SCHEMA_INVALID".to_owned()),
            message: format!("editor preferences are invalid: {error}"),
        }
    })
}

fn save_editor_preferences(
    root: &Path,
    preferences: &ClientEditorPreferences,
) -> Result<(), DesktopBridgeError> {
    let path = editor_preferences_path(root)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            DesktopBridgeError::Initialization(format!("create editor preferences directory: {error}"))
        })?;
    }
    let path = editor_preferences_path(root)?;
    let bytes = serde_json::to_vec_pretty(preferences).map_err(|error| {
        DesktopBridgeError::Initialization(format!("encode editor preferences: {error}"))
    })?;
    std::fs::write(&path, bytes).map_err(|error| {
        DesktopBridgeError::Initialization(format!("write editor preferences: {error}"))
    })
}

#[tauri::command]
fn editor_preferences_load(
    state: State<'_, AppState>,
) -> Result<ClientEditorPreferences, DesktopBridgeError> {
    let root = terminal_root(&state)?;
    load_editor_preferences(&root)
}

#[tauri::command]
fn editor_preferences_save(
    preferences: ClientEditorPreferences,
    state: State<'_, AppState>,
) -> Result<(), DesktopBridgeError> {
    let root = terminal_root(&state)?;
    save_editor_preferences(&root, &preferences)
}

/// Log in to LM Studio: persist endpoint and optionally store an API token,
/// then trigger a provider catalog refresh so the new connection shows up.
///
/// An empty token clears any stored token (keyless mode). The address is
/// normalized; pass `"default"` to keep the current address.
#[tauri::command]
async fn auth_lm_studio_login(
    address: String,
    token: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let workspace = std::env::current_dir()
        .map_err(|error| format!("could not resolve project: {error}"))?;

    let endpoint = if address == "default" {
        mjolnr::providers::openai_compat::configured_lm_studio_base_url(&workspace)?
    } else {
        mjolnr::providers::openai_compat::persist_lm_studio_base_url(&workspace, &address)?
    };

    let secrets = OsSecretStore::new();
    let provider = ProviderId::new("lm-studio");
    if token.trim().is_empty() {
        let _ = secrets.delete(&provider);
    } else {
        secrets
            .store(&provider, Credential::ApiKey(Secret::new(token)))
            .map_err(|error| format!("could not store token: {error}"))?;
    }

    state
        .bridge
        .dispatch(ClientCommand::RefreshCredentials)
        .await
        .map_err(|error| format!("credential refresh failed: {error}"))?;

    Ok(endpoint)
}

/// Store an API key for a provider, then trigger a provider catalog refresh.
#[tauri::command]
async fn auth_api_key_login(
    provider: String,
    key: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("no key entered".to_owned());
    }
    let secrets = OsSecretStore::new();
    let id = ProviderId::new(&provider);
    secrets
        .store(&id, Credential::ApiKey(Secret::new(key)))
        .map_err(|error| format!("could not store credential: {error}"))?;

    state
        .bridge
        .dispatch(ClientCommand::RefreshCredentials)
        .await
        .map_err(|error| format!("credential refresh failed: {error}"))?;

    Ok(())
}

/// Verify Jules before persisting its API key. A failed verification never
/// leaves an unusable credential behind and returns no provider response body.
#[tauri::command]
async fn auth_jules_login(key: String) -> Result<(), String> {
    if key.trim().is_empty() {
        return Err("no Jules API key entered".to_owned());
    }
    let client = mjolnr::integrations::jules::JulesClient::new(
        mjolnr::core::secrets::Secret::new(key.clone()),
    );
    client
        .list_sources()
        .await
        .map_err(|error| format!("Jules connection refused: {error}"))?;
    let secrets = OsSecretStore::new();
    secrets
        .store(
            &ProviderId::new("jules"),
            Credential::ApiKey(Secret::new(key)),
        )
        .map_err(|error| format!("could not store Jules credential: {error}"))
}

/// Remove a stored credential for a provider, then trigger a catalog refresh.
#[tauri::command]
async fn auth_logout(
    provider: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let secrets = OsSecretStore::new();
    let id = ProviderId::new(&provider);
    secrets
        .delete(&id)
        .map_err(|error| format!("could not remove credential: {error}"))?;

    state
        .bridge
        .dispatch(ClientCommand::RefreshCredentials)
        .await
        .map_err(|error| format!("credential refresh failed: {error}"))?;

    Ok(())
}

/// One bounded, deterministic code-graph projection for the E7 surface.
#[tauri::command]
async fn query_graph(
    query: mjolnr::core::client::graph::ClientGraphQuery,
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::graph::ClientGraphPage, DesktopBridgeError> {
    state
        .bridge
        .query_graph(query)
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error.reason_code().map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

/// The board surface: what is decidable now, and why the rest is fogged
/// (Phase E5, step 3). Pure query; refuses without an open workspace.
#[tauri::command]
async fn query_board(
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::board::ClientBoardOverview, DesktopBridgeError> {
    state
        .bridge
        .query_board()
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error.reason_code().map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

/// One bounded newest-first repository-history query. History is fetched only
/// when the operator opens it; it is not added to every snapshot.
#[tauri::command]
async fn query_repository_history(
    limit: u32,
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::workspace::RepositoryHistory, DesktopBridgeError> {
    state
        .bridge
        .query_repository_history(limit)
        .await
        .map_err(|error| DesktopBridgeError::Refused {
            code: error.reason_code().map(|code| code.as_str().to_owned()),
            message: error.to_string(),
        })
}

fn terminal_root(state: &State<'_, AppState>) -> Result<PathBuf, DesktopBridgeError> {
    state
        .bridge
        .snapshot()
        .workspace_root
        .map(PathBuf::from)
        .ok_or_else(|| DesktopBridgeError::Refused {
            code: Some("WORKSPACE_CAPABILITY_UNAVAILABLE".to_owned()),
            message: "the terminal needs an open workspace".to_owned(),
        })
}

#[tauri::command]
fn terminal_start(
    rows: u16,
    cols: u16,
    cwd: Option<String>,
    state: State<'_, AppState>,
) -> Result<ClientTerminalSnapshot, DesktopBridgeError> {
    let root = terminal_root(&state)?;
    state
        .terminal
        .start_in(&root, cwd.as_deref(), rows, cols)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_snapshot(
    id: String,
    state: State<'_, AppState>,
) -> Result<ClientTerminalSnapshot, DesktopBridgeError> {
    state
        .terminal
        .snapshot(&id)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_input(
    input: ClientTerminalInput,
    state: State<'_, AppState>,
) -> Result<(), DesktopBridgeError> {
    state
        .terminal
        .input(&input)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_resize(
    resize: ClientTerminalResize,
    state: State<'_, AppState>,
) -> Result<(), DesktopBridgeError> {
    state
        .terminal
        .resize(&resize)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_scroll(
    scroll: ClientTerminalScroll,
    state: State<'_, AppState>,
) -> Result<ClientTerminalSnapshot, DesktopBridgeError> {
    state
        .terminal
        .scroll(&scroll)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_search(
    search: ClientTerminalSearch,
    state: State<'_, AppState>,
) -> Result<mjolnr::core::client::terminal::ClientTerminalSearchResult, DesktopBridgeError> {
    state
        .terminal
        .search(&search)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_layout_load(
    state: State<'_, AppState>,
) -> Result<ClientTerminalLayout, DesktopBridgeError> {
    let root = terminal_root(&state)?;
    state
        .terminal
        .load_layout(&root)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_layout_save(
    layout: ClientTerminalLayout,
    state: State<'_, AppState>,
) -> Result<(), DesktopBridgeError> {
    let root = terminal_root(&state)?;
    state
        .terminal
        .save_layout(&root, &layout)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_stop(
    id: String,
    state: State<'_, AppState>,
) -> Result<ClientTerminalSnapshot, DesktopBridgeError> {
    state
        .terminal
        .stop(&id)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
fn terminal_close(id: String, state: State<'_, AppState>) -> Result<(), DesktopBridgeError> {
    state
        .terminal
        .close(&id)
        .map_err(|error| DesktopBridgeError::Refused {
            code: None,
            message: error.to_string(),
        })
}

#[tauri::command]
async fn subscribe_updates(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), DesktopBridgeError> {
    let bridge = Arc::clone(&state.bridge);
    if let Some(mut rx) = bridge.take_updates() {
        tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                if app.emit("mjolnr-update", &update).is_err() {
                    break;
                }
            }
        });
    }
    Ok(())
}

fn provider_registry(secrets: &Arc<dyn SecretStore>) -> Vec<Arc<dyn Provider>> {
    use std::path::Path;
    // Desktop has no project file tree before a workspace is opened; the
    // workspace-local LM Studio endpoint (.mjolnr/providers/lm-studio.url)
    // therefore resolves against the current directory. The env override
    // MJOLNR_LM_STUDIO_BASE_URL still wins unconditionally.
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let mut providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(OpenAiProvider::new(Arc::clone(secrets))),
        Arc::new(AnthropicProvider::new(Arc::clone(secrets))),
        Arc::new(OpenAiCodexProvider::new(Arc::clone(secrets))),
        Arc::new(gemini::GeminiProvider::new(Arc::clone(secrets))),
        Arc::new(mjolnr::providers::gemini_cli::GeminiCliProvider::new(
            &mjolnr::providers::gemini_cli::GEMINI_CLI,
            Arc::clone(secrets),
        )),
        Arc::new(mjolnr::providers::gemini_cli::GeminiCliProvider::new(
            &mjolnr::providers::gemini_cli::ANTIGRAVITY,
            Arc::clone(secrets),
        )),
        Arc::new(openrouter::OpenRouterProvider::new(Arc::clone(secrets))),
        Arc::new(ollama::OllamaProvider::new()),
    ];
    for descriptor in mjolnr::providers::openai_compat::CATALOG {
        providers.push(Arc::new(
            mjolnr::providers::openai_compat::OpenAiCompatProvider::for_workspace(
                descriptor,
                Arc::clone(secrets),
                &workspace_root,
            ),
        ));
    }
    providers
}

/// Build the client bridge against an on-disk SQLite store. The
/// `runtime` must be entered (or `block_on` called) before invoking.
/// Used by `run()` for the real Tauri app and by the native smoke
/// harness to exercise the bridge without a window.
pub async fn init_bridge(database_path: PathBuf) -> Result<Arc<ClientBridge>, DesktopBridgeError> {
    if let Some(parent) = database_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            DesktopBridgeError::Initialization(format!("create data directory: {e}"))
        })?;
    }

    let store_sqlite = SqliteEventStore::open(&database_path)
        .await
        .map_err(|e| DesktopBridgeError::Initialization(format!("open SQLite store: {e}")))?;
    let store: Arc<dyn EventStore> = Arc::new(store_sqlite);

    let workspace_root = database_path
        .parent()
        .and_then(|p| p.parent())
        .map_or_else(|| PathBuf::from("."), |p| p.to_path_buf());
    let discovery = DiscoveryConfig::for_workspace(workspace_root)
        .map_err(|e| DesktopBridgeError::Initialization(format!("discovery config: {e}")))?;

    let project_context = tokio::task::spawn_blocking(move || ProjectContext::discover(discovery))
        .await
        .map_err(|e| {
            DesktopBridgeError::Initialization(format!("discover context join error: {e}"))
        })?
        .map_err(|e| DesktopBridgeError::Initialization(format!("discover context: {e}")))?;

    let secrets: Arc<dyn SecretStore> = Arc::new(OsSecretStore::new());
    let providers = provider_registry(&secrets);

    let runtime = Runtime::spawn_with_tools_project_context_and_triggers(
        providers,
        store,
        mjolnr::tools::ToolRegistry::default(),
        project_context,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(RouteTable::default()),
    );

    let bridge_runtime: Arc<dyn MjolnrRuntime> = Arc::new(runtime);
    Ok(Arc::new(ClientBridge::start(bridge_runtime)))
}

/// The desktop app always stores its database in the platform data directory
/// (`~/Library/Application Support/mjolnr` on macOS,
/// `$XDG_DATA_HOME/mjolnr` on Linux). The cwd-derived branch was removed
/// because `tauri dev` and a packaged build have different working
/// directories, which silently split sessions across two stores.
fn launch_database_path() -> Result<PathBuf, DesktopBridgeError> {
    let path = mjolnr::store::paths::default_database_path().map_err(|error| {
        DesktopBridgeError::Initialization(format!(
            "resolve desktop application data directory: {error}"
        ))
    })?;
    Ok(path.with_file_name("mjolnr-desktop.db"))
}

pub fn run() {
    let tokio_runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            error!("mjolnr-desktop: failed to initialize Tokio runtime: {err}");
            return;
        }
    };

    let _guard = tokio_runtime.enter();

    let database_path = match launch_database_path() {
        Ok(path) => path,
        Err(err) => {
            error!("mjolnr-desktop initialization error: {err}");
            return;
        }
    };
    let setup_result: Result<Arc<ClientBridge>, DesktopBridgeError> =
        tokio_runtime.block_on(init_bridge(database_path));

    let bridge = match setup_result {
        Ok(b) => b,
        Err(err) => {
            error!("mjolnr-desktop initialization error: {err}");
            return;
        }
    };

    let terminal_manager = Arc::new(TerminalManager::new());
    let state = AppState {
        bridge: Arc::clone(&bridge),
        terminal: Arc::clone(&terminal_manager),
    };

    let shutdown_executed = Arc::new(AtomicBool::new(false));

    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            dispatch_command,
            get_snapshot,
            onboarding_preview,
            onboarding_write,
            search_workspace,
            list_directory,
            open_file,
            editor_preferences_load,
            editor_preferences_save,
            auth_lm_studio_login,
            auth_api_key_login,
            auth_jules_login,
            auth_logout,
            query_graph,
            query_board,
            query_repository_history,
            terminal_start,
            terminal_snapshot,
            terminal_input,
            terminal_resize,
            terminal_scroll,
            terminal_search,
            terminal_stop,
            terminal_close,
            terminal_layout_load,
            terminal_layout_save,
            subscribe_updates
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(err) => {
            error!("mjolnr-desktop: failed to build Tauri application: {err}");
            return;
        }
    };

    app.run(move |_app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) && !shutdown_executed.swap(true, Ordering::SeqCst)
        {
            let bridge_clone = Arc::clone(&bridge);
            let terminal_manager_clone = Arc::clone(&terminal_manager);
            tokio_runtime.block_on(async move {
                // A frontend close is not itself process evidence. Ask the
                // manager to stop every live child, then let its watcher own
                // the eventual Exited/Failed state.
                if let Err(err) = terminal_manager_clone.stop_all() {
                    error!("mjolnr-desktop: error stopping terminal sessions: {err}");
                }
                if let Err(err) = bridge_clone.close().await {
                    error!("mjolnr-desktop: error closing client bridge: {err}");
                } else {
                    info!("mjolnr-desktop: client bridge closed cleanly");
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_preferences_root(label: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("mjolnr-{label}-{}-{stamp}", std::process::id()))
    }

    #[test]
    fn editor_preferences_default_and_round_trip_under_dot_mjolnr() {
        let root = temporary_preferences_root("editor-preferences");
        std::fs::create_dir_all(root.join(".mjolnr")).expect("create preferences directory");

        assert_eq!(
            load_editor_preferences(&root).expect("missing preferences default"),
            ClientEditorPreferences::default()
        );

        let preferences: ClientEditorPreferences =
            serde_json::from_str(r#"{"autosave":true}"#).expect("parse test preferences");
        save_editor_preferences(&root, &preferences).expect("save preferences");
        assert_eq!(
            load_editor_preferences(&root).expect("load preferences"),
            preferences
        );

        std::fs::remove_dir_all(root).expect("remove temporary preferences directory");
    }

    #[test]
    fn malformed_editor_preferences_refuse_with_schema_code() {
        let root = temporary_preferences_root("invalid-editor-preferences");
        let directory = root.join(".mjolnr");
        std::fs::create_dir_all(&directory).expect("create preferences directory");
        std::fs::write(directory.join(EDITOR_PREFERENCES_FILE), b"not json")
            .expect("write malformed preferences");

        let error = load_editor_preferences(&root).expect_err("malformed preferences");
        assert!(matches!(
            error,
            DesktopBridgeError::Refused {
                code: Some(code),
                ..
            } if code == "SCHEMA_INVALID"
        ));

        std::fs::remove_dir_all(root).expect("remove temporary preferences directory");
    }

    #[test]
    fn onboarding_previews_and_never_overwrites_setup_files() {
        let root = temporary_preferences_root("onboarding");
        std::fs::create_dir_all(&root).expect("create onboarding workspace");
        let draft = OnboardingDraft {
            root: root.to_string_lossy().into_owned(),
            soul: "# generated soul\n".to_owned(),
            user_profile: Some("# generated profile\n".to_owned()),
        };

        let first = onboarding_preview(draft.clone()).expect("preview onboarding files");
        assert_eq!(first.files.len(), 2);
        assert!(first.files.iter().all(|file| file.action == "write"));

        let written = onboarding_write(draft.clone()).expect("write onboarding files");
        assert!(written.files.iter().all(|file| file.action == "preserve"));
        assert_eq!(
            std::fs::read_to_string(root.join(".mjolnr/SOUL.md")).expect("read soul"),
            "# generated soul\n"
        );

        let second = onboarding_write(OnboardingDraft {
            soul: "# replacement must be refused\n".to_owned(),
            ..draft
        })
        .expect("preserve existing onboarding files");
        assert!(second.files.iter().all(|file| file.action == "preserve"));
        assert_eq!(
            std::fs::read_to_string(root.join(".mjolnr/SOUL.md")).expect("read preserved soul"),
            "# generated soul\n"
        );

        std::fs::remove_dir_all(root).expect("remove onboarding workspace");
    }

    /// The reason code is a public contract (AGENTS.md §6), and this is the
    /// last hop before it reaches TypeScript. It travels as its own field:
    /// flattened into the message it would be unrecoverable on the other side,
    /// which is what left the frontend with prose it could only print.
    #[test]
    fn a_refusal_carries_its_reason_code_beside_the_message() {
        let refusal = DesktopBridgeError::Refused {
            code: Some("WORKSPACE_ROOT_LOCKED".to_owned()),
            message: "a session is already open on this workspace root".to_owned(),
        };

        let wire = serde_json::to_value(&refusal).expect("serialize refusal");

        assert_eq!(wire["type"], "refused");
        assert_eq!(wire["detail"]["code"], "WORKSPACE_ROOT_LOCKED");
        assert_eq!(
            wire["detail"]["message"],
            "a session is already open on this workspace root"
        );
    }

    /// A refusal without a code (a closed runtime) still reaches the client as
    /// a refusal with a readable message. `code` is absent, never a guess.
    #[test]
    fn a_refusal_without_a_code_still_carries_its_message() {
        let refusal = DesktopBridgeError::Refused {
            code: None,
            message: "the runtime is closed".to_owned(),
        };

        let wire = serde_json::to_value(&refusal).expect("serialize refusal");

        assert_eq!(wire["type"], "refused");
        assert!(wire["detail"]["code"].is_null());
        assert_eq!(wire["detail"]["message"], "the runtime is closed");
    }

    #[test]
    fn bridge_start_outside_tokio_context_panics_as_expected() {
        let (events_tx, _) = tokio::sync::broadcast::channel(16);
        let (snapshot_tx, _) =
            tokio::sync::watch::channel(mjolnr::core::runtime::RuntimeSnapshot::default());

        struct DummyRuntime {
            events_tx: tokio::sync::broadcast::Sender<mjolnr::core::event::MjolnrEvent>,
            snapshot_tx: tokio::sync::watch::Sender<mjolnr::core::runtime::RuntimeSnapshot>,
        }
        use async_trait::async_trait;

        #[async_trait]
        impl MjolnrRuntime for DummyRuntime {
            fn snapshot(&self) -> mjolnr::core::runtime::RuntimeSnapshot {
                self.snapshot_tx.borrow().clone()
            }
            fn snapshots(&self) -> mjolnr::core::runtime::SnapshotStream {
                mjolnr::core::runtime::SnapshotStream::new(self.snapshot_tx.subscribe())
            }
            fn subscribe(&self) -> mjolnr::core::runtime::RuntimeSubscription {
                mjolnr::core::runtime::RuntimeSubscription::new(self.events_tx.subscribe())
            }
            async fn dispatch(
                &self,
                _command: mjolnr::core::command::MjolnrCommand,
            ) -> Result<(), mjolnr::core::error::MjolnrError> {
                Ok(())
            }
            // Refuses rather than returning an empty page, matching every other
            // double for this method: an empty page claims "nothing matched"
            // when nothing was searched (AGENTS.md §1.3).
            async fn search_workspace(
                &self,
                _filter: mjolnr::core::store::WorkspaceSearchFilter,
            ) -> Result<mjolnr::core::store::WorkspaceSearchPage, mjolnr::core::error::MjolnrError>
            {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "workspace search is not yet implemented (contract landed in D4)",
                ))
            }
            async fn query_board(
                &self,
            ) -> Result<mjolnr::core::frontier::BoardOverview, mjolnr::core::error::MjolnrError> {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "this dummy runtime has no board projection",
                ))
            }
            async fn query_repository_history(
                &self,
                _limit: u32,
            ) -> Result<mjolnr::core::repository::RepositoryHistory, mjolnr::core::error::MjolnrError>
            {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "this dummy runtime has no repository history",
                ))
            }
            async fn read_workspace_files(
                &self,
                _request: mjolnr::core::workspace_files::WorkspaceFileRequest,
            ) -> Result<
                mjolnr::core::workspace_files::WorkspaceFileAnswer,
                mjolnr::core::error::MjolnrError,
            > {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "this dummy runtime opens no project, so there is nothing to read files from",
                ))
            }
            async fn close(&self) -> Result<(), mjolnr::core::error::MjolnrError> {
                Ok(())
            }
        }
        impl std::fmt::Debug for DummyRuntime {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("DummyRuntime")
            }
        }

        let dummy = Arc::new(DummyRuntime {
            events_tx,
            snapshot_tx,
        });

        // ClientBridge::start calls tokio::spawn inside its constructor.
        // Calling it outside an entered Tokio runtime context must panic with "there is no reactor running".
        let result = std::panic::catch_unwind(|| {
            let _ = ClientBridge::start(dummy);
        });

        assert!(
            result.is_err(),
            "ClientBridge::start must panic when called outside an entered Tokio context"
        );
    }

    #[tokio::test]
    async fn bridge_start_inside_tokio_context_succeeds() {
        let (events_tx, _) = tokio::sync::broadcast::channel(16);
        let (snapshot_tx, _) =
            tokio::sync::watch::channel(mjolnr::core::runtime::RuntimeSnapshot::default());

        struct DummyRuntime {
            events_tx: tokio::sync::broadcast::Sender<mjolnr::core::event::MjolnrEvent>,
            snapshot_tx: tokio::sync::watch::Sender<mjolnr::core::runtime::RuntimeSnapshot>,
        }
        use async_trait::async_trait;

        #[async_trait]
        impl MjolnrRuntime for DummyRuntime {
            fn snapshot(&self) -> mjolnr::core::runtime::RuntimeSnapshot {
                self.snapshot_tx.borrow().clone()
            }
            fn snapshots(&self) -> mjolnr::core::runtime::SnapshotStream {
                mjolnr::core::runtime::SnapshotStream::new(self.snapshot_tx.subscribe())
            }
            fn subscribe(&self) -> mjolnr::core::runtime::RuntimeSubscription {
                mjolnr::core::runtime::RuntimeSubscription::new(self.events_tx.subscribe())
            }
            async fn dispatch(
                &self,
                _command: mjolnr::core::command::MjolnrCommand,
            ) -> Result<(), mjolnr::core::error::MjolnrError> {
                Ok(())
            }
            // Refuses rather than returning an empty page, matching every other
            // double for this method: an empty page claims "nothing matched"
            // when nothing was searched (AGENTS.md §1.3).
            async fn search_workspace(
                &self,
                _filter: mjolnr::core::store::WorkspaceSearchFilter,
            ) -> Result<mjolnr::core::store::WorkspaceSearchPage, mjolnr::core::error::MjolnrError>
            {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "workspace search is not yet implemented (contract landed in D4)",
                ))
            }
            async fn query_board(
                &self,
            ) -> Result<mjolnr::core::frontier::BoardOverview, mjolnr::core::error::MjolnrError> {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "this dummy runtime has no board projection",
                ))
            }
            async fn query_repository_history(
                &self,
                _limit: u32,
            ) -> Result<mjolnr::core::repository::RepositoryHistory, mjolnr::core::error::MjolnrError>
            {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "this dummy runtime has no repository history",
                ))
            }
            async fn read_workspace_files(
                &self,
                _request: mjolnr::core::workspace_files::WorkspaceFileRequest,
            ) -> Result<
                mjolnr::core::workspace_files::WorkspaceFileAnswer,
                mjolnr::core::error::MjolnrError,
            > {
                Err(mjolnr::core::error::MjolnrError::workspace_refused(
                    mjolnr::core::error::ReasonCode::WorkspaceCapabilityUnavailable,
                    "this dummy runtime opens no project, so there is nothing to read files from",
                ))
            }
            async fn close(&self) -> Result<(), mjolnr::core::error::MjolnrError> {
                Ok(())
            }
        }
        impl std::fmt::Debug for DummyRuntime {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("DummyRuntime")
            }
        }

        let dummy = Arc::new(DummyRuntime {
            events_tx,
            snapshot_tx,
        });

        let bridge = ClientBridge::start(dummy);
        assert_eq!(bridge.snapshot().revision, 0);
        assert!(bridge.close().await.is_ok());
    }
}
