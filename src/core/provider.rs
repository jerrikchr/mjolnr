//! The provider boundary.
//!
//! Adapters live in `providers/` and know exactly one thing: how to turn their
//! upstream's wire format into [`ProviderEvent`] values. They know nothing about
//! policy, persistence, or the UI — that is the dependency direction in
//! `AGENTS.md` §2.1, and `tests/architecture.rs` enforces it.
//!
//! Every provider maps onto the same event vocabulary, but they must **not** be
//! forced through one fake OpenAI-compatible wire model : five
//! providers use three tool-call models, two auth placements, and two
//! transports. A shared abstraction over that would be a lie.

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::error::ProviderError;
use crate::core::message::CanonicalMessage;
use crate::core::model::{ModelDescriptor, ModelId, ProviderId, Usage};
use crate::core::tool::ToolDefinition;

/// One request to a provider.
///
/// Carries canonical history; the adapter translates it to the provider's own
/// shape. It does not carry a credential: secrets are resolved inside the
/// adapter from the `SecretStore` (Phase 2/6.5) so they never travel through the
/// runtime, the event log, or a `Debug` output (AGENTS.md §3).
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub model: ModelId,
    pub messages: Vec<CanonicalMessage>,
    pub system: Option<String>,
    pub tools: Vec<ToolDefinition>,
    /// Bytes for the `ImageRef` blocks in `messages`, keyed by `source`
    /// .
    ///
    /// A sidecar rather than bytes inside the block, because the block is also
    /// the persisted shape and base64 does not belong in the event log. Loaded
    /// by the runtime at assembly time; adapters only encode. **A miss is a
    /// typed error, never a fallback** — encoding a message whose image is
    /// absent would send the model a request the user believes contains a
    /// picture, which is the exact class of lie `AGENTS.md` §1.3 forbids.
    pub images: crate::core::image::ImageSidecar,
}

/// How a provider exchange concluded.
///
/// **The adapter's return value is the authority on how a stream ended**, not
/// the [`Finished`](crate::core::event::ProviderEvent::Finished) event, which is
/// narration for anything watching the raw feed. So `reason` lives here.
///
/// There is deliberately no `Default`: an adapter must state how its stream
/// ended. A default would silently mean `Stop`, and "assume success" is exactly
/// the failure this type exists to prevent — an earlier version of the runtime
/// mapped every success to `Stop`, which made
/// [`Incomplete`](crate::core::event::FinishReason::Incomplete) unreachable and
/// reported truncated answers as complete ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCompletion {
    /// Why the stream ended. Must match the `Finished` event the adapter
    /// emitted, if it emitted one.
    pub reason: crate::core::event::FinishReason,
    pub usage: Option<Usage>,
}

/// A provider adapter.
///
/// `async_trait` is used here deliberately: this is an object-safe plugin
/// boundary (`AGENTS.md` §8 /  restrict `async-trait` to exactly that).
/// The runtime holds `Arc<dyn Provider>` and cannot know the concrete type.
#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug {
    fn id(&self) -> ProviderId;

    /// Models this provider offers, with declared capabilities.
    fn models(&self) -> Vec<ModelDescriptor>;

    /// Whether this provider can authenticate right now.
    ///
    /// Answered by the adapter because only it knows what it needs: a local
    /// runtime needs nothing and is always ready, while a hosted provider must
    /// resolve a credential. The runtime uses this to start discovery. `/auth`
    /// remains the complete provider/remedy surface; `/model` deliberately
    /// contains only models whose discovery succeeded.
    ///
    /// Defaults to `true`, which is correct for adapters needing no credential.
    /// Credentialed adapters override it.
    fn credentialed(&self) -> bool {
        true
    }

    /// Discover the models this configured provider can currently serve.
    ///
    /// Static adapters inherit this implementation. Providers with a model
    /// endpoint override it so a successful login or server start can change
    /// the catalog without rebuilding mjolnr.
    async fn discover_models(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<ModelDescriptor>, ProviderError> {
        if !self.credentialed() {
            return Err(ProviderError::Auth);
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        Ok(self.models())
    }

    /// Stream one exchange, emitting normalised events.
    ///
    /// Contract:
    ///
    /// - `events` is **bounded**. Sending applies backpressure, which is the
    ///   point (AGENTS.md §4). An adapter must not buffer the stream to avoid
    ///   waiting on a slow consumer.
    /// - `cancel` must be honoured promptly. Cancellation is a client-side
    ///   stream drop for every provider mjolnr supports
    ///   (`docs/provider-contract.md` §6.7).
    /// - **Never retry after output has been produced.** A stream that emitted
    ///   tokens and then failed is not safe to replay.
    /// - Exactly one terminal event: `Finished` or `Failed`, never both.
    async fn stream(
        &self,
        request: ProviderRequest,
        events: mpsc::Sender<crate::core::event::ProviderEvent>,
        cancel: CancellationToken,
    ) -> Result<ProviderCompletion, ProviderError>;
}

/// Looks up a model descriptor across the registered providers.
#[must_use]
pub fn find_model<'a>(
    providers: impl IntoIterator<Item = &'a std::sync::Arc<dyn Provider>>,
    provider: &ProviderId,
    model: &ModelId,
) -> Option<ModelDescriptor> {
    providers
        .into_iter()
        .filter(|candidate| &candidate.id() == provider)
        .flat_map(|candidate| candidate.models())
        .find(|descriptor| &descriptor.id == model)
}
