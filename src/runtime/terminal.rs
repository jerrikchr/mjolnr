//! Rust-owned operator terminal sessions (Phase D8).
//!
//! One reason to change: the lifecycle and bounded screen projection of a PTY
//! owned by mjolnr. The model cannot reach this manager; the desktop client is
//! the operator surface and receives only [`ClientTerminalSnapshot`].

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;
use uuid::Uuid;

use crate::core::client::terminal::{
    ClientTerminalInput, ClientTerminalLayout, ClientTerminalResize, ClientTerminalScroll,
    ClientTerminalSearch, ClientTerminalSearchMatch, ClientTerminalSearchResult,
    ClientTerminalSnapshot, ClientTerminalStatus, MAX_TERMINAL_COLS, MAX_TERMINAL_CWD_BYTES,
    MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_QUERY_BYTES, MAX_TERMINAL_ROWS,
    MAX_TERMINAL_SCREEN_BYTES, MAX_TERMINAL_SEARCH_MATCHES,
};

const MAX_TERMINAL_SESSIONS: usize = 16;
const MAX_TERMINAL_SCROLLBACK_ROWS: usize = 2_000;
const MAX_TERMINAL_DETAIL_BYTES: usize = 1_024;
const PTY_READ_BUFFER_BYTES: usize = 8_192;
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("terminal id is empty")]
    EmptyId,
    #[error("terminal session is not found: {id}")]
    NotFound { id: String },
    #[error("terminal session is still running: {id}")]
    StillRunning { id: String },
    #[error("terminal dimensions are outside the supported bounds: {rows} rows x {cols} columns")]
    InvalidSize { rows: u16, cols: u16 },
    #[error("terminal input exceeds {limit} bytes")]
    InputTooLarge { limit: usize },
    #[error("terminal query exceeds {limit} bytes")]
    QueryTooLarge { limit: usize },
    #[error("terminal session limit reached: {limit}")]
    SessionLimit { limit: usize },
    #[error("terminal workspace root is unavailable: {detail}")]
    Root { detail: String },
    #[error("terminal session is not running: {id}")]
    NotRunning { id: String },
    #[error("terminal working directory is invalid: {detail}")]
    InvalidCwd { detail: String },
    #[error("terminal layout could not be read: {detail}")]
    LayoutRead { detail: String },
    #[error("terminal layout could not be written: {detail}")]
    LayoutWrite { detail: String },
    #[error("terminal session state is unavailable")]
    Poisoned,
    #[error("terminal PTY operation failed: {detail}")]
    Pty { detail: String },
}

#[derive(Debug)]
struct SessionState {
    status: ClientTerminalStatus,
    exit_code: Option<u32>,
    detail: Option<String>,
}

struct TerminalSession {
    id: String,
    cwd: String,
    process_id: Option<u32>,
    parser: Mutex<vt100::Parser>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    state: Mutex<SessionState>,
}

impl std::fmt::Debug for TerminalSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSession")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct TerminalManager {
    sessions: Mutex<BTreeMap<String, Arc<TerminalSession>>>,
}

