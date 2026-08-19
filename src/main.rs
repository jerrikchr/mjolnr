//! The `mjolnr` binary.
//!
//! Composition root: this is the one place that knows a TUI, a runtime, a store,
//! and providers all exist. Everything below it sees only traits.

// The terminal is restored before either of these run (RAII guard + panic
// hook), so stderr is safe. This is the only place the binary prints, and it
// runs when the TUI is already gone.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "stderr is outside the restored terminal; stdout is headless JSON only"
)]

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use smed::cli::{self, Cli, Command, ExecArgs, triggers::TriggersCommand};
use smed::context::{DiscoveryConfig, ProjectContext};
use smed::core::command::SmedCommand;
use smed::core::event::SessionId;
use smed::core::model::{ModelId, ProviderId};
use smed::core::provider::Provider;
use smed::core::runtime::SmedRuntime;
use smed::core::secrets::{CredentialKind, SecretStore};
use smed::core::store::{EventStore, SessionStatus, StoreError};
use smed::mcp;
use smed::providers::anthropic::{self, AnthropicProvider};
use smed::providers::openai::{self, OpenAiProvider};
use smed::providers::openai_codex::{self, OpenAiCodexProvider};
use smed::providers::{gemini, ollama, openrouter};
use smed::routing::scaffold::ProviderSeed;
use smed::runtime::Runtime;
use smed::store::secrets::OsSecretStore;
use smed::store::sqlite::SqliteEventStore;
use smed::tui::app;

/// The model a configured OpenAI session opens on. Cheapest of the offered set,
/// so an accidental session is not an expensive one.
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";

