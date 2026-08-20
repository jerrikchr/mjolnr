//! Runtime-owned provider connection and model-catalog discovery.

use std::time::Duration;

use crate::core::error::ProviderError;
use crate::core::model::{ModelDescriptor, ProviderId};
use crate::core::runtime::{ProviderConnection, ProviderConnectionState};

use super::{Actor, Mail};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

impl Actor {
    /// Start one bounded discovery request per configured provider.
    ///
    /// Results return through the actor mailbox, so network latency never
    /// blocks cancellation, the TUI, or provider stream traffic.
    pub(super) fn refresh_provider_catalogs(&mut self) {
        self.catalog_generation = self.catalog_generation.saturating_add(1);
        let generation = self.catalog_generation;
        self.catalog_cancel.cancel();
        self.catalog_cancel = tokio_util::sync::CancellationToken::new();

        for provider in &self.providers {
            let id = provider.id();
            if !provider.credentialed() {
                self.model_catalogs.remove(&id);
                self.provider_connections.insert(
                    id.clone(),
                    ProviderConnection {
                        provider: id,
                        state: ProviderConnectionState::Disconnected,
                        detail: Some("connect this provider in /auth".to_owned()),
                    },
                );
                continue;
            }

            self.provider_connections.insert(
                id.clone(),
                ProviderConnection {
                    provider: id.clone(),
                    state: ProviderConnectionState::Discovering,
                    detail: None,
                },
            );

            let provider = provider.clone();
            let mailbox = self.mailbox.clone();
            let cancel = self.catalog_cancel.child_token();
            tokio::spawn(async move {
                let outcome = tokio::select! {
                    () = cancel.cancelled() => Err(ProviderError::Cancelled),
                    result = tokio::time::timeout(
                        DISCOVERY_TIMEOUT,
                        provider.discover_models(cancel.clone()),
                    ) => match result {
                        Ok(outcome) => outcome,
                        Err(_) => Err(ProviderError::Transport {
                            detail: "model discovery timed out".to_owned(),
                        }),
                    },
                };
                let _ = mailbox
                    .send(Mail::CatalogDiscovered {
                        generation,
                        provider: id,
                        outcome,
                    })
                    .await;
            });
        }
        self.publish_snapshot();
    }

    pub(super) async fn handle_catalog_discovered(
        &mut self,
        generation: u64,
        provider: ProviderId,
        outcome: Result<Vec<ModelDescriptor>, ProviderError>,
    ) {
        if generation != self.catalog_generation {
            return;
        }

        match outcome {
            Ok(models) => {
                self.model_catalogs.insert(provider.clone(), models);
                self.provider_connections.insert(
                    provider.clone(),
                    ProviderConnection {
                        provider,
                        state: ProviderConnectionState::Connected,
                        detail: None,
                    },
                );
            }
            Err(ProviderError::Cancelled) => return,
            Err(ProviderError::Auth) => {
                self.model_catalogs.remove(&provider);
                self.provider_connections.insert(
                    provider.clone(),
                    ProviderConnection {
                        provider,
                        state: ProviderConnectionState::NeedsReauth,
                        detail: Some("authentication was rejected; reconnect in /auth".to_owned()),
                    },
                );
            }
            Err(error) => {
                self.model_catalogs.remove(&provider);
                self.provider_connections.insert(
                    provider.clone(),
                    ProviderConnection {
                        provider,
                        state: ProviderConnectionState::Unavailable,
                        detail: Some(error.to_string()),
                    },
                );
            }
        }
        self.publish_snapshot();
        self.replay_catalog_commands().await;
    }

    pub(super) fn command_waits_for_catalog(
        &self,
        command: &crate::core::command::MjolnrCommand,
    ) -> bool {
        let provider = match command {
            crate::core::command::MjolnrCommand::SelectModel { provider, .. }
            | crate::core::command::MjolnrCommand::ResumeCompact {
                provider: Some(provider),
                ..
            } => Some(provider),
            _ => None,
        };
        provider.is_some_and(|provider| {
            self.provider_connections
                .get(provider)
                .is_some_and(|connection| connection.state == ProviderConnectionState::Discovering)
        })
    }

    async fn replay_catalog_commands(&mut self) {
        loop {
            let Some(command) = self.pending_catalog_commands.front() else {
                return;
            };
            if self.command_waits_for_catalog(command) {
                return;
            }
            let Some(command) = self.pending_catalog_commands.pop_front() else {
                return;
            };
            self.handle_command(command).await;
        }
    }
}