impl TerminalManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(
        &self,
        workspace_root: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<ClientTerminalSnapshot, TerminalError> {
        self.start_in(workspace_root, None, rows, cols)
    }

    pub fn start_in(
        &self,
        workspace_root: &Path,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> Result<ClientTerminalSnapshot, TerminalError> {
        validate_size(rows, cols)?;
        let root = workspace_root
            .canonicalize()
            .map_err(|error| TerminalError::Root {
                detail: error.to_string(),
            })?;
        if !root.is_dir() {
            return Err(TerminalError::Root {
                detail: format!("{} is not a directory", root.display()),
            });
        }
        let working_directory = resolve_cwd(&root, cwd)?;

        let mut sessions = lock(&self.sessions)?;
        // Keep live and stopping sessions addressable so opening a new tab
        // cannot orphan a sibling. Reap only sessions whose watcher proved
        // that the child exited.
        sessions.retain(|_, session| !settled(session).unwrap_or(false));
        if sessions.len() >= MAX_TERMINAL_SESSIONS {
            return Err(TerminalError::SessionLimit {
                limit: MAX_TERMINAL_SESSIONS,
            });
        }

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })?;
        let command = shell_command(&working_directory);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })?;
        let killer = child.clone_killer();
        let process_id = child.process_id();
        let id = Uuid::now_v7().to_string();
        let session = Arc::new(TerminalSession {
            id: id.clone(),
            cwd: relative_cwd(&root, &working_directory),
            process_id,
            parser: Mutex::new(vt100::Parser::new(rows, cols, MAX_TERMINAL_SCROLLBACK_ROWS)),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            killer: Mutex::new(killer),
            state: Mutex::new(SessionState {
                status: ClientTerminalStatus::Running,
                exit_code: None,
                detail: None,
            }),
        });
        sessions.insert(id.clone(), Arc::clone(&session));
        drop(sessions);

        start_reader(Arc::clone(&session), reader);
        start_watcher(Arc::clone(&session));
        snapshot(&session)
    }

    pub fn snapshot(&self, id: &str) -> Result<ClientTerminalSnapshot, TerminalError> {
        let session = self.session(id)?;
        snapshot(&session)
    }

    pub fn input(&self, input: &ClientTerminalInput) -> Result<(), TerminalError> {
        if input.id.is_empty() {
            return Err(TerminalError::EmptyId);
        }
        if input.data.len() > MAX_TERMINAL_INPUT_BYTES {
            return Err(TerminalError::InputTooLarge {
                limit: MAX_TERMINAL_INPUT_BYTES,
            });
        }
        let session = self.session(&input.id)?;
        require_running(&session)?;
        let mut writer = lock(&session.writer)?;
        writer
            .write_all(input.data.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })
    }

    pub fn scroll(
        &self,
        scroll: &ClientTerminalScroll,
    ) -> Result<ClientTerminalSnapshot, TerminalError> {
        let session = self.session(&scroll.id)?;
        let mut parser = lock(&session.parser)?;
        let current = parser.screen().scrollback();
        let magnitude = usize::try_from(scroll.rows.unsigned_abs()).unwrap_or(usize::MAX);
        let next = if scroll.rows.is_negative() {
            current.saturating_sub(magnitude)
        } else {
            current.saturating_add(magnitude)
        };
        parser.screen_mut().set_scrollback(next);
        drop(parser);
        snapshot(&session)
    }

    pub fn search(
        &self,
        search: &ClientTerminalSearch,
    ) -> Result<ClientTerminalSearchResult, TerminalError> {
        if search.query.is_empty() {
            return Ok(ClientTerminalSearchResult {
                matches: Vec::new(),
                truncated: false,
            });
        }
        if search.query.len() > MAX_TERMINAL_QUERY_BYTES {
            return Err(TerminalError::QueryTooLarge {
                limit: MAX_TERMINAL_QUERY_BYTES,
            });
        }
        let session = self.session(&search.id)?;
        let mut parser = lock(&session.parser)?;
        let original = parser.screen().scrollback();
        let mut matches = Vec::new();
        let mut truncated = false;
        let mut previous_screen: Option<String> = None;
        for offset in 0..=MAX_TERMINAL_SCROLLBACK_ROWS {
            parser.screen_mut().set_scrollback(offset);
            let screen = parser.screen().contents();
            if previous_screen.as_ref() == Some(&screen) && offset > 0 {
                break;
            }
            previous_screen = Some(screen.clone());
            if screen
                .to_ascii_lowercase()
                .contains(&search.query.to_ascii_lowercase())
            {
                matches.push(ClientTerminalSearchMatch {
                    scrollback_offset: offset,
                    text: bounded_text(&screen, 512).0,
                });
                if matches.len() >= MAX_TERMINAL_SEARCH_MATCHES {
                    truncated = true;
                    break;
                }
            }
        }
        parser.screen_mut().set_scrollback(original);
        Ok(ClientTerminalSearchResult { matches, truncated })
    }

    pub fn load_layout(
        &self,
        workspace_root: &Path,
    ) -> Result<ClientTerminalLayout, TerminalError> {
        let path = layout_path(workspace_root)?;
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                serde_json::from_str(&contents).map_err(|error| TerminalError::LayoutRead {
                    detail: error.to_string(),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default_layout()),
            Err(error) => Err(TerminalError::LayoutRead {
                detail: error.to_string(),
            }),
        }
    }

    pub fn save_layout(
        &self,
        workspace_root: &Path,
        layout: &ClientTerminalLayout,
    ) -> Result<(), TerminalError> {
        let root = workspace_root
            .canonicalize()
            .map_err(|error| TerminalError::Root {
                detail: error.to_string(),
            })?;
        let path = layout_path(workspace_root)?;
        if layout.primary_cwd.len() > MAX_TERMINAL_CWD_BYTES
            || layout
                .secondary_cwd
                .as_ref()
                .is_some_and(|cwd| cwd.len() > MAX_TERMINAL_CWD_BYTES)
        {
            return Err(TerminalError::InvalidCwd {
                detail: "terminal layout working directory is too long".to_owned(),
            });
        }
        resolve_cwd(&root, Some(&layout.primary_cwd))?;
        if let Some(cwd) = layout.secondary_cwd.as_deref() {
            resolve_cwd(&root, Some(cwd))?;
        }
        let parent = path.parent().ok_or_else(|| TerminalError::LayoutWrite {
            detail: "terminal layout has no parent directory".to_owned(),
        })?;
        if parent.exists() {
            let canonical_parent =
                parent
                    .canonicalize()
                    .map_err(|error| TerminalError::LayoutWrite {
                        detail: error.to_string(),
                    })?;
            if !canonical_parent.starts_with(&root) || !canonical_parent.is_dir() {
                return Err(TerminalError::LayoutWrite {
                    detail: "terminal layout directory escapes the workspace".to_owned(),
                });
            }
        } else {
            std::fs::create_dir_all(parent).map_err(|error| TerminalError::LayoutWrite {
                detail: error.to_string(),
            })?;
            let canonical_parent =
                parent
                    .canonicalize()
                    .map_err(|error| TerminalError::LayoutWrite {
                        detail: error.to_string(),
                    })?;
            if !canonical_parent.starts_with(&root) {
                return Err(TerminalError::LayoutWrite {
                    detail: "terminal layout directory escapes the workspace".to_owned(),
                });
            }
        }
        let contents =
            serde_json::to_vec_pretty(layout).map_err(|error| TerminalError::LayoutWrite {
                detail: error.to_string(),
            })?;
        std::fs::write(path, contents).map_err(|error| TerminalError::LayoutWrite {
            detail: error.to_string(),
        })
    }

    pub fn resize(&self, resize: &ClientTerminalResize) -> Result<(), TerminalError> {
        validate_size(resize.rows, resize.cols)?;
        let session = self.session(&resize.id)?;
        require_running(&session)?;
        lock(&session.master)?
            .resize(PtySize {
                rows: resize.rows,
                cols: resize.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })?;
        lock(&session.parser)?
            .screen_mut()
            .set_size(resize.rows, resize.cols);
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<ClientTerminalSnapshot, TerminalError> {
        let session = self.session(id)?;
        let mut state = lock(&session.state)?;
        match state.status {
            ClientTerminalStatus::Running => {
                let group_result = kill_process_group(session.process_id);
                if group_result.is_err() {
                    lock(&session.killer)?
                        .kill()
                        .map_err(|error| TerminalError::Pty {
                            detail: error.to_string(),
                        })?;
                }
                state.status = ClientTerminalStatus::Stopping;
            }
            ClientTerminalStatus::Stopping
            | ClientTerminalStatus::Exited
            | ClientTerminalStatus::Failed => {}
        }
        drop(state);
        snapshot(&session)
    }

    pub fn close(&self, id: &str) -> Result<(), TerminalError> {
        let mut sessions = lock(&self.sessions)?;
        let session = sessions
            .get(id)
            .cloned()
            .ok_or_else(|| TerminalError::NotFound { id: id.to_owned() })?;
        if !settled(&session)? {
            return Err(TerminalError::StillRunning { id: id.to_owned() });
        }
        sessions.remove(id);
        Ok(())
    }

    pub fn stop_all(&self) -> Result<(), TerminalError> {
        let ids = lock(&self.sessions)?.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            self.stop(&id)?;
        }
        Ok(())
    }

    fn session(&self, id: &str) -> Result<Arc<TerminalSession>, TerminalError> {
        if id.is_empty() {
            return Err(TerminalError::EmptyId);
        }
        lock(&self.sessions)?
            .get(id)
            .cloned()
            .ok_or_else(|| TerminalError::NotFound { id: id.to_owned() })
    }
}