/// Whether an alternate screen exists for us to leave.
///
/// The panic hook is installed before the client is chosen, so it cannot assume
/// a terminal it owns. `ratatui::try_restore` writes an escape sequence to
/// **stdout**, and in a headless run stdout carries the NDJSON report and
/// nothing else (`AGENTS.md` §1.3) — restoring a screen that was never entered
/// corrupts machine-readable output for whatever is parsing it.
///
/// Set once the TUI has entered the alternate screen, cleared once it has left.
static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Restores the terminal on every exit path that returns or unwinds.
///
/// `ratatui::try_init` installs a panic hook covering panics; this guard covers
/// what a hook cannot — an early `?` return. Both are needed.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Never panic while unwinding, never unwrap in cleanup (AGENTS.md §4).
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = ratatui::try_restore();
        // Before any panic that unwinds past this point: the screen is gone, so
        // the hook must not write to stdout a second time.
        TUI_ACTIVE.store(false, Ordering::Release);
    }
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();

    if let Some(conflict) = cli.conflict() {
        eprintln!("mjolnr: {conflict}");
        return ExitCode::FAILURE;
    }

    let file_secrets = OsSecretStore::new();
    // One-shot, and a no-op for anyone who never ran a keychain build. Before
    // the auth subcommands so `smed auth list` reflects the move immediately.
    let migrated = smed::store::secrets::migrate_from_keyring(&file_secrets, &keyring_providers());
    if !migrated.is_empty() {
        let names: Vec<&str> = migrated.iter().map(ProviderId::as_str).collect();
        eprintln!(
            "mjolnr: moved {} credential(s) out of the OS keychain and into owner-only files: {}",
            migrated.len(),
            names.join(", ")
        );
        eprintln!("mjolnr: no keychain password will be requested again");
    }
    let secrets: Arc<dyn SecretStore> = Arc::new(file_secrets);

    // Credential subcommands run instead of the TUI, need no database, and are
    // deliberately synchronous — outside the async runtime entirely.
    if let Some(Command::Auth(command)) = cli.command.take_if(is_auth) {
        let code = cli::run_auth(command, &secrets);
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }

    // Plugin scaffolding runs instead of the TUI and needs no database — only
    // the working directory it writes into (ADR-0016).
    if let Some(Command::Plugin(command)) = cli.command.take_if(is_plugin) {
        let project_root = std::env::current_dir().unwrap_or_default();
        let code = match command {
            smed::cli::plugin::PluginCommand::Create(args) => {
                smed::cli::plugin::run_create(args, &project_root)
            }
            smed::cli::plugin::PluginCommand::List => {
                print_plugin_list(&project_root);
                0
            }
        };
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }

    // Setup runs instead of the TUI and needs no database — only the resolved
    // credentials and the working directory it writes into.
    //
    // Bare `smed init` is the guided wizard, because that is what `init` means
    // in every other tool and what someone reaching for it actually wants. The
    // scaffolder it used to be is one step inside that wizard now.
    //
    // `--yes` stays the scaffold-only path: it exists for scripts and CI, where
    // a wizard is a hang rather than a help, and where "write the obvious files
    // and say nothing" is the whole request.
    if let Some(command) = cli.command.take_if(is_setup) {
        let project_root = std::env::current_dir().unwrap_or_default();
        if matches!(command, Command::Init { yes: true }) {
            let seeds = configured_seeds(configured_providers(&secrets));
            let code = cli::init::run(
                &seeds,
                &project_root,
                &cli::init::InitOptions { assume_yes: true },
            );
            return ExitCode::from(u8::try_from(code).unwrap_or(1));
        }
        if let Err(error) =
            cli::onboard::run_onboarding(&project_root, &secrets, &onboarding_theme_step())
        {
            eprintln!("mjolnr: {error}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    // Path resolution may create the platform data directory, so keep that
    // blocking filesystem work outside Tokio's async workers.
    let database_path = match cli.database_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("mjolnr: {error}");
            return ExitCode::FAILURE;
        }
    };

    install_panic_reporter(&database_path);

    let needs_default_provider = match &cli.command {
        None => true,
        Some(Command::Exec(args)) => args.provider.is_none(),
        Some(_) => false,
    };
    let mut configured = if needs_default_provider {
        configured_providers(&secrets)
    } else {
        ConfiguredProviders::default()
    };

    // First-run detection : a fresh machine — no credential
    // resolves, no session store exists yet, and no standing decline — opens the
    // guided wizard instead of falling back to the silent local default. Only
    // the plain interactive launch qualifies; a subcommand or a resume does not.
    if cli.command.is_none()
        && cli.resume.is_none()
        && cli::onboard::global_first_run(
            configured.any(),
            database_path.exists(),
            cli::onboard::has_declined(),
        )
    {
        let project_root = std::env::current_dir().unwrap_or_default();
        if let Err(error) =
            cli::onboard::run_onboarding(&project_root, &secrets, &onboarding_theme_step())
        {
            eprintln!("mjolnr: {error}");
        }
        // The wizard may have connected a provider; re-resolve so the session
        // it opens sees the credential it just wrote.
        configured = configured_providers(&secrets);
    }

    match run(cli, &secrets, &database_path, configured) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            // The terminal is restored by now (guard + panic hook), so stderr is
            // safe to use and this is the only place the binary prints.
            eprintln!("mjolnr: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Install a bounded, redacted panic report before the TUI can start.
///
/// The panic payload is deliberately excluded: it may contain provider text,
/// source excerpts, or a credential echoed by an upstream error. The terminal
/// is restored first, then a fixed-shape report is written and named for the
/// operator. A single report file is enough for the MVP and cannot grow without
/// bound; the next panic replaces the previous report.
fn install_panic_reporter(database_path: &Path) {
    let report_path = database_path.with_file_name("panic-report.txt");
    std::panic::set_hook(Box::new(move |panic| {
        // Only when there is a screen to leave. A headless run's stdout is the
        // NDJSON report; an escape sequence appended to it is a lie about the
        // shape of machine-readable output, not a cosmetic blemish.
        if TUI_ACTIVE.load(Ordering::Acquire) {
            let _ = ratatui::try_restore();
        }
        let location = panic
            .location()
            .map_or_else(|| "unknown".to_owned(), std::string::ToString::to_string);
        let report = format!(
            "mjolnr panic report\nversion={}\nlocation={location}\n\nThe panic payload was intentionally omitted.\n",
            env!("CARGO_PKG_VERSION")
        );
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&report_path)
        {
            use std::io::Write;
            let _ = file.write_all(report.as_bytes());
        }
        eprintln!(
            "mjolnr: panic; redacted report written to {}",
            report_path.display()
        );
    }));
}

/// The theme step's inputs for the onboarding flow, resolved here in the
/// composition root so the flow itself never imports `tui` (AGENTS.md §2.1).
fn onboarding_theme_step() -> cli::onboard::ThemeStep {
    cli::onboard::ThemeStep {
        options: smed::tui::theme::preference_options(),
        active: smed::tui::theme::active_preference_name(),
        persist: smed::tui::theme::persist_preference,
    }
}

fn is_auth(command: &mut Command) -> bool {
    matches!(command, Command::Auth(_))
}

fn is_plugin(command: &mut Command) -> bool {
    matches!(command, Command::Plugin(_))
}

/// The two spellings of "set smed up". `onboard` is kept as a hidden alias so
/// existing muscle memory and any scripted invocation keep working.
fn is_setup(command: &mut Command) -> bool {
    matches!(command, Command::Init { .. } | Command::Onboard)
}

/// The providers `smed init` scaffolds a route for, primary first.
///
/// Reuses the same preference order and per-provider default-model constants as
/// [`default_model`], so the route `init` writes for a provider opens on exactly
/// the model a new session would. Only credentialed cloud providers are seeded:
/// scaffolding a route to a local server that may not be running would be a
/// guess the file then states as fact, so an empty result is left to `init` to
/// explain as "authenticate first".
fn configured_seeds(configured: ConfiguredProviders) -> Vec<ProviderSeed> {
    let mut seeds = Vec::new();
    let mut push = |provider: &str, model: &str| {
        seeds.push(ProviderSeed {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        });
    };
    if configured.openai_codex {
        push(openai_codex::PROVIDER_ID, openai_codex::DEFAULT_MODEL);
    }
    if configured.openai {
        push(openai::PROVIDER_ID, DEFAULT_OPENAI_MODEL);
    }
    if configured.anthropic {
        push(anthropic::PROVIDER_ID, anthropic::DEFAULT_MODEL);
    }
    if configured.gemini {
        push(gemini::PROVIDER_ID, gemini::DEFAULT_MODEL);
    }
    if configured.openrouter {
        push(openrouter::PROVIDER_ID, openrouter::DEFAULT_MODEL);
    }
    seeds
}

#[tokio::main]
async fn run(
    cli: Cli,
    secrets: &Arc<dyn SecretStore>,
    database_path: &Path,
    configured: ConfiguredProviders,
) -> io::Result<i32> {
    let store = match SqliteEventStore::open(&database_path).await {
        Ok(store) => Arc::new(store),
        Err(error) => {
            // Before the terminal opens, so a database that cannot be read is a
            // legible message rather than an empty screen.
            eprintln!("mjolnr: {error}");
            return Ok(1);
        }
    };

    let outcome = dispatch(cli, secrets, Arc::clone(&store), configured).await;

    // The store's last use, and deliberately inside this runtime: closing awaits
    // SQLite's final checkpoint, and a runtime that shut down first would drop
    // that work mid-flight (see `SqliteEventStore::close`). Reported, not
    // swallowed — a database that did not close cleanly is a durability fact.
    if let Err(error) = store.close().await {
        eprintln!("mjolnr: {error}");
    }

    outcome
}

/// Choose and run the client, with the store already open.
///
/// Split from [`run`] so every exit path passes through one close, rather than
/// each `return` having to remember it.
async fn dispatch(
    mut cli: Cli,
    secrets: &Arc<dyn SecretStore>,
    store: Arc<SqliteEventStore>,
    configured: ConfiguredProviders,
) -> io::Result<i32> {
    if let Some(command) = cli.command.take() {
        if let Command::Exec(args) = command {
            return run_exec(args, secrets, store, configured).await;
        }
        if let Command::Triggers(TriggersCommand::Run) = command {
            return run_triggers(secrets, store).await;
        }
        return match cli::run_with_store(command, &store).await {
            Ok(code) => Ok(code),
            Err(error) => {
                eprintln!("mjolnr: {error}");
                Ok(1)
            }
        };
    }

    run_tui(&cli, secrets, store, configured).await
}

async fn run_exec(
    args: ExecArgs,
    secrets: &Arc<dyn SecretStore>,
    store: Arc<SqliteEventStore>,
    configured: ConfiguredProviders,
) -> io::Result<i32> {
    let workspace_root = std::env::current_dir()?;
    let discovery = DiscoveryConfig::for_workspace(workspace_root.clone())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let project_context = tokio::task::spawn_blocking(move || ProjectContext::discover(discovery))
        .await
        .map_err(|error| io::Error::other(format!("project context task failed: {error}")))?
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mcp = match mcp::connect_project(&workspace_root).await {
        Ok(catalog) => catalog,
        Err(error) => {
            return print_headless_report(&smed::headless::HeadlessReport::setup_failure(
                error.reason_code(),
            ));
        }
    };
    let (provider, model) = match (args.provider, args.model) {
        (Some(provider), Some(model)) => (ProviderId::new(provider), ModelId::new(model)),
        (None, None) => default_model(configured),
        _ => {
            return print_headless_report(&smed::headless::HeadlessReport::setup_failure(
                smed::core::error::ReasonCode::SchemaInvalid,
            ));
        }
    };
    let (route_table, _routing_diagnostics) = smed::routing::load_dir(&workspace_root);
    let route_table = Arc::new(route_table);
    let providers = provider_registry(secrets, &workspace_root);
    if !providers.iter().any(|candidate| candidate.id() == provider) {
        return print_headless_report(&smed::headless::HeadlessReport::setup_failure(
            smed::core::error::ReasonCode::ProviderIncompatibleModel,
        ));
    }
    let runtime = Runtime::spawn_with_tools_and_project_context(
        providers,
        Arc::clone(&store) as Arc<dyn EventStore>,
        mcp.registry,
        project_context,
        mcp.servers,
        Arc::clone(&route_table),
    );
    let setup = async {
        runtime
            .dispatch(SmedCommand::OpenProject {
                root: workspace_root,
            })
            .await?;
        runtime
            .dispatch(SmedCommand::CreateSession { provider, model })
            .await?;
        // A headless run is a "turn" too : it may open on a
        // route's first hop rather than the requested provider/model. A
        // no-op whenever no `default` task class resolves.
        if !route_table.is_empty() {
            runtime
                .dispatch(SmedCommand::AttachRoute {
                    route: None,
                    role: None,
                    task_class: "default".to_owned(),
                })
                .await?;
        }
        runtime
            .dispatch(SmedCommand::SetPolicy {
                mode: args.policy.into(),
            })
            .await
    };
    if setup.await.is_err() {
        let _ = runtime.close().await;
        return print_headless_report(&smed::headless::HeadlessReport::setup_failure(
            smed::core::error::ReasonCode::ToolExecution,
        ));
    }
    let expected_policy = args.policy.into();
    if !wait_headless_ready(&runtime, expected_policy).await {
        let _ = runtime.close().await;
        return print_headless_report(&smed::headless::HeadlessReport::setup_failure(
            smed::core::error::ReasonCode::ProviderIncompatibleModel,
        ));
    }
    let report = match smed::headless::run(&runtime, args.directive).await {
        Ok(report) => report,
        Err(_) => smed::headless::HeadlessReport::setup_failure(
            smed::core::error::ReasonCode::ToolExecution,
        ),
    };
    if runtime.close().await.is_err() {
        return print_headless_report(&smed::headless::HeadlessReport::setup_failure(
            smed::core::error::ReasonCode::ToolExecution,
        ));
    }
    print_headless_report(&report)
}

/// `smed triggers run`: the scheduler process.
///
/// The composition-root twin of [`run_exec`] — same providers, same project
/// context, same MCP catalogue — handed to
/// [`smed::triggers::scheduler::run`] instead of one directive. Runs until
/// SIGINT/SIGTERM.
async fn run_triggers(
    secrets: &Arc<dyn SecretStore>,
    store: Arc<SqliteEventStore>,
) -> io::Result<i32> {
    let workspace_root = std::env::current_dir()?;
    let discovery = DiscoveryConfig::for_workspace(workspace_root.clone())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let project_context = tokio::task::spawn_blocking(move || ProjectContext::discover(discovery))
        .await
        .map_err(|error| io::Error::other(format!("project context task failed: {error}")))?
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mcp = match mcp::connect_project(&workspace_root).await {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("mjolnr: {}: {error}", error.reason_code());
            return Ok(1);
        }
    };

    let (route_table, _routing_diagnostics) = smed::routing::load_dir(&workspace_root);
    let deps = smed::triggers::SchedulerDeps {
        providers: provider_registry(secrets, &workspace_root),
        store: Arc::clone(&store) as Arc<dyn EventStore>,
        workspace_root,
        project_context,
        mcp_servers: mcp.servers,
        tools: mcp.registry,
        route_table: Arc::new(route_table),
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_cancel.cancel();
    });

    println!("mjolnr: scheduler running // Ctrl-C to stop");
    match smed::triggers::scheduler::run(deps, cancel).await {
        Ok(()) => Ok(0),
        Err(error) => {
            eprintln!("mjolnr: scheduler stopped: {error}");
            Ok(1)
        }
    }
}

