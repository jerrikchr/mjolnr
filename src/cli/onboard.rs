//! Guided onboarding flow for first-time setup.
//!
//! One reason to change: how a first-time user is walked from nothing to a
//! working, fully configured mjolnr.
//!
//! # Why this module may print to stdout
//!
//! Like [`super::auth`] and [`super::init`], the wizard runs **instead of** the
//! TUI, never alongside it, so stdout is not the alternate screen. The allowance
//! is per-module and justified rather than crate-wide (`AGENTS.md` §4).
//!
//! # What it may and may not do
//!
//! The flow is auth, file-scaffold, and selection only — it never *acts* in the
//! repo (the Phase 18/21 constraint). Every artifact it produces is a diffable
//! file under `.mjolnr/` (or the owner-only credential store, via `auth`), it
//! previews every write, and it never overwrites a file that already exists.
//! Nothing is sprung.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the onboarding wizard runs instead of the TUI, so stdout is not the alternate screen"
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::model::{ModelId, ModelTier, ProviderId};
use crate::core::secrets::{CredentialKind, SecretStore};
use crate::routing::scaffold::{self, ScaffoldFile, SeededRoute};

use super::auth::{self, AuthCommand, AuthProvider};

/// The default `SOUL.md` a first run offers — mjolnr's standing identity. Inert
/// prose; it confers no capability (see [`crate::context`]).
const STARTER_SOUL: &str = "# SOUL.md — mjolnr's identity\n\n\
    You are mjolnr, a local-first, governed coding harness. You are deliberate and\n\
    honest: you say what you did and did not do, you never report success you have\n\
    not earned, and you seek explicit approval before any side effect.\n";

/// A provider the wizard can offer to connect, with the model its starter route
/// opens on. Ordered by the same preference the runtime uses to pick a new
/// session's default, so the primary route is deterministic.
struct MenuProvider {
    auth: AuthProvider,
    id: &'static str,
    default_model: &'static str,
    /// The credential kind that means "this provider is connected".
    kind: CredentialKind,
}

/// The providers onboarding walks through, OAuth primary then API-key secondary
/// (E8). A subset of the full `auth` catalog: the common held-account providers
/// a first run is likely to have. Anything else remains reachable by
/// `mjolnr auth login` afterwards. The subscription caveat is unchanged —
/// losing one degrades to a narrower mjolnr, never a broken one.
const MENU: &[MenuProvider] = &[
    MenuProvider {
        auth: AuthProvider::Anthropic,
        id: "anthropic",
        default_model: "claude-opus-4-8",
        kind: CredentialKind::OAuth,
    },
    MenuProvider {
        auth: AuthProvider::OpenaiCodex,
        id: "openai-codex",
        default_model: "gpt-5.4",
        kind: CredentialKind::OAuth,
    },
    MenuProvider {
        auth: AuthProvider::GeminiCli,
        id: "gemini-cli",
        default_model: "gemini-2.5-flash",
        kind: CredentialKind::OAuth,
    },
    MenuProvider {
        auth: AuthProvider::Antigravity,
        id: "antigravity",
        default_model: "gemini-2.5-flash",
        kind: CredentialKind::OAuth,
    },
    MenuProvider {
        auth: AuthProvider::Openai,
        id: "openai",
        default_model: "gpt-4o-mini",
        kind: CredentialKind::ApiKey,
    },
    MenuProvider {
        auth: AuthProvider::Gemini,
        id: "gemini",
        default_model: "gemini-2.5-flash",
        kind: CredentialKind::ApiKey,
    },
    MenuProvider {
        auth: AuthProvider::Openrouter,
        id: "openrouter",
        default_model: "openai/gpt-4o-mini",
        kind: CredentialKind::ApiKey,
    },
];

// ---------------------------------------------------------------------------
// First-run detection (Phase 21 Pillar 1 folded in).
//
// Detection is observational and idempotent: it reads the world (credentials,
// the session store, `.mjolnr/`, a decline marker) rather than a flag, so a
// declined user is a durable fact, not a bit that can be lost.
// ---------------------------------------------------------------------------

/// Whether a plain `mjolnr` launch should open the wizard rather than fall back
/// to the silent local default.
///
/// Global first run is: no credential resolves for *any* provider, *and* no
/// session store exists yet, *and* the user has not already declined. All three
/// must hold — a returning user with a session history, a configured user, and a
/// user who said "no thanks" are each left alone.
#[must_use]
pub fn global_first_run(any_credential: bool, session_store_exists: bool, declined: bool) -> bool {
    !any_credential && !session_store_exists && !declined
}