fn shell_command(root: &Path) -> CommandBuilder {
    #[cfg(windows)]
    let mut command = CommandBuilder::new("cmd.exe");
    #[cfg(not(windows))]
    let mut command = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-i");
        command
    };
    // Do not inherit the process environment. Provider credentials and other
    // ambient values are not part of an operator terminal's declared scope.
    command.env_clear();
    command.env("PATH", safe_path());
    command.env("TERM", "xterm-256color");
    command.cwd(root);
    command
}

fn resolve_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf, TerminalError> {
    let requested = cwd.unwrap_or_default();
    if requested.len() > MAX_TERMINAL_CWD_BYTES {
        return Err(TerminalError::InvalidCwd {
            detail: format!("working directory exceeds {MAX_TERMINAL_CWD_BYTES} bytes"),
        });
    }
    let relative = Path::new(requested);
    if relative.is_absolute() {
        return Err(TerminalError::InvalidCwd {
            detail: "working directory must be project-relative".to_owned(),
        });
    }
    let path = if requested.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let canonical = path
        .canonicalize()
        .map_err(|error| TerminalError::InvalidCwd {
            detail: error.to_string(),
        })?;
    if !canonical.starts_with(root) || !canonical.is_dir() {
        return Err(TerminalError::InvalidCwd {
            detail: "working directory must remain inside the workspace and be a directory"
                .to_owned(),
        });
    }
    Ok(canonical)
}