async fn wait_headless_ready(
    runtime: &Runtime,
    expected_policy: smed::core::policy::PolicyMode,
) -> bool {
    if runtime.snapshot().session.is_some() && runtime.snapshot().policy == expected_policy {
        return true;
    }
    let mut snapshots = runtime.snapshots();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Ok(snapshot) = snapshots.changed().await else {
                return false;
            };
            if snapshot.session.is_some() && snapshot.policy == expected_policy {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false)
}

fn print_plugin_list(project_root: &Path) {
    use smed::context::{DiscoveryConfig, ProjectContext};
    let discovery = match DiscoveryConfig::for_workspace(project_root.to_owned()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("smed plugin list: {e}");
            return;
        }
    };
    let ctx = match ProjectContext::discover(discovery) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("smed plugin list: {e}");
            return;
        }
    };
    if ctx.plugins().is_empty() {
        println!("no plugins discovered in .mjolnr/plugins/*.yaml or user config dir");
        println!("hint: smed plugin create <name> [--template node|rust|python] [--yes]");
        return;
    }
    for summary in ctx.plugins().list() {
        println!(
            "{} v{} by {} — {} tool(s), {} hook(s){}",
            summary.name,
            summary.version,
            summary.publisher,
            summary.tool_count,
            summary.hook_count,
            if summary.required_credentials.is_empty() {
                String::new()
            } else {
                format!(" [{}]", summary.required_credentials.join(", "))
            }
        );
    }
}

