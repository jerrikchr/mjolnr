//! Credential subcommands.
//!
//! One reason to change: how a provider credential is stored, shown, or removed.
//!
//! # Why this module may print to stdout
//!
//! `AGENTS.md` §4 denies `print_stdout` because the TUI owns the alternate
//! screen and a stray print corrupts it. These paths run **instead of** the TUI,
//! never alongside it, so stdout is theirs. The allowance is per-module and
//! justified rather than crate-wide.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommands run instead of the TUI, so stdout is not the alternate screen"
)]

use std::sync::Arc;

use clap::Subcommand;

use crate::core::model::ProviderId;
use crate::core::secrets::{
    Credential, CredentialKind, Secret, SecretError, SecretSource, SecretStore,
    environment_variable,
};

/// Providers a credential can be stored for.
///
/// A closed set rather than a free string: a typo'd `mjolnr auth login openai2`
/// would otherwise store a key under a name nothing reads, and the user would
/// debug a "missing credential" error with the credential sitting right there.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AuthProvider {
    Jules,
    Anthropic,
    Gemini,
    Openai,
    OpenaiCodex,
    Openrouter,
    GeminiCli,
    Antigravity,
    Nvidia,
    Xai,
    OpencodeZen,
    OpencodeGo,
    VercelGateway,
    CloudflareGateway,
    Vllm,
    LmStudio,
    Deepseek,
    Mistral,
    Groq,
    Together,
    Fireworks,
    Perplexity,
    Moonshot,
    Zhipu,
    Qwen,
    Huggingface,
    TokenRouter,
}

impl AuthProvider {
    fn id(self) -> ProviderId {
        match self {
            Self::Jules => ProviderId::new("jules"),
            Self::Anthropic => ProviderId::new(crate::providers::anthropic::PROVIDER_ID),
            Self::Gemini => ProviderId::new(crate::providers::gemini::PROVIDER_ID),
            Self::Openai => ProviderId::new(crate::providers::openai::PROVIDER_ID),
            Self::OpenaiCodex => ProviderId::new(crate::providers::openai_codex::PROVIDER_ID),
            Self::Openrouter => ProviderId::new(crate::providers::openrouter::PROVIDER_ID),
            Self::GeminiCli => {
                ProviderId::new(crate::providers::gemini_cli::GEMINI_CLI_PROVIDER_ID)
            }
            Self::Antigravity => {
                ProviderId::new(crate::providers::gemini_cli::ANTIGRAVITY_PROVIDER_ID)
            }
            // Phase 16 catalog providers; ids must match openai_compat::CATALOG.
            Self::Nvidia => ProviderId::new("nvidia"),
            Self::Xai => ProviderId::new("xai"),
            Self::OpencodeZen => ProviderId::new("opencode-zen"),
            Self::OpencodeGo => ProviderId::new("opencode-go"),
            Self::VercelGateway => ProviderId::new("vercel-gateway"),
            Self::CloudflareGateway => ProviderId::new("cloudflare-gateway"),
            Self::Vllm => ProviderId::new("vllm"),
            Self::LmStudio => ProviderId::new("lm-studio"),
            Self::Deepseek => ProviderId::new("deepseek"),
            Self::Mistral => ProviderId::new("mistral"),
            Self::Groq => ProviderId::new("groq"),
            Self::Together => ProviderId::new("together"),
            Self::Fireworks => ProviderId::new("fireworks"),
            Self::Perplexity => ProviderId::new("perplexity"),
            Self::Moonshot => ProviderId::new("moonshot"),
            Self::Zhipu => ProviderId::new("zhipu"),
            Self::Qwen => ProviderId::new("qwen"),
            Self::Huggingface => ProviderId::new("huggingface"),
            Self::TokenRouter => ProviderId::new("tokenrouter"),
        }
    }

    const fn credential_kind(self) -> CredentialKind {
        match self {
            Self::OpenaiCodex | Self::GeminiCli | Self::Antigravity => CredentialKind::OAuth,
            _ => CredentialKind::ApiKey,
        }
    }

