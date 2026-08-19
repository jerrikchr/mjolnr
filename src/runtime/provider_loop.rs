//! Provider-turn driving for the single-owner runtime actor.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::core::continuation::{QuotaReserveBasis, QuotaReservePhase, QuotaReserveStatus};
use crate::core::error::{ProviderError, ReasonCode};
use crate::core::event::{FinishReason, ProviderEvent, RunId, SmedEvent};
use crate::core::provider::{Provider, ProviderRequest};
use crate::runtime::routing::{RouteAttemptOutcome, RouteGateOutcome};
use crate::runtime::session::StreamAccumulator;

use super::{
    Actor, Mail, PROVIDER_EVENT_CAPACITY, PlanRun, RunIntent, RunPhase, StreamOutcome, interview,
};

impl Actor {
    pub(super) async fn begin_provider_turn(&mut self, run: RunId) {
        // Steering lands here and nowhere else: after the previous tool calls
        // settled, before this request is built. Any earlier and it would race
        // a tool in flight; any later and the model would not see it until the
        // turn it was meant to redirect had already finished (
        // 16.5).
        if !self.deliver_steering(run).await {
            return;
        }
        if self.apply_configured_quota(run).await {
            return;
        }
        let Some(active) = self.run.as_ref().filter(|active| active.id == run) else {
            return;
        };
        if active.provider_turns >= self.limits.max_provider_turns {
            self.exhaust_budget(run).await;
            return;
        }

        let hard_stop = self.run.as_ref().is_some_and(|active| {
            active.id == run && active.hard_stop.is_some() && active.intent == RunIntent::Normal
        });
        if hard_stop {
            self.stop_for_quota(run).await;
            return;
        }

        // Never send a request to a provider whose breaker is already open
        // . Looping lets an already-open next hop advance
        // again immediately, without ever making a wasted request; a route
        // with nowhere left to go is a typed stop, not a hang.
        loop {
            match self.route_breaker_gate(run).await {
                RouteGateOutcome::Proceed => break,
                RouteGateOutcome::Retried => {}
                RouteGateOutcome::Stopped => return,
            }
        }

        let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
            return;
        };
        if let Some(reserve) = active.pending_drain.take()
            && active.intent == RunIntent::Normal
        {
            active.intent = RunIntent::QuotaDrain;
            self.state.quota_reserve = reserve;
        }
        active.provider_turns += 1;
        active.phase = RunPhase::Provider;
        active.accumulator = StreamAccumulator::default();
        let provider_id = active.provider.clone();
        let model = active.model.clone();
        let cancel = active.cancel.clone();
        let intent = active.intent;
        let plan_run = active.plan_run.clone();
        self.state.budget.provider_turns = active.provider_turns;
        self.publish_snapshot();

        let Some(provider) = self.provider(&provider_id) else {
            self.fail_run(
                run,
                ReasonCode::ProviderProtocol,
                format!("unknown provider {provider_id}"),
            )
            .await;
            return;
        };

        let directive = Self::directive_for_turn(intent, plan_run.as_ref());
        // The persona for this turn : an explicit `/persona`
        // override wins, else the persona the active route wears. Voice only —
        // never a change to which model runs.
        let route_persona = self
            .state
            .route
            .as_ref()
            .and_then(|runtime| self.route_table.routes.get(&runtime.route))
            .and_then(|route| route.persona.as_deref());
        let active_persona = self.state.persona_override.as_deref().or(route_persona);
        // The harness describes itself before the turn's directive: it is the
        // most stable text in the prompt and states the gates the model is
        // actually subject to, including the policy mode in force *now*. Without
        // it the model holds tool schemas and no account of the machine holding
        // them, so a question about its own capabilities has no answer in
        // context and the only place left to look is the filesystem.
        let harness = crate::context::harness::prompt_section(
            self.state.policy,
            &crate::context::harness::WorkspaceFacts::observe(self.state.workspace_root.as_deref()),
        );
        let rules_prompt = self.state.rules_snapshot.prompt_section();
        let base = match rules_prompt {
            Some(rules) => format!("{harness}\n\n{rules}\n\n{directive}"),
            None => format!("{harness}\n\n{directive}"),
        };
        let system = self.context.system_prompt(&base, active_persona);
        let messages = self.state.provider_messages();
        // An `Err` here is already recorded as a typed run failure, and no
        // socket was opened to learn it.
        let Ok((messages, images)) = self.resolve_images(&model, messages).await else {
            return;
        };
        let request = ProviderRequest {
            model,
            messages,
            system: Some(system),
            tools: if plan_run.is_some() {
                Vec::new()
            } else {
                self.tools.definitions()
            },
            images,
        };