fn print_headless_report(report: &smed::headless::HeadlessReport) -> io::Result<i32> {
    let line = serde_json::to_string(report).map_err(io::Error::other)?;
    println!("{line}");
    Ok(report.exit_code)
}

/// The composition root's implementation of the TUI's injected OAuth logins
/// (only main.rs may wire providers to the TUI). Runs while the
/// TUI has suspended the terminal, so plain stdin/stdout prompts are fine.
struct OAuthLogins;

#[allow(
    clippy::print_stdout,
    reason = "these flows run while the TUI has suspended the alternate screen"
)]
#[async_trait::async_trait]
impl app::AuthFlows for OAuthLogins {
    async fn oauth_login(&self, provider: &str) -> Result<i64, String> {
        let secrets: Arc<dyn smed::core::secrets::SecretStore> =
            Arc::new(smed::store::secrets::OsSecretStore::new());
        match provider {
            "anthropic" => smed::providers::anthropic::paste_login(
                secrets,
                |prompt| {
                    println!("Open this URL in your browser and authorize smed:");
                    println!("{}", prompt.authorize_url);
                    open_browser(&prompt.authorize_url);
                    println!("The final page displays an authorization code.");
                },
                || {
                    print!("Paste the authorization code here: ");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    let mut pasted = String::new();
                    std::io::stdin().read_line(&mut pasted).map_err(|error| {
                        smed::providers::anthropic::OAuthError::Protocol {
                            detail: format!("could not read the pasted code: {error}"),
                        }
                    })?;
                    Ok(pasted)
                },
            )
            .await
            .map_err(|error| error.to_string()),
            "openai-codex" => smed::providers::openai_codex::device_login(secrets, |prompt| {
                println!("Open {} in your browser.", prompt.verification_url);
                open_browser(&prompt.verification_url);
                println!("Enter this one-time code: {}", prompt.user_code);
                println!("Waiting for authorization (up to 15 minutes)…");
            })
            .await
            .map_err(|error| error.to_string()),
            "gemini-cli" | "antigravity" => {
                let config = if provider == "gemini-cli" {
                    &smed::providers::gemini_cli::GEMINI_CLI
                } else {
                    &smed::providers::gemini_cli::ANTIGRAVITY
                };
                smed::providers::gemini_cli::browser_login(config, secrets, |prompt| {
                    println!("Open this URL in your browser and authorize smed:");
                    println!("{}", prompt.authorize_url);
                    open_browser(&prompt.authorize_url);
                    println!("Waiting for the browser callback (up to 15 minutes)…");
                })
                .await
                .map_err(|error| error.to_string())
            }
            other => Err(format!("{other} has no OAuth login; register an API key")),
        }
    }