/// Whether this workspace's project half is unconfigured — no `.mjolnr/` at all.
/// The presence of `.mjolnr/` is itself the durable "already offered" record, so
/// a project that has one is never re-offered the project flow.
#[must_use]
pub fn project_first_run(project_root: &Path) -> bool {
    !project_root.join(".mjolnr").exists() && !project_root.join(".mjolnr").exists()
}

/// The owner-scoped file whose presence records "the user declined onboarding".
/// In the user config directory, beside the theme preference — a durable record
/// read the way triggers read state from facts, not a mutable flag.
#[must_use]
pub fn decline_marker_path() -> Option<PathBuf> {
    use etcetera::app_strategy::{AppStrategy, AppStrategyArgs, choose_native_strategy};
    choose_native_strategy(AppStrategyArgs {
        top_level_domain: String::new(),
        author: String::new(),
        app_name: "mjolnr".to_owned(),
    })
    .ok()
    .map(|strategy| strategy.config_dir().join("onboarding-declined"))
}

/// Whether the user has a standing decline on record.
#[must_use]
pub fn has_declined() -> bool {
    decline_marker_path().is_some_and(|path| path.exists())
}

/// Record a durable decline so a second launch does not re-nag. Best-effort: if
/// the marker cannot be written the worst case is being asked once more, which
/// is a nuisance, not a failure — so it never aborts the launch.
fn record_decline() {
    let Some(path) = decline_marker_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _ = std::fs::write(&path, b"declined\n");
}

/// Clear a standing decline. Called when the wizard actually runs to completion,
/// so a user who declined once and later ran `mjolnr init` is not treated as
/// still-declined on the next launch.
fn clear_decline() {
    if let Some(path) = decline_marker_path() {
        let _ = std::fs::remove_file(path);
    }
}

// ---------------------------------------------------------------------------
// Selections → files. The pure core: given what the person chose, what diffable
// `.mjolnr/` files describe it. Every path is under `.mjolnr/`; nothing else.
// ---------------------------------------------------------------------------

/// Everything the wizard collected, ready to be turned into files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selections {
    /// The routes to write, primary first, each with its confirmed roles.
    pub routes: Vec<SeededRoute>,
    /// `SOUL.md` content, or `None` to write no Soul.
    pub soul: Option<String>,
    /// `USER.md` content, or `None` to write no profile.
    pub user_profile: Option<String>,
    /// Remote MCP servers to declare (name, url), or empty to write no config.
    pub mcp_servers: Vec<(String, String)>,
}

/// The diffable `.mjolnr/` files these selections describe. Pure over its input:
/// it reads nothing and writes nothing, so the whole shape can be tested and the
/// CLI decides which of them actually land (never overwriting an existing one).
///
/// Every returned path is under `.mjolnr/` — the flow's confinement invariant.
#[must_use]
pub fn plan_files(selections: &Selections) -> Vec<ScaffoldFile> {
    let mut files = scaffold::generate_with_roles(&selections.routes);
    // A skipped Soul capture leaves the starting template rather than nothing:
    // an owner who pressed Enter still gets a file to read and edit, which is
    // the point of keeping identity on disk at all.
    if let Some(soul) = &selections.soul {
        files.push(identity_file("SOUL.md", soul));
    } else {
        let (_, contents) = crate::context::soul::default_soul();
        files.push(identity_file("SOUL.md", &contents));
    }
    if let Some(profile) = &selections.user_profile {
        files.push(identity_file("USER.md", profile));
    }
    if !selections.mcp_servers.is_empty() {
        files.push(ScaffoldFile {
            relative_path: PathBuf::from(".mjolnr").join("mcp.yaml"),
            contents: mcp_config(&selections.mcp_servers),
        });
    }
    files
}

fn identity_file(name: &str, contents: &str) -> ScaffoldFile {
    let contents = if contents.ends_with('\n') {
        contents.to_owned()
    } else {
        format!("{contents}\n")
    };
    ScaffoldFile {
        relative_path: PathBuf::from(".mjolnr").join(name),
        contents,
    }
}