        spawn_provider_turn(provider, request, cancel, run, self.mailbox.clone());
    }

    fn directive_for_turn(intent: RunIntent, plan_run: Option<&PlanRun>) -> String {
        if let Some(plan_run) = plan_run {
            return interview::system_instruction(plan_run);
        }
        match intent {
            RunIntent::Normal => {
                "Answer directly when the request is conversational or already answerable. Use the available tools when the request needs work done or facts you do not have."
                    .to_owned()
            }
            RunIntent::ManualHandoff | RunIntent::QuotaDrain => {
                "LANDING ONLY. Record a concise handoff with: done, remaining, exact next steps, and open risks. Perform only safe finishing work already authorised; do not start new work. End after this status."
                    .to_owned()
            }
        }
    }

    /// Append every queued steering message to the transcript, oldest first.
    ///
    /// Returns `false` when the run should stop — a store failure here is the
    /// same class of problem as failing to record the original user message,
    /// and continuing would send the provider a transcript the log disagrees
    /// with.
    async fn deliver_steering(&mut self, run: RunId) -> bool {
        if self.state.steering.is_empty() {
            return true;
        }
        let Some(session) = self.state.session else {
            self.state.steering.clear();
            return true;
        };
        // Only the run that is actually in flight consumes steering.
        if self.run.as_ref().is_none_or(|active| active.id != run) {
            return true;
        }
        while let Some(text) = self.state.steering.pop_front() {
            let message = crate::core::message::CanonicalMessage::user(text);
            let stored = match self
                .persist(SmedEvent::MessageAppended {
                    session,
                    message: Box::new(message.clone()),
                })
                .await
            {
                Ok(stored) => stored,
                Err(error) => {
                    self.note_store_failure(&error);
                    return false;
                }
            };
            self.state.push_message(Some(stored.sequence), message);
        }
        self.publish_snapshot();
        true
    }

    pub(super) async fn handle_provider_event(&mut self, run: RunId, event: ProviderEvent) {
        // A late event from a run that already ended must not touch the current
        // one. Without this guard, cancelling and immediately resending would
        // let the dead run's text bleed into the new one.
        let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
            return;
        };

        let session = active.session;

        match event {
            // `Started` needs no action: `RunStarted` was already emitted when
            // the run was created. `Failed`/`Finished` are narration — the
            // adapter's return value is the authority on how a stream ended, and
            // acting on both would risk two terminal events for one run.
            ProviderEvent::Started
            | ProviderEvent::Failed { .. }
            | ProviderEvent::Finished { .. } => {}
            ProviderEvent::TextDelta { text } => {
                active.accumulator.push_text(&text);
                // Ephemeral: broadcast to render, never stored. The coalesced
                // block is what becomes durable.
                self.broadcast(SmedEvent::TextDelta { session, run, text });
            }
            ProviderEvent::ReasoningDelta { text } => {
                self.broadcast(SmedEvent::ReasoningDelta { session, run, text });
            }
            ProviderEvent::ToolCallStarted { id, name } => {
                active.accumulator.start_tool_call(id);
                self.broadcast(SmedEvent::ToolAssembling { session, run, name });
            }
            ProviderEvent::ToolArgumentsDelta { id, fragment } => {
                active.accumulator.push_arguments(&id, &fragment);
            }
            ProviderEvent::ToolCallCompleted { call } => {
                active.accumulator.complete_tool_call(call);
            }
            ProviderEvent::Usage { usage } => {
                active.accumulator.set_usage(usage);
                // Accumulated here, at the same moment the event becomes
                // durable, so that live state and a recovered projection apply
                // one identical rule (`runtime::recovery`). Accumulating at turn
                // end instead would make the two diverge for any provider that
                // reported usage more than once, and the divergence would only
                // ever appear after a crash — the worst place to discover it.
                //
                // Adapters normalise to totals for one exchange
                // (`docs/provider-contract.md` §6.6), so one event per turn is
                // the contract this relies on.
                if let Err(error) = self
                    .persist(SmedEvent::UsageReported {
                        session,
                        run,
                        usage,
                    })
                    .await
                {
                    self.halt_for_store(run, &error);
                    return;
                }
                self.state.usage.input_tokens += usage.input_tokens;
                self.state.usage.output_tokens += usage.output_tokens;
                self.publish_snapshot();
            }
            ProviderEvent::Quota { snapshot } => {
                self.broadcast(SmedEvent::QuotaReported {
                    session,
                    run,
                    snapshot: snapshot.clone(),
                });
                self.observe_quota(run, snapshot).await;
            }
            // Retained, never fatal: providers add event types and expect
            // clients to cope (docs/provider-contract.md §2).
            ProviderEvent::UnknownUpstream { kind } => active.accumulator.note_unknown(kind),
        }
    }

    pub(super) async fn handle_provider_turn_ended(&mut self, run: RunId, outcome: StreamOutcome) {
        let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
            return;
        };
        let session = active.session;
        let provider = active.provider.clone();
        let model = active.model.clone();
        let accumulator = std::mem::take(&mut active.accumulator);
        let plan_run = active.plan_run.clone();
        // Taken, not read: the first assistant message of this run is the
        // answer to the review request that started it, and only the first.
        // Leaving the marker in place would relabel every later turn of the
        // same run as another answer.
        let answering = std::mem::take(&mut active.pending_review_threads);

        // Usage was accumulated as each `UsageReported` was emitted; adding it
        // again here would double-count it.
        let message = accumulator.finish(provider, model);
        let calls = message
            .as_ref()
            .map(|message| message.tool_calls().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(message) = message {
            let stored = match self
                .persist(SmedEvent::MessageAppended {
                    session,
                    message: Box::new(message.clone()),
                })
                .await
            {
                Ok(stored) => stored,
                Err(error) => {
                    self.halt_for_store(run, &error);
                    return;
                }
            };
            let response_message = message.id;
            self.state.push_message(Some(stored.sequence), message);

            // The §D3 link, recorded only once an answer exists. A run that was
            // cancelled or failed before producing a message reaches none of
            // this, and its threads stay linked to no response — which is the
            // honest record of what happened, not a gap to be filled in.
            if !answering.is_empty() {
                let answered = SmedEvent::ReviewRequestAnswered {
                    session,
                    threads: answering,
                    response_message,
                };
                match self.persist(answered.clone()).await {
                    Ok(_) => {
                        super::review::apply_event(&mut self.state.review_threads, &answered);
                    }
                    Err(error) => {
                        self.halt_for_store(run, &error);
                        return;
                    }
                }
            }
        }

        // Structured planning runs are deliberately terminal after one
        // provider response. They have no tools and their final text is an
        // input to the deterministic parser, never a source of authority by
        // itself.
        if let Some(plan_run) = plan_run
            && self
                .finish_structured_plan_run(run, plan_run, &outcome, &calls)
                .await
        {
            return;
        }

        let state = self
            .run
            .as_ref()
            .filter(|active| active.id == run)
            .map(|active| {
                (
                    active.intent,
                    active.hard_stop.is_some(),
                    active.pending_drain.is_some(),
                )
            });
        if matches!(state, Some((RunIntent::Normal, true, _))) {
            self.stop_for_quota(run).await;
            return;
        }

        self.route_provider_outcome(run, outcome, calls, state)
            .await;
    }

    async fn finish_structured_plan_run(
        &mut self,
        run: RunId,
        plan_run: PlanRun,
        outcome: &StreamOutcome,
        calls: &[crate::core::message::ToolCall],
    ) -> bool {
        if !matches!(outcome, Ok(Ok(_))) {
            return false;
        }
        if !calls.is_empty() {
            self.fail_run(
                run,
                ReasonCode::ProviderProtocol,
                "structured planning response attempted a tool call".to_owned(),
            )
            .await;
            return true;
        }
        let response = self
            .state
            .messages()
            .iter()
            .rev()
            .find(|entry| entry.message.provider.is_some())
            .map(|entry| entry.message.text())
            .unwrap_or_default();
        self.finish_run(run, FinishReason::Stop).await;
        self.finish_plan_run(plan_run, response).await;
        true
    }

    #[allow(
        clippy::cognitive_complexity,
        reason = "one linear outcome dispatch;  added the route-advance-on-failure branch alongside the existing terminal branches"
    )]
    async fn route_provider_outcome(
        &mut self,
        run: RunId,
        outcome: StreamOutcome,
        calls: Vec<crate::core::message::ToolCall>,
        state: Option<(RunIntent, bool, bool)>,
    ) {
        if matches!(outcome, Ok(Ok(_))) {
            // A completed stream, whatever it carried, means the current
            // hop's provider answered — that closes a `HalfOpen` breaker
            //  exactly as a successful probe should.
            self.note_route_provider_success().await;
        }

        match outcome {
            Ok(Ok(completion))
                if !calls.is_empty() || completion.reason == FinishReason::ToolCalls =>
            {
                if calls.is_empty() {
                    self.fail_run(
                        run,
                        ReasonCode::ProviderProtocol,
                        "provider reported tool calls without a completed call".to_owned(),
                    )
                    .await;
                    return;
                }
                if let Some(active) = self.run.as_mut().filter(|active| active.id == run) {
                    active.pending_tools.extend(calls);
                }
                self.drive_tools(run).await;
            }
            Ok(Ok(_)) if matches!(state, Some((RunIntent::Normal, false, true))) => {
                self.begin_provider_turn(run).await;
            }
            Ok(Ok(completion)) => self.finish_run(run, completion.reason).await,
            Ok(Err(ProviderError::Cancelled)) => {
                self.finish_run(run, FinishReason::Cancelled).await;
            }
            Ok(Err(error)) => {
                let code = error.reason_code();
                match self.note_route_provider_failure(run, code).await {
                    RouteAttemptOutcome::Advanced => {
                        self.begin_provider_turn(run).await;
                    }
                    RouteAttemptOutcome::Exhausted => {}
                    RouteAttemptOutcome::NotRouted => {
                        self.fail_run(run, code, error.to_string()).await;
                    }
                }
            }
            Err(join_error) => {
                self.fail_run(
                    run,
                    ReasonCode::ProviderProtocol,
                    format!("provider task did not complete: {join_error}"),
                )
                .await;
            }
        }
    }
}