    fn configure_lm_studio_endpoint(&self, address: &str) -> Result<String, String> {
        let workspace = std::env::current_dir()
            .map_err(|error| format!("could not resolve project: {error}"))?;
        if address.is_empty() {
            smed::providers::openai_compat::configured_lm_studio_base_url(&workspace)
        } else {
            smed::providers::openai_compat::persist_lm_studio_base_url(&workspace, address)
        }
    }

    fn clear_lm_studio_token(&self) -> Result<bool, String> {
        let secrets = smed::store::secrets::OsSecretStore::new();
        secrets
            .delete(&ProviderId::new("lm-studio"))
            .map_err(|error| error.to_string())?;
        Ok(std::env::var("LM_API_TOKEN").is_ok_and(|value| !value.trim().is_empty()))
    }
}

/// Best-effort browser launch; the URL is always printed as the fallback.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = url;
}

fn provider_registry(
    secrets: &Arc<dyn SecretStore>,
    workspace_root: &Path,
) -> Vec<Arc<dyn Provider>> {
    let mut providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(OpenAiProvider::new(Arc::clone(secrets))),
        Arc::new(AnthropicProvider::new(Arc::clone(secrets))),
        Arc::new(OpenAiCodexProvider::new(Arc::clone(secrets))),
        Arc::new(gemini::GeminiProvider::new(Arc::clone(secrets))),
        Arc::new(smed::providers::gemini_cli::GeminiCliProvider::new(
            &smed::providers::gemini_cli::GEMINI_CLI,
            Arc::clone(secrets),
        )),
        Arc::new(smed::providers::gemini_cli::GeminiCliProvider::new(
            &smed::providers::gemini_cli::ANTIGRAVITY,
            Arc::clone(secrets),
        )),
        Arc::new(openrouter::OpenRouterProvider::new(Arc::clone(secrets))),
        Arc::new(ollama::OllamaProvider::new()),
    ];
    for descriptor in smed::providers::openai_compat::CATALOG {
        providers.push(Arc::new(
            smed::providers::openai_compat::OpenAiCompatProvider::for_workspace(
                descriptor,
                Arc::clone(secrets),
                workspace_root,
            ),
        ));
    }
    providers
}

