//! The command line.
//!
//! `smed` with no arguments opens the TUI on a new session. The subcommands
//! exist for the things a terminal UI is the wrong shape for: storing a
//! credential without echoing it, listing sessions before choosing one, and
//! inspecting the database when something is wrong with it.
//!
//! # Why sessions and diagnostics are commands, not slash commands
//!
//!  lists `/sessions`, `/new`, `/resume`, and `/diagnostics` as possible
//! TUI commands. The operator-critical forms remain CLI commands because they
//! must work before a runtime exists or when the TUI cannot start. The TUI keeps
//! its compact `/skills` and `/model` commands; session selection and database
//! integrity remain explicit process-level operations.
//!
//! - `smed diagnostics` must work when the TUI cannot start, which is exactly
//!   when a database diagnostic matters most.
//! - Choosing a session happens *before* a runtime exists to hold it. A slash
//!   command would have to tear down and rebuild the session it is running in.
//!
//! Recorded as a deviation in the Phase 4 report rather than resolved silently.

pub mod auth;
pub mod init;
pub mod onboard;
pub mod plugin;
pub mod sessions;
pub mod triggers;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::core::secrets::SecretStore;
use crate::core::store::StoreError;
use crate::store::paths;

#[derive(Debug, Parser)]
#[command(
    name = "mjolnr",
    version,
    about = "A local-first terminal AI coding harness"
)]
pub struct Cli {
    /// Use this directory for mjolnr's database instead of the platform default.
    ///
    /// The seam that makes a disposable smoke test possible: without it, trying
    /// mjolnr out means writing to the same database as real work.
    #[arg(long, global = true, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Resume an existing session instead of starting a new one.
    #[arg(long, value_name = "SESSION_ID")]
    pub resume: Option<String>,

    /// Seed a resumed provider with the latest handoff and bounded recent turns.
    #[arg(long)]
    pub compact: bool,

    /// Provider for a cross-model compact resume; requires `--model`.
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Model for a cross-model compact resume; requires `--provider`.
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,

    /// Start a fresh session in full-auto policy. Structural guards remain in
    /// force; this is not an OS sandbox.
    #[arg(long)]
    pub full_auto: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Where the database lives for this invocation.
    ///
    /// # Errors
    /// When no platform data directory resolves, or it cannot be created.
    pub fn database_path(&self) -> Result<PathBuf, paths::PathError> {
        match &self.data_dir {
            Some(directory) => paths::database_path_in(directory),
            None => paths::default_database_path(),
        }
    }

    /// Why this invocation makes no sense, if it does not.
    ///
    /// `--resume` opens a terminal; a subcommand prints and exits. Asking for
    /// both is a typo, and running one while silently dropping the other is the
    /// kind of "helpful" behaviour that has someone staring at a session list
    /// wondering why their session did not open.
    ///
    /// Checked here rather than with clap's `args_conflicts_with_subcommands`,
    /// which also rejects the **global** `--data-dir` when it precedes a
    /// subcommand — turning `smed --data-dir /tmp diagnostics` into an error.
    /// A manual smoke test caught that; the unit test had written the flag on
    /// the other side of the subcommand and never saw it.
    #[must_use]
    pub fn conflict(&self) -> Option<&'static str> {
        if self.command.is_some()
            && (self.resume.is_some()
                || self.full_auto
                || self.compact
                || self.provider.is_some()
                || self.model.is_some())
        {
            return Some("terminal launch flags cannot be combined with a subcommand");
        }
        if self.resume.is_some() && self.full_auto {
            return Some("--full-auto cannot re-grant authority while resuming a session");
        }
        if self.compact && self.resume.is_none() {
            return Some("--compact requires --resume");
        }
        if self.provider.is_some() != self.model.is_some() {
            return Some("--provider and --model must be supplied together");
        }
        if self.provider.is_some() && !self.compact {
            return Some("cross-model resume requires --compact");
        }
        None
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run one governed directive without opening a terminal UI.
    Exec(ExecArgs),

