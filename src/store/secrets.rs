//! The owner-only file implementation of the
//! [`SecretStore`](crate::core::secrets::SecretStore) port.
//!
//! The port and its types live in `core::secrets`; only the storage-specific
//! code is here. That split is what lets a provider adapter resolve a credential
//! without depending on `store` (AGENTS.md §2.1).
//!
//! # Why not the OS keyring
//!
//! smed used the platform keychain until it became clear what that costs. The
//! keychain binds each item's ACL to the *code signature* of the binary that
//! created it, so every rebuild or upgrade reads as a different application and
//! challenges the user for their login password. Worse, on Linux it resolves to
//! Secret Service over D-Bus, which simply does not exist in a container, over
//! SSH, or on a headless box — so `install && run` was never true there.
//!
//! The comparison set is one-sided: `pi`, `opencode`, and xAI's `grok-build` all
//! store credentials in an owner-only JSON file and depend on no keyring at all.
//! Claude Code is the lone keychain holdout, and its changelog is a running
//! ledger of the price — refresh races clobbering fresh tokens, locked keychains
//! surfacing as "Not logged in", `security -i` buffer overflows corrupting
//! entries, per-turn UI stalls, and parallel sessions all logging out together
//! after wake-from-sleep. That last one is aimed at smed specifically, which
//! runs concurrent leased sessions by design.
//!
//! What is given up is real but narrow: an owner-only file is readable by any
//! process already running as this user. Such a process can also read smed's
//! memory, so the keychain's marginal protection did not survive the trade.

use std::path::{Path, PathBuf};