/// Render `.mjolnr/mcp.yaml` for the chosen remote servers. Each server is a
/// streamable-HTTP endpoint (the Phase 26 rung-a shape); a bearer token, if any,
/// is read from an environment variable named here, never inlined.
fn mcp_config(servers: &[(String, String)]) -> String {
    let mut out = String::from(
        "# Generated by `mjolnr onboard`. Remote MCP servers connect over HTTP.\n\
         # A bearer token, if the server needs one, is read from the named env var.\n\
         servers:\n",
    );
    for (name, url) in servers {
        use std::fmt::Write as _;
        let _ = write!(out, "  - name: \"{name}\"\n    url: \"{url}\"\n");
    }
    out
}

/// mjolnr's curated role suggestion for a chosen model, if it has an opinion.
/// Rendered by the role step as *mjolnr's suggestion*, never a provider fact; a
/// model with no curated tier yields `None`, and the step then asks with no
/// suggestion rather than guessing.
#[must_use]
pub fn suggested_role(provider: &ProviderId, model: &ModelId) -> Option<&'static str> {
    ModelTier::curated(provider, model).map(ModelTier::suggested_role)
}

// ---------------------------------------------------------------------------
// The interactive host.
// ---------------------------------------------------------------------------

/// The theme step's inputs, supplied by the composition root so this flow never
/// depends on `tui` (AGENTS.md §2.1): the shipped themes as `(name, display)`,
/// the currently active name, and a persist function that writes the choice.
#[derive(Debug)]
pub struct ThemeStep {
    pub options: Vec<(String, String)>,
    pub active: String,
    pub persist: fn(&str) -> bool,
}

/// Run the guided onboarding flow. Returns `Ok(())` whether the user completed
/// or declined it; only an unexpected I/O failure is an `Err`.
///
/// `secrets` is the owner-only credential store the provider step writes to (via
/// [`auth`]); `project_root` is where the `.mjolnr/` files are scaffolded; `theme`
/// carries the theme step's data and persist hook from the composition root.
pub fn run_onboarding(
    project_root: &Path,
    secrets: &Arc<dyn SecretStore>,
    theme: &ThemeStep,
) -> Result<(), String> {
    println!("── mjolnr · guided setup ──\n");
    println!(
        "This walks you from nothing to a working mjolnr: connect a provider, confirm\n\
         a model, assign roles, set an identity, and pick a theme. Sensible defaults\n\
         are pre-selected — press Enter to accept each. Nothing is written until you\n\
         confirm a preview at the end.\n"
    );

    if !prompt_yes_no("Set up mjolnr now?", true) {
        record_decline();
        println!("\nNo problem — nothing was written. Run `mjolnr init` any time to pick this up.");
        return Ok(());
    }

    let mut selections = Selections::default();

    // Step 1/6 — connect providers (reuses `mjolnr auth login`'s machinery).
    connect_providers(secrets);

    // Step 2 & 3 — confirm models and assign roles into routes.
    selections.routes = confirm_models_and_roles(secrets);
    if selections.routes.is_empty() {
        println!(
            "\nNo provider is connected yet, so there is no working session to configure.\n\
             You can still set an identity below, then run `mjolnr auth login <provider>`\n\
             followed by `mjolnr init --yes` when you have a key."
        );
    }

    // Step 4 — the Soul and who mjolnr works for.
    capture_identity(project_root, &mut selections);

    // Step 5 — optional remote MCP servers.
    capture_mcp(&mut selections);

    // Step 6 — theme.
    choose_theme(theme);

    // Preview and write, non-destructively, through the same path as `init`.
    let files = plan_files(&selections);
    if files.is_empty() {
        println!("\nNothing to write. Setup complete.");
        clear_decline();
        return Ok(());
    }
    let plan = super::init::plan_writes(&files, project_root);
    println!("\n── Preview ── these files will be written; existing files are left untouched:\n");
    super::init::print_preview(&plan);
    if plan.to_write.is_empty() {
        println!("Every file already exists and was left untouched. Setup complete.");
        clear_decline();
        return Ok(());
    }
    if !super::init::confirm() {
        println!("Aborted — nothing was written.");
        return Ok(());
    }
    if let Err(error) = super::init::write_all(&plan.to_write, project_root) {
        return Err(format!("could not write the setup files: {error}"));
    }

    clear_decline();
    print_summary(&selections, plan.to_write.len());
    Ok(())
}

