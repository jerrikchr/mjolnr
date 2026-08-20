//! Route attachment, breaker gating, and advance-on-condition.
//!
//! Everything here is additive to the Phase 10 machinery it sits on top of:
//! when no route is attached — including whenever a project has no routing
//! config at all — every function here is a no-op and the session behaves
//! exactly as it did before this phase.

use time::OffsetDateTime;

use crate::core::continuation::QuotaReserveStatus;
use crate::core::error::ReasonCode;
use crate::core::event::{MjolnrEvent, RunId};
use crate::core::model::ProviderId;
use crate::core::routing::{
    AdvanceOutcome, CircuitBreaker, RouteAdvanceCondition, RouteRuntime, advance_position,
};

use super::{Actor, RunIntent};

/// What attempting a route advance came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteAttemptOutcome {
    /// No route is attached; the caller must fall back to its pre-Phase-15
    /// behaviour unchanged.
    NotRouted,
    /// The route moved to a new hop; the caller should retry on it.
    Advanced,
    /// No viable position remained. A typed stop was already recorded and
    /// `fail_run` already called — the caller must simply return.
    Exhausted,
}

/// What gating a provider turn on the current hop's breaker came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteGateOutcome {
    /// No breaker objects; send the request.
    Proceed,
    /// The breaker was open and the route advanced to a new hop without ever
    /// sending a request to the blocked provider.
    Retried,
    /// The breaker was open and the route had nowhere left to go; the run was
    /// already stopped.
    Stopped,
}