fn relative_cwd(root: &Path, cwd: &Path) -> String {
    cwd.strip_prefix(root)
        .unwrap_or(cwd)
        .to_string_lossy()
        .replace('\\', "/")
}

fn layout_path(workspace_root: &Path) -> Result<PathBuf, TerminalError> {
    let root = workspace_root
        .canonicalize()
        .map_err(|error| TerminalError::Root {
            detail: error.to_string(),
        })?;
    if !root.is_dir() {
        return Err(TerminalError::Root {
            detail: format!("{} is not a directory", root.display()),
        });
    }
    let directory = crate::core::paths::resolve_workspace_config_dir(&root);
    if directory.exists() {
        let canonical = directory
            .canonicalize()
            .map_err(|error| TerminalError::LayoutRead {
                detail: error.to_string(),
            })?;
        if !canonical.starts_with(&root) || !canonical.is_dir() {
            return Err(TerminalError::LayoutRead {
                detail: "terminal layout directory escapes the workspace".to_owned(),
            });
        }
    }
    Ok(directory.join("terminal-layout.json"))
}

fn default_layout() -> ClientTerminalLayout {
    ClientTerminalLayout {
        primary_cwd: String::new(),
        split_direction: None,
        secondary_cwd: None,
    }
}

#[cfg(unix)]
fn kill_process_group(process_id: Option<u32>) -> Result<(), TerminalError> {
    if let Some(process_id) = process_id {
        let status = Command::new("/bin/kill")
            .args(["-TERM", &format!("-{process_id}")])
            .status()
            .map_err(|error| TerminalError::Pty {
                detail: error.to_string(),
            })?;
        if !status.success() {
            return Err(TerminalError::Pty {
                detail: format!("process group {process_id} did not accept termination"),
            });
        }
    }
    Ok(())
}

fn safe_path() -> &'static str {
    #[cfg(windows)]
    {
        "C:\\Windows\\System32;C:\\Windows"
    }
    #[cfg(not(windows))]
    {
        "/usr/bin:/bin:/usr/sbin:/sbin"
    }
}

fn start_reader(session: Arc<TerminalSession>, mut reader: Box<dyn Read + Send>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; PTY_READ_BUFFER_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes) => {
                    let Some(chunk) = buffer.get(..bytes) else {
                        mark_failed(&session, "terminal reader returned an invalid byte count");
                        break;
                    };
                    if let Ok(mut parser) = session.parser.lock() {
                        parser.process(chunk);
                    } else {
                        mark_failed(&session, "terminal screen state was poisoned");
                        break;
                    }
                }
                Err(error) => {
                    if !is_settled(&session) {
                        mark_failed(&session, &format!("terminal output failed: {error}"));
                    }
                    break;
                }
            }
        }
    });
}