use crate::core::model::ProviderId;
use crate::core::secrets::{
    Credential, CredentialKind, OAuthCredential, ResolvedCredential, Secret, SecretError,
    SecretSource, SecretStore, environment_variable,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// The keyring service name, retained only so the one-shot migration can find
/// credentials written by earlier versions.
const SERVICE: &str = "dev.smed";

/// Credentials live in their own file per provider, each owner-only.
///
/// One file per provider rather than a shared `auth.json` (which is what `pi`
/// and `opencode` do) because a shared map has to be read, mutated, and written
/// back: two smed sessions refreshing different providers at once can then lose
/// one another's writes. Separate files make each save an independent atomic
/// rename, so that whole class of race does not arise.
#[derive(Debug)]
pub struct OsSecretStore {
    directory: PathBuf,
}

impl Default for OsSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl OsSecretStore {
    /// The store rooted at the platform data directory.
    ///
    /// A directory that cannot be resolved is not fatal here: it surfaces as
    /// `Unavailable` on first use, where there is a user to tell. Failing at
    /// construction would take the whole TUI down over a credential problem.
    #[must_use]
    pub fn new() -> Self {
        Self {
            directory: crate::store::paths::default_credentials_dir().unwrap_or_default(),
        }
    }

    /// The store rooted at an explicit directory. The seam that keeps every test
    /// off the developer's real credentials (`AGENTS.md` §7).
    #[must_use]
    pub fn with_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The file backing one provider.
    ///
    /// The id is sanitised into the file name rather than trusted: provider ids
    /// are internal today, but a `../` reaching out of the credentials directory
    /// is not a bug worth leaving available to a future caller.
    fn path(&self, provider: &ProviderId) -> Result<PathBuf, SecretError> {
        if self.directory.as_os_str().is_empty() {
            return Err(SecretError::Unavailable {
                detail: "no credentials directory could be resolved for this user".to_owned(),
            });
        }
        let name: String = provider
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        Ok(self.directory.join(format!("{name}.json")))
    }

    fn decode(
        provider: &ProviderId,
        expected: CredentialKind,
        mut value: String,
    ) -> Result<Credential, SecretError> {
        let parsed = serde_json::from_str::<StoredCredential>(&value);
        if parsed.is_err() && expected == CredentialKind::ApiKey {
            return Ok(Credential::ApiKey(Secret::new(value)));
        }
        value.zeroize();
        let stored = parsed.map_err(|error| SecretError::Unavailable {
            detail: format!("stored credential for {provider} could not be decoded: {error}"),
        })?;
        let credential = stored.into_credential();
        let found = credential.kind();
        if found != expected {
            return Err(SecretError::KindMismatch {
                provider: provider.clone(),
                expected,
                found,
            });
        }
        Ok(credential)
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum StoredCredential {
    ApiKey {
        value: String,
    },
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at_unix: i64,
        account_id: String,
    },
}

impl StoredCredential {
    fn from_credential(credential: &Credential) -> Self {
        match credential {
            Credential::ApiKey(secret) => Self::ApiKey {
                value: secret.expose().to_owned(),
            },
            Credential::OAuth(oauth) => Self::OAuth {
                access_token: oauth.access_token().expose().to_owned(),
                refresh_token: oauth.refresh_token().expose().to_owned(),
                expires_at_unix: oauth.expires_at_unix(),
                account_id: oauth.account_id().to_owned(),
            },
        }
    }

    fn into_credential(mut self) -> Credential {
        match &mut self {
            Self::ApiKey { value } => Credential::ApiKey(Secret::new(std::mem::take(value))),
            Self::OAuth {
                access_token,
                refresh_token,
                expires_at_unix,
                account_id,
            } => Credential::OAuth(OAuthCredential::new(
                Secret::new(std::mem::take(access_token)),
                Secret::new(std::mem::take(refresh_token)),
                *expires_at_unix,
                std::mem::take(account_id),
            )),
        }
    }
}

/// Move any credentials still in the OS keyring into the file store.
///
/// Runs once at startup and is a no-op for anyone who never used a keychain
/// build — which is everyone installing smed from here on. Kept deliberately
/// small: when enough time has passed that no keychain-era install is plausible,
/// this function and the `keyring` dependency come out together.
///
/// Best-effort by construction. A keychain read may prompt for the login
/// password (that is the very friction being removed, one last time) and the
/// user may cancel it. A cancelled or failed migration must not stop smed from
/// starting — the credential is simply still in the keychain, and `smed auth
/// login` remains the way out. The keychain entry is left in place rather than
/// deleted, so a failure part-way cannot lose a credential.
///
/// Returns the providers actually migrated, so the caller can say so.
#[must_use]
pub fn migrate_from_keyring(store: &OsSecretStore, providers: &[ProviderId]) -> Vec<ProviderId> {
    let mut migrated = Vec::new();
    for provider in providers {
        // Never overwrite a file-store credential with a stale keychain one.
        if store.path(provider).is_ok_and(|path| path.exists()) {
            continue;
        }
        let Ok(entry) = keyring::Entry::new(SERVICE, provider.as_str()) else {
            continue;
        };
        let Ok(value) = entry.get_password() else {
            continue;
        };
        let Ok(path) = store.path(provider) else {
            continue;
        };
        if OsSecretStore::write_atomically(&path, &Zeroizing::new(value)).is_ok() {
            migrated.push(provider.clone());
        }
    }
    migrated
}

/// Force `path` to owner-only, and report what it was.
///
/// Applied on read as well as write, so a file some other tool (or a careless
/// `chmod -R`) loosened is tightened the next time smed touches it rather than
/// staying world-readable until someone notices.
#[cfg(unix)]
fn ensure_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let mut permissions = metadata.permissions();
            if permissions.mode() & 0o777 != 0o600 {
                permissions.set_mode(0o600);
                std::fs::set_permissions(path, permissions)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Windows has no mode bits; the file inherits the user profile's ACL.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn ensure_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

impl OsSecretStore {
    /// Write `contents` to `path` atomically, owner-only throughout.
    ///
    /// Via a temporary file and a rename so a crash mid-write cannot leave a
    /// truncated credential behind: readers see either the old file or the new
    /// one. The temporary is created `0o600` and re-checked before the rename,
    /// because the create mode is subject to the process umask.
    fn write_atomically(path: &Path, contents: &str) -> Result<(), SecretError> {
        let unavailable = |error: std::io::Error| SecretError::Unavailable {
            detail: format!("credential file could not be written: {error}"),
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(unavailable)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(parent) {
                    let mut permissions = metadata.permissions();
                    if permissions.mode() & 0o777 != 0o700 {
                        permissions.set_mode(0o700);
                        let _ = std::fs::set_permissions(parent, permissions);
                    }
                }
            }
        }

        let temporary = path.with_extension("tmp");
        {
            use std::io::Write;
            #[cfg(unix)]
            let file = {
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&temporary)
                    .map_err(unavailable)?
            };
            #[cfg(not(unix))]
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temporary)
                .map_err(unavailable)?;

            let mut writer = std::io::BufWriter::new(file);
            writer.write_all(contents.as_bytes()).map_err(unavailable)?;
            // Durable before the rename: a rename that lands ahead of the
            // contents would publish an empty credential across a power loss.
            writer.flush().map_err(unavailable)?;
            writer.get_ref().sync_all().map_err(unavailable)?;
        }

        // Fail hard here: nothing is published yet, so a permission we could not
        // set is a reason not to proceed.
        ensure_owner_only(&temporary).map_err(unavailable)?;
        std::fs::rename(&temporary, path).map_err(unavailable)?;
        Ok(())
    }
}