fn connect_providers(secrets: &Arc<dyn SecretStore>) {
    println!(
        "\nStep 1/6 · Connect a provider  (OAuth subscriptions first; API keys are secondary)"
    );
    println!(
        "  Note: subscription OAuth is opportunistic — losing one narrows mjolnr, never breaks it.\n"
    );
    loop {
        let connected: Vec<&str> = MENU
            .iter()
            .filter(|menu| is_configured(secrets, menu))
            .map(|menu| menu.id)
            .collect();
        if connected.is_empty() {
            println!("  Nothing connected yet.");
        } else {
            println!("  Connected: {}", connected.join(", "));
        }
        println!("  Providers you can connect:");
        for (index, menu) in MENU.iter().enumerate() {
            let mark = if is_configured(secrets, menu) {
                "✓"
            } else {
                " "
            };
            let kind_label = if menu.kind == CredentialKind::OAuth {
                "OAuth"
            } else {
                "API key"
            };
            println!("    [{}] {mark} {}  ({})", index + 1, menu.id, kind_label);
        }
        let answer = prompt_line("  Enter a number to connect one, or press Enter to continue: ");
        let answer = answer.trim();
        if answer.is_empty() {
            return;
        }
        let Some(menu) = answer
            .parse::<usize>()
            .ok()
            .and_then(|choice| MENU.get(choice.wrapping_sub(1)))
        else {
            println!("  Not a listed number.");
            continue;
        };
        let subscription = matches!(
            menu.auth,
            AuthProvider::OpenaiCodex | AuthProvider::Anthropic
        );
        let code = auth::run(
            AuthCommand::Login {
                provider: menu.auth,
                subscription,
            },
            secrets,
        );
        if code != 0 {
            println!("  That connection did not complete; you can try again or continue.");
        }
    }
}

fn confirm_models_and_roles(secrets: &Arc<dyn SecretStore>) -> Vec<SeededRoute> {
    let configured: Vec<&MenuProvider> = MENU
        .iter()
        .filter(|menu| is_configured(secrets, menu))
        .collect();
    if configured.is_empty() {
        return Vec::new();
    }
    println!("\nStep 2/6 · Confirm the model each provider opens on");
    println!("Step 3/6 · Assign a role (mjolnr suggests one from the model's tier)\n");

    let mut routes = Vec::with_capacity(configured.len());
    for menu in configured {
        let provider = ProviderId::new(menu.id);
        // Model: default to the starter model, allow a typed override.
        let model_answer = prompt_line(&format!("  {} model [{}]: ", menu.id, menu.default_model));
        let model_id = {
            let typed = model_answer.trim();
            if typed.is_empty() {
                menu.default_model.to_owned()
            } else {
                typed.to_owned()
            }
        };
        let model = ModelId::new(&model_id);

        // Role: render mjolnr's suggestion if it has one, else ask with none.
        let suggestion = suggested_role(&provider, &model);
        let prompt = suggestion.map_or_else(
            || format!("  role for {} (e.g. plan, smol; blank for none): ", menu.id),
            |role| {
                format!(
                    "  role for {} [mjolnr suggests \"{role}\" for this model]: ",
                    menu.id
                )
            },
        );
        let role_answer = prompt_line(&prompt);
        let roles = resolve_roles(role_answer.trim(), suggestion);
        routes.push(SeededRoute {
            provider,
            model,
            roles,
        });
    }
    routes
}

/// Decide a route's roles from what the person typed and mjolnr's suggestion.
/// Empty input accepts the suggestion (if any); a typed value overrides it;
/// the literal `none` clears it. Pure, so the accept/override/clear rules are
/// testable without a terminal.
fn resolve_roles(typed: &str, suggestion: Option<&str>) -> Vec<String> {
    if typed.is_empty() {
        return suggestion
            .map(|role| vec![role.to_owned()])
            .unwrap_or_default();
    }
    if typed.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    typed
        .split(',')
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(str::to_owned)
        .collect()
}

fn capture_identity(project_root: &Path, selections: &mut Selections) {
    println!("\nStep 4/6 · Identity");
    let config_dir = crate::store::paths::resolve_workspace_config_dir(project_root);
    let existing_soul = config_dir.join("SOUL.md").exists();
    if existing_soul {
        println!("  `SOUL.md` already exists and will be left untouched.");
    } else if prompt_yes_no("  Write a starter SOUL.md (mjolnr's identity)?", true) {
        selections.soul = Some(STARTER_SOUL.to_owned());
    }

    let existing_user = config_dir.join("USER.md").exists();
    if existing_user {
        println!("  `USER.md` already exists and will be left untouched.");
        return;
    }
    println!("  A line or two about who you are and how mjolnr should work for you");
    println!("  becomes `.mjolnr/USER.md`. Press Enter to skip.");
    let about = prompt_line("  You: ");
    let about = about.trim();
    if !about.is_empty() {
        selections.user_profile = Some(format!("# USER.md — who mjolnr works for\n\n{about}\n"));
    }
}

