//! The credential boundary — the *port* (AGENTS.md §3).
//!
//! Lives in `core` for the same reason [`EventStore`](crate::core::store::EventStore)
//! does: it is a contract, and adapters must be able to resolve a credential
//! without depending on how it is stored. `tests/architecture.rs` caught this —
//! the trait started in `store` and made `providers` depend on `store`, which
//! the dependency direction forbids.
//!
//! Two rules drive every decision here, and they are absolute:
//!
//! 1. **A secret never leaves this boundary in a readable form.** Not into logs,
//!    argv, SQLite, `Debug` output, panics, fixtures, or child environments.
//! 2. **There is no plaintext fallback.** Not even "just for development". A
//!    fallback that exists gets used, and then it is the default.
//!
//! The defence is a type, not discipline. [`Secret`] cannot be printed,
//! formatted, serialised, or compared, and it zeroes itself on drop. Leaking one
//! requires calling [`Secret::expose`], which is greppable and reviewable —
//! whereas `#[derive(Debug)]` on a struct holding a `String` key plus one
//! `tracing` call is a leak nobody sees in review.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::core::model::ProviderId;

/// The storage and lifecycle shape a provider expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    ApiKey,
    OAuth,
}

impl CredentialKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::OAuth => "oauth",
        }
    }
}

impl fmt::Display for CredentialKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A credential.
///
/// Deliberately missing: `Debug`, `Display`, `Serialize`, `Clone`, `PartialEq`.
/// Each omission removes a way to leak this by accident:
///
/// - No derived `Debug` — the classic leak is a struct that holds a key,
///   derives `Debug`, and gets `tracing::debug!("{state:?}")`'d.
/// - No `Display` — so it cannot be formatted into a URL or a log line.
/// - No `Serialize` — so it cannot be written to SQLite or a fixture.
/// - No `PartialEq` — comparison would be non-constant-time, and nothing here
///   needs it.
///
/// The manual `Debug` prints a redaction marker rather than refusing to compile,
/// because a containing struct may legitimately want to derive `Debug`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Secret {
    value: String,
}

impl Secret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self { value }
    }

    /// Read the secret.
    ///
    /// **The only way out.** Named to be obvious in review and greppable in CI:
    /// every call site is a place a credential could escape, so there should be
    /// exactly one per adapter — building the `Authorization` header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.value
    }

    /// Whether the value is empty or whitespace.
    ///
    /// A blank key produces a confusing 401 rather than an honest "you have not
    /// configured this yet", so it is worth catching before a request is sent.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.value.trim().is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the length either: that is information about the credential.
        formatter.write_str("Secret(<redacted>)")
    }
}

/// A smed-owned OAuth token chain.
///
/// Both tokens are secret types and the account id is deliberately omitted
/// from `Debug`: it is an authenticated account identifier, not UI copy.
pub struct OAuthCredential {
    access_token: Secret,
    refresh_token: Secret,
    expires_at_unix: i64,
    account_id: String,
}

impl OAuthCredential {
    #[must_use]
    pub fn new(
        access_token: Secret,
        refresh_token: Secret,
        expires_at_unix: i64,
        account_id: String,
    ) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_at_unix,
            account_id,
        }
    }

    #[must_use]
    pub fn access_token(&self) -> &Secret {
        &self.access_token
    }

    #[must_use]
    pub fn refresh_token(&self) -> &Secret {
        &self.refresh_token
    }

    #[must_use]
    pub const fn expires_at_unix(&self) -> i64 {
        self.expires_at_unix
    }

    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    #[must_use]
    pub fn is_valid_after(&self, unix_time: i64) -> bool {
        self.expires_at_unix > unix_time
    }

    #[must_use]
    pub fn into_parts(self) -> (Secret, Secret, i64, String) {
        (
            self.access_token,
            self.refresh_token,
            self.expires_at_unix,
            self.account_id,
        )
    }
}

impl fmt::Debug for OAuthCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OAuthCredential(<redacted>)")
    }
}

/// A provider credential stored behind the OS-keychain port.
pub enum Credential {
    ApiKey(Secret),
    OAuth(OAuthCredential),
}

