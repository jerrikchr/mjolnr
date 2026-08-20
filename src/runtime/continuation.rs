//! Quota landing, durable handoff creation, and resume-choice handling.

use std::collections::{BTreeMap, BTreeSet};

use tokio_util::sync::CancellationToken;

use crate::core::continuation::{
    CommandFact, HandoffCheckpoint, HandoffId, QuotaReservePhase, ResumeChoice,
};
use crate::core::error::ReasonCode;
use crate::core::event::{MjolnrEvent, RunId};
use crate::core::message::ToolEffect;
use crate::core::routing::RouteAdvanceCondition;
use crate::runtime::routing::RouteAttemptOutcome;

use super::{ActiveRun, Actor, Mail, RunIntent, RunPhase};

const COMPACT_RECENT_TURNS: usize = 2;

impl Actor {
    /// Start one system-authored landing turn without pretending it came from
    /// the user, optionally setting target provider/model for live handoff.
    pub(super) async fn start_handoff(&mut self, target: Option<String>) {
        let (Some(session), Some(provider), Some(model)) = (
            self.state.session,
            self.state.provider.clone(),
            self.state.model.clone(),
        ) else {
            return;
        };

        let target_spec = if let Some(ref target_str) = target {
            if let Some((def, _reason)) =
                self.route_table
                    .resolve(Some(target_str), Some(target_str), "")
            {
                def.hop(0)
                    .map(|hop| (hop.provider.clone(), hop.model.clone()))
            } else if let Some((prov, mdl)) = target_str.split_once('/') {
                Some((
                    crate::core::model::ProviderId::new(prov),
                    crate::core::model::ModelId::new(mdl),
                ))
            } else if let Some((prov, mdl)) = target_str.split_once(':') {
                Some((
                    crate::core::model::ProviderId::new(prov),
                    crate::core::model::ModelId::new(mdl),
                ))
            } else {
                self.state.provider.as_ref().map(|cur_prov| {
                    (
                        cur_prov.clone(),
                        crate::core::model::ModelId::new(target_str.as_str()),
                    )
                })
            }
        } else {
            None
        };

        // The swap is never applied here — that would change the model mid-turn
        // ( anti-pattern). The target rides on the run and is
        // applied at the landing in `finish_run` → `apply_handoff_swap`.
        if self.run.is_some() || self.blocked().is_some() || self.provider(&provider).is_none() {
            if let Some(active) = self.run.as_mut() {
                active.handoff_target = target_spec;
                // Drain the in-flight turn to a safe landing as a handoff, not a
                // quota event: the next provider turn becomes a landing turn and
                // `finish_run` records `FinishReason::Handoff`.
                if active.intent == RunIntent::Normal {
                    active.intent = RunIntent::ManualHandoff;
                }
            }
            return;
        }

        let run = RunId::new();
        if let Err(error) = self.persist(MjolnrEvent::RunStarted { session, run }).await {
            self.note_store_failure(&error);
            return;
        }
        let cancel = CancellationToken::new();
        self.run = Some(ActiveRun {
            id: run,
            session,
            provider,
            model,
            cancel: cancel.clone(),
            accumulator: crate::runtime::session::StreamAccumulator::default(),
            pending_tools: std::collections::VecDeque::new(),
            awaiting_approval: None,
            pending_load_authority: None,
            phase: RunPhase::Provider,
            provider_turns: 0,
            tool_calls: 0,
            intent: RunIntent::ManualHandoff,
            pending_drain: None,
            hard_stop: None,
            handoff_target: target_spec,
            // A handoff run answers no review request.
            pending_review_threads: Vec::new(),
            plan_run: None,
        });
        self.state.budget.provider_turns = 0;
        self.state.budget.tool_calls = 0;
        self.publish_snapshot();

        let mailbox = self.mailbox.clone();
        let wall_time = self.limits.max_wall_time;
        tokio::spawn(async move {
            tokio::select! {
                () = cancel.cancelled() => {}
                () = tokio::time::sleep(wall_time) => {
                    let _ = mailbox.send(Mail::BudgetExpired { run }).await;
                }
            }
        });
        self.begin_provider_turn(run).await;
    }