    /// Manage provider credentials.
    #[command(subcommand)]
    Auth(auth::AuthCommand),

    /// Set mjolnr up: connect a provider, confirm a model, assign roles, choose
    /// an identity and a theme, then write `.mjolnr/`. Nothing is written until
    /// you confirm a preview, and an existing file is never overwritten.
    Init {
        /// Skip the wizard and only scaffold the routing config, without
        /// prompting. For scripts and CI. Still never overwrites.
        #[arg(long)]
        yes: bool,
    },

    /// Inspect and manage stored sessions.
    #[command(subcommand)]
    Sessions(sessions::SessionsCommand),

    /// Manage and run scheduled/webhook triggers.
    #[command(subcommand)]
    Triggers(triggers::TriggersCommand),

    /// Deprecated alias for `mjolnr init`. Hidden rather than
    /// removed: it is what the wizard was called before `init` became the
    /// obvious name for it, and breaking a scripted invocation to tidy the
    /// help output is a bad trade.
    #[command(hide = true)]
    Onboard,

    /// Report on the database: path, schema version, WAL state, and counts.
    Diagnostics {
        /// Also run `PRAGMA integrity_check`.
        ///
        /// Off by default because it is O(N log N) over the whole database and
        ///  forbids running it on every launch. This flag is the
        /// "explicit diagnostic action" that requirement asks for.
        #[arg(long)]
        integrity: bool,
    },

    /// Manage third-party plugins (ADR-0016).
    #[command(subcommand)]
    Plugin(plugin::PluginCommand),
}

#[derive(Debug, Args)]
pub struct ExecArgs {
    /// The directive to run. This is ordinary task text, never a credential.
    pub directive: String,

    /// Non-interactive policy. Ask is intentionally unavailable.
    #[arg(long, value_enum, default_value_t = ExecPolicy::ReadOnly)]
    pub policy: ExecPolicy,

    /// Provider to use; requires --model.
    #[arg(long, requires = "model")]
    pub provider: Option<String>,

    /// Model to use; requires --provider.
    #[arg(long, requires = "provider")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExecPolicy {
    ReadOnly,
    WorkspaceWrite,
    FullAuto,
}

impl From<ExecPolicy> for crate::core::policy::PolicyMode {
    fn from(policy: ExecPolicy) -> Self {
        match policy {
            ExecPolicy::ReadOnly => Self::ReadOnly,
            ExecPolicy::WorkspaceWrite => Self::WorkspaceWrite,
            ExecPolicy::FullAuto => Self::FullAuto,
        }
    }
}

/// Run a credential subcommand. Returns the process exit code.
#[must_use]
pub fn run_auth(command: auth::AuthCommand, secrets: &Arc<dyn SecretStore>) -> i32 {
    auth::run(command, secrets)
}