fn start_watcher(session: Arc<TerminalSession>) {
    thread::spawn(move || {
        loop {
            let result = session
                .child
                .lock()
                .map_err(|_| TerminalError::Poisoned)
                .and_then(|mut child| {
                    child.try_wait().map_err(|error| TerminalError::Pty {
                        detail: error.to_string(),
                    })
                });
            match result {
                Ok(Some(status)) => {
                    if let Ok(mut state) = session.state.lock()
                        && state.status != ClientTerminalStatus::Failed
                    {
                        state.status = ClientTerminalStatus::Exited;
                        state.exit_code = Some(status.exit_code());
                    }
                    break;
                }
                Ok(None) => thread::sleep(CHILD_POLL_INTERVAL),
                Err(error) => {
                    mark_failed(&session, &error.to_string());
                    break;
                }
            }
        }
    });
}

fn snapshot(session: &TerminalSession) -> Result<ClientTerminalSnapshot, TerminalError> {
    let state = lock(&session.state)?;
    let parser = lock(&session.parser)?;
    let screen = parser.screen().contents();
    let (screen, screen_truncated) = bounded_text(&screen, MAX_TERMINAL_SCREEN_BYTES);
    let (rows, cols) = parser.screen().size();
    Ok(ClientTerminalSnapshot {
        id: session.id.clone(),
        status: state.status,
        cwd: session.cwd.clone(),
        screen,
        rows,
        cols,
        scrollback_rows: MAX_TERMINAL_SCROLLBACK_ROWS,
        scrollback_offset: parser.screen().scrollback(),
        screen_truncated,
        exit_code: state.exit_code,
        detail: state.detail.clone(),
    })
}

fn validate_size(rows: u16, cols: u16) -> Result<(), TerminalError> {
    if rows == 0 || cols == 0 || rows > MAX_TERMINAL_ROWS || cols > MAX_TERMINAL_COLS {
        return Err(TerminalError::InvalidSize { rows, cols });
    }
    Ok(())
}

fn require_running(session: &TerminalSession) -> Result<(), TerminalError> {
    let state = lock(&session.state)?;
    if state.status == ClientTerminalStatus::Running {
        Ok(())
    } else {
        Err(TerminalError::NotRunning {
            id: session.id.clone(),
        })
    }
}

fn settled(session: &TerminalSession) -> Result<bool, TerminalError> {
    let state = lock(&session.state)?;
    Ok(matches!(
        state.status,
        ClientTerminalStatus::Exited | ClientTerminalStatus::Failed
    ))
}

fn is_settled(session: &TerminalSession) -> bool {
    match session.state.lock() {
        Ok(state) => matches!(
            state.status,
            ClientTerminalStatus::Exited | ClientTerminalStatus::Failed
        ),
        Err(_) => true,
    }
}

fn mark_failed(session: &TerminalSession, detail: &str) {
    if let Ok(mut state) = session.state.lock() {
        state.status = ClientTerminalStatus::Failed;
        state.detail = Some(bounded_text(detail, MAX_TERMINAL_DETAIL_BYTES).0);
    }
}