impl Actor {
    async fn observe_quota(&mut self, run: RunId, snapshot: crate::core::model::QuotaSnapshot) {
        let Some(worst) = snapshot
            .windows
            .iter()
            .max_by(|left, right| left.used_fraction.total_cmp(&right.used_fraction))
        else {
            return;
        };
        let phase = if worst.used_fraction >= self.limits.quota_hard_fraction {
            QuotaReservePhase::Stopped
        } else if worst.used_fraction >= self.limits.quota_soft_fraction {
            QuotaReservePhase::Draining
        } else {
            QuotaReservePhase::Monitoring
        };
        let reserve = QuotaReserveStatus {
            basis: QuotaReserveBasis::ProviderReported {
                window: worst.label.clone(),
            },
            used_fraction: Some(worst.used_fraction),
            soft_threshold: self.limits.quota_soft_fraction,
            hard_threshold: self.limits.quota_hard_fraction,
            resets_at: worst.resets_at,
            phase,
        };
        self.state.quota_reserve = reserve.clone();
        self.state.quota = Some(snapshot);
        if phase == QuotaReservePhase::Monitoring {
            self.publish_snapshot();
            return;
        }
        let Some(session) = self.state.session else {
            return;
        };
        if let Err(error) = self
            .persist(SmedEvent::QuotaBoundaryReached {
                session,
                run,
                reserve: reserve.clone(),
            })
            .await
        {
            self.halt_for_store(run, &error);
            return;
        }
        if let Some(active) = self.run.as_mut().filter(|active| active.id == run) {
            match phase {
                QuotaReservePhase::Draining if active.intent == RunIntent::Normal => {
                    active.pending_drain = Some(reserve);
                }
                QuotaReservePhase::Stopped => active.hard_stop = Some(reserve),
                QuotaReservePhase::Monitoring | QuotaReservePhase::Draining => {}
            }
        }
        self.publish_snapshot();
    }