/// The provider/model this workspace last ran on, if it is still usable.
///
/// The selection is already durable: every `SessionCreated` and `ModelChanged`
/// mirrors onto the session row, so the newest session for this project carries
/// the answer without replaying history. It is validated against the declared
/// adapter registry before being trusted, so a removed adapter or model falls
/// back to `default_model`. Live credential and catalogue readiness remains the
/// runtime discovery layer's responsibility.
async fn remembered_model(
    store: &SqliteEventStore,
    workspace_root: &std::path::Path,
    providers: &[Arc<dyn smed::core::provider::Provider>],
) -> Option<(ProviderId, ModelId)> {
    // Sessions record the project's *realpath*. Comparing a raw `current_dir()`
    // against it never matches wherever the path crosses a symlink (`/tmp` on
    // macOS, most obviously), which would silently disable the memory.
    let root_realpath = workspace_root.canonicalize().ok()?;
    let sessions = store.sessions().await.ok()?;
    sessions
        .into_iter()
        .filter(|session| session.project_root == root_realpath)
        .max_by_key(|session| session.updated_at)
        .and_then(|session| Some((session.provider?, session.model?)))
        .filter(|(provider, model)| {
            smed::core::provider::find_model(providers.iter(), provider, model).is_some()
        })
}

/// Every provider id that a keychain-era build could have written an entry for.
///
/// Listed rather than derived from the provider registry because the registry is
/// built from *credentialed* providers — the ones whose credentials are still
/// stuck in the keychain would be missing from it, which is precisely backwards
/// for a migration.
fn keyring_providers() -> Vec<ProviderId> {
    let mut providers = vec![
        ProviderId::new(openai::PROVIDER_ID),
        ProviderId::new(anthropic::PROVIDER_ID),
        ProviderId::new(openai_codex::PROVIDER_ID),
        ProviderId::new(gemini::PROVIDER_ID),
        ProviderId::new(openrouter::PROVIDER_ID),
        ProviderId::new(smed::providers::gemini_cli::GEMINI_CLI_PROVIDER_ID),
        ProviderId::new(smed::providers::gemini_cli::ANTIGRAVITY_PROVIDER_ID),
    ];
    providers.extend(
        smed::providers::openai_compat::CATALOG
            .iter()
            .map(|descriptor| ProviderId::new(descriptor.id)),
    );
    providers
}