fn bounded_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (text[..end].to_owned(), true)
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, TerminalError> {
    mutex.lock().map_err(|_| TerminalError::Poisoned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::client::terminal::ClientTerminalSplitDirection;

    #[test]
    fn ansi_output_becomes_bounded_screen_text() {
        let mut parser = vt100::Parser::new(2, 8, 1);
        parser.process(b"\x1b[31mred\x1b[0m\r\nblue");
        assert_eq!(
            parser.screen().rows(0, 8).collect::<Vec<_>>(),
            ["red", "blue"]
        );
        assert!(!parser.screen().contents().contains('\u{1b}'));
    }

    #[test]
    fn alternate_screen_unicode_and_high_volume_output_remain_bounded() {
        let mut parser = vt100::Parser::new(8, 32, MAX_TERMINAL_SCROLLBACK_ROWS);
        parser.process(b"\x1b[?1049h\x1b[2J\x1b[Halternate \xF0\x9F\x99\x82\x1b[?1049l");
        for _ in 0..10_000 {
            parser.process(b"high-volume-line\r\n");
        }
        let screen = parser.screen().contents();
        assert!(screen.len() <= MAX_TERMINAL_SCREEN_BYTES);
        assert!(screen.contains("high-volume-line"));
        assert!(!screen.contains('\u{1b}'));
    }

    #[test]
    fn screen_projection_truncates_on_utf8_boundaries() {
        let (text, truncated) = bounded_text("hello \u{1f642}", 7);
        assert_eq!(text, "hello ");
        assert!(truncated);
    }

    #[test]
    fn shell_command_does_not_inherit_provider_credentials() {
        let command = shell_command(Path::new("/tmp"));
        assert_eq!(
            command.get_env("TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        assert!(command.get_env("OPENAI_API_KEY").is_none());
        assert!(command.get_env("ANTHROPIC_API_KEY").is_none());
        assert!(
            command
                .iter_full_env_as_str()
                .all(|(key, _)| { matches!(key, "PATH" | "TERM") })
        );
    }

    #[test]
    fn dimensions_and_input_are_bounded() {
        assert!(validate_size(MAX_TERMINAL_ROWS, MAX_TERMINAL_COLS).is_ok());
        assert!(validate_size(0, 10).is_err());
        assert!(validate_size(10, MAX_TERMINAL_COLS.saturating_add(1)).is_err());
        assert!(
            TerminalManager::new()
                .input(&ClientTerminalInput {
                    id: "missing".to_owned(),
                    data: "x".repeat(MAX_TERMINAL_INPUT_BYTES + 1),
                })
                .is_err()
        );
    }

    #[test]
    fn manager_owns_a_pty_and_reports_resize_and_stop_lifecycle() {
        let manager = TerminalManager::new();
        let started = manager.start(Path::new("."), 4, 20).unwrap();
        assert_eq!(started.status, ClientTerminalStatus::Running);

        manager
            .resize(&ClientTerminalResize {
                id: started.id.clone(),
                rows: 5,
                cols: 30,
            })
            .unwrap();
        let resized = manager.snapshot(&started.id).unwrap();
        assert_eq!((resized.rows, resized.cols), (5, 30));

        let stopped = manager.stop(&started.id).unwrap();
        assert!(matches!(
            stopped.status,
            ClientTerminalStatus::Stopping | ClientTerminalStatus::Exited
        ));
    }

    #[test]
    fn opening_a_second_tab_keeps_the_first_live() {
        let manager = TerminalManager::new();
        let first = manager.start(Path::new("."), 4, 20).unwrap();
        let second = manager.start(Path::new("."), 4, 20).unwrap();

        assert_eq!(
            manager.snapshot(&first.id).unwrap().status,
            ClientTerminalStatus::Running
        );
        assert_eq!(
            manager.snapshot(&second.id).unwrap().status,
            ClientTerminalStatus::Running
        );

        manager.stop(&first.id).unwrap();
        manager.stop(&second.id).unwrap();
    }

    #[test]
    fn working_directory_is_contained_and_reported_relative_to_the_workspace() {
        let manager = TerminalManager::new();
        let started = manager
            .start_in(Path::new("."), Some("src"), 4, 20)
            .unwrap();
        assert_eq!(started.cwd, "src");
        assert!(matches!(
            manager.start_in(Path::new("."), Some("../"), 4, 20),
            Err(TerminalError::InvalidCwd { .. })
        ));
        manager.stop(&started.id).unwrap();
    }

    #[test]
    fn layout_is_bounded_and_round_trips_as_workspace_state() {
        let directory = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new();
        let layout = ClientTerminalLayout {
            primary_cwd: String::new(),
            split_direction: Some(ClientTerminalSplitDirection::Vertical),
            secondary_cwd: Some(String::new()),
        };
        manager.save_layout(directory.path(), &layout).unwrap();
        assert_eq!(manager.load_layout(directory.path()).unwrap(), layout);
    }

    #[test]
    fn scroll_and_search_are_bounded_and_keep_the_view_addressable() {
        let manager = TerminalManager::new();
        let started = manager.start(Path::new("."), 4, 20).unwrap();
        let scrolled = manager
            .scroll(&ClientTerminalScroll {
                id: started.id.clone(),
                rows: 50_000,
            })
            .unwrap();
        assert!(scrolled.scrollback_offset <= scrolled.scrollback_rows);
        let result = manager
            .search(&ClientTerminalSearch {
                id: started.id.clone(),
                query: "shell".to_owned(),
            })
            .unwrap();
        assert!(result.matches.len() <= MAX_TERMINAL_SEARCH_MATCHES);
        manager.stop(&started.id).unwrap();
    }
}
