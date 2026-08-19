//! Idle model switching and durable switch refusals.

use crate::core::error::ReasonCode;
use crate::core::event::{RunId, SessionId, SmedEvent};
use crate::core::model::{Capability, ModelId, ProviderId};
use crate::core::runtime::ProviderConnectionState;

use super::Actor;

impl Actor {
    pub(super) async fn select_model(&mut self, provider: ProviderId, model: ModelId) -> bool {
        let Some(session) = self.state.session else {
            return false;
        };
        if let Some(run) = self.run.as_ref().map(|active| active.id) {
            self.refuse_model_change(
                session,
                provider,
                model,
                ReasonCode::RunActive,
                "finish or cancel the active run before switching models".to_owned(),
                Some(run),
            )
            .await;
            return false;
        }
        let connection = self
            .provider_connections
            .get(&provider)
            .map(|connection| connection.state);
        if connection != Some(ProviderConnectionState::Connected) {
            self.refuse_model_change(
                session,
                provider,
                model,
                ReasonCode::ProviderAuth,
                "the requested provider is not connected; inspect /auth".to_owned(),
                None,
            )
            .await;
            return false;
        }
        let descriptor = self
            .model_catalogs
            .get(&provider)
            .and_then(|catalog| catalog.iter().find(|candidate| candidate.id == model))
            .cloned();
        let Some(descriptor) = descriptor else {
            self.refuse_model_change(
                session,
                provider,
                model,
                ReasonCode::ProviderIncompatibleModel,
                "the requested model is not in the provider's current catalog".to_owned(),
                None,
            )
            .await;
            return false;
        };
        if let Some(capability) =
            descriptor.missing_capability(&[Capability::Streaming, Capability::Tools])
        {
            self.refuse_model_change(
                session,
                provider,
                model,
                ReasonCode::ProviderIncompatibleModel,
                format!(
                    "the requested model does not support {}",
                    capability.as_str()
                ),
                None,
            )
            .await;
            return false;
        }

        if let Err(error) = self
            .persist(SmedEvent::ModelChanged {
                session,
                provider: provider.clone(),
                model: model.clone(),
            })
            .await
        {
            self.note_store_failure(&error);
            return false;
        }
        self.state.provider = Some(provider);
        self.state.model = Some(model);
        // A provider's window cannot govern another provider. The new adapter
        // must report its own facts or use this runtime's configured fallback.
        self.state.quota_reserve = crate::core::continuation::QuotaReserveStatus::default();
        // Nor can the outgoing model's tier govern the incoming one. This is
        // the hole the phase was written to close: before it, switching to a
        // model the owner governs tightly, mid-full-auto, changed who was
        // acting and nothing about what they were allowed to do (
        // 33). Applied here rather than only at `start_run` so the header
        // tells the truth the moment the switch lands, instead of a turn
        // later.
        self.apply_governance_floor().await;
        self.publish_snapshot();
        true
    }

    async fn refuse_model_change(
        &mut self,
        session: SessionId,
        provider: ProviderId,
        model: ModelId,
        code: ReasonCode,
        detail: String,
        active_run: Option<RunId>,
    ) {
        if let Err(error) = self
            .persist(SmedEvent::ModelChangeRefused {
                session,
                provider,
                model,
                code,
                detail,
            })
            .await
        {
            if let Some(run) = active_run {
                self.halt_for_store(run, &error);
            } else {
                self.note_store_failure(&error);
            }
        }
    }
}