impl OsSecretStore {
    /// `resolve` with the environment read already performed.
    ///
    /// Split out purely for testability: mutating the process environment is
    /// `unsafe` in edition 2024 and `unsafe` is forbidden here, tests included
    /// (`AGENTS.md` §3). Passing the value in is how the precedence rule gets
    /// covered without reaching for `set_var`.
    fn resolve_with_environment(
        &self,
        provider: &ProviderId,
        kind: CredentialKind,
        environment: Option<String>,
    ) -> Result<ResolvedCredential, SecretError> {
        // Environment first: an override must not require touching stored state,
        // which is what makes CI and throwaway keys workable.
        if kind == CredentialKind::ApiKey
            && let Some(value) = environment
            && !value.trim().is_empty()
        {
            return Ok(ResolvedCredential {
                credential: Credential::ApiKey(Secret::new(value)),
                source: SecretSource::Environment,
            });
        }

        let path = self.path(provider)?;
        match std::fs::read_to_string(&path) {
            Ok(value) => {
                // Heal a file something else loosened, before handing the secret
                // on — the next reader should not find it world-readable either.
                let _ = ensure_owner_only(&path);
                Ok(ResolvedCredential {
                    credential: Self::decode(provider, kind, value)?,
                    source: SecretSource::File,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(SecretError::NotFound {
                    provider: provider.clone(),
                })
            }
            // The path may appear in the message; the contents never do.
            Err(error) => Err(SecretError::Unavailable {
                detail: format!("credential file could not be read: {error}"),
            }),
        }
    }
}

impl SecretStore for OsSecretStore {
    fn resolve(
        &self,
        provider: &ProviderId,
        kind: CredentialKind,
    ) -> Result<ResolvedCredential, SecretError> {
        let environment = std::env::var(environment_variable(provider)).ok();
        self.resolve_with_environment(provider, kind, environment)
    }

    fn store(&self, provider: &ProviderId, credential: Credential) -> Result<(), SecretError> {
        let stored = StoredCredential::from_credential(&credential);
        let encoded = serde_json::to_string(&stored)
            .map(Zeroizing::new)
            .map_err(|error| SecretError::Unavailable {
                detail: format!("credential for {provider} could not be encoded: {error}"),
            })?;
        Self::write_atomically(&self.path(provider)?, &encoded)
    }

    fn delete(&self, provider: &ProviderId) -> Result<(), SecretError> {
        match std::fs::remove_file(self.path(provider)?) {
            Ok(()) => Ok(()),
            // Already absent is the requested end state, not a failure.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SecretError::Unavailable {
                detail: format!("credential file could not be removed: {error}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_reveals_the_value_or_its_length() {
        let secret = Secret::new("sk-super-secret-value-12345".to_owned());
        let rendered = format!("{secret:?}");

        assert_eq!(rendered, "Secret(<redacted>)");
        assert!(!rendered.contains("sk-"));
        assert!(!rendered.contains("12345"));
        // Length is information about the credential too.
        assert!(!rendered.contains("27"));
    }

    #[test]
    fn a_struct_deriving_debug_cannot_leak_a_contained_secret() {
        // The realistic leak: a config struct derives Debug, holds a key, and
        // someone logs it. The manual Debug on Secret is what stops that.
        // The fields exist to be *rendered* by the derived Debug, not read —
        // that is the whole point of the test.
        #[derive(Debug)]
        #[allow(dead_code, reason = "fields are exercised through the derived Debug")]
        struct Config {
            provider: String,
            key: Secret,
        }

        let config = Config {
            provider: "openai".to_owned(),
            key: Secret::new("sk-leaked".to_owned()),
        };

        let rendered = format!("{config:?}");
        assert!(
            rendered.contains("openai"),
            "non-secret fields still render"
        );
        assert!(
            !rendered.contains("sk-leaked"),
            "a derived Debug on the container leaked the secret: {rendered}"
        );
    }

    #[test]
    fn expose_is_the_only_way_out() {
        let secret = Secret::new("sk-value".to_owned());
        assert_eq!(secret.expose(), "sk-value");
    }

    #[test]
    fn blank_credentials_are_recognised_before_a_request_is_sent() {
        assert!(Secret::new(String::new()).is_blank());
        assert!(Secret::new("   \n".to_owned()).is_blank());
        assert!(!Secret::new("sk-x".to_owned()).is_blank());
    }

    #[test]
    fn environment_variables_use_each_providers_conventional_name() {
        assert_eq!(
            environment_variable(&ProviderId::new("openai")),
            "OPENAI_API_KEY"
        );
        assert_eq!(
            environment_variable(&ProviderId::new("anthropic")),
            "ANTHROPIC_API_KEY"
        );
    }

    #[test]
    fn secret_errors_never_carry_a_credential() {
        let error = SecretError::NotFound {
            provider: ProviderId::new("openai"),
        };
        let rendered = format!("{error} {error:?}");
        assert!(rendered.contains("openai"));
        assert!(!rendered.contains("sk-"));
    }

    #[test]
    fn oauth_credentials_round_trip_through_the_storage_envelope() {
        let provider = ProviderId::new("openai-codex");
        let stored = StoredCredential::OAuth {
            access_token: "access-token".to_owned(),
            refresh_token: "refresh-token".to_owned(),
            expires_at_unix: 1_700_000_000,
            account_id: "account-id".to_owned(),
        };
        let encoded = serde_json::to_string(&stored).expect("encode fixture");
        let credential =
            OsSecretStore::decode(&provider, CredentialKind::OAuth, encoded).expect("decode oauth");
        let oauth = credential.oauth().expect("oauth kind");

        assert_eq!(oauth.access_token().expose(), "access-token");
        assert_eq!(oauth.refresh_token().expose(), "refresh-token");
        assert_eq!(oauth.expires_at_unix(), 1_700_000_000);
        assert_eq!(oauth.account_id(), "account-id");
    }

    #[test]
    fn legacy_raw_api_keys_remain_readable() {
        let provider = ProviderId::new("openai");
        let credential =
            OsSecretStore::decode(&provider, CredentialKind::ApiKey, "sk-legacy".to_owned())
                .expect("decode legacy key");

        assert_eq!(credential.api_key().expect("api key").expose(), "sk-legacy");
    }

    #[test]
    fn credential_kind_mismatches_fail_closed() {
        let provider = ProviderId::new("openai-codex");
        let stored = StoredCredential::ApiKey {
            value: "sk-wrong-kind".to_owned(),
        };
        let encoded = serde_json::to_string(&stored).expect("encode fixture");
        let error = OsSecretStore::decode(&provider, CredentialKind::OAuth, encoded)
            .expect_err("wrong kind must be refused");

        assert!(matches!(error, SecretError::KindMismatch { .. }));
        assert!(!format!("{error:?}").contains("sk-wrong-kind"));
    }

    /// A store over a temporary directory, so no test touches real credentials.
    fn temporary_store() -> (tempfile::TempDir, OsSecretStore) {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = OsSecretStore::with_directory(directory.path().join("credentials"));
        (directory, store)
    }

    #[test]
    fn a_stored_credential_comes_back_from_the_file() {
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("openai");

        store
            .store(
                &provider,
                Credential::ApiKey(Secret::new("sk-round".into())),
            )
            .expect("store");
        let resolved = store
            .resolve(&provider, CredentialKind::ApiKey)
            .expect("resolve");

        assert_eq!(
            resolved.credential.api_key().expect("api key").expose(),
            "sk-round"
        );
        assert_eq!(resolved.source, SecretSource::File);
    }

    #[test]
    fn an_oauth_credential_survives_the_file_round_trip() {
        // The subscription logins are the ones that hurt to lose, so the whole
        // envelope has to survive, not just an API key string.
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("openai-codex");
        let credential = Credential::OAuth(OAuthCredential::new(
            Secret::new("access".into()),
            Secret::new("refresh".into()),
            1_700_000_000,
            "account".into(),
        ));

        store.store(&provider, credential).expect("store");
        let resolved = store
            .resolve(&provider, CredentialKind::OAuth)
            .expect("resolve");
        let oauth = resolved.credential.oauth().expect("oauth kind");

        assert_eq!(oauth.refresh_token().expose(), "refresh");
        assert_eq!(oauth.account_id(), "account");
    }

    #[cfg(unix)]
    #[test]
    fn credentials_are_owner_only_on_disk() {
        use std::os::unix::fs::PermissionsExt;
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("openai");

        store
            .store(&provider, Credential::ApiKey(Secret::new("sk-perm".into())))
            .expect("store");

        let path = store.path(&provider).expect("path");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a credential file must not be group- or world-readable"
        );

        let directory = path.parent().expect("parent");
        let dir_mode = std::fs::metadata(directory)
            .expect("dir metadata")
            .permissions()
            .mode();
        assert_eq!(
            dir_mode & 0o777,
            0o700,
            "the credentials directory must not be listable by others"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_loosened_credential_file_is_tightened_on_read() {
        // The realistic case is a stray `chmod -R` or a restore from an archive
        // that dropped the mode. Reading should heal it rather than hand the
        // secret over and leave it exposed for the next reader.
        use std::os::unix::fs::PermissionsExt;
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("openai");
        store
            .store(&provider, Credential::ApiKey(Secret::new("sk-heal".into())))
            .expect("store");
        let path = store.path(&provider).expect("path");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("loosen");

        store
            .resolve(&provider, CredentialKind::ApiKey)
            .expect("resolve");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "a loose credential file must be tightened"
        );
    }

    #[test]
    fn an_absent_credential_is_not_found_rather_than_unavailable() {
        // "you have not logged in" and "the store is broken" need different
        // words to the user, so the distinction has to survive the file layer.
        let (_guard, store) = temporary_store();
        let error = store
            .resolve(&ProviderId::new("openai"), CredentialKind::ApiKey)
            .expect_err("must not resolve");

        assert!(matches!(error, SecretError::NotFound { .. }));
    }

    #[test]
    fn deleting_is_idempotent() {
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("openai");
        store
            .store(&provider, Credential::ApiKey(Secret::new("sk-gone".into())))
            .expect("store");

        store.delete(&provider).expect("first delete");
        store
            .delete(&provider)
            .expect("deleting an absent credential is the requested end state");
        assert!(matches!(
            store.resolve(&provider, CredentialKind::ApiKey),
            Err(SecretError::NotFound { .. })
        ));
    }

    #[test]
    fn a_write_leaves_no_temporary_file_behind() {
        // A leftover `.tmp` would be a second copy of the credential on disk.
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("openai");
        store
            .store(&provider, Credential::ApiKey(Secret::new("sk-tmp".into())))
            .expect("store");

        let directory = store.path(&provider).expect("path");
        let directory = directory.parent().expect("parent");
        let strays: Vec<_> = std::fs::read_dir(directory)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| {
                std::path::Path::new(name)
                    .extension()
                    .is_some_and(|e| e == "tmp")
            })
            .collect();

        assert!(strays.is_empty(), "temporary files left behind: {strays:?}");
    }

    #[test]
    fn a_provider_id_cannot_escape_the_credentials_directory() {
        // Provider ids are internal today. This keeps a future caller that
        // forwards a user-supplied id from writing outside the directory.
        let (_guard, store) = temporary_store();
        let directory = store.path(&ProviderId::new("openai")).expect("path");
        let directory = directory.parent().expect("parent").to_path_buf();

        let path = store
            .path(&ProviderId::new("../../escaped"))
            .expect("a sanitised path");

        assert_eq!(
            path.parent(),
            Some(directory.as_path()),
            "a credential file must stay inside the credentials directory: {path:?}"
        );
    }

    #[test]
    fn the_environment_still_beats_a_stored_credential() {
        // Regression guard for the ordering: a stored key silently winning over
        // an explicit export is the confusing half hour SecretSource exists for.
        let (_guard, store) = temporary_store();
        let provider = ProviderId::new("smed-test-env-provider");
        store
            .store(
                &provider,
                Credential::ApiKey(Secret::new("sk-stored".into())),
            )
            .expect("store");

        let resolved = store
            .resolve_with_environment(
                &provider,
                CredentialKind::ApiKey,
                Some("sk-from-env".to_owned()),
            )
            .expect("resolve");
        assert_eq!(
            resolved.credential.api_key().expect("api key").expose(),
            "sk-from-env"
        );
        assert_eq!(resolved.source, SecretSource::Environment);
    }
}