impl Credential {
    #[must_use]
    pub const fn kind(&self) -> CredentialKind {
        match self {
            Self::ApiKey(_) => CredentialKind::ApiKey,
            Self::OAuth(_) => CredentialKind::OAuth,
        }
    }

    #[must_use]
    pub fn api_key(&self) -> Option<&Secret> {
        match self {
            Self::ApiKey(secret) => Some(secret),
            Self::OAuth(_) => None,
        }
    }

    #[must_use]
    pub fn oauth(&self) -> Option<&OAuthCredential> {
        match self {
            Self::OAuth(credential) => Some(credential),
            Self::ApiKey(_) => None,
        }
    }

    #[must_use]
    pub fn into_oauth(self) -> Option<OAuthCredential> {
        match self {
            Self::OAuth(credential) => Some(credential),
            Self::ApiKey(_) => None,
        }
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey(_) => formatter.write_str("Credential::ApiKey(<redacted>)"),
            Self::OAuth(_) => formatter.write_str("Credential::OAuth(<redacted>)"),
        }
    }
}

/// Where a credential came from.
///
/// Surfaced in the UI so a user can tell why smed is using a key they did not
/// expect — an environment override silently beating a stored key is a genuinely
/// confusing half hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSource {
    /// A process environment variable. Never persisted (/// "environment override resolution without persistence").
    Environment,
    /// The operating system credential store.
    ///
    /// Only reachable through the one-shot migration off the keychain; nothing
    /// writes here any more.
    Keyring,
    /// An owner-only file in smed's data directory.
    File,
}

impl SecretSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Keyring => "keychain",
            Self::File => "file",
        }
    }
}

/// A resolved credential and where it came from.
#[derive(Debug)]
pub struct ResolvedCredential {
    pub credential: Credential,
    pub source: SecretSource,
}

/// Failures from the credential store.
///
/// No variant carries a secret, and `NotFound` is deliberately distinct from a
/// backend failure: "you have not configured a key" and "the keychain is broken"
/// need different words to the user.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("no credential stored for {provider}")]
    NotFound { provider: ProviderId },

    #[error("credential store unavailable: {detail}")]
    Unavailable { detail: String },

    #[error("credential for {provider} is {found}, expected {expected}")]
    KindMismatch {
        provider: ProviderId,
        expected: CredentialKind,
        found: CredentialKind,
    },
}

/// Reads and writes credentials.
///
/// A trait so tests never touch the real keychain — a test suite that prompts
/// for a login password, or worse, writes to a developer's actual keychain, is
/// one nobody runs.
pub trait SecretStore: Send + Sync + std::fmt::Debug {
    /// Resolve a provider's credential.
    ///
    /// Environment first, then the stored credential. Environment wins so a user can
    /// override without mutating stored state, which is what makes CI and
    /// throwaway keys workable.
    fn resolve(
        &self,
        provider: &ProviderId,
        kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError>;

    fn store(&self, provider: &ProviderId, credential: Credential) -> Result<(), SecretError>;

    fn delete(&self, provider: &ProviderId) -> Result<(), SecretError>;
}

/// The environment variable a provider's key may be supplied through.
///
/// Uses each provider's conventional name (`OPENAI_API_KEY`) rather than a
/// smed-specific one: a user who already exports it expects it to work, and
/// inventing `MJOLNR_OPENAI_KEY` would be a papercut with no benefit.
#[must_use]
pub fn environment_variable(provider: &ProviderId) -> String {
    if provider.as_str() == "lm-studio" {
        return "LM_API_TOKEN".to_owned();
    }
    // Hyphenated provider ids (`lm-studio`) must still yield names a POSIX
    // shell can export, so every non-alphanumeric byte becomes an underscore.
    let name: String = provider
        .as_str()
        .to_uppercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{name}_API_KEY")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_debug_never_reveals_tokens_or_account() {
        let credential = Credential::OAuth(OAuthCredential::new(
            Secret::new("access-secret".to_owned()),
            Secret::new("refresh-secret".to_owned()),
            1_700_000_000,
            "account-secret".to_owned(),
        ));
        let rendered = format!("{credential:?}");

        assert_eq!(rendered, "Credential::OAuth(<redacted>)");
        assert!(!rendered.contains("access-secret"));
        assert!(!rendered.contains("refresh-secret"));
        assert!(!rendered.contains("account-secret"));
    }
}