impl Actor {
    /// Write a route's persona binding to its diffable file and reload the
    /// table so the change is live. The `/config` surface is a
    /// lens over files: this edits `.mjolnr/routes/<route>.yaml` exactly as a
    /// hand-editor would, then rebuilds the `RouteTable` from disk so the next
    /// turn wears the new binding. A missing workspace, missing route file, or
    /// failed write leaves everything untouched — the write is the change, so a
    /// change that cannot be written is no change, stated by doing nothing.
    pub(super) fn bind_route_persona(&mut self, route: &str, persona: Option<&str>) {
        let Some(root) = self.state.workspace_root.clone() else {
            return;
        };
        let config_dir = crate::core::paths::resolve_workspace_config_dir(&root);
        let path = config_dir.join("routes").join(format!("{route}.yaml"));
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return;
        };
        let edited = crate::routing::edit::set_route_persona(&contents, persona);
        if std::fs::write(&path, &edited).is_err() {
            return;
        }
        // Reload from disk so the binding is live this session, not only on the
        // next launch. Diagnostics are dropped here as `/reload` drops them; a
        // freshly hand-written file is the loader's job to validate, and the
        // previous table stays live for any route that failed to reparse.
        let (table, _diagnostics) = crate::routing::definition::load_dir(&root);
        self.route_table = std::sync::Arc::new(table);
        self.publish_snapshot();
    }

    /// Resolve and attach a route, or leave the session untouched.
    pub(super) async fn attach_route(
        &mut self,
        route: Option<String>,
        role: Option<String>,
        task_class: String,
    ) {
        let Some(session) = self.state.session else {
            return;
        };
        // Repointing mid-run would change the provider a live turn is
        // mid-flight against; refuse silently rather than tearing down state
        // a running turn still holds a reference to. Nothing calls this while
        // a run is active in the shipped wiring, but the guard is cheap and
        // the alternative is a state a run outlives incorrectly.
        if self.run.is_some() {
            return;
        }
        let Some((definition, reason)) =
            self.route_table
                .resolve(route.as_deref(), role.as_deref(), &task_class)
        else {
            return;
        };
        let Some(hop) = definition.hop(0) else {
            return;
        };
        let route_name = definition.name.clone();
        let provider = hop.provider.clone();
        let model = hop.model.clone();

        if let Err(error) = self
            .persist(MjolnrEvent::RouteSelected {
                session,
                child: None,
                route: route_name.clone(),
                position: 0,
                provider: provider.clone(),
                model: model.clone(),
                reason,
            })
            .await
        {
            self.note_store_failure(&error);
            return;
        }

        self.state.provider = Some(provider);
        self.state.model = Some(model);
        self.state.route = Some(RouteRuntime {
            route: route_name,
            position: 0,
        });
        self.state.breakers.clear();
        self.state.quota_reserve = QuotaReserveStatus::default();
        self.publish_snapshot();
    }

    /// Reconcile the current hop's breaker against the clock, and refuse a
    /// request to a provider whose breaker is open by advancing the route
    /// instead — never a wasted request against a known-broken provider.
    pub(super) async fn route_breaker_gate(&mut self, run: RunId) -> RouteGateOutcome {
        if self.state.route.is_none() {
            return RouteGateOutcome::Proceed;
        }
        let Some(provider_id) = self.state.provider.clone() else {
            return RouteGateOutcome::Proceed;
        };
        let now = OffsetDateTime::now_utc();
        let config = self.route_table.breaker_config(&provider_id);
        let breaker = self
            .state
            .breakers
            .entry(provider_id.clone())
            .or_insert_with(|| CircuitBreaker::new(config));
        let transition = breaker.poll(now);
        let permits = breaker.permits_request();
        if let Some(transition) = transition {
            self.record_breaker_transition(provider_id.clone(), transition)
                .await;
        }
        if permits {
            return RouteGateOutcome::Proceed;
        }
        match self
            .try_advance_route(run, RouteAdvanceCondition::BreakerOpen)
            .await
        {
            RouteAttemptOutcome::Advanced => RouteGateOutcome::Retried,
            RouteAttemptOutcome::Exhausted => RouteGateOutcome::Stopped,
            // The breaker only gates when a route is attached, so this cannot
            // happen in practice; treated as "nothing to gate on" rather than
            // panicking on an impossible state.
            RouteAttemptOutcome::NotRouted => RouteGateOutcome::Proceed,
        }
    }

    /// Record a provider turn that completed without a transport/protocol
    /// failure. Closes a `HalfOpen` breaker; does nothing to a `Closed` one
    /// beyond resetting its failure count.
    pub(super) async fn note_route_provider_success(&mut self) {
        if self.state.route.is_none() {
            return;
        }
        let Some(provider_id) = self.state.provider.clone() else {
            return;
        };
        let now = OffsetDateTime::now_utc();
        let config = self.route_table.breaker_config(&provider_id);
        let breaker = self
            .state
            .breakers
            .entry(provider_id.clone())
            .or_insert_with(|| CircuitBreaker::new(config));
        if let Some(transition) = breaker.on_success(now) {
            self.record_breaker_transition(provider_id, transition)
                .await;
        }
    }

    /// Record a provider failure against the current hop's breaker, then try
    /// to advance the route on it. Returns `NotRouted` when no route is
    /// attached, so the caller's original failure handling applies unchanged.
    pub(super) async fn note_route_provider_failure(
        &mut self,
        run: RunId,
        code: ReasonCode,
    ) -> RouteAttemptOutcome {
        if self.state.route.is_none() {
            return RouteAttemptOutcome::NotRouted;
        }
        if let Some(provider_id) = self.state.provider.clone() {
            let now = OffsetDateTime::now_utc();
            let config = self.route_table.breaker_config(&provider_id);
            let breaker = self
                .state
                .breakers
                .entry(provider_id.clone())
                .or_insert_with(|| CircuitBreaker::new(config));
            if let Some(transition) = breaker.on_failure(now) {
                self.record_breaker_transition(provider_id, transition)
                    .await;
            }
        }
        self.try_advance_route(run, RouteAdvanceCondition::ProviderFailure(code))
            .await
    }

    /// Attempt to advance the session's attached route on `condition`.
    pub(super) async fn try_advance_route(
        &mut self,
        run: RunId,
        condition: RouteAdvanceCondition,
    ) -> RouteAttemptOutcome {
        let Some(session) = self.state.session else {
            return RouteAttemptOutcome::NotRouted;
        };
        let Some(current) = self.state.route.clone() else {
            return RouteAttemptOutcome::NotRouted;
        };
        let Some(definition) = self.route_table.routes.get(&current.route).cloned() else {
            return RouteAttemptOutcome::NotRouted;
        };

        match advance_position(&definition, current.position) {
            AdvanceOutcome::Advanced { position, hop } => {
                if let Err(error) = self
                    .persist(MjolnrEvent::RouteAdvanced {
                        session,
                        run,
                        route: definition.name.clone(),
                        from_position: current.position,
                        to_position: position,
                        provider: hop.provider.clone(),
                        model: hop.model.clone(),
                        condition,
                    })
                    .await
                {
                    self.halt_for_store(run, &error);
                    // The caller must not also fail_run: the run is already
                    // halted for a store failure, a distinct terminal path.
                    return RouteAttemptOutcome::Exhausted;
                }
                self.state.route = Some(RouteRuntime {
                    route: definition.name.clone(),
                    position,
                });
                self.state.provider = Some(hop.provider.clone());
                self.state.model = Some(hop.model.clone());
                // Phase 10 cross-model continuation rule: no provider-private
                // state pretends to migrate. Canonical messages are already
                // provider-neutral, so the durable transcript is the correct
                // request payload for the new hop without rewriting anything
                // in transit; only the quota reserve — which is provider-
                // specific — is reset, exactly as a manual model switch does.
                self.state.quota_reserve = QuotaReserveStatus::default();
                if let Some(active) = self.run.as_mut().filter(|active| active.id == run) {
                    active.provider = hop.provider;
                    active.model = hop.model;
                    active.hard_stop = None;
                    active.pending_drain = None;
                    active.intent = RunIntent::Normal;
                }
                self.publish_snapshot();
                RouteAttemptOutcome::Advanced
            }
            AdvanceOutcome::Exhausted => {
                if let Err(error) = self
                    .persist(MjolnrEvent::RouteExhausted {
                        session,
                        run,
                        route: definition.name.clone(),
                        condition,
                    })
                    .await
                {
                    self.halt_for_store(run, &error);
                    return RouteAttemptOutcome::Exhausted;
                }
                self.fail_run(
                    run,
                    ReasonCode::RouteExhausted,
                    format!(
                        "route `{}` had no viable position left after {}",
                        definition.name,
                        condition.label()
                    ),
                )
                .await;
                RouteAttemptOutcome::Exhausted
            }
        }
    }

    async fn record_breaker_transition(
        &mut self,
        provider: ProviderId,
        transition: crate::core::routing::BreakerTransition,
    ) {
        let Some(session) = self.state.session else {
            return;
        };
        if let Err(error) = self
            .persist(MjolnrEvent::BreakerStateChanged {
                session,
                provider,
                from: transition.from,
                to: transition.to,
            })
            .await
        {
            self.note_store_failure(&error);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::core::command::MjolnrCommand;
    use crate::core::model::{ModelId, ProviderId};
    use crate::core::routing::{RouteDefinition, RouteHop, RouteTable};
    use crate::core::runtime::MjolnrRuntime;
    use crate::core::store::EventStore;
    use crate::providers::fake::FakeProvider;
    use crate::runtime::Runtime;
    use crate::store::memory::InMemoryEventStore;

    fn table_with_one_route() -> RouteTable {
        let mut table = RouteTable::default();
        table.routes.insert(
            "main".to_owned(),
            RouteDefinition {
                name: "main".to_owned(),
                hops: vec![RouteHop {
                    provider: ProviderId::new(FakeProvider::ID),
                    model: ModelId::new(FakeProvider::MODEL),
                }],
                roles: Vec::new(),
                persona: None,
            },
        );
        table
            .task_classes
            .insert("default".to_owned(), "main".to_owned());
        table
    }

    async fn wait_for_session(runtime: &Runtime) {
        let mut snapshots = runtime.snapshots();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if runtime.snapshot().session.is_some() {
                    return;
                }
                snapshots.changed().await.expect("snapshot");
            }
        })
        .await
        .expect("session created");
    }

    #[tokio::test]
    async fn attaching_a_route_selects_its_first_hop_and_records_the_reason() {
        let store = Arc::new(InMemoryEventStore::new());
        let runtime = Runtime::spawn_with_routes(
            vec![Arc::new(FakeProvider::default())],
            store.clone() as Arc<dyn EventStore>,
            Arc::new(table_with_one_route()),
        );
        runtime
            .dispatch(MjolnrCommand::OpenProject {
                root: std::env::current_dir().expect("cwd"),
            })
            .await
            .expect("open");
        runtime
            .dispatch(MjolnrCommand::CreateSession {
                provider: ProviderId::new(FakeProvider::ID),
                model: ModelId::new(FakeProvider::MODEL),
            })
            .await
            .expect("create session");
        wait_for_session(&runtime).await;

        let mut snapshots = runtime.snapshots();
        runtime
            .dispatch(MjolnrCommand::AttachRoute {
                route: None,
                role: None,
                task_class: "default".to_owned(),
            })
            .await
            .expect("attach route");
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let snapshot = snapshots.changed().await.expect("snapshot");
                if snapshot.route.is_some() {
                    return snapshot;
                }
            }
        })
        .await
        .expect("route attached");

        assert_eq!(snapshot.route.as_ref().map(|route| route.position), Some(0));
        let session = snapshot.session.expect("session");
        let history = store.events(session).await.expect("history");
        assert!(history.iter().any(|stored| matches!(
            stored.event,
            crate::core::event::MjolnrEvent::RouteSelected {
                reason: crate::core::routing::RouteSelectionReason::TaskClass(_),
                ..
            }
        )));
    }

    #[tokio::test]
    async fn no_routing_config_leaves_attach_route_a_no_op() {
        let store = Arc::new(InMemoryEventStore::new());
        let runtime = Runtime::spawn(
            vec![Arc::new(FakeProvider::default())],
            store as Arc<dyn EventStore>,
        );
        runtime
            .dispatch(MjolnrCommand::OpenProject {
                root: std::env::current_dir().expect("cwd"),
            })
            .await
            .expect("open");
        runtime
            .dispatch(MjolnrCommand::CreateSession {
                provider: ProviderId::new(FakeProvider::ID),
                model: ModelId::new(FakeProvider::MODEL),
            })
            .await
            .expect("create session");
        wait_for_session(&runtime).await;
        runtime
            .dispatch(MjolnrCommand::AttachRoute {
                route: None,
                role: None,
                task_class: "default".to_owned(),
            })
            .await
            .expect("attach route is accepted, but resolves to nothing");

        // No routing config means `AttachRoute` has nothing to resolve; give
        // the actor a beat to (not) process it, then assert nothing changed.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snapshot = runtime.snapshot();
        assert!(
            snapshot.route.is_none(),
            "no routing config means no route attaches"
        );
        assert_eq!(
            snapshot.provider,
            Some(ProviderId::new(FakeProvider::ID)),
            "the session keeps its explicitly configured provider"
        );
    }
}