fn capture_mcp(selections: &mut Selections) {
    println!("\nStep 5/6 · MCP servers (optional, advanced)");
    if !prompt_yes_no("  Add a remote MCP server now?", false) {
        return;
    }
    loop {
        let name = prompt_line("  Server name [A-Za-z0-9_-], blank to stop: ");
        let name = name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        if !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
        {
            println!("  Names must use only letters, digits, - and _.");
            continue;
        }
        let url = prompt_line("  Server URL (http:// or https://): ");
        let url = url.trim().to_owned();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            println!("  A remote MCP URL must begin with http:// or https://.");
            continue;
        }
        selections.mcp_servers.push((name, url));
        if !prompt_yes_no("  Add another?", false) {
            return;
        }
    }
}

fn choose_theme(theme: &ThemeStep) {
    println!("\nStep 6/6 · Theme");
    if theme.options.is_empty() {
        return;
    }
    for (index, (name, display)) in theme.options.iter().enumerate() {
        let mark = if *name == theme.active { "•" } else { " " };
        println!("    [{}] {mark} {display}", index + 1);
    }
    let answer = prompt_line("  Pick a number, or press Enter to keep the current theme: ");
    let answer = answer.trim();
    if answer.is_empty() {
        return;
    }
    let Some((name, _)) = answer
        .parse::<usize>()
        .ok()
        .and_then(|choice| theme.options.get(choice.wrapping_sub(1)))
    else {
        println!("  Not a listed number; keeping the current theme.");
        return;
    };
    if (theme.persist)(name) {
        println!("  Theme set to {name}.");
    }
}

fn print_summary(selections: &Selections, written: usize) {
    println!("\n── Setup complete ──");
    println!("  Wrote {written} file(s) under .mjolnr/.");
    if let Some(primary) = selections.routes.first() {
        println!(
            "  Default session: {} on {}.",
            primary.provider.as_str(),
            primary.model.as_str()
        );
        for route in &selections.routes {
            if !route.roles.is_empty() {
                println!(
                    "    {} → roles: {}",
                    route.provider.as_str(),
                    route.roles.join(", ")
                );
            }
        }
    }
    println!("  New project: describe it and mjolnr will draft a PRD with you.");
    println!("  Existing project: run `mjolnr` and try `/help` → discovery.");
    println!("  Run `mjolnr` to open a session. Edit anything under .mjolnr/ freely.");
    println!("\n  Closing step:");
    if project_first_run(std::env::current_dir().as_deref().unwrap_or(Path::new("."))) {
        println!("    → New project → `mjolnr` will interview for a PRD on first session.");
    } else {
        println!("    → Existing project → try `mjolnr` then `/discover` (bounded scan, no LLM).");
    }
}

// ---------------------------------------------------------------------------
// Small terminal helpers.
// ---------------------------------------------------------------------------

fn is_configured(secrets: &Arc<dyn SecretStore>, menu: &MenuProvider) -> bool {
    let id = ProviderId::new(menu.id);
    // Anthropic may hold a subscription OAuth login *or* an API key.
    if matches!(menu.auth, AuthProvider::Anthropic)
        && secrets.resolve(&id, CredentialKind::OAuth).is_ok()
    {
        return true;
    }
    secrets.resolve(&id, menu.kind).is_ok()
}

fn prompt_line(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return String::new();
    }
    answer
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> bool {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let answer = prompt_line(&format!("{prompt} {hint} "));
    match answer.trim() {
        "" => default_yes,
        other => matches!(other, "y" | "Y" | "yes" | "YES"),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "AGENTS.md §7: tests may panic freely"
)]
mod tests {
    use super::*;

    fn route(provider: &str, model: &str, roles: &[&str]) -> SeededRoute {
        SeededRoute {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
        }
    }

    #[test]
    fn global_first_run_needs_all_three_conditions() {
        // The fresh machine: nothing configured, no history, no decline.
        assert!(global_first_run(false, false, false));
        // Any one condition failing leaves the user alone.
        assert!(!global_first_run(true, false, false), "a configured user");
        assert!(!global_first_run(false, true, false), "a returning user");
        assert!(!global_first_run(false, false, true), "a declined user");
    }