/// Run a subcommand that needs the database. Returns the process exit code.
///
/// `main` removes [`Command::Auth`] before the database is opened, so that arm
/// is unreachable. It returns an exit code rather than panicking: a wrong route
/// is a bug, but a bug in the CLI must not corrupt the terminal (`AGENTS.md`
/// §4).
pub async fn run_with_store(command: Command, store: &sessions::Store) -> Result<i32, StoreError> {
    match command {
        // Auth, Exec, Init, Onboard, and Plugin run before the store is opened; main removes
        // them from this path, so these arms are unreachable defence.
        Command::Auth(_)
        | Command::Exec(_)
        | Command::Init { .. }
        | Command::Onboard
        | Command::Plugin(_) => Ok(2),
        Command::Sessions(command) => sessions::run(command, store).await,
        Command::Diagnostics { integrity } => sessions::diagnostics(store, integrity).await,
        Command::Triggers(command) => {
            let workspace_root = std::env::current_dir().unwrap_or_default();
            triggers::run(command, store, &workspace_root).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_subcommand_accepts_a_key_as_an_argument() {
        // The invariant: argv is world-readable and lands in shell history, so
        // there must be no way to pass a credential on the command line. If
        // someone adds `--key`, this test is where the conversation happens.
        fn check(command: &clap::Command) {
            for argument in command.get_arguments() {
                let name = argument.get_id().as_str().to_ascii_lowercase();
                assert!(
                    !name.contains("key") && !name.contains("secret") && !name.contains("token"),
                    "`{}` takes a credential-shaped argument `{name}`: secrets must never \
                     travel through argv (AGENTS.md §3)",
                    command.get_name()
                );
            }

            for subcommand in command.get_subcommands() {
                check(subcommand);
            }
        }

        check(&Cli::command());
    }

    #[test]
    fn bare_smed_opens_the_tui() {
        let cli = Cli::try_parse_from(["smed"]).expect("parse");
        assert!(cli.command.is_none());
        assert!(cli.resume.is_none());
    }

    #[test]
    fn headless_policy_defaults_closed_and_has_no_ask_value() {
        let cli = Cli::try_parse_from(["smed", "exec", "inspect the repository"]).expect("parse");
        let Some(Command::Exec(args)) = cli.command else {
            panic!("exec command");
        };
        assert_eq!(args.policy, ExecPolicy::ReadOnly);
        assert!(Cli::try_parse_from(["smed", "exec", "work", "--policy", "ask"]).is_err());
    }

    #[test]
    fn auth_login_requires_a_known_provider() {
        assert!(Cli::try_parse_from(["smed", "auth", "login", "openai"]).is_ok());
        assert!(Cli::try_parse_from(["smed", "auth", "login", "anthropic"]).is_ok());
        assert!(Cli::try_parse_from(["smed", "auth", "login", "openai-codex"]).is_ok());
        assert!(
            Cli::try_parse_from(["smed", "auth", "login", "opeanai"]).is_err(),
            "a typo'd provider must be rejected, not stored under a name nothing reads"
        );
    }

    #[test]
    fn init_is_the_wizard_and_only_yes_is_the_silent_scaffold() {
        // The distinction main.rs branches on. `smed init` bare must stay the
        // guided flow — a user reaching for `init` on a fresh machine wants
        // setup, not two YAML files and a prompt about them.
        let bare = Cli::try_parse_from(["smed", "init"]).expect("parse");
        assert!(matches!(bare.command, Some(Command::Init { yes: false })));

        // `--yes` is the scriptable path. If this ever parsed as the wizard, CI
        // would hang on a prompt nothing is there to answer.
        let scripted = Cli::try_parse_from(["smed", "init", "--yes"]).expect("parse");
        assert!(matches!(
            scripted.command,
            Some(Command::Init { yes: true })
        ));
    }

    #[test]
    fn onboard_still_parses_as_the_hidden_alias() {
        // Hidden from help, not removed: a scripted `smed onboard` must keep
        // working even though `init` is the name we now document.
        let cli = Cli::try_parse_from(["smed", "onboard"]).expect("parse");
        assert!(matches!(cli.command, Some(Command::Onboard)));
    }

    #[test]
    fn a_disposable_data_directory_can_be_selected() {
        // Without this the smoke test in the Phase 4 report would have to write
        // to the developer's real database.
        let cli = Cli::try_parse_from(["smed", "--data-dir", "/tmp/smed-test"]).expect("parse");
        assert_eq!(cli.data_dir, Some(PathBuf::from("/tmp/smed-test")));
    }

    #[test]
    fn the_data_directory_applies_to_subcommands_on_either_side() {
        // Both orders, because a manual smoke test found that only one of them
        // worked: `args_conflicts_with_subcommands` rejected the global flag when
        // it came *first*, which is where anyone would naturally type it. The
        // original test wrote it last and passed while `smed --data-dir /tmp
        // diagnostics` was broken.
        for arguments in [
            ["smed", "diagnostics", "--data-dir", "/tmp/x"],
            ["smed", "--data-dir", "/tmp/x", "diagnostics"],
        ] {
            let cli = Cli::try_parse_from(arguments).expect("parse");
            assert_eq!(
                cli.data_dir,
                Some(PathBuf::from("/tmp/x")),
                "--data-dir must work before and after a subcommand: {arguments:?}"
            );
            assert!(matches!(cli.command, Some(Command::Diagnostics { .. })));
        }
    }

    #[test]
    fn the_data_directory_reaches_every_subcommand() {
        for arguments in [
            vec!["smed", "--data-dir", "/tmp/x", "sessions", "list"],
            vec!["smed", "--data-dir", "/tmp/x", "auth", "status"],
        ] {
            let cli = Cli::try_parse_from(&arguments).expect("parse");
            assert_eq!(cli.data_dir, Some(PathBuf::from("/tmp/x")), "{arguments:?}");
        }
    }

    #[test]
    fn resume_takes_a_session_id() {
        let cli = Cli::try_parse_from(["smed", "--resume", "abc"]).expect("parse");
        assert_eq!(cli.resume.as_deref(), Some("abc"));
    }

    #[test]
    fn compact_resume_can_name_a_new_provider_and_model() {
        let cli = Cli::try_parse_from([
            "smed",
            "--resume",
            "abc",
            "--compact",
            "--provider",
            "fake",
            "--model",
            "fake-1",
        ])
        .expect("parse");
        assert!(cli.compact);
        assert_eq!(cli.provider.as_deref(), Some("fake"));
        assert!(cli.conflict().is_none());

        let cli = Cli::try_parse_from(["smed", "--compact"]).expect("parse");
        assert!(cli.conflict().is_some());
    }

    #[test]
    fn full_auto_is_explicit_and_cannot_be_combined_with_resume() {
        let cli = Cli::try_parse_from(["smed", "--full-auto"]).expect("parse");
        assert!(cli.full_auto);
        assert!(cli.conflict().is_none());

        let cli = Cli::try_parse_from(["smed", "--resume", "abc", "--full-auto"]).expect("parse");
        assert!(cli.conflict().is_some());
    }

    #[test]
    fn integrity_is_opt_in() {
        //  forbids running integrity_check on every launch, so it must not
        // be the default for the command either.
        let cli = Cli::try_parse_from(["smed", "diagnostics"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Command::Diagnostics { integrity: false })
        ));

        let cli = Cli::try_parse_from(["smed", "diagnostics", "--integrity"]).expect("parse");
        assert!(matches!(
            cli.command,
            Some(Command::Diagnostics { integrity: true })
        ));
    }

    #[test]
    fn sessions_can_be_listed_and_released() {
        assert!(matches!(
            Cli::try_parse_from(["smed", "sessions", "list"]).map(|cli| cli.command),
            Ok(Some(Command::Sessions(sessions::SessionsCommand::List)))
        ));
        assert!(Cli::try_parse_from(["smed", "sessions", "release", "abc"]).is_ok());
        // Release without a target would be ambiguous about which lease to break.
        assert!(Cli::try_parse_from(["smed", "sessions", "release"]).is_err());
    }

    #[test]
    fn resume_and_a_subcommand_are_reported_as_a_conflict() {
        // One opens a terminal, the other prints and exits. Doing one silently is
        // worse than refusing both.
        let cli = Cli::try_parse_from(["smed", "--resume", "abc", "diagnostics"]).expect("parse");
        assert!(cli.conflict().is_some());

        // Each alone is fine, and so is neither.
        assert!(
            Cli::try_parse_from(["smed", "diagnostics"])
                .expect("parse")
                .conflict()
                .is_none()
        );
        assert!(
            Cli::try_parse_from(["smed", "--resume", "abc"])
                .expect("parse")
                .conflict()
                .is_none()
        );
        assert!(
            Cli::try_parse_from(["smed"])
                .expect("parse")
                .conflict()
                .is_none()
        );
    }
}