    /// Credential kinds this provider may hold, most preferred first.
    /// Anthropic can hold either a subscription OAuth login or an API key.
    const fn credential_kinds(self) -> &'static [CredentialKind] {
        match self {
            Self::OpenaiCodex | Self::GeminiCli | Self::Antigravity => &[CredentialKind::OAuth],
            Self::Anthropic => &[CredentialKind::OAuth, CredentialKind::ApiKey],
            _ => &[CredentialKind::ApiKey],
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Store a provider credential in mjolnr's owner-only credential store.
    ///
    /// The key is read from the terminal without echo, never from an argument.
    /// There is deliberately no `--key` flag: argv is world-readable on most
    /// systems and lands in shell history.
    Login {
        provider: AuthProvider,

        /// Log in with the provider's subscription OAuth flow instead of an
        /// API key (Claude Pro/Max for `anthropic`).
        #[arg(long)]
        subscription: bool,
    },

    /// Show which providers have a credential, and where it came from.
    Status,

    /// Remove a stored credential.
    Logout { provider: AuthProvider },
}

/// Run a credential subcommand. Returns the process exit code.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the command is consumed conceptually; taking it by value keeps callers from reusing it"
)]
pub fn run(command: AuthCommand, secrets: &Arc<dyn SecretStore>) -> i32 {
    match command {
        AuthCommand::Login {
            provider,
            subscription,
        } => login(provider, subscription, secrets),
        AuthCommand::Status => status(secrets),
        AuthCommand::Logout { provider } => logout(provider, secrets),
    }
}

fn login(provider: AuthProvider, subscription: bool, secrets: &Arc<dyn SecretStore>) -> i32 {
    if subscription
        && !matches!(
            provider,
            AuthProvider::Anthropic | AuthProvider::OpenaiCodex
        )
    {
        eprintln!(
            "--subscription is available for anthropic (Claude Pro/Max); openai-codex is always a subscription login"
        );
        return 1;
    }
    if subscription || provider.credential_kind() == CredentialKind::OAuth {
        return login_oauth(provider, secrets);
    }
    if matches!(provider, AuthProvider::LmStudio) {
        return login_lm_studio(secrets);
    }

    login_api_key(provider, secrets)
}

fn login_lm_studio(secrets: &Arc<dyn SecretStore>) -> i32 {
    use std::io::Write as _;

    print!("LM Studio server IP or URL [blank keeps current]: ");
    let _ = std::io::stdout().flush();
    let mut address = String::new();
    if let Err(error) = std::io::stdin().read_line(&mut address) {
        eprintln!("could not read the LM Studio server address: {error}");
        return 1;
    }
    let workspace = match std::env::current_dir() {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("could not resolve the current project: {error}");
            return 1;
        }
    };
    let endpoint = if address.trim().is_empty() {
        crate::providers::openai_compat::configured_lm_studio_base_url(&workspace)
    } else {
        crate::providers::openai_compat::persist_lm_studio_base_url(&workspace, address.trim())
    };
    let endpoint = match endpoint {
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    println!(
        "saved LM Studio endpoint {endpoint} in {}",
        crate::providers::openai_compat::lm_studio_config_path(&workspace).display()
    );

    let entered =
        match rpassword::prompt_password("LM Studio API token (optional; blank for keyless): ") {
            Ok(value) => value,
            Err(error) => {
                eprintln!("could not read the token: {error}");
                return 1;
            }
        };
    let secret = Secret::new(entered);
    let provider = ProviderId::new("lm-studio");
    if secret.is_blank() {
        if let Err(error) = secrets.delete(&provider) {
            eprintln!("could not clear the stored LM Studio token: {error}");
            return 1;
        }
        if std::env::var("LM_API_TOKEN").is_ok_and(|value| !value.trim().is_empty()) {
            println!("cleared the stored token; LM_API_TOKEN remains active");
        } else {
            println!("cleared any stored token; mjolnr will connect keylessly");
        }
        return 0;
    }
    match secrets.store(&provider, Credential::ApiKey(secret)) {
        Ok(()) => {
            println!("stored the optional LM Studio token in an owner-only file");
            0
        }
        Err(error) => {
            eprintln!("could not store the token: {error}");
            1
        }
    }
}