    #[test]
    fn project_first_run_is_the_absence_of_dot_mjolnr() {
        let temp = tempfile::tempdir().unwrap();
        assert!(project_first_run(temp.path()), "no .mjolnr/ yet");
        std::fs::create_dir_all(temp.path().join(".mjolnr")).unwrap();
        assert!(
            !project_first_run(temp.path()),
            "the presence of .mjolnr/ is the durable 'already offered' record"
        );
    }

    #[test]
    fn a_wizard_generated_mjolnr_loads_without_diagnostics() {
        // The whole feature rests on this: a defaults run produces a `.mjolnr/`
        // that the Phase 15 loader reads clean and that resolves a default role.
        let selections = Selections {
            routes: vec![
                route("anthropic", "claude-opus-4-8", &["plan"]),
                route("openai", "gpt-4o-mini", &["smol"]),
            ],
            soul: Some(STARTER_SOUL.to_owned()),
            user_profile: Some("# USER.md\n\nJerrik.\n".to_owned()),
            mcp_servers: vec![("docs".to_owned(), "https://example.test/mcp".to_owned())],
        };
        let files = plan_files(&selections);
        let temp = tempfile::tempdir().unwrap();
        super::super::init::write_all(&files, temp.path()).unwrap();

        let (table, diagnostics) = crate::routing::load_dir(temp.path());
        assert!(
            diagnostics.is_empty(),
            "a wizard-generated .mjolnr/ must load clean: {diagnostics:?}"
        );
        // The primary route answers `default` (so a session resolves) and the
        // confirmed roles landed.
        assert_eq!(table.roles.get("default"), Some(&"anthropic".to_owned()));
        assert_eq!(table.roles.get("plan"), Some(&"anthropic".to_owned()));
        assert_eq!(table.roles.get("smol"), Some(&"openai".to_owned()));
    }

    #[test]
    fn every_planned_file_is_confined_to_dot_mjolnr() {
        // The confinement invariant: no step writes outside `.mjolnr/`.
        let selections = Selections {
            routes: vec![route("openai", "gpt-4o", &[])],
            soul: Some("x".to_owned()),
            user_profile: Some("y".to_owned()),
            mcp_servers: vec![("s".to_owned(), "https://h.test".to_owned())],
        };
        for file in plan_files(&selections) {
            assert!(
                file.relative_path.starts_with(".mjolnr"),
                "onboarding wrote outside .mjolnr/: {}",
                file.relative_path.display()
            );
        }
    }

    #[test]
    fn mcp_config_is_only_written_when_a_server_was_added() {
        let mut selections = Selections {
            routes: vec![route("openai", "gpt-4o", &[])],
            ..Selections::default()
        };
        assert!(
            !plan_files(&selections)
                .iter()
                .any(|file| file.relative_path.ends_with("mcp.yaml")),
            "no server means no mcp.yaml — nothing sprung"
        );
        selections
            .mcp_servers
            .push(("s".to_owned(), "https://h.test".to_owned()));
        assert!(
            plan_files(&selections)
                .iter()
                .any(|file| file.relative_path.ends_with("mcp.yaml"))
        );
    }

    #[test]
    fn suggested_role_is_a_suggestion_only_where_mjolnr_has_an_opinion() {
        assert_eq!(
            suggested_role(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-opus-4-8")
            ),
            Some("plan")
        );
        assert_eq!(
            suggested_role(
                &ProviderId::new("anthropic"),
                &ModelId::new("claude-haiku-4-5-20251001")
            ),
            Some("smol")
        );
        assert_eq!(
            suggested_role(
                &ProviderId::new("openai"),
                &ModelId::new("some-unranked-model")
            ),
            None,
            "an unranked model gets no suggestion, not a fabricated one"
        );
    }

    #[test]
    fn resolve_roles_accepts_overrides_and_clears() {
        // Empty input accepts the suggestion.
        assert_eq!(resolve_roles("", Some("plan")), vec!["plan".to_owned()]);
        // Empty input with no suggestion is no roles.
        assert!(resolve_roles("", None).is_empty());
        // A typed value overrides the suggestion.
        assert_eq!(resolve_roles("smol", Some("plan")), vec!["smol".to_owned()]);
        // Comma-separated values become multiple roles.
        assert_eq!(
            resolve_roles("plan, slow", Some("default")),
            vec!["plan".to_owned(), "slow".to_owned()]
        );
        // The literal "none" clears the suggestion.
        assert!(resolve_roles("none", Some("plan")).is_empty());
    }
}