    /// Returns true when the run was stopped before a provider request.
    async fn apply_configured_quota(&mut self, run: RunId) -> bool {
        if matches!(
            self.state.quota_reserve.basis,
            QuotaReserveBasis::ProviderReported { .. }
        ) {
            return false;
        }
        let Some(limit) = self.limits.quota_token_budget.filter(|limit| *limit > 0) else {
            self.state.quota_reserve = QuotaReserveStatus::default();
            return false;
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "a quota ratio only needs threshold precision, not lossless integer recovery"
        )]
        let used = self.state.usage.total() as f64 / limit as f64;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "fraction is clamped before display and policy comparison"
        )]
        let used_fraction = used as f32;
        let phase = if used_fraction >= self.limits.quota_hard_fraction {
            QuotaReservePhase::Stopped
        } else if used_fraction >= self.limits.quota_soft_fraction {
            QuotaReservePhase::Draining
        } else {
            QuotaReservePhase::Monitoring
        };
        let reserve = QuotaReserveStatus {
            basis: QuotaReserveBasis::ConfiguredTokens { limit },
            used_fraction: Some(used_fraction),
            soft_threshold: self.limits.quota_soft_fraction,
            hard_threshold: self.limits.quota_hard_fraction,
            resets_at: None,
            phase,
        };
        self.state.quota_reserve = reserve.clone();
        let Some(active) = self.run.as_mut().filter(|active| active.id == run) else {
            return true;
        };
        match phase {
            QuotaReservePhase::Draining if active.intent == RunIntent::Normal => {
                active.pending_drain = Some(reserve);
                false
            }
            QuotaReservePhase::Stopped if active.intent == RunIntent::Normal => {
                active.hard_stop = Some(reserve);
                self.stop_for_quota(run).await;
                true
            }
            QuotaReservePhase::Monitoring
            | QuotaReservePhase::Draining
            | QuotaReservePhase::Stopped => false,
        }
    }
}

fn spawn_provider_turn(
    provider: Arc<dyn Provider>,
    request: ProviderRequest,
    cancel: CancellationToken,
    run: RunId,
    mailbox: mpsc::Sender<Mail>,
) {
    tokio::spawn(async move {
        let (event_tx, mut event_rx) = mpsc::channel(PROVIDER_EVENT_CAPACITY);

        let stream = tokio::spawn({
            let cancel = cancel.clone();
            async move { provider.stream(request, event_tx, cancel).await }
        });

        while let Some(event) = event_rx.recv().await {
            // `send` awaits capacity: a slow actor stalls this forwarder, which
            // stalls the adapter. Backpressure composes rather than buffering.
            if mailbox
                .send(Mail::ProviderEvent { run, event })
                .await
                .is_err()
            {
                // The actor is gone; stop the adapter rather than stream into a
                // void.
                cancel.cancel();
                break;
            }
        }

        let outcome = stream.await;
        let _ = mailbox.send(Mail::ProviderTurnEnded { run, outcome }).await;
    });
}