fn login_api_key(provider: AuthProvider, secrets: &Arc<dyn SecretStore>) -> i32 {
    let id = provider.id();

    // No echo: a key typed into a terminal otherwise lives in scrollback, and
    // in whatever the user's terminal logs (AGENTS.md §3).
    let entered = match rpassword::prompt_password(format!("{id} API key (input hidden): ")) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("could not read the key: {error}");
            return 1;
        }
    };

    let secret = Secret::new(entered);
    if secret.is_blank() {
        // Storing a blank key produces a confusing 401 later rather than an
        // honest "you have not configured this".
        eprintln!("no key entered; nothing stored");
        return 1;
    }

    match secrets.store(&id, Credential::ApiKey(secret)) {
        Ok(()) => {
            println!("stored a credential for {id} in an owner-only file");

            // An environment override silently beating the key just stored is a
            // genuinely confusing half hour. Say so now.
            let variable = environment_variable(&id);
            if std::env::var(&variable).is_ok_and(|value| !value.trim().is_empty()) {
                println!(
                    "note: {variable} is set and takes precedence over the stored key. \
                     Unset it to use what you just stored."
                );
            }
            0
        }
        Err(error) => {
            eprintln!("could not store the credential: {error}");
            1
        }
    }
}

fn login_oauth(provider: AuthProvider, secrets: &Arc<dyn SecretStore>) -> i32 {
    let id = provider.id();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("could not start the login runtime: {error}");
            return 1;
        }
    };
    // The two flows carry different error types; the exit path only needs
    // words, so both collapse to the rendered message.
    let result: Result<i64, String> = match provider {
        AuthProvider::Anthropic => runtime
            .block_on(crate::providers::anthropic::paste_login(
                Arc::clone(secrets),
                |prompt| {
                    println!("Open this URL in your browser and authorize mjolnr:");
                    println!("{}", prompt.authorize_url);
                    println!("The final page displays an authorization code.");
                },
                || {
                    print!("Paste the authorization code here: ");
                    let _ = std::io::Write::flush(&mut std::io::stdout());
                    let mut pasted = String::new();
                    std::io::stdin().read_line(&mut pasted).map_err(|error| {
                        crate::providers::anthropic::OAuthError::Protocol {
                            detail: format!("could not read the pasted code: {error}"),
                        }
                    })?;
                    Ok(pasted)
                },
            ))
            .map_err(|error| error.to_string()),
        AuthProvider::GeminiCli | AuthProvider::Antigravity => {
            let config = if matches!(provider, AuthProvider::GeminiCli) {
                &crate::providers::gemini_cli::GEMINI_CLI
            } else {
                &crate::providers::gemini_cli::ANTIGRAVITY
            };
            runtime
                .block_on(crate::providers::gemini_cli::browser_login(
                    config,
                    Arc::clone(secrets),
                    |prompt| {
                        println!("Open this URL in your browser and authorize mjolnr:");
                        println!("{}", prompt.authorize_url);
                        println!("Waiting for the browser callback (up to 15 minutes)…");
                    },
                ))
                .map_err(|error| error.to_string())
        }
        _ => runtime
            .block_on(crate::providers::openai_codex::device_login(
                Arc::clone(secrets),
                |prompt| {
                    println!("Open {} in your browser.", prompt.verification_url);
                    println!("Enter this one-time code: {}", prompt.user_code);
                    println!("Waiting for authorization (up to 15 minutes)…");
                },
            ))
            .map_err(|error| error.to_string()),
    };
    match result {
        Ok(expires_at_unix) => {
            println!(
                "stored a mjolnr-owned OAuth credential for {id} in an owner-only file\n\
                 access token expires at Unix time {expires_at_unix}; mjolnr refreshes it automatically"
            );
            0
        }
        Err(error) => {
            eprintln!("could not complete {id} login: {error}");
            1
        }
    }
}

