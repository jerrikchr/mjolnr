//! Arming, drawing against, and clearing a spawn envelope.
//!
//! The envelope lives in [`SessionState`](crate::runtime::session::SessionState)
//! and is never checkpointed — the same treatment `exact_commands` gets, for the
//! same reason. Everything durable about it lives in the ledger instead: the
//! arming, each draw, and the ending. That is what lets the audit answer *what
//! did this human authorise, and what was done with it?* without depending on
//! in-memory state that a restart throws away.

use crate::core::envelope::{ActiveEnvelope, EnvelopeRefusal, SpawnEnvelope};
use crate::core::event::{EnvelopeEnd, MjolnrEvent, RunId};
use crate::core::policy::PolicyMode;

use super::Actor;

/// What one `spawn_subagent` call would take from an envelope: children, the
/// provider turns they may collectively spend, and the routes they name.
///
/// Turns are summed rather than maxed because the envelope's turn bound is a
/// spend bound, and four children of eight turns each is thirty-two turns of
/// spend however concurrently they run.
pub(super) fn draw_shape(arguments: &serde_json::Value) -> (u32, u32, Vec<String>) {
    let Some(children) = arguments
        .get("children")
        .and_then(serde_json::Value::as_array)
    else {
        return (0, 0, Vec::new());
    };
    let mut turns = 0_u32;
    let mut routes = Vec::new();
    for child in children {
        let child_turns = child
            .get("max_provider_turns")
            .and_then(serde_json::Value::as_u64)
            .map_or(crate::tools::subagent::DEFAULT_CHILD_TURNS, |value| {
                u32::try_from(value).unwrap_or(u32::MAX)
            });
        turns = turns.saturating_add(child_turns);
        // A role is resolved through the project's route table, so both spellings
        // are what the envelope's route list is written against.
        for key in ["role", "route"] {
            if let Some(named) = child.get(key).and_then(serde_json::Value::as_str) {
                routes.push(named.to_owned());
            }
        }
    }
    (
        u32::try_from(children.len()).unwrap_or(u32::MAX),
        turns,
        routes,
    )
}

impl Actor {
    /// Arm an envelope for this session.
    ///
    /// Refused while a run is active, for the same reason a policy change is: an
    /// authorisation that lands mid-run applies to spawns the human has not seen
    /// proposed.
    pub(super) async fn arm_spawn_envelope(&mut self, envelope: SpawnEnvelope) {
        if self.run.is_some() {
            self.state.envelope_refusal =
                Some("an envelope can only be armed while idle".to_owned());
            self.publish_snapshot();
            return;
        }
        if let Err(refusal) = envelope.validate(self.state.policy) {
            self.state.envelope_refusal = Some(refusal.detail());
            self.publish_snapshot();
            return;
        }
        let Some(session) = self.state.session else {
            return;
        };
        if let Err(error) = self
            .persist(MjolnrEvent::SpawnEnvelopeArmed {
                session,
                ceiling: envelope.ceiling,
                max_children: envelope.max_children,
                max_per_call: envelope.max_per_call,
                max_provider_turns: envelope.max_provider_turns,
                expires_after_turns: envelope.expires_after_turns,
            })
            .await
        {
            self.note_store_failure(&error);
            return;
        }
        self.state.envelope_refusal = None;
        self.state.envelope = Some(ActiveEnvelope::new(envelope));
        self.publish_snapshot();
    }

    /// End the envelope and say why.
    ///
    /// Cleared rather than left at zero: a visible "0 remaining" invites the
    /// reading that it is still in force, and an authorisation that looks alive
    /// when it is spent is the kind of thing someone relies on by accident.
    pub(super) async fn clear_spawn_envelope(&mut self, reason: EnvelopeEnd) {
        if self.state.envelope.is_none() {
            return;
        }
        self.state.envelope = None;
        if let Some(session) = self.state.session
            && let Err(error) = self
                .persist(MjolnrEvent::SpawnEnvelopeCleared { session, reason })
                .await
        {
            self.note_store_failure(&error);
        }
        self.publish_snapshot();
    }

    /// Narrowing the policy invalidates any envelope it no longer justifies.
    ///
    /// An envelope that outlived the policy which allowed it would be a standing
    /// grant nobody re-authorised — precisely the laundering `carried_forward`
    /// exists to prevent, arriving by a different route.
    pub(super) async fn reconcile_envelope_with_policy(&mut self, mode: PolicyMode) {
        let stale = self
            .state
            .envelope
            .as_ref()
            .is_some_and(|active| active.envelope.validate(mode).is_err());
        if stale {
            self.clear_spawn_envelope(EnvelopeEnd::PolicyNarrowed).await;
        }
    }

    /// What a `spawn_subagent` call would draw, checked against the envelope.
    ///
    /// `None` for any other tool, and for a spawn when no envelope is in force —
    /// which is the ordinary path, unchanged by this phase.
    pub(super) fn envelope_draw(
        &self,
        call: &crate::core::message::ToolCall,
    ) -> Option<Result<(), EnvelopeRefusal>> {
        if call.name != crate::tools::subagent::SpawnSubagent::NAME {
            return None;
        }
        let (children, turns, routes) = draw_shape(&call.arguments);
        let Some(active) = self.state.envelope.as_ref().filter(|a| !a.expired()) else {
            // No envelope in force. The schema admits the widest *legal* call
            // because it cannot see session state, so the narrow cap is enforced
            // here — with a refusal that names the remedy rather than a bare
            // "too many children".
            let allowed = u32::try_from(crate::tools::subagent::MAX_CHILDREN).unwrap_or(u32::MAX);
            if children > allowed {
                return Some(Err(EnvelopeRefusal::PerCallExceeded {
                    asked: children,
                    allowed,
                }));
            }
            return None;
        };
        Some(active.check_draw(children, turns, &routes))
    }

    /// Record a draw the envelope authorised, and clear it if that spent it.
    pub(super) async fn record_envelope_draw(&mut self, run: RunId, children: u32, turns: u32) {
        let Some(active) = self.state.envelope.as_mut() else {
            return;
        };
        active.draw(children, turns);
        let remaining = active.children_remaining();
        let spent = active.exhausted();
        if let Some(session) = self.state.session
            && let Err(error) = self
                .persist(MjolnrEvent::SpawnEnvelopeDrawn {
                    session,
                    run,
                    children,
                    provider_turns: turns,
                    children_remaining: remaining,
                })
                .await
        {
            self.note_store_failure(&error);
            return;
        }
        if spent {
            self.clear_spawn_envelope(EnvelopeEnd::Spent).await;
        } else {
            self.publish_snapshot();
        }
    }

    /// Advance the envelope's clock by one turn, clearing it if it lapsed.
    pub(super) async fn tick_envelope(&mut self) {
        let lapsed = self
            .state
            .envelope
            .as_mut()
            .is_some_and(ActiveEnvelope::tick);
        if lapsed {
            self.clear_spawn_envelope(EnvelopeEnd::Lapsed).await;
        }
    }
}