fn default_model(configured: ConfiguredProviders) -> (ProviderId, ModelId) {
    if configured.openai_codex {
        return (
            ProviderId::new(openai_codex::PROVIDER_ID),
            ModelId::new(openai_codex::DEFAULT_MODEL),
        );
    }
    if configured.openai {
        return (
            ProviderId::new(openai::PROVIDER_ID),
            ModelId::new(DEFAULT_OPENAI_MODEL),
        );
    }
    if configured.anthropic {
        return (
            ProviderId::new(anthropic::PROVIDER_ID),
            ModelId::new(anthropic::DEFAULT_MODEL),
        );
    }
    if configured.gemini {
        return (
            ProviderId::new(gemini::PROVIDER_ID),
            ModelId::new(gemini::DEFAULT_MODEL),
        );
    }
    if configured.openrouter {
        return (
            ProviderId::new(openrouter::PROVIDER_ID),
            ModelId::new(openrouter::DEFAULT_MODEL),
        );
    }
    (
        ProviderId::new(ollama::PROVIDER_ID),
        ModelId::new(ollama::DEFAULT_MODEL),
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "provider composition and startup selection remain in the binary composition root"
)]
async fn run_tui(
    cli: &Cli,
    secrets: &Arc<dyn SecretStore>,
    store: Arc<SqliteEventStore>,
    configured: ConfiguredProviders,
) -> io::Result<i32> {
    let workspace_root = std::env::current_dir()?;
    let discovery = DiscoveryConfig::for_workspace(workspace_root.clone())
        .map_err(|error| io::Error::other(error.to_string()))?;
    let project_context = tokio::task::spawn_blocking(move || ProjectContext::discover(discovery))
        .await
        .map_err(|error| io::Error::other(format!("project context task failed: {error}")))?
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mcp = match mcp::connect_project(&workspace_root).await {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("mjolnr: {}: {error}", error.reason_code());
            return Ok(1);
        }
    };

    let providers = provider_registry(secrets, &workspace_root);

    // Precedence for a *new* session's model: what the user typed on the command
    // line, then what this workspace last ran on, then the configured-provider
    // default. Reaching for the default every launch is what made the picker
    // feel mandatory.
    let (provider, model) = match (cli.provider.as_deref(), cli.model.as_deref()) {
        (Some(provider), Some(model)) => (ProviderId::new(provider), ModelId::new(model)),
        _ => match remembered_model(store.as_ref(), &workspace_root, &providers).await {
            Some(remembered) => remembered,
            None => default_model(configured),
        },
    };

    // Resolve the session *before* opening a terminal: "that session is open
    // elsewhere" is a sentence, not a screen.
    let resume = match resolve_resume(cli, store.as_ref()).await {
        Ok(session) => session,
        Err(message) => {
            eprintln!("mjolnr: {message}");
            return Ok(1);
        }
    };

    // Best-effort and computed once at startup, exactly like `mcp.servers`
    // below: a bad or missing `.mjolnr/triggers/` must never keep the TUI from
    // opening, so a read failure here degrades to "no triggers shown", not an
    // error.
    let triggers = match smed::triggers::control::root_realpath(&workspace_root) {
        Ok(root_realpath) => {
            smed::triggers::status::collect(store.as_ref(), &workspace_root, &root_realpath)
                .await
                .map(|(statuses, _diagnostics)| statuses)
                .unwrap_or_default()
        }
        Err(_) => Vec::new(),
    };

    // Best-effort and computed once at startup, exactly like `triggers` above:
    // a missing or malformed `.mjolnr/routes/` must never keep the TUI from
    // opening. An empty table restores present-day behaviour exactly (plan
    // §Phase 15).
    let (route_table, _routing_diagnostics) = smed::routing::load_dir(&workspace_root);
    let route_table = Arc::new(route_table);

    let runtime = Runtime::spawn_with_tools_project_context_and_triggers(
        providers,
        Arc::clone(&store) as Arc<dyn EventStore>,
        mcp.registry,
        project_context,
        mcp.servers,
        Arc::new(triggers),
        Arc::clone(&route_table),
    );

    // A dispatch failure here means the actor died before the UI opened — a bug,
    // not a user-facing condition. Fail before opening a terminal that cannot
    // work, rather than showing an empty screen.
    let setup = async {
        runtime
            .dispatch(SmedCommand::OpenProject {
                root: workspace_root,
            })
            .await?;
        if let Some(session) = resume {
            if cli.compact {
                runtime
                    .dispatch(SmedCommand::ResumeCompact {
                        session,
                        provider: cli.provider.as_deref().map(ProviderId::new),
                        model: cli.model.as_deref().map(ModelId::new),
                    })
                    .await?;
            } else {
                runtime
                    .dispatch(SmedCommand::ResumeSession { session })
                    .await?;
            }
        } else {
            runtime
                .dispatch(SmedCommand::CreateSession { provider, model })
                .await?;
            // A brand-new session may open on a route's first hop rather than
            // the configured default. A no-op whenever no
            // `default` task class resolves, including whenever the project
            // has no routing config at all.
            if !route_table.is_empty() {
                runtime
                    .dispatch(SmedCommand::AttachRoute {
                        route: None,
                        role: None,
                        task_class: "default".to_owned(),
                    })
                    .await?;
            }
        }
        if cli.full_auto {
            runtime
                .dispatch(SmedCommand::SetPolicy {
                    mode: smed::core::policy::PolicyMode::FullAuto,
                })
                .await?;
        }
        Ok::<(), smed::core::error::SmedError>(())
    };

    if let Err(error) = setup.await {
        return Err(io::Error::other(error.to_string()));
    }

    let mut terminal = ratatui::try_init()?;
    // The alternate screen exists from here on, so the panic hook may restore
    // it. Set before the guard, which is what clears it again.
    TUI_ACTIVE.store(true, Ordering::Release);
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    );
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let guard = TerminalGuard;

    let result = app::run(&mut terminal, &runtime, &OAuthLogins).await;

    // Teardown is structural: an early `?` inside the loop must not skip it.
    // `close` checkpoints settled state (or preserves an interrupted event
    // tail), drains the store, releases the lease, and only then returns.
    let closed = runtime.close().await;

    // The terminal is restored by the guard when this returns. A durability
    // failure at shutdown is reported rather than swallowed: it is the user's
    // last chance to learn their session did not save.
    if let Err(error) = closed {
        drop(guard);
        eprintln!("mjolnr: the session may not have saved cleanly: {error}");
        return result.map(|()| 1);
    }

    result.map(|()| 0)
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these independent readiness facts are startup diagnostics, not a state machine"
)]
struct ConfiguredProviders {
    openai: bool,
    anthropic: bool,
    openai_codex: bool,
    gemini: bool,
    openrouter: bool,
}