/// Every provider a credential can be stored for.
///
/// One array so `status` cannot drift out of sync with [`AuthProvider`]: adding
/// a provider without listing it here would make it invisible to `auth status`,
/// and a credential you cannot see is one you cannot debug.
const ALL_AUTH_PROVIDERS: &[AuthProvider] = &[
    AuthProvider::Jules,
    AuthProvider::Anthropic,
    AuthProvider::Gemini,
    AuthProvider::Openai,
    AuthProvider::OpenaiCodex,
    AuthProvider::Openrouter,
    AuthProvider::GeminiCli,
    AuthProvider::Antigravity,
    AuthProvider::Nvidia,
    AuthProvider::Xai,
    AuthProvider::OpencodeZen,
    AuthProvider::OpencodeGo,
    AuthProvider::VercelGateway,
    AuthProvider::CloudflareGateway,
    AuthProvider::Vllm,
    AuthProvider::LmStudio,
    AuthProvider::Deepseek,
    AuthProvider::Mistral,
    AuthProvider::Groq,
    AuthProvider::Together,
    AuthProvider::Fireworks,
    AuthProvider::Perplexity,
    AuthProvider::Moonshot,
    AuthProvider::Zhipu,
    AuthProvider::Qwen,
    AuthProvider::Huggingface,
    AuthProvider::TokenRouter,
];

fn status(secrets: &Arc<dyn SecretStore>) -> i32 {
    // Never prints the credential or its length — only whether one resolves and
    // where from.
    for provider in ALL_AUTH_PROVIDERS {
        let id = provider.id();
        // Try each kind the provider may hold (e.g. anthropic: subscription
        // OAuth first, then API key) so a valid credential of either kind
        // never reports as "wrong kind".
        let mut outcome = None;
        for kind in provider.credential_kinds() {
            match secrets.resolve(&id, *kind) {
                Ok(resolved) => {
                    outcome = Some(Ok((*kind, resolved)));
                    break;
                }
                Err(error) => {
                    let terminal = !matches!(
                        error,
                        SecretError::NotFound { .. } | SecretError::KindMismatch { .. }
                    );
                    outcome = Some(Err(error));
                    if terminal {
                        break;
                    }
                }
            }
        }
        match outcome {
            Some(Ok((kind, resolved))) => {
                println!("{id}: configured ({kind}, {})", resolved.source.label());
                if resolved.source == SecretSource::Environment {
                    println!("  from {}", environment_variable(&id));
                }
                if let Some(oauth) = resolved.credential.oauth() {
                    println!(
                        "  access token expires at Unix time {}",
                        oauth.expires_at_unix()
                    );
                }
            }
            Some(Err(SecretError::NotFound { .. } | SecretError::KindMismatch { .. })) | None => {
                if matches!(provider, AuthProvider::LmStudio | AuthProvider::Vllm) {
                    println!(
                        "{id}: no API token configured (optional); runtime checks the local server"
                    );
                } else {
                    println!("{id}: not configured — run `mjolnr auth login {id}`");
                }
            }
            Some(Err(error)) => {
                println!("{id}: credential store unavailable ({error})");
            }
        }
    }
    0
}

fn logout(provider: AuthProvider, secrets: &Arc<dyn SecretStore>) -> i32 {
    let id = provider.id();
    match secrets.delete(&id) {
        Ok(()) => {
            println!("removed any stored credential for {id}");

            let variable = environment_variable(&id);
            if provider.credential_kind() == CredentialKind::ApiKey
                && std::env::var(&variable).is_ok_and(|value| !value.trim().is_empty())
            {
                // Otherwise "I logged out but it still works" looks like a bug.
                println!(
                    "note: {variable} is still set, so {id} remains configured from the environment"
                );
            }
            0
        }
        Err(error) => {
            eprintln!("could not remove the credential: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalog provider missing here could stream but never be logged into,
    /// and its credential would be invisible to `auth status`.
    #[test]
    fn every_catalog_provider_is_reachable_from_auth() {
        for descriptor in crate::providers::openai_compat::CATALOG {
            assert!(
                ALL_AUTH_PROVIDERS
                    .iter()
                    .any(|provider| provider.id().as_str() == descriptor.id),
                "catalog provider {} has no auth command",
                descriptor.id
            );
        }
    }

    #[test]
    fn hyphenated_ids_produce_shell_safe_environment_variables() {
        assert_eq!(
            environment_variable(&ProviderId::new("lm-studio")),
            "LM_API_TOKEN"
        );
    }
}