    pub(super) async fn create_handoff_artifact(
        &mut self,
        run: RunId,
    ) -> Result<(), crate::core::store::StoreError> {
        let Some(active) = self.run.as_ref().filter(|active| active.id == run) else {
            return Ok(());
        };
        let session = active.session;
        let provider = active.provider.clone();
        let model = active.model.clone();
        let events = self.store.events(session).await?;
        let mut files_read = BTreeSet::new();
        let mut files_changed = BTreeSet::new();
        let mut commands = Vec::new();
        let mut proposals = BTreeMap::new();
        for stored in &events {
            match &stored.event {
                MjolnrEvent::ToolProposed { call, preview, .. } => {
                    proposals.insert(call.id.clone(), preview.clone());
                }
                MjolnrEvent::ToolCompleted {
                    call_id,
                    name,
                    result,
                    ..
                } => match &result.effect {
                    ToolEffect::Read { path, .. } => {
                        files_read.insert(std::path::PathBuf::from(path));
                    }
                    ToolEffect::Mutation { path, .. } => {
                        files_changed.insert(std::path::PathBuf::from(path));
                    }
                    ToolEffect::Command {
                        exit_code, success, ..
                    } => commands.push(CommandFact {
                        command: proposals
                            .get(call_id)
                            .cloned()
                            .unwrap_or_else(|| name.clone()),
                        outcome: format!("success={success}, exit={exit_code:?}"),
                    }),
                    ToolEffect::None
                    | ToolEffect::Completion { .. }
                    | ToolEffect::SkillActivated { .. } => {}
                },
                MjolnrEvent::ToolFailed { call_id, code, .. } => {
                    if let Some(command) = proposals.get(call_id) {
                        commands.push(CommandFact {
                            command: command.clone(),
                            outcome: code.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
        let status = self
            .state
            .messages()
            .iter()
            .rev()
            .find(|message| message.provider.is_some())
            .map_or_else(
                || "No model-written status was available.".to_owned(),
                |entry| entry.text(),
            );
        let handoff = HandoffCheckpoint {
            id: HandoffId::new(),
            created_at: time::OffsetDateTime::now_utc(),
            status,
            provider,
            model,
            files_read: files_read.into_iter().collect(),
            files_changed: files_changed.into_iter().collect(),
            commands,
            usage: self.state.usage,
            budget: self.state.budget,
            activated_skills: self.state.activated_skills.iter().cloned().collect(),
        };
        self.persist(MjolnrEvent::HandoffCreated {
            session,
            handoff: Box::new(handoff.clone()),
        })
        .await?;
        self.state.handoff = Some(handoff);
        self.state.enable_compact_context(COMPACT_RECENT_TURNS);
        Ok(())
    }

    /// Apply a live handoff's provider/model swap at the run's landing (plan
    /// §Phase 24). Emits `ModelChanged` so the swap is evidenced — recovery
    /// replay and `/model` treat it exactly like an interactive switch — and
    /// resets a full-auto policy, which never survives a handoff, as on resume.
    pub(super) async fn apply_handoff_swap(&mut self, run: RunId) {
        let Some(target) = self
            .run
            .as_ref()
            .filter(|active| active.id == run)
            .and_then(|active| active.handoff_target.clone())
        else {
            return;
        };
        let Some(session) = self.state.session else {
            return;
        };
        if let Err(error) = self
            .persist(MjolnrEvent::ModelChanged {
                session,
                provider: target.0.clone(),
                model: target.1.clone(),
            })
            .await
        {
            self.note_store_failure(&error);
            return;
        }
        self.state.provider = Some(target.0);
        self.state.model = Some(target.1);
        // A provider's window cannot govern another provider's — the new
        // adapter reports its own facts (mirrors the `/model` swap path).
        self.state.quota_reserve = crate::core::continuation::QuotaReserveStatus::default();
        if self.state.policy == crate::core::policy::PolicyMode::FullAuto {
            self.state.policy = crate::core::policy::PolicyMode::Ask;
        }
    }

    pub(super) async fn resolve_resume(&mut self, choice: ResumeChoice) {
        if self.run.is_some() || self.state.resume_advice.is_none() {
            return;
        }
        match choice {
            ResumeChoice::Compact => {
                if self.state.enable_compact_context(COMPACT_RECENT_TURNS) {
                    self.state.resume_advice = None;
                }
            }
            ResumeChoice::Full => {
                self.state.disable_compact_context();
                self.state.resume_advice = None;
            }
            ResumeChoice::NewFromHandoff => self.new_session_from_handoff().await,
        }
        self.publish_snapshot();
    }

    async fn new_session_from_handoff(&mut self) {
        let (Some(handoff), Some(root)) = (
            self.state.handoff.clone(),
            self.state.workspace_root.clone(),
        ) else {
            return;
        };
        let provider = self
            .state
            .provider
            .clone()
            .unwrap_or_else(|| handoff.provider.clone());
        let model = self
            .state
            .model
            .clone()
            .unwrap_or_else(|| handoff.model.clone());
        if let Err(error) = self.release_lease().await {
            self.note_store_failure(&error);
            return;
        }
        self.state.reset_keeping_project();
        self.state.workspace_root = Some(root);
        self.create_session(provider, model).await;
        let Some(session) = self.state.session else {
            return;
        };
        let seed = crate::core::message::CanonicalMessage::system(handoff.compact_seed());
        let stored = match self
            .persist(MjolnrEvent::MessageAppended {
                session,
                message: Box::new(seed.clone()),
            })
            .await
        {
            Ok(stored) => stored,
            Err(error) => {
                self.note_store_failure(&error);
                return;
            }
        };
        self.state.push_message(Some(stored.sequence), seed);
        if let Err(error) = self
            .persist(MjolnrEvent::HandoffCreated {
                session,
                handoff: Box::new(handoff.clone()),
            })
            .await
        {
            self.note_store_failure(&error);
            return;
        }
        self.state.handoff = Some(handoff);
        self.state.enable_compact_context(0);
        self.state.resume_advice = None;
    }

    pub(super) async fn stop_for_quota(&mut self, run: RunId) {
        let reserve = self
            .run
            .as_ref()
            .filter(|active| active.id == run)
            .and_then(|active| active.hard_stop.clone());
        let Some(mut reserve) = reserve else {
            return;
        };
        reserve.phase = QuotaReservePhase::Stopped;
        self.state.quota_reserve = reserve.clone();

        // : a quota reserve breach may advance a route instead
        // of stopping outright. `create_handoff_artifact` never sends a
        // provider request — it derives its facts mechanically from the
        // event log and the last model-written text — so producing it here
        // does not spend the tokens the hard threshold exists to protect.
        // Compact context then bounds what the new hop's first turn sends,
        // obeying the Phase 10 cross-model continuation rule.
        if self.state.route.is_some() {
            if let Err(error) = self.create_handoff_artifact(run).await {
                self.halt_for_store(run, &error);
                return;
            }
            match self
                .try_advance_route(run, RouteAdvanceCondition::QuotaReserveBreached)
                .await
            {
                RouteAttemptOutcome::Advanced => {
                    self.state.enable_compact_context(0);
                    Box::pin(self.begin_provider_turn(run)).await;
                    return;
                }
                // Exhausted already recorded its own typed stop; NotRouted
                // cannot happen here since `self.state.route` was checked
                // just above, but either way falling through to the original
                // stop below is safe.
                RouteAttemptOutcome::Exhausted => return,
                RouteAttemptOutcome::NotRouted => {}
            }
        }

        let reset = reserve
            .resets_at
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string());
        self.fail_run(
            run,
            ReasonCode::ProviderPlanQuota,
            format!("quota reserve reached; reset at {reset}"),
        )
        .await;
    }

    pub(super) async fn complete_quota_drain(
        &mut self,
        run: RunId,
    ) -> Result<(), crate::core::store::StoreError> {
        let Some(session) = self.state.session else {
            return Ok(());
        };
        let mut reserve = self.state.quota_reserve.clone();
        reserve.phase = QuotaReservePhase::Stopped;
        self.persist(MjolnrEvent::QuotaBoundaryReached {
            session,
            run,
            reserve: reserve.clone(),
        })
        .await?;
        self.state.quota_reserve = reserve;
        Ok(())
    }
}