impl ConfiguredProviders {
    /// Whether a credential resolves for any of the held-account providers —
    /// the "no credential resolves for any provider" half of first-run detection
    /// .
    const fn any(self) -> bool {
        self.openai || self.anthropic || self.openai_codex || self.gemini || self.openrouter
    }
}

fn configured_providers(secrets: &Arc<dyn SecretStore>) -> ConfiguredProviders {
    // Keychain access is blocking and macOS may require it on the main thread.
    // Resolve before entering Tokio rather than blocking an async worker or
    // moving the Security framework call to a background thread.
    ConfiguredProviders {
        openai: secrets
            .resolve(
                &ProviderId::new(openai::PROVIDER_ID),
                CredentialKind::ApiKey,
            )
            .is_ok(),
        anthropic: secrets
            .resolve(
                &ProviderId::new(anthropic::PROVIDER_ID),
                CredentialKind::ApiKey,
            )
            .is_ok(),
        openai_codex: secrets
            .resolve(
                &ProviderId::new(openai_codex::PROVIDER_ID),
                CredentialKind::OAuth,
            )
            .is_ok(),
        gemini: secrets
            .resolve(
                &ProviderId::new(gemini::PROVIDER_ID),
                CredentialKind::ApiKey,
            )
            .is_ok(),
        openrouter: secrets
            .resolve(
                &ProviderId::new(openrouter::PROVIDER_ID),
                CredentialKind::ApiKey,
            )
            .is_ok(),
    }
}

/// Which session to open, if resuming.
///
/// The lease is *checked* here and *taken* by the runtime. The check is for the
/// error message; the runtime's `acquire_session` is the atomic gate that
/// actually prevents two writers (`docs/persistence.md` §5). Two processes that
/// raced this check would still be caught there.
async fn resolve_resume(cli: &Cli, store: &SqliteEventStore) -> Result<Option<SessionId>, String> {
    let Some(raw) = cli.resume.as_deref() else {
        return Ok(None);
    };

    let session = uuid::Uuid::parse_str(raw.trim())
        .map(SessionId::from_uuid)
        .map_err(|_| format!("`{raw}` is not a session id — `smed sessions list` shows them"))?;

    let summaries = store
        .sessions()
        .await
        .map_err(|error: StoreError| error.to_string())?;
    let Some(summary) = summaries.into_iter().find(|summary| summary.id == session) else {
        return Err(format!(
            "no session {session} — `smed sessions list` shows them"
        ));
    };

    if summary.status == SessionStatus::Ended {
        return Err(format!(
            "session {session} has ended and cannot accept new work"
        ));
    }

    if summary.leased {
        return Err(format!(
            "session {session} is already open in another smed process.\n       \
             If that process is gone, `smed sessions release {session}` reclaims it."
        ));
    }

    Ok(Some(session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use smed::core::secrets::{Credential, ResolvedCredential, SecretError};

    #[derive(Debug)]
    struct EmptySecrets;

    impl SecretStore for EmptySecrets {
        fn resolve(
            &self,
            provider: &ProviderId,
            _kind: CredentialKind,
        ) -> Result<ResolvedCredential, SecretError> {
            Err(SecretError::NotFound {
                provider: provider.clone(),
            })
        }

        fn store(
            &self,
            _provider: &ProviderId,
            _credential: Credential,
        ) -> Result<(), SecretError> {
            Ok(())
        }

        fn delete(&self, _provider: &ProviderId) -> Result<(), SecretError> {
            Ok(())
        }
    }

    #[test]
    fn the_production_registry_does_not_offer_the_fake_provider() {
        let secrets: Arc<dyn SecretStore> = Arc::new(EmptySecrets);
        let workspace = tempfile::tempdir().unwrap();
        let provider_ids: Vec<ProviderId> = provider_registry(&secrets, workspace.path())
            .iter()
            .map(|provider| provider.id())
            .collect();

        assert!(
            !provider_ids.iter().any(|id| id.as_str() == "fake"),
            "the deterministic fake is a test fixture, not a selectable provider"
        );
        assert!(
            !provider_ids.iter().any(|id| id.as_str() == "forge"),
            "Forge is hidden until its upstream relay works again"
        );
    }

    #[test]
    fn an_unconfigured_setup_falls_back_to_local_ollama_not_the_fake() {
        assert_eq!(
            default_model(ConfiguredProviders::default()),
            (
                ProviderId::new(ollama::PROVIDER_ID),
                ModelId::new(ollama::DEFAULT_MODEL)
            )
        );
    }
}
